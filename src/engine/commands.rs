use crate::world::{Kind, World};

pub fn do_look(world: &World, actor_ref: &str) -> String {
    let actor = match world.get(actor_ref) {
        Some(a) => a,
        None => return "You don't exist.\r\n".to_string(),
    };

    let room_ref = match &actor.location_ref {
        Some(r) => r,
        None => return "You're floating in the void.\r\n".to_string(),
    };

    format_look(world, room_ref, actor_ref)
}

pub fn format_look(world: &World, room_ref: &str, viewer_ref: &str) -> String {
    let room = match world.get(room_ref) {
        Some(r) => r,
        None => return "You see nothing.\r\n".to_string(),
    };

    let mut out = String::new();
    out.push_str(&format!("\r\n{}\r\n", room.display_name()));
    out.push_str(&format!("{}\r\n", room.description));

    let exits = world.exits_from(room_ref);
    if !exits.is_empty() {
        let exit_names: Vec<&str> = exits.iter().map(|e| e.key.as_str()).collect();
        out.push_str(&format!("[Exits: {}]\r\n", exit_names.join(", ")));
    }

    let contents: Vec<_> = world
        .objects_in(room_ref)
        .into_iter()
        .filter(|o| o.ref_id != viewer_ref)
        .collect();

    if !contents.is_empty() {
        for obj in &contents {
            let label = match obj.kind {
                Kind::Player => format!("{} is here.", obj.display_name()),
                Kind::Npc => format!("{} is here.", obj.display_name()),
                Kind::Item => format!("{} is here.", obj.display_name()),
                Kind::Room => continue,
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

    let target = match world.find_exit(&room_ref, args) {
        Some(e) => e.target_ref.clone(),
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
        let mut out = "You are carrying:\r\n".to_string();
        for obj in carrying {
            out.push_str(&format!("  {}\r\n", obj.display_name()));
        }
        out
    }
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
                    || o.display_name().to_lowercase().contains(&target_name))
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
                    || o.display_name().to_lowercase().contains(&target_name))
        })
        .map(|o| o.ref_id.clone());

    match item_ref {
        Some(ref_id) => {
            let name = world.get(&ref_id).unwrap().display_name().to_string();
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

    let target = world
        .objects_in(&room_ref)
        .into_iter()
        .chain(world.objects_in(actor_ref))
        .find(|o| {
            o.key.to_lowercase().contains(&target_name)
                || o.display_name().to_lowercase().contains(&target_name)
        });

    match target {
        Some(obj) => {
            let mut out = format!("{} ({})\r\n", obj.display_name(), obj.kind);
            if !obj.description.is_empty() {
                out.push_str(&format!("{}\r\n", obj.description));
            }
            out.push_str(&format!("Ref: {}\r\n", obj.ref_id));
            if !obj.attrs.is_empty() {
                out.push_str("Attributes:\r\n");
                for (k, v) in &obj.attrs {
                    out.push_str(&format!("  {}: {}\r\n", k, v));
                }
            }
            if !obj.tags.is_empty() {
                let tags: Vec<String> = obj.tags.iter().map(|t| t.as_spec()).collect();
                out.push_str(&format!("Tags: {}\r\n", tags.join(", ")));
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
        "  drop <item>       - Drop an item\r\n",
        "  inventory (i)     - Check what you're carrying\r\n",
        "  examine <target>  - Examine something closely\r\n",
        "  emote <action>    - Emote (or :action)\r\n",
        "  who               - See who's online\r\n",
        "  quit              - Disconnect\r\n",
        "  help (?)          - This message\r\n",
    )
    .to_string();

    if is_builder {
        out.push_str(concat!(
            "\r\nBuilder commands:\r\n",
            "  @dig <key> = <title>             - Create a new room\r\n",
            "  @open <dir> = <room_ref>         - Create an exit from here\r\n",
            "  @create <key> = <title>          - Create an item here\r\n",
            "  @destroy <ref>                   - Destroy an object\r\n",
            "  @describe [<ref> =] <text>       - Set description (default: room)\r\n",
            "  @name [<ref> =] <name>           - Rename an object (default: room)\r\n",
            "  @set <ref>/<attr> = <value>      - Set an attribute\r\n",
            "  @teleport <room_ref>             - Teleport to a room\r\n",
            "  @program <ref>/<hook> = <luau>   - Attach a Luau program to a hook\r\n",
            "  @programs [<ref>]                - List programs (default: room)\r\n",
            "  @rmprogram <ref>/<hook>          - Remove a program\r\n",
            "  @tag <ref> = <tag_spec>          - Add a tag\r\n",
            "  @untag <ref> = <tag_spec>        - Remove a tag\r\n",
            "  @script <name> = <luau>          - Create/update a global script\r\n",
            "  @scripts                        - List global scripts\r\n",
            "  @rmscript <name>                - Remove a global script\r\n",
            "  @script-interval <name> = <N>   - Set tick interval\r\n",
            "  @lock <ref>/<type> = <expr>      - Set a lock\r\n",
            "  @unlock <ref>/<type>             - Remove a lock\r\n",
            "  @locks [<ref>]                   - View locks\r\n",
        ));
    }

    if is_admin {
        out.push_str(concat!(
            "\r\nAdmin commands:\r\n",
            "  @grant <user> <scope>            - Grant a scope (player/builder/admin)\r\n",
            "  @revoke <user> <scope>           - Revoke a scope\r\n",
            "  @scopes [<user>]                 - View scopes\r\n",
            "  @wall <message>                  - Broadcast to all players\r\n",
            "  @boot <user>                     - Disconnect a player\r\n",
            "  @save                            - Save world to database\r\n",
            "  @shutdown                        - Graceful server shutdown\r\n",
        ));
    }

    out.push_str("\r\n");
    out
}
