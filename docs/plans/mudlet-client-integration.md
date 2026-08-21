# Plan: Client map integration (GMCP / Mudlet)

## Goal

Get structured room and terrain data out of the engine and into map-aware
clients — Mudlet's built-in mapper first, then any graphical client — so a
player sees a live map as they move, colored (and eventually tiled) by
terrain. The engine already *has* the data (theme, passability, coords,
exits, and now `color` / `tile_image` / `tile_rotation` on each terrain); this
plan is about *carrying* it over a transport a client can consume.

## Current state

Two transports, very different capabilities:

- **WebSocket (`src/net/web.rs`)** — already structured. Outbound frames are
  tagged JSON: `text`, `prompt`, `room`, `commands`, `game`. The `game` frame
  is the general out-of-band channel, driven from softcode by
  `emit_data(target, channel, data)` → `Intent::EmitData` →
  `Engine::send_emit_data` (`src/engine/mod.rs`). So the web client already has
  a clean path for arbitrary structured data.
- **Telnet (`src/net/telnet.rs`)** — where Mudlet connects — is **text-only**.
  It runs IAC negotiation but *refuses every option*: on `WILL`/`DO` it replies
  `DONT`/`WONT` (see `process_bytes`). Only `ECHO`/`SGA` are asserted. There is
  **no GMCP** (telnet option 201). Mudlet gets a plain text stream today.

Terrain metadata is loaded and merged (game-level `terrain.toml` palette +
per-map overrides, see the terrain-palette work) and exposed to **softcode**
via `get_map_template`, which returns the grid, per-cell data, and a terrain
legend including `color`, `tile_image`, `tile_rotation`, and custom `attrs`.
Nothing carries that legend or per-room terrain to a *client* yet.

## Background: how Mudlet actually consumes maps

- **GMCP** (Generic MUD Communication Protocol) is telnet option **201**.
  After the server offers `IAC WILL GMCP` and the client `IAC DO GMCP`,
  messages are sent as `IAC SB 201 "<Package.Message> <json>" IAC SE`, e.g.
  `IAC SB 201 Room.Info {"num":42,"name":"Iron Peak Pass",...} IAC SE`.
- Mudlet's **built-in mapper** is driven by GMCP room packages. The de-facto
  shape most muds use is `Room.Info` with `num` (stable id), `name`, `area`,
  `environment`, `coords`, and `exits`. Mudlet places a room from `coords` +
  `exits` and colors it by `environment`.
- **Mudlet's native 2D mapper is color-per-environment, not tile-image based.**
  Each environment id carries a color; there is no per-room PNG in the stock
  mapper. So `tile_image` / `tile_rotation` do **not** feed the built-in
  Mudlet map — for Mudlet, terrain maps to an *environment color* (our new
  `color` field). Tile images are for our own graphical client (or a custom
  Mudlet Geyser/HTML overlay), not the stock mapper.

Design consequence, decided up front: the engine carries the same terrain
record to every client; each client renders to its strengths — **full tiles on
the web/custom client, environment color on Mudlet**.

## Target state

- Telnet negotiates GMCP and frames out-of-band messages.
- On login / area entry the server sends a **terrain legend** package: for
  each terrain char, `{ color, tile_image, tile_rotation, passable, title_prefix }`
  plus a stable environment id. This is the client's tileset/environment key.
- On every move the server sends a **room** package: `{ num, name, area,
  environment (terrain char), coords:{x,y}, exits }`.
- The web client keeps getting the same data through its JSON channel (it can
  already; the room/legend payloads just become two more `emit_data` channels
  or tagged frames).
- A small **Mudlet package** (client-side Lua) subscribes to the legend +
  room packages, registers environments/colors, and calls the mapper API.

## Stage 1 — GMCP negotiation in telnet (`src/net/telnet.rs`)

Extend the negotiator instead of blanket-refusing:

- Add `const GMCP: u8 = 201;`.
- On connect, additionally send `IAC WILL GMCP`.
- In `process_bytes`, when the client replies `IAC DO GMCP`, mark the session
  GMCP-enabled; on `IAC DONT GMCP`, leave it off. Keep refusing all other
  options as today.
- Add subnegotiation handling: buffer bytes between `IAC SB 201` and `IAC SE`.
  Inbound GMCP (client→server, e.g. `Core.Supports.Set`, `External.Discord`)
  can be parsed-and-ignored initially — we only need the enable handshake to
  start *sending*.
- Expose a per-session `supports_gmcp: bool` and a helper
  `send_gmcp(package: &str, json: &serde_json::Value)` that frames
  `IAC SB 201 <package> <json> IAC SE`.

Also handle `Core.Hello` / `Core.Supports.Set` politely (Mudlet sends these on
connect) — at minimum don't choke on them.

