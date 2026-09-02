# Hearth MUD

A Rust MUD framework for building programmable, persistent virtual worlds. Not a game — a platform that games are built on.

## Language

### World model

**Object:**
The universal building block. Everything in the world — rooms, items, NPCs, players — is an Object with a kind, attributes, tags, and hooks.
_Avoid_: thing, entity, node, dbref

**Kind:**
A string label on an Object that tells the engine what role it plays (e.g. `room`, `item`, `npc`, `player`). The engine uses kind to decide behavior (rooms can contain things, items can be picked up).
_Avoid_: type, class, flag

**Attribute:**
A named key-value pair on an Object. Persisted attributes survive restarts. Transient attributes (nattrs) die on restart.
_Avoid_: property, field, stat

**Tag:**
A category:key label on an Object used for classification and search. Tags have no value — they're present or absent.
_Avoid_: label, marker, flag

**Exit:**
A one-way link between two rooms. Has its own attributes, tags, and locks.
_Avoid_: link, door, passage

**Area:**
The unit of persistence and organization. A group of rooms, objects, and exits that save and load together.
_Avoid_: zone, region

### Identity & presence

**Account:**
A persistent identity a person logs in with — its credentials and Scopes. An Account owns Characters but is not itself present in the world.
_Avoid_: user, login, profile

**Character:**
An Object of kind `player` that an Account owns and plays as. An Account may own several and plays one at a time.
_Avoid_: player, avatar, hero

**Session:**
A live connection between a person and the world, and the lifecycle it moves through — authenticate an Account, then drive one of its Characters. Distinct from the Account (which persists) and the Character (which is an Object).
_Avoid_: connection, client, socket

**Scope:**
A permission tier granted to an Account — `player`, `builder`, `admin` (and `puppeteer`) — gating which authoring and admin actions its Session may take. Admin implies the others.
_Avoid_: role, permission, grant

**Puppet:**
An Object a playing Session drives in place of its Character — e.g. an admin animating an NPC. Commands route to the Puppet until it is released.
_Avoid_: proxy, mount, avatar

### Softcode

**Program:**
A Luau script attached to an Object via a named hook. The code lives on the object.
_Avoid_: script, verb, function, handler

**Hook:**
A named slot on an Object where a Program can be attached. `can_` hooks gate permission (return true/false). `on_` hooks run after an action succeeds. `cmd_` hooks define player-typeable commands.
_Avoid_: event, trigger, callback, listener

**Actor:**
The Object on whose behalf the current action runs — the one that typed the command, fired the hook, or was made to act. Bound into every hook and Lock evaluation. Usually a Character or NPC; the Puppet when one is active. A playing Session distinguishes three identities that were once conflated (see ADR-0008): the **Character** it plays (the ownership/authoring identity, used by `@`-verbs), the **effective actor** it acts as for gameplay (the Puppet when one is driven, else the Character), and the account's **Scopes** (authorization, never the Puppet's).
_Avoid_: caller, subject, agent, user

**Intent:**
A typed mutation request. Programs cannot mutate the world directly — they enqueue Intents, and the engine validates and applies the batch atomically after the script finishes. Intents are the world's single mutation mechanism: the authoring surface (`@`-edits, REST writes) translates to Intents too, so a given mutation's semantics live in exactly one place. See ADR-0007.
_Avoid_: command, action, mutation, effect

**Ownership authority:**
The authorization model for softcode mutations: a Program runs with the `authority` of the object it is attached to, and `apply_batch` refuses any Intent whose target that authority does not own. Contrast **Authoring authority**.
_Avoid_: permission, owner check

**Authoring authority:**
The authorization model for the authoring surface: scope- and lock-gated at the transport edge (Builder/Admin Scope, `system:locked`, `system:global`), not ownership-gated — any Builder may edit any managed Object. Authorized authoring applies its Intent batch as system-trusted (`authority = None`), so `apply_batch` supplies integrity, not a second authorization. Contrast **Ownership authority**. See ADR-0007.
_Avoid_: builder permission, edit rights

**Budget:**
An instruction count limit for Program execution. Prevents runaway scripts from blocking the world tick.
_Avoid_: quota, limit, timeout

### Persistence

**Checkpoint:**
A save of the current world state to SQLite. Triggered by autosave timer, explicit save command, or graceful shutdown.
_Avoid_: snapshot, dump, backup

### Ticks

**Tick:**
The global heartbeat of the engine, fired at a fixed interval (1 second). Scripts declare how many ticks between runs.
_Avoid_: pulse, frame, cycle, heartbeat

### Locks

**Lock:**
A DSL expression on an Object or Exit that gates an action (traverse, get, drop, enter, use, look, put). Evaluated against an AccessContext.
_Avoid_: permission, guard, ACL

**AccessContext:**
The evaluation environment for a Lock: the actor, the target object, the room, the actor's account, and the actor's inventory.
_Avoid_: security context, auth context

### Command resolution

**Command Resolution:**
When a player types input, the engine searches for a matching handler in priority order: builtin commands, then `cmd_` hooks on objects in the room and player inventory. First match wins.
_Avoid_: dispatch, routing
