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
    let game_dir = Path::new("../the-last-stag-mud/world");
    if !game_dir.exists() {
        eprintln!("skipping: {} not present", game_dir.display());
        return;
    }

    let mut world = World::new();
    let result = load_game_dir(game_dir, &mut world, &HashMap::new())
        .expect("game world should load");
    assert!(result.created > 0, "expected objects to be created");

    // The system "rules" object aggregates every command file + the map hooks
    // into one script. Its derived hooks must include the commands and the
    // enter/connect lifecycle hooks — and must NOT include the templated
    // functions (cmd_talk/on_leave/on_expire) that live inside string literals
    // in on_enter_map.luau.
    let rules = world
        .objects
        .values()
        .find(|o| o.key == "rules")
        .expect("rules object exists");
    let script = rules.script.as_ref().expect("rules has a script");
    for expected in ["cmd_hero", "cmd_delve", "cmd_wilderness", "on_enter", "on_connect"] {
        assert!(
            script.hooks.iter().any(|h| h == expected),
            "rules script should define {expected}; got {:?}",
            script.hooks
        );
    }
    for templated in ["cmd_talk", "on_dialog_choice", "on_leave", "on_expire"] {
        assert!(
            !script.hooks.iter().any(|h| h == templated),
            "rules script must NOT expose templated fn {templated} (it lives in a string literal); got {:?}",
            script.hooks
        );
    }

    // The barkeep's single script defines both its talk command and its
    // dialogue-choice handler.
    let barkeep = world
        .objects
        .values()
        .find(|o| o.key == "barkeep")
        .expect("barkeep exists");
    assert!(hooks::object_defines_hook(barkeep, "cmd_talk"));
    assert!(hooks::object_defines_hook(barkeep, "on_dialog_choice"));
}
