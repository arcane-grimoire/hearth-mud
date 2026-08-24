use crate::ansi;
use crate::world::{Kind, Tag, World};

pub fn do_look(world: &World, actor_ref: &str) -> String {
    let actor = match world.get(actor_ref) {
        Some(a) => a,
        None => return "You don't exist.\r\n".to_string(),
    };

    let room_ref = match &actor.location_ref {
        Some(r) => r,
        None => return "You're floating in the void.\r\n".to_string(),
    };

    format_look(world, room_ref, actor_ref, &[])
}

pub fn format_look(
    world: &World,
    room_ref: &str,
    viewer_ref: &str,
    hidden_refs: &[String],
) -> String {
    let room = match world.get(room_ref) {
        Some(r) => r,
        None => return "You see nothing.\r\n".to_string(),
    };

    let mut out = String::new();
    out.push_str(&format!("\r\n{}\r\n", ansi::room_title(&world.display_name(room))));
    out.push_str(&format!("{}\r\n", world.resolved_description(room)));

    let exits = world.exits_from(room_ref);
    if !exits.is_empty() {
        let exit_names: Vec<&str> = exits.iter().map(|e| e.key.as_str()).collect();
        out.push_str(&format!("{}\r\n", ansi::exit_list(&exit_names)));
    }

    let offline_tag = Tag { category: "system".into(), key: "offline".into() };
    let contents: Vec<_> = world
        .objects_in(room_ref)
        .into_iter()
        .filter(|o| {
            o.ref_id != viewer_ref
                && !o.tags.contains(&offline_tag)
                && !hidden_refs.contains(&o.ref_id)
        })
        .collect();

    if !contents.is_empty() {
        for obj in &contents {
            if obj.kind == Kind::Npc && obj.tags.iter().any(|t| t.category == "troupe") {
                continue;
            }
            let label = match obj.kind {
                Kind::Player => {
                    let troupe_count = world.objects.values()
                        .filter(|o| o.tags.contains(&Tag {
                            category: "troupe".into(),
                            key: obj.ref_id.clone(),
                        }))
                        .count();
                    if troupe_count > 0 {
                        format!("{} is here, leading a troupe of {}.",
                            ansi::player_name(&world.display_name(obj)), troupe_count)
                    } else {
                        format!("{} is here.", ansi::player_name(&world.display_name(obj)))
                    }
                }
                Kind::Npc => format!("{} is here.", world.display_name(obj)),
                Kind::Item => format!("{}{}{} is here.", ansi::DIM, world.display_name(obj), ansi::RESET),
                Kind::Room | Kind::Exit | Kind::Code => continue,
            };
            out.push_str(&label);
            out.push_str("\r\n");
        }
    }

    out
}

pub fn do_go(world: &mut World, actor_ref: &str, args: &str) -> String {
    if args.is_empty() {
        return "Go where?\r\n".to_string();
    }

    let room_ref = match world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
        Some(r) => r,
        None => return "You're nowhere.\r\n".to_string(),
    };

    let target = match world
        .find_exit(&room_ref, args)
        .and_then(|e| e.target_ref.clone())
    {
        Some(t) => t,
        None => return format!("You can't go '{}'.\r\n", args),
    };

    move_player(world, actor_ref, &target)
}

pub fn move_player(world: &mut World, actor_ref: &str, target_ref: &str) -> String {
    if world.get(target_ref).is_none() {
        return "That destination doesn't exist.\r\n".to_string();
    }

    if let Some(actor) = world.get_mut(actor_ref) {
        actor.location_ref = Some(target_ref.to_string());
    }

    do_look(world, actor_ref)
}

pub fn do_inventory(world: &World, actor_ref: &str) -> String {
    let carrying: Vec<_> = world
        .objects_in(actor_ref)
        .into_iter()
        .filter(|o| o.kind == Kind::Item)
        .collect();

    if carrying.is_empty() {
        "You aren't carrying anything.\r\n".to_string()
    } else {
        let container_tag = Tag {
            category: "item".into(),
            key: "container".into(),
        };
        let mut out = "You are carrying:\r\n".to_string();
        for obj in carrying {
            out.push_str(&format!("  {}\r\n", world.display_name(obj)));
            if world.resolved_tags(obj).contains(&container_tag) {
                format_container_contents(world, &obj.ref_id, &mut out, 2);
            }
        }
        out
    }
}

