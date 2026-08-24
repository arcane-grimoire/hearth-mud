# Object scripts — one script per object, hooks as methods

> **Status: implemented.** Cut over directly (nothing had shipped, so no
> migration/compat window). Per-hook programs are gone; each object has one
> `ObjectScript` (hooks as top-level functions in a shared scope) plus a
> separate `libs` map for `require`able modules. Program version history was
> dropped. Hook detection uses the `full_moon` Luau parser (`derive_hooks`),
> which correctly ignores functions inside string literals — a text scanner
> could not. A dormant `archetype_ref` field is reserved for the follow-on
> MOO-style prototype/instance work (`docs/plans/archetypes.md`). What follows
> is the original plan.


## Goal

Make **the object**, not the individual hook, the unit of softcode. One Luau
script per object, run once, defining its hooks as methods that share a single
top-level scope — a "class body" in the Godot sense. This gives an object's
hooks a real place for shared helpers, constants, and state, and gives the
builder a coherent "one file per object" view without lying about scope.

Today softcode is authored, stored, versioned, and edited per *hook*. The
object is only an incidental bag of unrelated programs. This plan flips the
unit to the object while keeping the engine's per-hook *dispatch* intact.

## Why now / the tell

The current model already leaks toward this. `world/town/town.toml` points
*both* `cmd_talk` and `on_dialog_choice` at the same `barkeep_talk.luau` — the
author wanted one file of shared behavior and the format made them register it
twice. The moment you ask "where do an object's shared helpers/globals live?"
the per-hook model has no answer: each hook compiles in its own env, so a
helper defined at the top of one hook is invisible to the others.

## Current state (how a hook fires today)

A `ProgramRecord` (`src/softcode/hooks.rs:126`) holds one hook's `source`,
`enabled`, and per-hook persistent `state`. Objects carry
`programs: HashMap<String, ProgramRecord>` (`src/world/object.rs:72`),
serialized whole into `objects.programs_json`. Version history and the
`@import` baseline are keyed per `(obj_ref, hook)`
(`program_versions`, `import_hashes` in `src/db.rs`).

Crucially, `run_hook` (`src/softcode/mod.rs:1214`) already treats a program's
source as a chunk that *defines* a function, not as the hook body:

```rust
let compiled = self.get_or_compile(&program.source, &program.hook)?;
compiled.set_environment(env.clone())?;
compiled.call::<()>(())?;                              // run the chunk body
let func: Option<mlua::Function> = env.get(program.hook.as_str())?;  // then pull the hook fn out
// ... call func with (this, actor, ...)
```

So the runtime is already 90% shaped for object-scripts. Everything above
`function on_get(...)` in a source is a private prelude the callback closes
over; the env is fresh each fire, so top-level `local`s are recomputed and
persistence goes through the `state` API / attrs. The *only* gap is that each
hook has its **own source and its own env**, so preludes can't be shared.

## The model (Godot mapping)

| Godot | Hearth object script |
| ----- | -------------------- |
| Node with one attached script | `GameObject` with one script |
| Script = a class (`extends Node`) | Script = a chunk defining hook fns |
| `_ready` / `_process` / `_input` | `on_create`/`on_startup`, `on_tick`, hooks |
| Class body: `var`, `const`, `func helper()` | Top-level scope shared by all hooks = **globals** |
| `var health = 10` (persists for node lifetime) | per-object `state` (persisted) |
| `signal` / `emit_signal` | existing `signal.luau` — the cross-object path |
| `@export var` surfaced in inspector | attr-schema declaration → builder Properties form |

Example — the barkeep as one script:

```lua
-- object "barkeep"
local GREETINGS = { "Evening.", "What'll it be?" }
local function is_regular(actor) return get_attr(actor, "visits") > 5 end

function cmd_talk(this, actor)                    -- sees GREETINGS, is_regular
  ...
end

function on_dialog_choice(this, actor, choice)   -- same shared scope
  ...
end
```

The engine compiles the whole script into one env, runs the body once, then
looks up whichever hook is firing (`env.get(hook)`) — the lookup already
exists.

### Deliberately NOT copying from Godot

- **Script inheritance / `extends`.** Objects are flat `GameObject`s with tags,
  not a class hierarchy. Reuse stays tags + `require`d libs — no fragile base
  class.
- **`_process` every frame.** We tick at 1s over a large world; `on_tick`
  stays opt-in and rare. Don't encourage per-tick logic the way Godot
  encourages `_process`.
- **Scene/script (data/behavior) split.** We deliberately fused them —
  everything is one `GameObject`. Don't reintroduce the split.

## What changes

### 1. Runtime (`src/softcode/mod.rs`)

