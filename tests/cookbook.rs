//! The cookbooks' recipes are executable, and this runs them.
//!
//! The cookbooks promise their recipes are pasteable. A parser check
//! can't keep that promise: `luau-analyze` treats `emit`, `get_attr` and the
//! rest as unknown globals, so a misspelled API name, a wrong argument order,
//! or a hook that never fires all parse perfectly and fail at runtime. (The
//! `+finger` recipe shipped with exactly that class of bug — a greedy `%S+`
//! that swallowed the `=` in `fset themesong=...` — and only running it found
//! it.)
//!
//! So the doc is the source of truth for the *code*: every ```lua block
//! labelled with a `world/<area>/<name>.luau` path immediately above it is
//! extracted at test time into a temporary game directory, alongside checked-in
//! scaffolding TOML that wires the recipes into a loadable world. The `.session`
//! fixtures then drive the real session handler against it. Edit a recipe in the
//! doc and this test runs the edited version — the two cannot drift.

use std::path::{Path, PathBuf};

use hearth_mud::config::Config;
use hearth_mud::session_test::run_file_blocking;

/// Pull every path-labelled Luau block out of the cookbook.
///
/// A recipe block looks like this in the markdown:
///
/// ```text
/// `world/system/finger.luau`:
///
/// ```lua
/// ...the script...
/// ```
/// ```
///
/// Returns `(relative path, source)` pairs.
fn extract_recipes(doc: &str) -> Vec<(PathBuf, String)> {
    let lines: Vec<&str> = doc.lines().collect();
    let mut found = Vec::new();
    let mut pending: Option<PathBuf> = None;

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();

        // Fences first: "```lua" also starts with a backtick, so the label
        // check below would otherwise swallow it.
        if line == "```lua" {
            if let Some(path) = pending.take() {
                let start = i + 1;
                let mut end = start;
                while end < lines.len() && lines[end].trim() != "```" {
                    end += 1;
                }
                found.push((path, lines[start..end].join("\n")));
                i = end;
            }
        } else if line.starts_with("```") {
            // Any other fence clears a dangling label so a TOML block between
            // a label and its script can't capture the wrong body.
            pending = None;
        } else if let Some(rest) = line.strip_prefix('`') {
            // A label line: exactly a backticked `world/<area>/<file>.luau`.
            if let Some(path) = rest.strip_suffix("`:") {
                if path.starts_with("world/") && path.ends_with(".luau") {
                    // Strip the leading `world/` — the game_dir *is* `world/`.
                    pending = Some(PathBuf::from(path.trim_start_matches("world/")));
                }
            }
        }

        i += 1;
    }
    found
}

/// Copy a directory tree (the scaffolding TOMLs) into `dest`.
fn copy_tree(src: &Path, dest: &Path) {
    std::fs::create_dir_all(dest).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Extract `doc`'s recipes into a temp game dir built from `fixtures_rel`'s
/// scaffolding, then run every `.session` file beside that scaffolding.
fn run_cookbook(doc_rel: &str, fixtures_rel: &str, spawn_room: &str, min_recipes: usize) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc_path = root.join(doc_rel);
    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("reading {}: {}", doc_path.display(), e));

    let recipes = extract_recipes(&doc);
    assert!(
        recipes.len() >= min_recipes,
        "{}: expected path-labelled recipes; found {} ({:?}). \
         Did a `world/<area>/<name>.luau`: label get dropped?",
        doc_path.display(),
        recipes.len(),
        recipes.iter().map(|(p, _)| p.display().to_string()).collect::<Vec<_>>()
    );

    // A unique scratch dir, cleaned up on the way out.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let slug = fixtures_rel.rsplit('/').next().unwrap_or("cookbook");
    let tmp = std::env::temp_dir()
        .join(format!("hearth-cookbook-{}-{}-{}", slug, std::process::id(), stamp));
    let game_dir = tmp.join("world");

    let fixtures = root.join(fixtures_rel);
    copy_tree(&fixtures.join("scaffold"), &game_dir);

    for (rel, source) in &recipes {
        let dest = game_dir.join(rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, source).unwrap();
    }

    let config = Config {
        spawn_room: spawn_room.into(),
        game_dir: Some(game_dir.to_string_lossy().into_owned()),
        db_path: tmp.join("cookbook.db").to_string_lossy().into_owned(),
        ..Config::default()
    };

    let mut sessions: Vec<PathBuf> = std::fs::read_dir(&fixtures)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "session"))
        .collect();
    sessions.sort();
    assert!(!sessions.is_empty(), "no .session fixtures in {}", fixtures.display());

    let mut failures = Vec::new();
    for file in &sessions {
        // Each session starts from a clean database so account creation and
        // the board/mail stores don't leak between files.
        let _ = std::fs::remove_file(&config.db_path);
        match run_file_blocking(&config, None, file) {
            Ok(outcome) if outcome.passed() => {
                eprintln!("  PASS {} ({} checks)", file.display(), outcome.checks);
            }
            Ok(outcome) => {
                for f in &outcome.failures {
                    let verb = if f.negate { "expect-not" } else { "expect" };
                    failures.push(format!(
                        "{}:{}  {} {} did not hold\n--- output searched ---\n{}\n---",
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

    let _ = std::fs::remove_dir_all(&tmp);

    assert!(
        failures.is_empty(),
        "cookbook recipes failed — the code in {} is broken:\n\n{}",
        doc_rel,
        failures.join("\n\n")
    );
}

#[test]
fn mush_cookbook_recipes_run() {
    run_cookbook(
        "docs/mush-cookbook.md",
        "tests/cookbook-fixtures/mush",
        "town/tavern",
        7,
    );
}

#[test]
fn diku_cookbook_recipes_run() {
    run_cookbook(
        "docs/diku-cookbook.md",
        "tests/cookbook-fixtures/diku",
        "midgaard/temple",
        4,
    );
}

#[test]
fn lpmud_cookbook_recipes_run() {
    run_cookbook(
        "docs/lpmud-cookbook.md",
        "tests/cookbook-fixtures/lpmud",
        "village/square",
        3,
    );
}