fn format_container_contents(world: &World, container_ref: &str, out: &mut String, depth: usize) {
    let contents: Vec<_> = world
        .objects_in(container_ref)
        .into_iter()
        .filter(|o| o.kind == Kind::Item)
        .collect();
    if contents.is_empty() {
        return;
    }
    let indent = "  ".repeat(depth);
    let container_tag = Tag {
        category: "item".into(),
        key: "container".into(),
    };
    for obj in contents {
        out.push_str(&format!("{}  {}\r\n", indent, world.display_name(obj)));
        if world.resolved_tags(obj).contains(&container_tag) && depth < 4 {
            format_container_contents(world, &obj.ref_id, out, depth + 1);
        }
    }
}

pub fn find_item_in_inventory_or_room(
    world: &World,
    actor_ref: &str,
    room_ref: &str,
    name: &str,
) -> Option<String> {
    find_item_ref(world, actor_ref, name).or_else(|| find_item_ref(world, room_ref, name))
}

pub fn split_on_preposition<'a>(args: &'a str, prep: &str) -> Option<(&'a str, &'a str)> {
    let pattern = format!(" {} ", prep);
    let lower = args.to_lowercase();
    let idx = lower.find(&pattern)?;
    let item = args[..idx].trim();
    let container = args[idx + pattern.len()..].trim();
    if item.is_empty() || container.is_empty() {
        return None;
    }
    Some((item, container))
}

/// Find an Item located at `room_ref` whose key or display name matches
/// `name`. Shared by the engine's hook-aware `get` command and anything else
/// that needs "the item the player is probably talking about".
pub fn find_item_ref(world: &World, room_ref: &str, name: &str) -> Option<String> {
    let target_name = name.to_lowercase();
    world
        .objects_in(room_ref)
        .into_iter()
        .find(|o| {
            o.kind == Kind::Item
                && (o.key.to_lowercase().contains(&target_name)
                    || world.display_name(o).to_lowercase().contains(&target_name))
        })
        .map(|o| o.ref_id.clone())
}

pub fn do_drop(world: &mut World, actor_ref: &str, args: &str) -> String {
    if args.is_empty() {
        return "Drop what?\r\n".to_string();
    }

    let room_ref = match world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
        Some(r) => r,
        None => return "You're nowhere.\r\n".to_string(),
    };

    let target_name = args.to_lowercase();
    let item_ref = world
        .objects_in(actor_ref)
        .into_iter()
        .find(|o| {
            o.kind == Kind::Item
                && (o.key.to_lowercase().contains(&target_name)
                    || world.display_name(o).to_lowercase().contains(&target_name))
        })
        .map(|o| o.ref_id.clone());

    match item_ref {
        Some(ref_id) => {
            let name = world.display_name(world.get(&ref_id).unwrap());
            if let Some(obj) = world.get_mut(&ref_id) {
                obj.location_ref = Some(room_ref);
            }
            format!("You drop {}.\r\n", name)
        }
        None => format!("You aren't carrying '{}'.\r\n", args),
    }
}

