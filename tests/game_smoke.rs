//! Smoke test: the real game world (The Last Stag) loads under the
//! one-script-per-object model and derives the hooks we expect. Skips silently
//! if the game dir isn't present (e.g. CI without the sibling repo).

use std::collections::HashMap;
use std::path::Path;

use hearth_mud::loader::load_game_dir;
use hearth_mud::softcode::hooks;
use hearth_mud::world::World;

#[test]
fn last_stag_world_loads_and_derives_hooks() {
    // The game's content lives under `world/` — the same path `hearth.toml`'s
    // `game_dir` names. Honour HEARTH_GAME_DIR like the softcode harness does.
    let game_dir = std::env::var("HEARTH_GAME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("../the-last-stag-mud/world"));
    if !game_dir.exists() {
        // Skip when the sibling repo isn't checked out (CI) — but if it IS
        // and only the game dir is missing, that's path drift, not absence.
        // Failing here is the point: this test spent a reorganisation silently
        // skipping because it pointed at a stale layout.
        assert!(
            !Path::new("../the-last-stag-mud").exists(),
            "the-last-stag-mud is checked out but {} is missing — has game_dir moved?",
            game_dir.display()
        );
        eprintln!("skipping: {} not present", game_dir.display());
        return;
    }

    let mut world = World::new();
    // `load_game_dir` returns `Err` on an unresolved reference, so a clean load
    // also proves every cross-area exit (e.g. forest→town, dungeon→forest)
    // resolves — the guard for the `<area>/<key>` ref convention.
    let result = load_game_dir(&game_dir, &mut world, &HashMap::new())
        .expect("game world should load");
    assert!(result.created > 0, "expected objects to be created");

    let by_key = |key: &str| {
        world
            .objects
            .values()
            .find(|o| o.key == key)
            .unwrap_or_else(|| panic!("{key} object exists"))
    };

    // The rooms the cross-area exits point at must all be present.
    for room in ["crossroads", "edge", "clearing", "depths", "entrance"] {
        by_key(room);
    }

    // The command surface is one `system:global` object per command file, so a
    // runtime error names the real file. Each object's script must define its
    // matching `cmd_<name>` hook.
    for cmd in [
        "cmd_hero",
        "cmd_troupe",
        "cmd_fight",
        "cmd_attack",
        "cmd_endturn",
        "cmd_status",
        "cmd_name",
    ] {
        assert!(
            hooks::object_defines_hook(by_key(cmd), cmd),
            "{cmd} object should define the {cmd} hook",
        );
    }
}
