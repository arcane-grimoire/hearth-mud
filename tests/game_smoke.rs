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
    // The game's content + code tiers both live under `game/` (game/world/*,
    // game/std/*) — the same path `hearth.toml`'s `game_dir` names. Honour
    // HEARTH_GAME_DIR like the softcode harness does.
    let game_dir = std::env::var("HEARTH_GAME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("../the-last-stag-mud/game"));
    if !game_dir.exists() {
        // Skip when the sibling repo isn't checked out (CI) — but if it IS
        // and only the game dir is missing, that's path drift, not absence.
        // Failing here is the point: this test spent a reorganisation silently
        // skipping because it still pointed at the pre-`game/` layout.
        assert!(
            !Path::new("../the-last-stag-mud").exists(),
            "the-last-stag-mud is checked out but {} is missing — has game_dir moved?",
            game_dir.display()
        );
        eprintln!("skipping: {} not present", game_dir.display());
        return;
    }

    let mut world = World::new();
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

    // The command surface is one `system:global` object per command file, so a
    // runtime error names the real file. `cmd_combat` is the multi-file case
    // (attack + endturn share one `this`), which exercises script concatenation.
    assert!(hooks::object_defines_hook(by_key("cmd_hero"), "cmd_hero"));
    assert!(hooks::object_defines_hook(by_key("cmd_wilderness"), "cmd_wilderness"));
    assert!(hooks::object_defines_hook(by_key("cmd_combat"), "cmd_attack"));
    assert!(hooks::object_defines_hook(by_key("cmd_combat"), "cmd_endturn"));

    // `onboarding` runs the map + tutorial hooks. Its derived hooks must
    // include the lifecycle ones — and must NOT include the functions that
    // only appear inside the GUIDE_SCRIPT / TUTORIAL_SCRIPT string literals
    // it hands to `set_script`. That's the derive_hooks guard.
    let onboarding = by_key("onboarding");
    let script = onboarding.script.as_ref().expect("onboarding has a script");
    for expected in ["on_enter", "on_connect"] {
        assert!(
            script.hooks.iter().any(|h| h == expected),
            "onboarding script should define {expected}; got {:?}",
            script.hooks
        );
    }
    for templated in ["cmd_talk", "on_dialog_choice", "on_leave", "on_expire"] {
        assert!(
            !script.hooks.iter().any(|h| h == templated),
            "onboarding must NOT expose templated fn {templated} (it lives in a string literal); got {:?}",
            script.hooks
        );
    }

    // The barkeep's single script defines both its talk command and its
    // dialogue-choice handler.
    let barkeep = by_key("barkeep");
    assert!(hooks::object_defines_hook(barkeep, "cmd_talk"));
    assert!(hooks::object_defines_hook(barkeep, "on_dialog_choice"));
}
