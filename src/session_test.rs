//! In-process end-to-end "session script" runner.
//!
//! `.test.luau` files exercise hook *logic* against a `test_world`, but nothing
//! there reaches the telnet *wire*: the login/account flow, command dispatch,
//! the prompt/dialogue state machine, and the actual text a player reads. Those
//! are exactly where wire-level bugs hide (a green Luau test while `explore`
//! crashes on spawn; a command swallowed as dialogue input inside an NPC
//! prompt). A real telnet client catches them but needs a socket and timing
//! sleeps — flaky.
//!
//! This runner drives the REAL session handler in-process, no socket. It reuses
//! the exact `EngineMessage` path a telnet byte stream travels — `PlayerConnected`
//! hands the engine an output sink, one `PlayerInput` per input line, and output
//! comes back as `ClientMessage`. So it exercises login, dispatch, prompt
//! routing, and the renderer — everything telnet does except IAC/GMCP
//! negotiation and the final BBCode→ANSI step, neither of which is where those
//! bugs live.
//!
//! **Determinism without sleeps.** The engine loop consumes one FIFO channel and
//! `handle_input` is synchronous, so after sending a `PlayerInput` we send an
//! `ApiRequest` (`ListRooms`) as a *fence*: when its reply comes back, the input
//! has been fully processed and every `ClientMessage` it produced is already
//! queued, ready to drain with `try_recv`. No socket, no sleep, no idle-timeout
//! guessing. (Deterministic for input-driven flows; tick/timer/clock-driven
//! output — `on_tick`, `after()` — is a follow-up that would need a manual
//! tick-advance.)
//!
//! ## `.session` format
//!
//! ```text
//! # comment lines start with '#'; blank lines are ignored
//! > create                  # a line sent to the session ('>' then one space)
//! > tester
//! > secretpass
//! > secretpass
//! expect: Account created    # a substring that must appear in the output
//! > look
//! expect: /Crossroads|Spawn/ # /.../ is a regex instead of a substring
//! expect-not: error          # must NOT appear
//! ```
//!
//! An `expect:`/`expect-not:` checks the output produced by every `>` line since
//! the previous assertion (so a batch of inputs followed by one assertion reads
//! naturally). Consecutive assertions with no input between them share the same
//! output window. Matching is against plain text — BBCode markup is stripped
//! (`markup::to_plain`), so patterns match what a player reads, not the wire
//! encoding.

use std::path::{Path, PathBuf};

use tokio::sync::mpsc;

use crate::config::Config;
use crate::db::Database;
use crate::engine::{ApiRequest, ClientMessage, Engine, EngineMessage};
use crate::markup;

const SESSION_ID: &str = "session-test";

/// One `expect:`/`expect-not:` assertion that didn't hold.
#[derive(Debug, Clone)]
pub struct Failure {
    /// 1-based line number in the `.session` file.
    pub line: usize,
    pub negate: bool,
    pub pattern: String,
    /// The plain-text output window the pattern was matched against.
    pub window: String,
}

/// Result of running one `.session` file.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub name: String,
    pub checks: usize,
    pub failures: Vec<Failure>,
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug)]
enum Matcher {
    Substring(String),
    Regex(regex::Regex),
}

impl Matcher {
    fn parse(raw: &str) -> Result<Matcher, String> {
        let s = raw.trim();
        // `/pattern/` is a regex; anything else is a literal substring. Needs at
        // least the two slashes plus one char so a bare `//` isn't a regex.
        if s.len() >= 3 && s.starts_with('/') && s.ends_with('/') {
            let pat = &s[1..s.len() - 1];
            regex::Regex::new(pat)
                .map(Matcher::Regex)
                .map_err(|e| format!("invalid regex /{}/: {}", pat, e))
        } else {
            Ok(Matcher::Substring(s.to_string()))
        }
    }

    fn is_match(&self, haystack: &str) -> bool {
        match self {
            Matcher::Substring(s) => haystack.contains(s.as_str()),
            Matcher::Regex(r) => r.is_match(haystack),
        }
    }

    fn describe(&self) -> String {
        match self {
            Matcher::Substring(s) => format!("{:?}", s),
            Matcher::Regex(r) => format!("/{}/", r.as_str()),
        }
    }
}

#[derive(Debug)]
enum Step {
    Input(String),
    Expect {
        line: usize,
        matcher: Matcher,
        negate: bool,
    },
}

fn parse(source: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for (idx, raw) in source.lines().enumerate() {
        let lineno = idx + 1;
        // `lines()` already drops '\n'; also drop a trailing '\r' for CRLF files.
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = line.trim_start();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(rest) = line.strip_prefix('>') {
            // Exactly one space after '>' is the delimiter; the rest is the
            // input verbatim (never trimmed — a password may be space-sensitive).
            let input = rest.strip_prefix(' ').unwrap_or(rest);
            steps.push(Step::Input(input.to_string()));
        } else if let Some(rest) = trimmed.strip_prefix("expect-not:") {
            steps.push(Step::Expect {
                line: lineno,
                matcher: Matcher::parse(rest)?,
                negate: true,
            });
        } else if let Some(rest) = trimmed.strip_prefix("expect:") {
            steps.push(Step::Expect {
                line: lineno,
                matcher: Matcher::parse(rest)?,
                negate: false,
            });
        } else {
            return Err(format!(
                "line {}: expected `> <input>`, `expect:`, `expect-not:`, `# comment`, or a blank line; got: {}",
                lineno, line
            ));
        }
    }
    Ok(steps)
}

/// A live in-process session driving the real engine over its message channels.
struct Harness {
    tx: mpsc::UnboundedSender<EngineMessage>,
    out_rx: mpsc::UnboundedReceiver<ClientMessage>,
    handle: tokio::task::JoinHandle<()>,
}

