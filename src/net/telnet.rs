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

async fn handle_connection(
    stream: tokio::net::TcpStream,
    engine_tx: mpsc::UnboundedSender<EngineMessage>,
) -> std::io::Result<()> {
    let session_id = Uuid::new_v4().to_string();
    let (mut reader, mut writer) = stream.into_split();

    // Tell the client we'll suppress Go Ahead (no GA after each prompt)
    // and that we will echo (so the client doesn't double-echo).
    writer.write_all(&[IAC, WILL, SGA, IAC, WILL, ECHO]).await?;
    writer.flush().await?;

    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<WriteCmd>();

    let (engine_out_tx, mut engine_out_rx) = mpsc::unbounded_channel::<ClientMessage>();
    let write_tx_for_engine = write_tx.clone();
    tokio::spawn(async move {
        while let Some(msg) = engine_out_rx.recv().await {
            match msg {
                ClientMessage::Text { text } => {
                    if write_tx_for_engine.send(WriteCmd::Text(text)).is_err() {
                        break;
                    }
                }
                ClientMessage::Prompt { echo } => {
                    let bytes = if echo {
                        vec![IAC, WONT, ECHO]
                    } else {
                        vec![IAC, WILL, ECHO]
                    };
                    if write_tx_for_engine.send(WriteCmd::Raw(bytes)).is_err() {
                        break;
                    }
                }
                _ => {}
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
    let mut iac_state = IacState::Normal;

    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };

        let (lines, negotiate_responses) =
            process_bytes(&buf[..n], &mut line_buf, &mut iac_state);

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

#[derive(Clone, Copy)]
enum IacState {
    Normal,
    Iac,
    Will,
    Wont,
    Do,
    Dont,
    Sb,
    SbIac,
}

fn process_bytes(
    data: &[u8],
    line_buf: &mut Vec<u8>,
    state: &mut IacState,
) -> (Vec<String>, Vec<Vec<u8>>) {
    let mut lines = Vec::new();
    let mut negotiate = Vec::new();

    for &byte in data {
        match *state {
            IacState::Normal => {
                if byte == IAC {
                    *state = IacState::Iac;
                } else if byte == b'\n' {
                    let line = String::from_utf8_lossy(line_buf).trim().to_string();
                    line_buf.clear();
                    lines.push(line);
                } else if byte != b'\r' {
                    line_buf.push(byte);
                }
            }
            IacState::Iac => match byte {
                WILL => *state = IacState::Will,
                WONT => *state = IacState::Wont,
                DO => *state = IacState::Do,
                DONT => *state = IacState::Dont,
                SB => *state = IacState::Sb,
                IAC => {
                    line_buf.push(IAC);
                    *state = IacState::Normal;
                }
                _ => *state = IacState::Normal,
            },
            IacState::Will => {
                negotiate.push(vec![IAC, DONT, byte]);
                *state = IacState::Normal;
            }
            IacState::Wont => {
                *state = IacState::Normal;
            }
            IacState::Do => {
                // Accept DO for options we offered (SGA, ECHO), refuse the rest
                if byte == SGA || byte == ECHO {
                    // Already sent WILL, client is confirming — no response needed
                } else {
                    negotiate.push(vec![IAC, WONT, byte]);
                }
                *state = IacState::Normal;
            }
            IacState::Dont => {
                *state = IacState::Normal;
            }
            IacState::Sb => {
                if byte == IAC {
                    *state = IacState::SbIac;
                }
            }
            IacState::SbIac => {
                if byte == SE {
                    *state = IacState::Normal;
                } else {
                    *state = IacState::Sb;
                }
            }
        }
    }

    (lines, negotiate)
}
