# Hearth MUD — VS Code extension

Edit Hearth softcode (Luau object scripts + library modules) directly against a
running Hearth server, over its REST API. No file access, no separate sync step:
opening a program fetches it with `get_script`, saving writes it back with
`set_script` (after a `check_program` lint), exactly like the web builder IDE.

Because it writes through the **API/DB** — not `game_dir` files — edits land in
the database, which is the source of truth at runtime. (Editing files on disk
would be silently reverted on the next deploy; see the project's data-boundary
rule.)

## Setup

```sh
cd clients/vscode
npm install
npm run compile      # or: npm run watch
```

Then press **F5** in VS Code (from this folder) to launch an Extension
Development Host, or package with `npx vsce package`.

### Configure

- `hearth.serverUrl` — default `http://localhost:8000` (the web/API port).
- `hearth.token` — an API token. Mint one in-game:

  ```
  @token create my-editor
  ```

  (type it in the telnet client or the web command input), then paste the
  printed token into the setting.

## Use

- The **Hearth MUD** view in the activity bar lists every object that carries a
  program, grouped by area, with each object's version (`v3`) and any edit-lock
  holder (`🔒 name`). Click an object to open its script; expand it for lib
  modules.
- **Save** (⌘S) updates your **local working copy only** — it never publishes.
- **Publish** (⌘⌥P, or the cloud icon in the editor title) pushes to the server
  with your base version. If someone else published since you opened it, the
  server 3-way-merges; on a real conflict you get *Open Diff (server ↔ mine)* to
  reconcile, then Publish again.
- **Version History** (the history icon, or right-click an object) lists every
  version — diff any against current, view it, or **revert** (which re-applies it
  as a new version).
- **Claim / Release Edit Lock** (right-click an object) takes a 30-minute,
  renewed-on-publish lock so a teammate's write is refused while you hold it.
  This is distinct from `system:locked` (the file-authoritative `std/*` tier,
  shown with a lock icon and always read-only).
- **Run Tests** runs an object's co-located `test_*` functions and prints to the
  *Hearth MUD* output channel.

## Scope

Focused on the softcode edit → publish loop with history, merge, and locks. Not
yet wired: object/attr creation, the map/terrain builder, ink dialogue,
`eval`/REPL, or a live `/ws` game console — all supported by the same REST
surface (see `ApiRequest` in `src/engine/mod.rs`). Good next steps.

For Luau autocomplete, point the Luau LSP at `../../types/hearth.d.luau` — but
note it only analyses `file:` documents, so it won't see the `hearth:` virtual
docs this extension opens (see `docs/plans/softcode-versioning.md` for the
history of that decision).
