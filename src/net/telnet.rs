use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::engine::{ClientMessage, EngineMessage};
use crate::markup;

const IAC: u8 = 255;
const WILL: u8 = 251;
const WONT: u8 = 252;
const DO: u8 = 253;
const DONT: u8 = 254;
const SB: u8 = 250;
const SE: u8 = 240;
const SGA: u8 = 3; // Suppress Go Ahead
const ECHO: u8 = 1;
const GMCP: u8 = 201; // Generic MUD Communication Protocol

pub async fn start_telnet(
    addr: &str,
    engine_tx: mpsc::UnboundedSender<EngineMessage>,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("Telnet listening on {}", addr);

    loop {
        let (stream, peer) = listener.accept().await?;
        stream.set_nodelay(true)?;
        tracing::info!(%peer, "New connection");

        let engine_tx = engine_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, engine_tx).await {
                tracing::warn!(%peer, error = %e, "Connection error");
            }
        });
    }
}

enum WriteCmd {
    Text(String),
    Raw(Vec<u8>),
}

/// Frame an out-of-band GMCP message: `IAC SB GMCP <package> <json> IAC SE`.
/// `package` and `json` are always valid UTF-8, which never contains byte 0xFF
/// (`IAC`), so no in-payload escaping is needed.
fn gmcp_frame(package: &str, json: &str) -> Vec<u8> {
    let mut out = vec![IAC, SB, GMCP];
    out.extend_from_slice(format!("{} {}", package, json).as_bytes());
    out.push(IAC);
    out.push(SE);
    out
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    engine_tx: mpsc::UnboundedSender<EngineMessage>,
) -> std::io::Result<()> {
    let session_id = Uuid::new_v4().to_string();
    let (mut reader, mut writer) = stream.into_split();

    // Tell the client we'll suppress Go Ahead (no GA after each prompt), that we
    // will echo (so the client doesn't double-echo), and that we can speak GMCP
    // (option 201) for out-of-band structured data — Mudlet et al. reply DO.
    writer
        .write_all(&[IAC, WILL, SGA, IAC, WILL, ECHO, IAC, WILL, GMCP])
        .await?;
    writer.flush().await?;

    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<WriteCmd>();

    // Shared with the read loop, which flips it when the client enables GMCP.
    let supports_gmcp = Arc::new(AtomicBool::new(false));

    let (engine_out_tx, mut engine_out_rx) = mpsc::unbounded_channel::<ClientMessage>();
    let write_tx_for_engine = write_tx.clone();
    let gmcp_out = Arc::clone(&supports_gmcp);
    tokio::spawn(async move {
        while let Some(msg) = engine_out_rx.recv().await {
            let cmd = match msg {
                ClientMessage::Text { text } => WriteCmd::Text(text),
                ClientMessage::Prompt { echo } => {
                    let bytes = if echo {
                        vec![IAC, WONT, ECHO]
                    } else {
                        vec![IAC, WILL, ECHO]
                    };
                    WriteCmd::Raw(bytes)
                }
                // Structured room data → GMCP `Room.Info` for map-aware clients.
                // Text-only clients never enabled GMCP, so this is dropped there.
                ClientMessage::Room {
                    name, exits, num, area, map, environment, x, y, ..
                } => {
                    if !gmcp_out.load(Ordering::Relaxed) {
                        continue;
                    }
                    let mut ex = serde_json::Map::new();
                    for e in &exits {
                        ex.insert(e.dir.clone(), serde_json::Value::String(e.to.clone()));
                    }
                    let mut obj = serde_json::json!({ "num": num, "name": name, "exits": ex });
                    if !area.is_empty() {
                        obj["area"] = serde_json::json!(area);
                    }
                    if let Some(env) = environment {
                        obj["environment"] = serde_json::json!(env);
                    }
                    if let Some(m) = map {
                        obj["map"] = serde_json::json!(m);
                    }
                    if let (Some(x), Some(y)) = (x, y) {
                        obj["coords"] = serde_json::json!({ "x": x, "y": y });
                    }
                    WriteCmd::Raw(gmcp_frame("Room.Info", &obj.to_string()))
                }
                // Softcode `emit_data(target, channel, data)` → GMCP `<channel>`.
                ClientMessage::Game { channel, data } => {
                    if !gmcp_out.load(Ordering::Relaxed) {
                        continue;
                    }
                    WriteCmd::Raw(gmcp_frame(&channel, &data.to_string()))
                }
                _ => continue,
            };
            if write_tx_for_engine.send(cmd).is_err() {
                break;
            }
        }
    });

    let _ = engine_tx.send(EngineMessage::PlayerConnected {
        session_id: session_id.clone(),
        tx: engine_out_tx,
    });

    let write_session_id = session_id.clone();
    let write_handle = tokio::spawn(async move {
        while let Some(cmd) = write_rx.recv().await {
            let result = match cmd {
                WriteCmd::Text(msg) => writer.write_all(markup::to_ansi(&msg).as_bytes()).await,
                WriteCmd::Raw(bytes) => writer.write_all(&bytes).await,
            };
            if result.is_err() {
                break;
            }
            let _ = writer.flush().await;
        }
        tracing::debug!(session_id = %write_session_id, "Write loop ended");
    });

    let mut buf = [0u8; 4096];
    let mut line_buf = Vec::new();
    let mut neg = Negotiator::new();

    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };

        let (lines, negotiate_responses) = process_bytes(&buf[..n], &mut line_buf, &mut neg);
        supports_gmcp.store(neg.gmcp, Ordering::Relaxed);

        for response in negotiate_responses {
            let _ = write_tx.send(WriteCmd::Raw(response));
        }

        for input in lines {
            let _ = engine_tx.send(EngineMessage::PlayerInput {
                session_id: session_id.clone(),
                input,
            });
        }
    }

    let _ = engine_tx.send(EngineMessage::PlayerDisconnected {
        session_id: session_id.clone(),
    });

    write_handle.abort();
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum IacState {
    Normal,
    Iac,
    Will,
    Wont,
    Do,
    Dont,
    SbOpt, // reading the option byte after IAC SB
    Sb,    // buffering subnegotiation payload
    SbIac, // saw IAC inside SB
}

