//! Integration coverage for the `.session` end-to-end runner.
//!
//! Two sources, both driving the real engine in-process (see
//! `hearth_mud::session_test`):
//!
//! 1. Framework fixtures under `tests/fixtures/*.session` — always run, against
//!    the bare default world (no game content), so this catches regressions in
//!    login/dispatch/render even without the sibling game repo.
//! 2. The game's own `.session` files, discovered recursively under its
//!    `game_dir` and run against the real Last Stag config — skipped silently
//!    when the sibling repo isn't checked out (e.g. CI without it).

use std::path::{Path, PathBuf};

use hearth_mud::config::Config;
use hearth_mud::session_test::run_file_blocking;

/// Recursively collect every `*.session` file under `dir`.
fn discover_sessions(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "session") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Run every discovered file against `config`, panicking with a readable report
/// if any assertion fails. Returns how many files ran.
fn run_all(config: &Config, files: &[PathBuf]) -> usize {
    let mut failures = Vec::new();
    for file in files {
        match run_file_blocking(config, None, file) {
            Ok(outcome) if outcome.passed() => {
                eprintln!("  PASS {} ({} checks)", file.display(), outcome.checks);
            }
            Ok(outcome) => {
                for f in &outcome.failures {
                    let verb = if f.negate { "expect-not" } else { "expect" };
                    failures.push(format!(
                        "{}:{}  {} {} did not hold\n--- output searched ---\n{}\n-----------------------",
                        file.display(),
                        f.line,
                        verb,
                        f.pattern,
                        f.window.trim_end()
                    ));
                }
            }
            Err(e) => failures.push(format!("{}: {}", file.display(), e)),
        }
    }
    assert!(
        failures.is_empty(),
        "session-test failures:\n\n{}",
        failures.join("\n\n")
    );
    files.len()
}

#[test]
fn framework_fixture_sessions_pass() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let files = discover_sessions(&dir);
    assert!(
        !files.is_empty(),
        "expected at least one fixture under {}",
        dir.display()
    );
    run_all(&Config::default(), &files);
}

#[test]
fn game_sessions_pass_if_present() {
    // Mirror game_smoke.rs: the real game config points game_dir at the sibling
    // repo. Skip cleanly when it isn't checked out.
    let config_path = Path::new("../the-last-stag-mud/hearth.toml");
    if !config_path.exists() {
        eprintln!("skipping: {} not present", config_path.display());
        return;
    }
    let config = Config::load(config_path);
    let Some(game_dir) = config.game_dir.as_deref() else {
        eprintln!("skipping: game config has no game_dir");
        return;
    };
    let files = discover_sessions(Path::new(game_dir));
    if files.is_empty() {
        eprintln!("no .session files under {} yet", game_dir);
        return;
    }
    let n = run_all(&config, &files);
    eprintln!("ran {} game .session file(s)", n);
}
