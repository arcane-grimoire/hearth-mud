---
description: Write Ink dialogue for Hearth MUD NPCs — .ink narrative scripts, dialog hooks, tag-based side effects, and the Ink Luau API. Use when creating or editing .ink files, wiring up NPC conversations, or working with the ink_* API functions.
---

# Writing Ink dialogue for Hearth MUD

Hearth MUD integrates [Ink](https://github.com/inkle/ink) (the narrative scripting language from Inkle, used in 80 Days and Heaven's Vault) via the [bladeink](https://crates.io/crates/bladeink) Rust runtime. Builders write `.ink` files or use the in-game `@dialogue` editor — changes compile and take effect immediately.

## How it works

1. An NPC gets Ink source — either a `.ink` file in the game directory or inline via `_ink_source` attr
2. A `cmd_talk` hook on the NPC calls `ink_start(actor, npc)` to begin the conversation
3. The engine compiles the Ink, runs it, and returns text + choices
4. The Luau hook renders the output and uses `prompt()` for player input
5. When the player chooses, `on_dialog_choice` fires and calls `ink_choose()`
6. Conversation state can be saved and resumed across sessions

## Ink source — two ways

### File-based (recommended for content)

Place `.ink` files in the game directory. Reference them in the hook:

```lua
function cmd_talk(this, actor, room, args)
    local dialog = require("dialog")
    dialog.start(actor, this, { file = "town/barkeep.ink" })
end
```

Files are read from disk on each `ink_start`, so edits take effect instantly — no `@reload-world` needed.

### Attribute-based (for dynamic or builder-created dialogue)

Set `_ink_source` attr on the NPC:

```
@set npc/_ink_source = === start ===\nHello, traveler!\n+ [Ask about the town] -> town\n+ [Leave] -> END\n=== town ===\nThis town has seen better days.\n-> END
```

Or use `@dialogue <ref> edit` for a multi-line editor.

## Ink language quick reference

Ink is a markup language for branching narratives. Full docs: [github.com/inkle/ink/blob/master/Documentation/WritingWithInk.md](https://github.com/inkle/ink/blob/master/Documentation/WritingWithInk.md)

### Basic flow

```ink
Hello, traveler. What brings you to these parts?
* [I'm looking for work] -> work
* [Just passing through] -> passing
* [Ask about the forest] -> forest

=== work ===
Hmm, we could use someone with your skills.
The mine to the east has been overrun with goblins.
* [I'll take the job] -> accept
* [Sounds dangerous] -> decline

=== accept ===
Good. Talk to the foreman at the mine entrance. # give:mine_key
Here's the key to the gate. # set_flag:mine_quest
-> END

=== decline ===
Your loss. The pay was good.
-> END
```

### Key Ink features

| Feature | Syntax | Use for |
|---------|--------|---------|
| **Knots** | `=== name ===` | Major story sections |
| **Stitches** | `= name` | Subsections within a knot |
| **Choices** | `* [text]` | One-time choices (consumed after picking) |
| **Sticky choices** | `+ [text]` | Reusable choices (persist) |
| **Diverts** | `-> knot` | Jump to a section |
| **Glue** | `<>` | Join lines without line break |
| **Tags** | `# tag` | Metadata passed to the game (see below) |
| **Variables** | `VAR name = value` | State within the conversation |
| **Conditionals** | `{condition: text}` | Show text based on state |
| **Alternatives** | `{~a\|b\|c}` | Cycle/shuffle through variants |
| **Tunnels** | `-> knot ->` | Call a section and return |

### Variables and game state

Ink variables live inside the conversation. Bridge them to game state with `ink_get_var` / `ink_set_var`:

```ink
VAR player_gold = 0
VAR has_key = false

{has_key: You already have the key.|}
{player_gold >= 50: I see you can afford it.|You don't have enough gold.}
```

```lua
-- Before starting, sync game state into Ink
ink_start(actor, npc)
ink_set_var(actor, npc, "player_gold", get_attr(actor, "gold") or 0)
ink_set_var(actor, npc, "has_key", has_tag(actor, "quest:mine_key"))
```

## Ink API (7 functions)

All functions take `(actor, npc)` to identify the conversation. Actor and NPC can be ref strings or object tables.

| Function | Description |
|----------|-------------|
| `ink_start(actor, npc, opts?)` | Start or resume a conversation. Source comes from `opts.file` (a `.ink` path) or the NPC's `_ink_source` attr. If `opts.resume` is true (default), resumes from saved state (`_ink_state_{player}` attr on the NPC). Returns an output table. |
| `ink_continue(actor, npc)` | Advance past text (when `can_continue` is true). Returns an output table. |
| `ink_choose(actor, npc, index)` | Make a choice by 0-based index and continue. Returns an output table. |
| `ink_get_var(actor, npc, name)` | Get an Ink variable's current value. |
| `ink_set_var(actor, npc, name, value)` | Set an Ink variable (string, number, or boolean). |
| `ink_end(actor, npc, save?)` | End the conversation. If `save` is true, persists state as `_ink_state_{player}` attr on the NPC for later resume. |
| `ink_goto(actor, npc, path)` | Jump to a named knot or stitch (e.g. `"shop"` or `"shop.weapons"`). Returns an output table. |

### Output table

Every function that returns output gives this shape:

```lua
{
    text = "Hello, traveler.\n",  -- rendered text
    choices = {                    -- available choices (empty if none)
        { index = 0, text = "Ask about the town", tags = {} },
        { index = 1, text = "Leave", tags = {} },
    },
    tags = {"quest:intro"},        -- tags on the current line
    can_continue = false,          -- true if more text follows (call ink_continue)
    ended = false,                 -- true if the story reached -> END
}
```

## Tag-based side effects

Ink tags (`# tag`) are passed through to Luau. The convention is `key:value` pairs that the dialog handler interprets:

| Tag | Effect |
|-----|--------|
| `# give:item_key` | Spawn an item and give it to the player |
| `# set_flag:name` | Add `flag:name` tag to the player |
| `# clear_flag:name` | Remove `flag:name` tag from the player |
| `# emit:message` | Send a message to the room |
| `# set_attr:key:value` | Set an attr on the player |
| `# trigger:ref:hook` | Fire a hook on another object |

Games define their own tag vocabulary — the `dialog.luau` module's `handle_tag` function is the place to add custom tags.

## Wiring up an NPC

### TOML definition

```toml
[[objects]]
key = "barkeep"
kind = "npc"
title = "Aldric the Barkeep"
description = "A heavyset man with a thick beard and kind eyes."
location = "tavern"
script = "barkeep_talk.luau"   # one script defining cmd_talk + on_dialog_choice
```

### Luau hook file

```lua
local dialog = require("dialog")

function cmd_talk(this, actor, room, args)
    dialog.start(actor, this, { file = "town/barkeep.ink" })
end

function on_dialog_choice(this, actor, room, args)
    dialog.on_choice(this, actor, room, args)
end
```

That's it — both hooks live in the NPC's one script, sharing the top-level `require("dialog")`. The `dialog` module handles rendering, choice prompting, and the conversation loop. The NPC just needs `cmd_talk` to start and `on_dialog_choice` to handle replies.

### The dialog module

The bundled `dialog.luau` module provides:

- `dialog.start(actor, npc, opts?)` — starts the conversation, renders initial output
- `dialog.render(actor, npc, result)` — renders text + numbered choices, sets up prompt
- `dialog.on_choice(this, actor, room, input)` — handles player input (number to choose, "leave" to end)
- `dialog.handle_tag(actor, npc, tag)` — processes `key:value` tags (override this for custom tags)

## Builder commands

| Command | Description |
|---------|-------------|
| `@dialogue <ref> edit` | Multi-line Ink editor — type `.` on a line by itself to finish, `@abort` to cancel |
| `@dialogue <ref> show` | Display the NPC's current Ink source |
| `@dialogue <ref> test` | Compile-check the Ink source and report errors |
| `@dialogue <ref> clear` | Remove the Ink source from the NPC |
| `@dialogue <ref> export` | Show the Ink source in a copyable format |
| `@dialog` | Alias for `@dialogue` |

## Patterns

### Branching shop

```ink
VAR player_gold = 0

Welcome to my shop!
+ [Browse weapons] -> weapons
+ [Browse armor] -> armor
+ [Leave] -> END

=== weapons ===
+ {player_gold >= 20} [Iron Sword (20g)] -> buy_sword
+ {player_gold >= 50} [Steel Sword (50g)] -> buy_steel
+ {player_gold < 20} [You can't afford anything here.]
    Come back when you have more gold.
    -> END
+ [Back] -> DONE

=== buy_sword ===
A fine choice. # give:iron_sword
That'll be 20 gold. # set_attr:gold:-20
-> END
```

### Persistent quest giver

```ink
VAR quest_started = false
VAR quest_complete = false

{quest_complete: -> thanks}
{quest_started: -> check_progress}

I have a task for you. The mine to the east is infested.
* [I'll clear it out] -> accept
* [Not interested] -> decline

=== accept ===
~ quest_started = true
Good luck. Come back when it's done. # set_flag:mine_quest
-> END

=== check_progress ===
{Have you cleared the mine yet?|}
+ [Yes, it's done] -> complete
+ [Still working on it] -> END

=== complete ===
~ quest_complete = true
Well done! Here's your reward. # give:gold_pouch
# clear_flag:mine_quest
# set_flag:mine_complete
-> END

=== thanks ===
Thanks again for clearing the mine.
-> END
```

### Conversation that remembers

Ink tracks which choices have been taken. One-time choices (`*`) disappear after being picked:

```ink
What would you like to know?
* [About the forest] The forest is dangerous at night. -> DONE
* [About the town] Founded a hundred years ago. -> DONE
+ [Goodbye] -> END
```

Second time talking, "About the forest" and "About the town" are gone if already asked. Only "Goodbye" remains (sticky `+`).

Use `ink_end(actor, npc, true)` to save this state — next `ink_start` will resume where the player left off.

## Tips

- Keep `.ink` files next to the TOML that references them, or in a `dialog/` subdirectory
- Use knots (`=== name ===`) for major conversation topics, stitches (`= name`) for subtopics
- Tags are your bridge to the game world — keep the conversation in Ink, side effects in tags
- Save state (`ink_end(actor, npc, true)`) for quest NPCs, don't save for casual chatter
- Test with `@dialogue <ref> test` before deploying — compile errors are caught early
- Ink files are re-read from disk on each `ink_start`, so edit and test without restarting