pub fn do_examine(world: &World, actor_ref: &str, args: &str) -> String {
    if args.is_empty() {
        return "Examine what?\r\n".to_string();
    }

    let room_ref = match world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
        Some(r) => r,
        None => return "You're nowhere.\r\n".to_string(),
    };

    let target_name = args.to_lowercase();

    let obj = if target_name == "me" || target_name == "self" {
        world.get(actor_ref)
    } else {
        world
            .objects_in(&room_ref)
            .into_iter()
            .chain(world.exits_from(&room_ref))
            .chain(world.objects_in(actor_ref))
            .find(|o| {
                o.key.to_lowercase().contains(&target_name)
                    || world.display_name(o).to_lowercase().contains(&target_name)
            })
    };

    match obj {
        Some(obj) => {
            // Resolved (instance-first, then archetype chain) rather than
            // the raw fields — see docs/plans/archetypes.md. An instance
            // with no title/description of its own shows its archetype's.
            let mut out = format!("{} ({})\r\n", world.display_name(obj), obj.kind);
            let description = world.resolved_description(obj);
            if !description.is_empty() {
                out.push_str(&format!("{}\r\n", description));
            }
            out.push_str(&format!("Ref: {}\r\n", obj.ref_id));
            if let Some(archetype_ref) = &obj.archetype_ref {
                let archetype_name = world.get(archetype_ref)
                    .map(|o| world.display_name(o))
                    .unwrap_or_else(|| archetype_ref.clone());
                out.push_str(&format!("Archetype: {} ({})\r\n", archetype_name, archetype_ref));
            }
            if let Some(owner) = &obj.owner_ref {
                let owner_name = world.get(owner)
                    .map(|o| world.display_name(o))
                    .unwrap_or_else(|| owner.clone());
                out.push_str(&format!("Owner: {} ({})\r\n", owner_name, owner));
            }
            if obj.kind == Kind::Exit {
                if let Some(target_ref) = &obj.target_ref {
                    let dest_name = world.get(target_ref)
                        .map(|r| world.display_name(r))
                        .unwrap_or_else(|| target_ref.clone());
                    out.push_str(&format!("Destination: {} ({})\r\n", dest_name, target_ref));
                }
                if !obj.aliases.is_empty() {
                    let aliases: Vec<&String> = obj.aliases.iter().collect();
                    out.push_str(&format!("Aliases: {}\r\n", aliases.iter().map(|a| a.as_str()).collect::<Vec<_>>().join(", ")));
                }
            }
            if obj.kind == Kind::Player {
                let troupe: Vec<_> = world.objects.values()
                    .filter(|o| o.tags.contains(&Tag {
                        category: "troupe".into(),
                        key: obj.ref_id.clone(),
                    }))
                    .collect();
                if !troupe.is_empty() {
                    out.push_str("Troupe:\r\n");
                    for h in &troupe {
                        let class = world.resolved_attr(h, "class")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string();
                        let hp = world.resolved_attr(h, "hp")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let max_hp = world.resolved_attr(h, "max_hp")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        out.push_str(&format!("  {} [{}] {}/{} HP\r\n",
                            world.display_name(h), class, hp, max_hp));
                    }
                }
            }
            // Merged with the archetype chain (World::resolved_attrs) — an
            // instance's examine shows its own overrides plus whatever it
            // inherits. Stage 1 doesn't mark which is which (see
            // docs/plans/archetypes.md Stage 2's inspector).
            let attrs = world.resolved_attrs(obj);
            if !attrs.is_empty() {
                out.push_str("Attributes:\r\n");
                for (k, v) in &attrs {
                    out.push_str(&format!("  {}: {}\r\n", k, v));
                }
            }
            // Merged with the archetype chain, same as attrs above.
            let resolved_tags = world.resolved_tags(obj);
            if !resolved_tags.is_empty() {
                let mut tags: Vec<String> = resolved_tags.iter().map(|t| t.as_spec()).collect();
                tags.sort();
                out.push_str(&format!("Tags: {}\r\n", tags.join(", ")));
            }
            let container_tag = Tag {
                category: "item".into(),
                key: "container".into(),
            };
            if resolved_tags.contains(&container_tag) {
                let contents: Vec<_> = world
                    .objects_in(&obj.ref_id)
                    .into_iter()
                    .filter(|o| o.kind == Kind::Item)
                    .collect();
                if contents.is_empty() {
                    out.push_str("It is empty.\r\n");
                } else {
                    out.push_str("Contents:\r\n");
                    for item in contents {
                        out.push_str(&format!("  {}\r\n", world.display_name(item)));
                    }
                }
            }
            out
        }
        None => format!("You don't see '{}' here.\r\n", args),
    }
}

