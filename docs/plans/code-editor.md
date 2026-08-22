# Code editor (`/builder/code`)

A dedicated, IDE-style workspace for authoring the game's Luau — the third lens
on the world alongside the room graph (spatial) and table (tabular). Scope for
v1: **object hooks**, which are DB-backed and versioned already. Lib modules and
Ink come later (lib modules are file-loaded today and would need the same
DB-backing dance maps got).

## Why this is mostly a frontend job

The hard backend already exists:

- **`Softcode::check_syntax(source)`** (src/softcode/mod.rs) compiles Luau
  without running it → `Ok(())` / `Err(message)`. `SetProgram` already calls it
  before saving. → the linter backend.
- **Version history** — `ProgramHistory` + `ProgramRestore` REST actions, with
  content-addressed dedupe.
- **`Eval`** runs code live as the caller's character → the test-run backend.
- A ~70-function Lua API (`emit*`, `spawn`, `get_object`, `get_attr`/`set_attr`,
  `has_tag`, `get_exits`, `after`/`trigger`, `ink_*`, `json_*`, `log`, …) plus
  object members (`.ref_id`, `.key`, `.title`, `.tags`, `.attrs`, …) → the
  autocomplete surface. Registered in src/softcode/api.rs.

## Editor library: CodeMirror 6

ESM-native (clean with Vite + Svelte 5), tree-shakeable, and its lint +
autocomplete APIs map directly onto `check_program` diagnostics and a curated
completion source. Ace works but its dynamic mode/worker loading is awkward
under Vite and its APIs are older; Monaco is VS-Code-heavy and Luau isn't
first-class. (Revisit if the user prefers Ace — the backend is editor-agnostic.)

## Engine additions

- **`CheckProgram { source } → { valid, error? }`** — wraps `check_syntax`;
  Builder-gated. *(done)*
- **`ListProgramsAll → [{ ref_id, key, title, kind, area, hooks[] }]`** — the
  explorer feed in one call; Builder-gated. *(done)*
- Reused as-is: `SetProgram`, `ListPrograms` (returns `source`),
  `ProgramHistory`, `ProgramRestore`, `Eval`.
- Later: an API-introspection endpoint if the static autocomplete list drifts.

## Frontend

- **Route**: lazy `/builder/code` (like `/builder/rooms`); entry from the
  builder-tools dropdown and an "open in code editor" jump from the room modal's
  hooks.
- **Layout**: explorer (objects by area → hooks) | CodeMirror editor | bottom
  panel (lint problems + Run output).
- **`<CodeEditor>` Svelte wrapper**: Luau highlighting (`@codemirror/legacy-
  modes/lua`), kit-ui-themed; lint via debounced `check_program` (parse the
  `…:LINE: msg` string into a diagnostic); autocomplete from the curated API
  list (functions + `this.`/`actor.` members); ⌘S = save (`set_program`).
- **History**: version dropdown (`ProgramHistory`) + diff + Restore
  (`ProgramRestore`).
- **Run (▶)**: `Eval` the buffer as the caller's character → output panel.
  Framed as a scratch runner (actor = you); "fire in real hook context" is a
  later refinement.

## Build order

1. Engine: `check_program` + `list_programs_all` (+ tests). *(done)*
2. Enumerate the API surface into an autocomplete data module.
3. CodeMirror foundation: deps + `<CodeEditor>` (highlight + lint + complete + ⌘S).
4. Workspace: explorer + editor + panel at `/builder/code`; dropdown + modal jump.
5. History/diff + Run wired to the panel.
