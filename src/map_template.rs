//! Hand-designed map templates, loaded from TOML files in
//! `<game_dir>/maps/*.toml`, instantiated into rooms and exits at runtime.
//!
//! Unlike [`crate::dungeon`] (which procedurally *generates* a layout from a
//! seed + BSP tree), a map template's layout is authored by hand as an ASCII
//! grid: each character names a terrain type, and terrain types carry theme
//! and passability data. Builders can override any individual cell — give it
//! a custom title/description, link it to an existing static room instead of
//! spawning a new one, drop NPCs/items into it, or attach an encounter
//! table.
//!
//! Like `dungeon.rs`, instantiation never touches [`crate::world::World`]
//! directly — it only ever produces [`Intent`]s, which the caller (the
//! `instantiate_map` Luau API, see `softcode::api`) pushes into the same
//! batch as everything else the calling script queued, so it all
//! applies-or-fails atomically together (see ADR 0001).

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::softcode::Intent;
use crate::theme::Theme;
use crate::world::{Kind, Tag, World};

#[derive(Debug, Clone, Deserialize)]
pub struct MapTemplateFile {
    pub map: MapHeader,
    #[serde(default)]
    pub terrain: HashMap<String, TerrainDef>,
    #[serde(default)]
    pub cells: HashMap<String, CellOverride>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MapHeader {
    pub name: String,
    pub grid: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerrainDef {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub title_prefix: Option<String>,
    #[serde(default = "default_true")]
    pub passable: bool,
}

fn default_theme() -> String {
    "plains".to_string()
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct CellOverride {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// A `"<area>/<key>"` file identity of an already-existing static room
    /// (see `loader::resolve_file_key`) to use for this cell instead of
    /// spawning a new one.
    #[serde(default)]
    pub fixed_room: Option<String>,
    #[serde(default)]
    pub lock: Option<String>,
    /// Overrides the terrain's default passability for this one cell.
    #[serde(default)]
    pub passable: Option<bool>,
    #[serde(default)]
    pub objects: Vec<CellObject>,
    #[serde(default)]
    pub encounters: Vec<CellEncounter>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CellObject {
    pub key: String,
    #[serde(default = "default_npc")]
    pub kind: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

fn default_npc() -> String {
    "npc".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct CellEncounter {
    pub monster: String,
    pub count: [u32; 2],
}

/// Load every `*.toml` file under `<game_dir>/maps/`, keyed by
/// `map.name`. Missing directory or unparseable files are logged and
/// skipped rather than treated as fatal — map loading never blocks engine
/// startup, same as [`crate::theme::load_themes`].
pub fn load_map_templates(game_dir: &Path) -> HashMap<String, MapTemplateFile> {
    let mut templates = HashMap::new();
    let maps_dir = game_dir.join("maps");
    if !maps_dir.exists() {
        return templates;
    }
    if let Ok(entries) = std::fs::read_dir(&maps_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                match std::fs::read_to_string(&path) {
                    Ok(content) => match toml::from_str::<MapTemplateFile>(&content) {
                        Ok(mt) => {
                            tracing::info!(map = %mt.map.name, path = %path.display(), "loaded map template");
                            templates.insert(mt.map.name.clone(), mt);
                        }
                        Err(e) => {
                            tracing::warn!(path = %path.display(), error = %e, "failed to parse map template");
                        }
                    },
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "failed to read map template file");
                    }
                }
            }
        }
    }
    templates
}

/// A parsed grid: `cells[y][x]` is `Some(ch)` for a named terrain cell, or
/// `None` for blank/absent space. Rows may be ragged in the source TOML
/// (trailing whitespace trimmed by editors, etc.) — short rows are padded
/// with `None` out to `width`.
pub struct ParsedGrid {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<Vec<Option<char>>>,
}

impl MapTemplateFile {
    pub fn parse_grid(&self) -> ParsedGrid {
        let rows: Vec<&str> = self.map.grid.trim_matches('\n').lines().collect();
        let height = rows.len();
        let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let cells = rows
            .iter()
            .map(|row| {
                let mut row_cells: Vec<Option<char>> = row
                    .chars()
                    .map(|ch| if ch == ' ' { None } else { Some(ch) })
                    .collect();
                while row_cells.len() < width {
                    row_cells.push(None);
                }
                row_cells
            })
            .collect();
        ParsedGrid { width, height, cells }
    }
}

/// The result of instantiating a map template: every `Intent` needed to
/// bring it into being, plus the grid-coordinate -> ref lookup and a couple
/// of summary fields the `instantiate_map` Luau API surfaces directly.
pub struct InstantiateResult {
    pub intents: Vec<Intent>,
    /// Grid coordinate `(x, y)` -> the ref that cell resolved to, whether
    /// freshly spawned or an existing `fixed_room`.
    pub room_refs: HashMap<(usize, usize), String>,
    /// The first passable cell in top-left scan order (row-major: rows
    /// top-to-bottom, columns left-to-right within a row).
    pub entrance_ref: String,
    pub room_count: usize,
}

fn parse_cell_key(s: &str) -> Option<(usize, usize)> {
    let (x_str, y_str) = s.split_once(',')?;
    let x: usize = x_str.trim().parse().ok()?;
    let y: usize = y_str.trim().parse().ok()?;
    Some((x, y))
}

fn default_room_description(theme: Option<&Theme>) -> String {
    match theme.and_then(|t| t.room_descriptions.first()).and_then(|rd| rd.texts.first()) {
        Some(text) => text.clone(),
        None => "An unremarkable stretch of terrain.".to_string(),
    }
}

fn opposite(dir: &str) -> &'static str {
    match dir {
        "north" => "south",
        "south" => "north",
        "east" => "west",
        "west" => "east",
        _ => "back",
    }
}

/// Instantiate `template` into a batch of [`Intent`]s.
///
/// `world` is only used to resolve `fixed_room` cell overrides (a
/// `"<area>/<key>"` file identity, see `loader::resolve_file_key`) to the
/// dbref they currently occupy — nothing here mutates it.
///
/// `dbref_start` is the first ref id to hand out (mirrors
/// `dungeon::generate`'s contract) — the caller reserves this range and
/// advances its own counter past whatever this returns.
///
/// `anchor_ref` is unused for placement (every room either spawns with a
/// `location` pointing at the room to its west/north — see below — or is a
/// pre-existing `fixed_room`) but is required by [`Intent::Spawn`]'s
/// location invariant for the very first spawned room in the map, which has
/// no prior sibling to anchor to.
pub fn instantiate(
    template: &MapTemplateFile,
    themes: &HashMap<String, Theme>,
    world: &World,
    dbref_start: u64,
    anchor_ref: &str,
) -> Result<InstantiateResult, String> {
    let name = &template.map.name;
    let grid = template.parse_grid();

    // Pre-parse cell override keys ("x,y") into coordinates once.
    let mut overrides: HashMap<(usize, usize), &CellOverride> = HashMap::new();
    for (key, ov) in &template.cells {
        let coord = parse_cell_key(key)
            .ok_or_else(|| format!("map_template '{}': invalid cell key '{}' (want \"x,y\")", name, key))?;
        overrides.insert(coord, ov);
    }

    let mut intents = Vec::new();
    let mut next_ref = dbref_start;
    let alloc = |n: &mut u64| {
        let r = format!("#{}", *n);
        *n += 1;
        r
    };

    let mut room_refs: HashMap<(usize, usize), String> = HashMap::new();
    let map_tag = Tag { category: "map".into(), key: name.clone() };
    let mut prev_room: Option<String> = None;

    for y in 0..grid.height {
        for x in 0..grid.width {
            let Some(ch) = grid.cells[y][x] else { continue };
            let terrain_key = ch.to_string();
            let terrain = template.terrain.get(&terrain_key).ok_or_else(|| {
                format!("map_template '{}': unknown terrain '{}' at ({},{})", name, terrain_key, x, y)
            })?;
            let cell_override = overrides.get(&(x, y)).copied();

            let passable = cell_override
                .and_then(|ov| ov.passable)
                .unwrap_or(terrain.passable);
            if !passable {
                continue;
            }

            let theme = themes.get(&terrain.theme);

            let room_ref = if let Some(fixed_room) = cell_override.and_then(|ov| ov.fixed_room.as_deref()) {
                
                crate::loader::resolve_file_key(world, fixed_room).ok_or_else(|| {
                    format!(
                        "map_template '{}': fixed_room '{}' at ({},{}) not found",
                        name, fixed_room, x, y
                    )
                })?
            } else {
                let ref_id = alloc(&mut next_ref);
                let location = prev_room.clone().unwrap_or_else(|| anchor_ref.to_string());

                let title = cell_override
                    .and_then(|ov| ov.title.clone())
                    .unwrap_or_else(|| {
                        let prefix = terrain
                            .title_prefix
                            .clone()
                            .unwrap_or_else(|| terrain.theme.clone());
                        format!("{} ({},{})", prefix, x, y)
                    });
                let description = cell_override
                    .and_then(|ov| ov.description.clone())
                    .unwrap_or_else(|| default_room_description(theme));

                intents.push(Intent::Spawn {
                    ref_id: ref_id.clone(),
                    key: format!("map_{}_{}_{}", name, x, y),
                    kind: Kind::Room,
                    title: Some(title),
                    description: Some(description),
                    location,
                    owner: None,
                });

                ref_id
            };

            for (key, value) in [
                ("map_name", serde_json::json!(name)),
                ("map_x", serde_json::json!(x)),
                ("map_y", serde_json::json!(y)),
                ("terrain", serde_json::json!(terrain_key)),
            ] {
                intents.push(Intent::SetAttr {
                    target: room_ref.clone(),
                    key: key.to_string(),
                    value,
                });
            }
            intents.push(Intent::SetTag { target: room_ref.clone(), tag: map_tag.clone() });

            if let Some(expr) = cell_override.and_then(|ov| ov.lock.clone()) {
                intents.push(Intent::SetLock {
                    target: room_ref.clone(),
                    hook: "can_enter".to_string(),
                    expr,
                });
            }

            if let Some(ov) = cell_override {
                for obj in &ov.objects {
                    let kind = Kind::parse(&obj.kind).ok_or_else(|| {
                        format!(
                            "map_template '{}': unknown object kind '{}' at ({},{})",
                            name, obj.kind, x, y
                        )
                    })?;
                    intents.push(Intent::Spawn {
                        ref_id: alloc(&mut next_ref),
                        key: obj.key.clone(),
                        kind,
                        title: obj.title.clone(),
                        description: obj.description.clone(),
                        location: room_ref.clone(),
                        owner: None,
                    });
                }

                if !ov.encounters.is_empty() {
                    let entries: Vec<serde_json::Value> = ov
                        .encounters
                        .iter()
                        .map(|e| {
                            serde_json::json!({
                                "monster": e.monster,
                                "count_min": e.count[0],
                                "count_max": e.count[1],
                            })
                        })
                        .collect();
                    intents.push(Intent::SetAttr {
                        target: room_ref.clone(),
                        key: "map_encounters".into(),
                        value: serde_json::Value::Array(entries),
                    });
                }
            }

            room_refs.insert((x, y), room_ref.clone());
            prev_room = Some(room_ref);
        }
    }

    if room_refs.is_empty() {
        return Err(format!("map_template '{}': no passable cells", name));
    }

    // Exits between 4-directionally adjacent passable cells. Walking east
    // and south from every cell visits each adjacent pair exactly once,
    // while still emitting a bidirectional pair of exits per pair.
    for y in 0..grid.height {
        for x in 0..grid.width {
            let Some(room_a) = room_refs.get(&(x, y)).cloned() else { continue };

            if let Some(room_b) = room_refs.get(&(x + 1, y)).cloned() {
                push_bidirectional_exit(&mut intents, &alloc, &mut next_ref, &room_a, &room_b, "east");
            }
            if let Some(room_b) = room_refs.get(&(x, y + 1)).cloned() {
                push_bidirectional_exit(&mut intents, &alloc, &mut next_ref, &room_a, &room_b, "south");
            }
        }
    }

    // Entrance: first passable cell in top-left (row-major) scan order.
    let entrance_ref = 'entrance: {
        for y in 0..grid.height {
            for x in 0..grid.width {
                if let Some(r) = room_refs.get(&(x, y)) {
                    break 'entrance r.clone();
                }
            }
        }
        unreachable!("room_refs is non-empty, checked above");
    };

