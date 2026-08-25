# Hearth → Mudlet mapper

A tiny client-side bridge that drives Mudlet's built-in mapper from the GMCP
data a Hearth server sends. Walk around; rooms appear and connect on the map.

## How it works

Hearth's telnet listener negotiates **GMCP** (telnet option 201). When a client
enables it, the server sends a `Room.Info` package on every look/movement:

```
Room.Info { "num": "#42", "name": "The Crossroads", "area": "town",
            "environment": "road", "coords": { "x": 3, "y": 5 },
            "exits": { "north": "#43", "east": "#51" } }
```

- `num` and exit targets are Hearth dbrefs (`#N`); the bridge maps them to
  Mudlet's integer room ids (`#42` → `42`).
- `coords` is present only for rooms built from a map template; hand-authored
  rooms have none, so Mudlet lays them out from their exits.
- Exits with a known destination are linked directly; unknown ones become
  stubs that resolve as you walk through them.

Text-only clients never enable GMCP, so they're completely unaffected — the
structured data is simply never sent to them.

## Install

1. Connect Mudlet to your Hearth server (host, port **4000**). GMCP is on by
   default in Mudlet (Settings → General → Enable GMCP).
2. Scripts (📜 toolbar) → **Add Item**, name it "Hearth Mapper", paste the
   entire contents of [`hearth-mapper.lua`](./hearth-mapper.lua) into the
   editor, and click the green ✓ to save.
3. Reconnect (or just move once). Open the mapper: Toolbox → **Mapper**.

## Notes / limitations

- **Room ids are dbrefs.** Fixed rooms (town, dungeon) have stable dbrefs, so
  their map persists. Dynamically instantiated wilderness rooms get a fresh
  dbref each time they're generated, so a wandering mapper there won't be
  stable yet — keying those on `map` + `coords` is a planned follow-up.
- **Environment colors** work in mapped areas. On entering an area built from a
  map template, the server sends a `Terrain.Legend` package (once per map) with
  each terrain char's color and a stable `env_id`; the bridge registers those
  as Mudlet custom environment colors and paints each room by its
  `environment`. Hand-authored rooms (no map) have no terrain, so they use
  Mudlet's default color.
- **Tile images** (`tile_image`/`tile_rotation` in Hearth terrain) are carried
  in the legend but are for a graphical/web client, not Mudlet's built-in
  mapper, which is color-per-environment only.