/// Per-connection telnet negotiation state: the IAC parser cursor, a buffer for
/// the current subnegotiation, and whether the client has enabled GMCP.
struct Negotiator {
    iac: IacState,
    sb_opt: u8,
    sb_buf: Vec<u8>,
    gmcp: bool,
}

impl Negotiator {
    fn new() -> Self {
        Self {
            iac: IacState::Normal,
            sb_opt: 0,
            sb_buf: Vec::new(),
            gmcp: false,
        }
    }
}

fn process_bytes(
    data: &[u8],
    line_buf: &mut Vec<u8>,
    neg: &mut Negotiator,
) -> (Vec<String>, Vec<Vec<u8>>) {
    let mut lines = Vec::new();
    let mut negotiate = Vec::new();

    for &byte in data {
        match neg.iac {
            IacState::Normal => {
                if byte == IAC {
                    neg.iac = IacState::Iac;
                } else if byte == b'\n' {
                    let line = String::from_utf8_lossy(line_buf).trim().to_string();
                    line_buf.clear();
                    lines.push(line);
                } else if byte != b'\r' {
                    line_buf.push(byte);
                }
            }
            IacState::Iac => match byte {
                WILL => neg.iac = IacState::Will,
                WONT => neg.iac = IacState::Wont,
                DO => neg.iac = IacState::Do,
                DONT => neg.iac = IacState::Dont,
                SB => neg.iac = IacState::SbOpt,
                IAC => {
                    line_buf.push(IAC);
                    neg.iac = IacState::Normal;
                }
                _ => neg.iac = IacState::Normal,
            },
            IacState::Will => {
                // A client offering GMCP: accept it. Everything else: refuse.
                if byte == GMCP {
                    negotiate.push(vec![IAC, DO, GMCP]);
                    neg.gmcp = true;
                } else {
                    negotiate.push(vec![IAC, DONT, byte]);
                }
                neg.iac = IacState::Normal;
            }
            IacState::Wont => {
                if byte == GMCP {
                    neg.gmcp = false;
                }
                neg.iac = IacState::Normal;
            }
            IacState::Do => {
                // Confirmations of what we offered (SGA, ECHO) need no reply;
                // `DO GMCP` accepts our `WILL GMCP` and turns the channel on.
                // Refuse anything else.
                if byte == SGA || byte == ECHO {
                    // already asserted; nothing to send
                } else if byte == GMCP {
                    neg.gmcp = true;
                } else {
                    negotiate.push(vec![IAC, WONT, byte]);
                }
                neg.iac = IacState::Normal;
            }
            IacState::Dont => {
                if byte == GMCP {
                    neg.gmcp = false;
                }
                neg.iac = IacState::Normal;
            }
            IacState::SbOpt => {
                neg.sb_opt = byte;
                neg.sb_buf.clear();
                neg.iac = IacState::Sb;
            }
            IacState::Sb => {
                if byte == IAC {
                    neg.iac = IacState::SbIac;
                } else {
                    neg.sb_buf.push(byte);
                }
            }
            IacState::SbIac => {
                if byte == SE {
                    // End of subnegotiation. Inbound GMCP (Core.Hello,
                    // Core.Supports.Set, …) is parsed-and-ignored for now — we
                    // only need the enable handshake to start *sending*.
                    if neg.sb_opt == GMCP {
                        tracing::trace!(
                            payload = %String::from_utf8_lossy(&neg.sb_buf),
                            "inbound GMCP (ignored)"
                        );
                    }
                    neg.sb_buf.clear();
                    neg.iac = IacState::Normal;
                } else if byte == IAC {
                    // Escaped IAC inside the payload (IAC IAC → literal 0xFF).
                    neg.sb_buf.push(IAC);
                    neg.iac = IacState::Sb;
                } else {
                    neg.iac = IacState::Sb;
                }
            }
        }
    }

    (lines, negotiate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmcp_frame_wraps_package_and_json() {
        let f = gmcp_frame("Room.Info", "{\"num\":\"#1\"}");
        assert_eq!(f[0], IAC);
        assert_eq!(f[1], SB);
        assert_eq!(f[2], GMCP);
        assert_eq!(&f[f.len() - 2..], &[IAC, SE]);
        let body = String::from_utf8_lossy(&f[3..f.len() - 2]);
        assert_eq!(body, "Room.Info {\"num\":\"#1\"}");
    }

    #[test]
    fn client_do_gmcp_enables_the_channel() {
        let mut neg = Negotiator::new();
        let mut lb = Vec::new();
        assert!(!neg.gmcp);
        let (_lines, resp) = process_bytes(&[IAC, DO, GMCP], &mut lb, &mut neg);
        assert!(neg.gmcp, "DO GMCP should enable the channel");
        assert!(resp.is_empty(), "DO of our own WILL needs no reply");
    }

    #[test]
    fn client_will_gmcp_is_accepted_with_do() {
        let mut neg = Negotiator::new();
        let mut lb = Vec::new();
        let (_lines, resp) = process_bytes(&[IAC, WILL, GMCP], &mut lb, &mut neg);
        assert!(neg.gmcp);
        assert_eq!(resp, vec![vec![IAC, DO, GMCP]]);
    }

    #[test]
    fn dont_gmcp_disables_the_channel() {
        let mut neg = Negotiator::new();
        let mut lb = Vec::new();
        process_bytes(&[IAC, DO, GMCP], &mut lb, &mut neg);
        assert!(neg.gmcp);
        process_bytes(&[IAC, DONT, GMCP], &mut lb, &mut neg);
        assert!(!neg.gmcp, "DONT GMCP turns it back off");
    }

    #[test]
    fn other_options_are_still_refused() {
        let mut neg = Negotiator::new();
        let mut lb = Vec::new();
        // Some arbitrary option (NAWS = 31) — we refuse WILL with DONT.
        let (_lines, resp) = process_bytes(&[IAC, WILL, 31], &mut lb, &mut neg);
        assert_eq!(resp, vec![vec![IAC, DONT, 31]]);
        assert!(!neg.gmcp);
    }

    #[test]
    fn gmcp_subnegotiation_is_consumed_without_leaking_into_input() {
        // An inbound GMCP SB (Core.Supports.Set) around a real command line
        // must be swallowed whole — none of its bytes should reach the parser.
        let mut neg = Negotiator::new();
        let mut lb = Vec::new();
        let mut data = vec![IAC, SB, GMCP];
        data.extend_from_slice(b"Core.Supports.Set [\"Room 1\"]");
        data.extend_from_slice(&[IAC, SE]);
        data.extend_from_slice(b"look\n");
        let (lines, _resp) = process_bytes(&data, &mut lb, &mut neg);
        assert_eq!(lines, vec!["look".to_string()]);
    }

    #[test]
    fn escaped_iac_inside_subnegotiation_is_handled() {
        // IAC IAC inside SB is a literal 0xFF, not the end of the block.
        let mut neg = Negotiator::new();
        let mut lb = Vec::new();
        let mut data = vec![IAC, SB, GMCP, b'a', IAC, IAC, b'b', IAC, SE];
        data.extend_from_slice(b"go n\n");
        let (lines, _resp) = process_bytes(&data, &mut lb, &mut neg);
        assert_eq!(lines, vec!["go n".to_string()]);
    }
}