    let room_count = room_refs.len();

    Ok(InstantiateResult {
        intents,
        room_refs,
        entrance_ref,
        room_count,
    })
}

fn push_bidirectional_exit(
    intents: &mut Vec<Intent>,
    alloc: &impl Fn(&mut u64) -> String,
    next_ref: &mut u64,
    room_a: &str,
    room_b: &str,
    dir_a_to_b: &str,
) {
    intents.push(Intent::CreateExit {
        ref_id: alloc(next_ref),
        source: room_a.to_string(),
        direction: dir_a_to_b.to_string(),
        target: room_b.to_string(),
        aliases: Vec::new(),
    });
    intents.push(Intent::CreateExit {
        ref_id: alloc(next_ref),
        source: room_b.to_string(),
        direction: opposite(dir_a_to_b).to_string(),
        target: room_a.to_string(),
        aliases: Vec::new(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::RoomDescriptions;
    use crate::world::GameObject;

    fn sample_template(grid: &str) -> MapTemplateFile {
        let mut terrain = HashMap::new();
        terrain.insert(
            "f".to_string(),
            TerrainDef { theme: "forest".into(), title_prefix: Some("Forest".into()), passable: true },
        );
        terrain.insert(
            "r".to_string(),
            TerrainDef { theme: "river".into(), title_prefix: Some("River".into()), passable: false },
        );
        MapTemplateFile {
            map: MapHeader { name: "test_map".into(), grid: grid.to_string() },
            terrain,
            cells: HashMap::new(),
        }
    }

    fn sample_themes() -> HashMap<String, Theme> {
        let mut themes = HashMap::new();
        themes.insert(
            "forest".to_string(),
            Theme {
                name: "forest".into(),
                title_prefix: "Forest".into(),
                room_descriptions: vec![RoomDescriptions {
                    shape: "chamber".into(),
                    texts: vec!["Trees loom overhead.".into()],
                }],
                encounters: vec![],
                loot: vec![],
            },
        );
        themes
    }

    #[test]
    fn parse_grid_handles_spaces_and_ragged_rows() {
        let tmpl = sample_template("f.\nf");
        let grid = tmpl.parse_grid();
        assert_eq!(grid.width, 2);
        assert_eq!(grid.height, 2);
        assert_eq!(grid.cells[0], vec![Some('f'), Some('.')]);
        assert_eq!(grid.cells[1], vec![Some('f'), None]);
    }

    #[test]
    fn instantiate_spawns_rooms_and_connects_adjacent_cells() {
        let tmpl = sample_template("ff\nff");
        let themes = sample_themes();
        let world = World::new();
        let result = instantiate(&tmpl, &themes, &world, 1, "#1").expect("instantiate should succeed");

        assert_eq!(result.room_count, 4);
        assert_eq!(result.entrance_ref, "#1");

        let spawn_count = result
            .intents
            .iter()
            .filter(|i| matches!(i, Intent::Spawn { .. }))
            .count();
        assert_eq!(spawn_count, 4);

        // 4 cells in a 2x2 grid have 4 shared edges (2 horizontal, 2
        // vertical), each producing 2 CreateExit intents.
        let exit_count = result
            .intents
            .iter()
            .filter(|i| matches!(i, Intent::CreateExit { .. }))
            .count();
        assert_eq!(exit_count, 8);
    }

    #[test]
    fn impassable_terrain_is_skipped() {
        let tmpl = sample_template("frf");
        let themes = sample_themes();
        let world = World::new();
        let result = instantiate(&tmpl, &themes, &world, 1, "#1").unwrap();
        assert_eq!(result.room_count, 2);
        assert!(!result.room_refs.contains_key(&(1, 0)));
    }

    #[test]
    fn cell_override_passable_true_overrides_terrain_default() {
        let mut tmpl = sample_template("frf");
        tmpl.cells.insert(
            "1,0".to_string(),
            CellOverride {
                title: None,
                description: None,
                fixed_room: None,
                lock: None,
                passable: Some(true),
                objects: Vec::new(),
                encounters: Vec::new(),
            },
        );
        let themes = sample_themes();
        let world = World::new();
        let result = instantiate(&tmpl, &themes, &world, 1, "#1").unwrap();
        assert_eq!(result.room_count, 3);
    }

    #[test]
    fn fixed_room_resolves_to_existing_ref_and_does_not_spawn() {
        let tmpl_grid = "f";
        let mut tmpl = sample_template(tmpl_grid);
        tmpl.cells.insert(
            "0,0".to_string(),
            CellOverride {
                title: None,
                description: None,
                fixed_room: Some("town/crossroads".into()),
                lock: None,
                passable: None,
                objects: Vec::new(),
                encounters: Vec::new(),
            },
        );
        let themes = sample_themes();
        let mut world = World::new();
        let mut obj = GameObject::new("#1", "crossroads", Kind::Room).with_title("The Crossroads");
        obj.attrs.insert("_file_key".into(), serde_json::json!("town/crossroads"));
        world.add_object(obj);

        let result = instantiate(&tmpl, &themes, &world, 100, "#1").unwrap();
        assert_eq!(result.room_count, 1);
        assert_eq!(result.entrance_ref, "#1");
        assert!(!result.intents.iter().any(|i| matches!(i, Intent::Spawn { .. })));
    }

    #[test]
    fn unknown_terrain_char_is_rejected() {
        let tmpl = sample_template("fx");
        let themes = sample_themes();
        let world = World::new();
        assert!(instantiate(&tmpl, &themes, &world, 1, "#1").is_err());
    }

    #[test]
    fn invalid_cell_key_is_rejected() {
        let mut tmpl = sample_template("f");
        tmpl.cells.insert(
            "not-a-coord".to_string(),
            CellOverride {
                title: None,
                description: None,
                fixed_room: None,
                lock: None,
                passable: None,
                objects: Vec::new(),
                encounters: Vec::new(),
            },
        );
        let themes = sample_themes();
        let world = World::new();
        assert!(instantiate(&tmpl, &themes, &world, 1, "#1").is_err());
    }

    #[test]
    fn all_impassable_is_rejected() {
        let tmpl = sample_template("r");
        let themes = sample_themes();
        let world = World::new();
        assert!(instantiate(&tmpl, &themes, &world, 1, "#1").is_err());
    }

    #[test]
    fn layout_applies_cleanly_via_intent_batch() {
        let tmpl = sample_template("ff\nff");
        let themes = sample_themes();
        let mut world = World::new();
        let anchor_ref = world.next_dbref();
        world.add_object(GameObject::new(&anchor_ref, "anchor", Kind::Room).with_title("Anchor"));

        let result = instantiate(&tmpl, &themes, &world, 2, &anchor_ref).unwrap();

        let mut batch = crate::softcode::IntentBatch::default();
        for intent in result.intents {
            batch.push(intent);
        }
        crate::softcode::apply_batch(&mut world, &batch).expect("map batch should apply");

        assert!(world.get(&result.entrance_ref).is_some());
        let room_count = world
            .objects
            .values()
            .filter(|o| o.kind == Kind::Room && o.attrs.contains_key("map_name"))
            .count();
        assert_eq!(room_count, 4);
    }
}
