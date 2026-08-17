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

### Softcode

**Program:**
A Luau script attached to an Object via a named hook. The code lives on the object.
_Avoid_: script, verb, function, handler

**Hook:**
A named slot on an Object where a Program can be attached. `can_` hooks gate permission (return true/false). `on_` hooks run after an action succeeds. `cmd_` hooks define player-typeable commands.
_Avoid_: event, trigger, callback, listener

**Intent:**
A typed mutation request produced by a Program during execution. Programs cannot mutate the world directly — they enqueue Intents. The engine validates and applies the batch atomically after the script finishes.
_Avoid_: command, action, mutation, effect

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
A DSL expression on an Object or Exit that gates an action (traverse, get, drop, enter, use, look, teleport). Evaluated against an AccessContext.
_Avoid_: permission, guard, ACL

**AccessContext:**
The evaluation environment for a Lock: the actor, the target object, the room, the actor's account, and the actor's inventory.
_Avoid_: security context, auth context

### Command resolution

**Command Resolution:**
When a player types input, the engine searches for a matching handler in priority order: builtin commands, then `cmd_` hooks on objects in the room and player inventory. First match wins.
_Avoid_: dispatch, routing