pub fn do_help_with_roles(is_builder: bool, is_admin: bool) -> String {
    let mut out = concat!(
        "\r\n",
        "Commands:\r\n",
        "  look (l)          - Look around\r\n",
        "  go <direction>    - Move (or just type the direction)\r\n",
        "  say <message>     - Say something\r\n",
        "  get <item>        - Pick up an item\r\n",
        "  get <item> from <container> - Get from a container\r\n",
        "  put <item> in <container>   - Put into a container\r\n",
        "  drop <item>       - Drop an item\r\n",
        "  use <target>      - Use an object\r\n",
        "  inventory (i)     - Check what you're carrying\r\n",
        "  examine <target>  - Examine something closely\r\n",
        "  whisper <who> <msg> - Whisper to a player\r\n",
        "  emote <action>    - Emote (or :action)\r\n",
        "  @password <old> <new> - Change your password\r\n",
        "  @token create|list|revoke - Manage API tokens\r\n",
        "  who               - See who's online\r\n",
        "  quit              - Disconnect\r\n",
        "  help (?)          - This message\r\n",
    )
    .to_string();

    if is_builder {
        out.push_str(concat!(
            "\r\nBuilder commands:\r\n",
            "  @dig <title>                     - Create a new room\r\n",
            "  @open <dir> = <target_ref>       - Create an exit from here\r\n",
            "  @create <title>                  - Create an item here\r\n",
            "  @destroy <ref>                   - Destroy an object\r\n",
            "  @describe [<ref> =] <text>       - Set description (default: room)\r\n",
            "  @name [<ref> =] <name>           - Rename an object (default: room)\r\n",
            "  @set <ref>/<attr> = <value>      - Set an attribute\r\n",
            "  @teleport <room_ref>             - Teleport to a room\r\n",
            "  @program <ref>/<hook> = <luau>   - Attach a Luau program to a hook\r\n",
            "                                     (leave <luau> blank for multi-line entry)\r\n",
            "  @programs [<ref>]                - List programs (default: room)\r\n",
            "  @rmprogram <ref>/<hook>          - Remove a program\r\n",
            "  @program/history <ref>/<hook>    - List a program's version history\r\n",
            "  @program/restore <ref>/<hook> <n> - Restore version <n> as a new version\r\n",
            "  @program/diff <ref>/<hook> <n> [<m>]\r\n",
            "                                   - Diff version <n> against <m> (or current)\r\n",
            "  @reload <ref>/<hook>             - Re-validate and re-enable a program\r\n",
            "  @tag <ref> = <tag_spec>          - Add a tag\r\n",
            "  @untag <ref> = <tag_spec>        - Remove a tag\r\n",
            "  @script <name> = <luau>          - Create/update a global script\r\n",
            "  @scripts                        - List global scripts\r\n",
            "  @rmscript <name>                - Remove a global script\r\n",
            "  @script-interval <name> = <N>   - Set tick interval\r\n",
            "  @lib <name> = <luau>             - Create/update a library\r\n",
            "  @libs                            - List libraries\r\n",
            "  @rmlib <name>                    - Remove a library\r\n",
            "  @dialogue <ref> [show|edit|test|clear|export]\r\n",
            "                                   - Manage ink dialogue on an object\r\n",
            "  @test [<path>]                   - Run softcode tests\r\n",
            "  @lock <ref>/<type> = <expr>      - Set a lock\r\n",
            "  @unlock <ref>/<type>             - Remove a lock\r\n",
            "  @locks [<ref>]                   - View locks\r\n",
        ));
    }

    if is_admin {
        out.push_str(concat!(
            "\r\nAdmin commands:\r\n",
            "  @chown <ref> = <player_ref>      - Change object owner\r\n",
            "  @grant <user> <scope>            - Grant a scope (player/builder/admin)\r\n",
            "  @revoke <user> <scope>           - Revoke a scope\r\n",
            "  @scopes [<user>]                 - View scopes\r\n",
            "  @wall <message>                  - Broadcast to all players\r\n",
            "  @boot <user>                     - Disconnect a player\r\n",
            "  @save                            - Save world to database\r\n",
            "  @shutdown                        - Graceful server shutdown\r\n",
            "  @eval <luau>                     - Run a one-shot Luau script (blank for editor)\r\n",
            "  @import <path> [--dry-run]       - Install a TOML+.luau bundle into the DB\r\n",
            "  @export <path>                   - Write DB-owned content back to files\r\n",
        ));
    }

    out.push_str("\r\n");
    out
}