**Test:** unit-test the framing (bytes in → `WILL GMCP` offered; a `DO GMCP`
flips the flag; `send_gmcp` produces the exact `IAC SB 201 … IAC SE` frame).

## Stage 2 — a transport-agnostic client-message seam

We do **not** want two hand-maintained emitters (JSON for web, GMCP for
telnet). Introduce one server-side notion of an "out-of-band client message"
`{ package: "Room.Info", data: <json> }` and fan it out per session:

- WebSocket session → existing tagged JSON frame (reuse `emit_data`'s path;
  `package` becomes the `channel`/`type`).
- Telnet session with `supports_gmcp` → `send_gmcp(package, data)`.
- Telnet session without GMCP → drop it (text-only client, no-op).

This is the key architectural decision: **one payload, N transports.** Put it
next to `send_emit_data` in `src/engine/mod.rs`.

## Stage 3 — Room package on move

Decide the emitter's home (see Open questions):

- **Softcode-first (no engine change):** an `on_enter` / `on_look` hook reads
  the room's `terrain` + `map_x`/`map_y` attributes (already stamped by
  `instantiate`) and the legend via `get_map_template`, then
  `emit_data(player, "Room.Info", {...})`. Fastest to ship; rendering policy
  stays in the game.
- **Engine-native:** the engine sends `Room.Info` whenever a player's location
  changes. Consistent and client-agnostic, but more engine surface.

Payload: `{ num, name, area, environment, coords:{x,y}, exits:{n,s,e,w,...} }`
where `num` is the room's dbref and `environment` is the terrain char.

## Stage 4 — Terrain legend package

Send once per session at login, and again on area/map entry:
`Terrain.Legend { "<char>": { color, tile_image, tile_rotation, passable,
title_prefix, env_id }, ... }`. Source is exactly `get_map_template(...).terrain`
(already carries color + tile fields). Assign each terrain char a stable
`env_id` integer for Mudlet's `setRoomEnv` / `setCustomEnvColor`.

## Stage 5 — Mudlet client package

A small installable Mudlet package (Lua + `mfile`/`.xml`):

- On `Terrain.Legend`: for each entry, `setCustomEnvColor(env_id, r,g,b,255)`
  parsed from `color`.
- On `Room.Info`: `addRoom(num)`, `setRoomCoordinates`, `setRoomEnv(num,
  env_id)`, `setExit` per direction, `setRoomArea`, then `centerview(num)`.
- Ship it in `docs/` or a `clients/mudlet/` folder with install notes.

## Stage 6 (optional) — tile images beyond the stock mapper

For a genuinely tiled view: either our own graphical web client (renders
`tile_image` rotated by `tile_rotation`, `color` as fallback fill), or a Mudlet
Geyser/HTML overlay that draws PNGs instead of the built-in map. Out of scope
until a tiled client actually exists; the fields are already carried for it.

## Files to modify

- `src/net/telnet.rs` — GMCP negotiation + `send_gmcp` framing + `supports_gmcp`.
- `src/engine/mod.rs` — transport-agnostic client-message fan-out (Stage 2),
  and (if engine-native) the `Room.Info` on-move emitter.
- `src/net/web.rs` — map the fan-out onto the existing JSON frame (likely no
  change if we route through `emit_data`).
- `src/softcode/api.rs` — only if we expose a dedicated `send_client(package,
  data)` softcode helper (Stage 3 softcode-first path); `get_map_template`
  already carries the legend.
- `clients/mudlet/` (new) — the Mudlet package + install notes.

## Open questions

- **Emitter home** — softcode-first (ship now, policy in game) vs engine-native
  (consistent, more surface). Lean softcode-first until a second client makes
  duplication real.
- **Room `num` stability** — Mudlet keys its map on `num`. Dynamically
  instantiated rooms (wilderness) get fresh dbrefs each instantiation; a
  wandering mapper needs *stable* ids per coordinate. Likely key Mudlet rooms
  on `map_name + x,y`, not raw dbref.
- **Coordinate model** — map templates are 2D grids; the wider world graph is
  not gridded. Decide whether GMCP coords are per-map-local or global.
- **Package namespace** — reuse common names (`Room.Info`) for Mudlet-community
  familiarity, or a `Hearth.*` namespace we fully control. Reuse is friendlier
  to generic Mudlet scripts.

## Risks

- **Mudlet mapper is color-only** — tile fields won't show there; set
  expectations (tiles = web/custom client only).
- **GMCP parsing surface** — inbound subnegotiation must be robust to
  partial/oversized frames and `IAC IAC` escaping inside `SB` data; fuzz the
  buffer handling.
- **Chattiness** — a `Room.Info` per move is fine; re-sending the full legend
  per move is not. Send legend once per session/area, room per move.
- **Non-GMCP telnet clients** — must degrade to plain text with zero breakage;
  the enable handshake gates everything.