impl Harness {
    fn start(config: &Config, db: Database) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let engine = Engine::new(rx, db, config);
        let handle = tokio::spawn(engine.run());

        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let _ = tx.send(EngineMessage::PlayerConnected {
            session_id: SESSION_ID.to_string(),
            tx: out_tx,
        });

        Self { tx, out_rx, handle }
    }

    /// Block until the engine has drained every message queued so far. The loop
    /// is FIFO and `handle_input` is synchronous, so once this API reply returns
    /// the preceding `PlayerInput`'s output is all queued. `ListRooms` replies
    /// regardless of auth — we only use it for its ordering guarantee.
    async fn fence(&self) {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(EngineMessage::ApiRequest {
            request: ApiRequest::ListRooms,
            token: None,
            reply: reply_tx,
        });
        let _ = reply_rx.await;
    }

    /// Drain all currently-queued output as stripped plain text.
    fn drain_plain(&mut self) -> String {
        let mut buf = String::new();
        while let Ok(msg) = self.out_rx.try_recv() {
            if let ClientMessage::Text { text } = msg {
                buf.push_str(&text);
            }
        }
        markup::to_plain(&buf)
    }

    async fn input(&mut self, line: &str) -> String {
        let _ = self.tx.send(EngineMessage::PlayerInput {
            session_id: SESSION_ID.to_string(),
            input: line.to_string(),
        });
        self.fence().await;
        self.drain_plain()
    }

    async fn shutdown(self) {
        let _ = self.tx.send(EngineMessage::Shutdown);
        let _ = self.handle.await;
    }
}

/// Run a parsed `.session` source against an engine built from `config` + `db`.
/// Returns the outcome, or a parse-error string if the file is malformed.
pub async fn run_source(
    config: &Config,
    db: Database,
    name: &str,
    source: &str,
) -> Result<Outcome, String> {
    let steps = parse(source)?;

    let mut harness = Harness::start(config, db);
    // Seed the window with the connect banner (emitted during PlayerConnected),
    // so an `expect:` before the first `>` can match the login prompt.
    harness.fence().await;
    let mut window = harness.drain_plain();

    let mut checks = 0;
    let mut failures = Vec::new();
    // The window accumulates across inputs and is reset by the FIRST input that
    // follows an assertion — so N inputs then one `expect:` checks all N, and
    // consecutive assertions share the same window.
    let mut assertion_since_input = false;

    for step in &steps {
        match step {
            Step::Input(line) => {
                if assertion_since_input {
                    window.clear();
                    assertion_since_input = false;
                }
                let out = harness.input(line).await;
                window.push_str(&out);
            }
            Step::Expect {
                line,
                matcher,
                negate,
            } => {
                checks += 1;
                assertion_since_input = true;
                let hit = matcher.is_match(&window);
                if hit == *negate {
                    failures.push(Failure {
                        line: *line,
                        negate: *negate,
                        pattern: matcher.describe(),
                        window: window.clone(),
                    });
                }
            }
        }
    }

    harness.shutdown().await;
    Ok(Outcome {
        name: name.to_string(),
        checks,
        failures,
    })
}

/// Open a session-test database: an on-disk file when `path` is given (it is
/// created if absent and MUTATED by the run — the session creates an account,
/// spawns a player, and moves through the world), or a private throwaway
/// in-memory database when `None` (the default, and what the cargo tests use).
pub fn open_db(path: Option<&Path>) -> Result<Database, String> {
    let path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(":memory:"));
    Database::open(&path).map_err(|e| format!("failed to open database {}: {}", path.display(), e))
}

/// Blocking entry point for the CLI: read a `.session` file and run it against a
/// fresh single-threaded runtime. `db_path` is `None` for an in-memory database.
pub fn run_file_blocking(
    config: &Config,
    db_path: Option<&Path>,
    path: &Path,
) -> Result<Outcome, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("session")
        .to_string();
    let db = open_db(db_path)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to build runtime: {}", e))?;
    runtime.block_on(run_source(config, db, &name, &source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Database {
        Database::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn parse_rejects_a_stray_line() {
        let err = parse("> ok\nnonsense line\n").unwrap_err();
        assert!(err.contains("line 2"), "got: {}", err);
    }

    #[test]
    fn parse_reads_inputs_and_assertions() {
        let steps = parse("# c\n> create\nexpect: Welcome\nexpect-not: /err/\n").unwrap();
        assert_eq!(steps.len(), 3);
        assert!(matches!(&steps[0], Step::Input(s) if s == "create"));
        assert!(matches!(&steps[1], Step::Expect { negate: false, .. }));
        assert!(matches!(&steps[2], Step::Expect { negate: true, .. }));
    }

    #[tokio::test]
    async fn account_creation_flow_reaches_the_world() {
        // The bare framework world (no game_dir) is enough for login + look:
        // Engine::new builds a fallback spawn room.
        let source = "\
> create
> tester
> secretpass
> secretpass
expect: Account created
> look
expect-not: /^\\s*$/
";
        let outcome = run_source(&Config::default(), mem(), "login.session", source)
            .await
            .expect("valid session file");
        assert!(
            outcome.passed(),
            "expected all checks to pass, failures: {:?}",
            outcome.failures
        );
        assert_eq!(outcome.checks, 2);
    }

    #[tokio::test]
    async fn a_failed_expectation_is_reported_with_its_line() {
        let source = "\
> create
> tester
> secretpass
> secretpass
expect: this text is definitely not in the output
";
        let outcome = run_source(&Config::default(), mem(), "fail.session", source)
            .await
            .expect("valid session file");
        assert!(!outcome.passed());
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].line, 5);
    }
}