`run_hook` takes the object's **whole** script (not one hook's source),
compiles it once into `env`, runs the body, then `env.get(hook_being_fired)`.
Bytecode cache keys on the object's script hash instead of per-hook source.
`on_tick`/`on_create` become methods looked up the same way.

Open question: the chunk body re-runs on every fire (fresh env). For an object
with an expensive prelude that's now paid once per fire instead of once per
hook — same order, but worth measuring. If it bites, cache the post-body env
per (object, script-hash) between fires within a tick.

### 2. Storage / `ProgramRecord` (`src/softcode/hooks.rs`, `src/world/object.rs`)

Replace `programs: HashMap<hook, ProgramRecord>` with a single object script
plus metadata:

```rust
pub struct ObjectScript {
    pub source: String,                                 // the whole class body
    pub state: HashMap<String, serde_json::Value>,      // per-object now, not per-hook
    pub origin: ProgramOrigin,                          // File | InGame, unchanged
    pub hooks: Vec<String>,                             // fns defined, for dispatch index
}
```

`hooks` is derived at set time (parse/introspect which `function <name>` the
script defines) so the engine knows which hooks an object responds to without
running it. `enabled` per-hook is dropped (see trade-offs).

`objects.programs_json` becomes `script_json` (or the map degenerates to a
single well-known key during migration — see below).

### 3. Persistent state — per-hook → per-object

Today `state` is a bucket per `ProgramRecord`. It becomes one bucket per
object (Godot member vars belong to the node). The `state` API surface is
unchanged from the script author's view; only the keying moves.

### 4. History / import baseline (`src/db.rs`)

`program_versions` and `import_hashes` re-key from `(obj_ref, hook)` to
`(obj_ref)`. History coarsens to the whole script — you version the object's
behavior as a unit (like Godot versions a `.gd` file). Diffs still show which
function changed; per-hook blame is lost.

### 5. Loader / on-disk TOML format (`src/loader.rs`)

Replace the `[*.programs]` hook→file table with a single script reference:

```toml
[objects.barkeep]
script = "barkeep.luau"     # one file, defines cmd_talk, on_dialog_choice, ...
```

`install_programs` still only reconciles `ProgramOrigin::File` records, so
in-game edits (which flip origin to `InGame`) survive reloads unchanged. The
per-file blake3 hash-skip logic is unchanged, just one file per object.

### 6. Builder (`web/src/components/builder/`)

`HooksPanel` becomes an object-script editor: one CodeMirror buffer, foldable
by defined function, "add hook" inserts a `function <hook>(...)` stub with the
right signature. `BuilderTree` shows one script node per object instead of N
hook leaves. The REST actions collapse: `set_program`/`remove_program` →
`set_script`, `list_programs` → returns the one script + its derived hook list.

### 7. Editor tie-in — declared attrs (later)

Once object-scripts land, a script can declare the attrs it expects
(Godot's `@export`), which the builder Properties panel renders as a typed
form. This dovetails with `docs/plans/attribute-schema.md`.

## Trade-offs (need explicit sign-off)

- **Per-hook `enabled` → gone.** Godot can't disable a single method. Model as
  commenting out, or an object-level enable. Low value in practice.
- **Per-hook version history → per-object.** Coarser blame; simpler mental
  model and store. This is the main thing to confirm before building.
- **The "bag of unrelated `cmd_*`" object** (e.g. `system` with `cmd_hero`,
  `cmd_troupe`, `cmd_fight`, ...): under this model that's one script with many
  methods — normal in Godot. If it genuinely sprawls, the answer is the same as
  Godot's: split into multiple objects/nodes.

## Migration

Existing objects have N per-hook `ProgramRecord`s. Migrate by concatenating
their sources into one script (each hook's source already defines a
`function <hook>` — they compose), merging the per-hook `state` buckets into
one per-object bucket (namespacing on key collision, logged), and preserving
`origin` (any `InGame` hook makes the merged script `InGame`). One-shot DB
migration in `src/db.rs`, gated like the existing `scripts`→`Kind::Code`
migration. On-disk TOMLs (`[*.programs]`) are rewritten to `script = "..."` by
a `hearth`-side codemod, or the loader accepts both forms during a deprecation
window.

## Staging

1. Runtime: `run_hook` compiles a whole-object script; keep per-hook storage,
   concatenate at fire time behind a flag. Proves the shared-scope semantics
   with zero storage/migration risk.
2. Storage + migration: `ObjectScript`, DB migration, history re-keying.
3. Loader/TOML: `script = "..."` form, codemod the game files.
4. Builder: object-script editor, tree collapse, REST action collapse.
5. Declared attrs → Properties form (ties into attribute-schema plan).
