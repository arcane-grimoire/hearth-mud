//! Procedural dungeon generation.
//!
//! Converts a seed string plus a list of zone configs into a deterministic
//! layout of Room objects and Exit connections, expressed entirely as
//! [`Intent`]s so the engine's existing batch-apply machinery (see
//! [`crate::softcode::apply_to`]) validates and commits it atomically like
//! any other softcode mutation. See `docs/dungeon-generation-design.md` in
//! the game repo for the full design this implements.
//!
//! Layout comes from a small BSP (binary space partition) tree over an
//! abstract rectangle: each split alternates between a vertical cut (west/
//! east children) and a horizontal cut (north/south children). The tree's
//! leaves become rooms; the split boundaries become the corridors (Exits)
//! that connect them, guaranteeing every room in a zone is reachable from
//! every other one (the tree is a spanning tree by construction) while
//! giving each connection an unambiguous cardinal direction.
//!
//! Room *size* (which drives combat `zone_count`/`zone_width`, and whether a
//! room is tagged `dungeon:room` or `dungeon:corridor`) is rolled
//! independently of the BSP geometry from a fixed distribution — the BSP
//! tree only decides adjacency and direction, not footprint. `mapgen`
//! (added to Cargo.toml) turned out not to expose room-to-room adjacency in
//! a form usable for cardinal-direction MUD exits (its corridor filters
//! carve paths through a tile grid, not a room graph), so generation here
//! is a small from-scratch BSP instead, per the task's documented fallback.

use std::collections::HashMap;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::softcode::Intent;
use crate::theme::{EncounterEntry, Theme};
use crate::world::{Kind, Tag};

/// One zone's generation parameters. A dungeon is a sequence of zones,
/// connected linearly (last room of zone N -> first room of zone N+1).
pub struct ZoneConfig {
    pub theme_name: String,
    pub room_count_min: u32,
    pub room_count_max: u32,
    pub zone_number: u32,
}

pub struct DungeonResult {
    pub entrance_ref: String,
    pub intents: Vec<Intent>,
    pub layout: crate::grid::Grid2D,
}

/// Hash an arbitrary string seed down to a u64 via the stdlib's
/// `DefaultHasher`. Not cryptographic — just needs to be stable across runs
/// of the same build for the same input, which is all determinism within a
/// single delve requires.
fn hash_seed(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Room footprint (in abstract "tiles wide") -> combat zone shape, per
/// docs/dungeon-generation-design.md's sizing table.
fn combat_zones_for_area(area: u32) -> (u32, u32) {
    match area {
        0..=2 => (1, 1),
        3 => (1, 2),
        4..=5 => (2, 3),
        6..=8 => (3, 3),
        9..=12 => (3, 4),
        _ => (3, 6),
    }
}

/// Attached to every generated room's `on_enter` hook. Reads the encounter
/// data the generator rolled into `encounter_monsters` and starts combat —
/// same pattern as `cmd_fight.luau`.
const ON_ENTER_SOURCE: &str = r#"
function on_enter(this, actor, room)
    if not is_player(actor) then return end
    if get_attr(room, "encounter_triggered") then return end
    if not get_attr(actor, "delve_active") then return end
    if get_attr(actor, "combat_active") then return end

    local monsters_data = get_attr(room, "encounter_monsters")
    if not monsters_data or #monsters_data == 0 then return end

    set_attr(room, "encounter_triggered", true)

    local templates = {
        skeleton = { defense = 4, attack_bonus = 1, damage = 1, hp = 3, title = "Skeleton" },
        bone_archer = { defense = 3, attack_bonus = 2, damage = 2, hp = 2, title = "Bone Archer" },
        bone_knight = { defense = 6, attack_bonus = 3, damage = 2, hp = 6, title = "Bone Knight" },
        goblin = { defense = 3, attack_bonus = 1, damage = 1, hp = 2, title = "Goblin" },
        goblin_archer = { defense = 3, attack_bonus = 2, damage = 1, hp = 2, title = "Goblin Archer" },
        goblin_chief = { defense = 5, attack_bonus = 3, damage = 2, hp = 5, title = "Goblin Chief" },
    }

    local monsters = {}
    for _, entry in ipairs(monsters_data) do
        local template = templates[entry.monster]
        if template then
            for i = 1, entry.count do
                table.insert(monsters, {
                    name = template.title .. " #" .. (#monsters + 1),
                    defense = template.defense,
                    attack_bonus = template.attack_bonus,
                    damage = template.damage,
                    hp = template.hp,
                    max_hp = template.hp,
                    alive = true,
                })
            end
        end
    end

    if #monsters == 0 then return end

    local troupe = find_by_tag("troupe:" .. actor.ref_id)
    local living = {}
    for _, h in ipairs(troupe) do
        if (get_attr(h, "hp") or 0) > 0 then
            table.insert(living, h)
        end
    end
    if #living == 0 then return end

    set_attr(actor, "combat_active", true)
    set_attr(actor, "combat_round", 1)
    set_attr(actor, "combat_phase", "troupe")
    set_attr(actor, "combat_monsters", monsters)
    set_attr(actor, "combat_acted", {})
    set_attr(actor, "combat_room", actor.location_ref)
    set_attr(actor, "movement_blocked", "You can't move while in combat!")

    emit(actor, "")
    emit(actor, "Enemies emerge from the shadows!")
    emit(actor, "")
    emit(actor, "=== Round 1 — Troupe Phase ===")
    emit(actor, "Your troupe:")
    for i, h in ipairs(living) do
        local class = get_attr(h, "class") or "?"
        local hp = get_attr(h, "hp") or 0
        local max_hp = get_attr(h, "max_hp") or 0
        emit(actor, "  " .. i .. ". " .. h.display_name .. " [" .. class .. "] " .. hp .. "/" .. max_hp .. " HP")
    end
    emit(actor, "")
    emit(actor, "Enemies:")
    for i, m in ipairs(monsters) do
        emit(actor, "  " .. i .. ". " .. m.name .. " (" .. m.hp .. "/" .. m.max_hp .. " HP)")
    end
    emit(actor, "")
    emit(actor, "Use: attack <hero> <target#>  |  endturn  |  status")
end
"#;

#[derive(Clone, Copy)]
struct Rect {
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
}

/// The boundary-representative room index for each side of a BSP subtree —
/// used to connect adjacent subtrees with a single corridor at the point
/// where they actually border each other, rather than an arbitrary pair.
struct Reps {
    north: usize,
    south: usize,
    east: usize,
    west: usize,
}

/// Build a BSP layout of exactly `room_count` rooms (>= 1), returning the
/// spanning-tree edges that connect them as
/// `(room_a_index, room_b_index, direction_from_a_to_b)`.
fn bsp_layout(room_count: usize, rng: &mut StdRng) -> (Vec<(usize, usize, &'static str)>, Vec<Rect>) {
    let mut room_rects: Vec<Rect> = Vec::with_capacity(room_count);
    let mut next_idx = 0usize;
    let mut edges = Vec::new();
    let root = Rect { x1: 0, y1: 0, x2: 1000, y2: 1000 };
    build_bsp(root, room_count, 0, rng, &mut next_idx, &mut room_rects, &mut edges);
    (edges, room_rects)
}

#[allow(clippy::too_many_arguments)]
fn build_bsp(
    rect: Rect,
    budget: usize,
    depth: usize,
    rng: &mut StdRng,
    next_idx: &mut usize,
    room_rects: &mut Vec<Rect>,
    edges: &mut Vec<(usize, usize, &'static str)>,
) -> Reps {
    if budget <= 1 {
        let idx = *next_idx;
        *next_idx += 1;
        room_rects.push(rect);
        return Reps { north: idx, south: idx, east: idx, west: idx };
    }

    let left_budget = budget / 2;
    let right_budget = budget - left_budget;
    let frac: i64 = rng.gen_range(35..=65);
    let vertical = depth.is_multiple_of(2);

    if vertical {
        let span = rect.x2 - rect.x1;
        let cut = (rect.x1 + (span * frac) / 100).clamp(rect.x1 + 1, rect.x2 - 1);
        let left = build_bsp(Rect { x2: cut, ..rect }, left_budget, depth + 1, rng, next_idx, room_rects, edges);
        let right = build_bsp(Rect { x1: cut, ..rect }, right_budget, depth + 1, rng, next_idx, room_rects, edges);
        edges.push((left.east, right.west, "east"));
        let north = if room_rects[left.north].y1 <= room_rects[right.north].y1 { left.north } else { right.north };
        let south = if room_rects[left.south].y2 >= room_rects[right.south].y2 { left.south } else { right.south };
        Reps { west: left.west, east: right.east, north, south }
    } else {
        let span = rect.y2 - rect.y1;
        let cut = (rect.y1 + (span * frac) / 100).clamp(rect.y1 + 1, rect.y2 - 1);
        let top = build_bsp(Rect { y2: cut, ..rect }, left_budget, depth + 1, rng, next_idx, room_rects, edges);
        let bottom = build_bsp(Rect { y1: cut, ..rect }, right_budget, depth + 1, rng, next_idx, room_rects, edges);
        edges.push((top.south, bottom.north, "south"));
        let west = if room_rects[top.west].x1 <= room_rects[bottom.west].x1 { top.west } else { bottom.west };
        let east = if room_rects[top.east].x2 >= room_rects[bottom.east].x2 { top.east } else { bottom.east };
        Reps { north: top.north, south: bottom.south, west, east }
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

/// Roll a room's footprint: `(is_corridor, area)`. Independent of the BSP
/// geometry — see module docs.
fn roll_room_size(rng: &mut StdRng) -> (bool, u32) {
    let roll: u32 = rng.gen_range(0..100);
    match roll {
        0..=9 => (true, rng.gen_range(1..=2)),      // narrow corridor
        10..=24 => (true, 3),                        // corridor
        25..=49 => (false, rng.gen_range(4..=5)),    // small room
        50..=74 => (false, rng.gen_range(6..=8)),    // medium room
        75..=89 => (false, rng.gen_range(9..=12)),   // large chamber
        _ => (false, rng.gen_range(13..=18)),        // cavern
    }
}

fn pick_description(theme: &Theme, shape: &str, rng: &mut StdRng) -> String {
    let pool = theme
        .room_descriptions
        .iter()
        .find(|rd| rd.shape == shape)
        .or_else(|| theme.room_descriptions.first());
    match pool {
        Some(rd) if !rd.texts.is_empty() => {
            let i = rng.gen_range(0..rd.texts.len());
            rd.texts[i].clone()
        }
        _ => "An unremarkable stretch of the dungeon.".to_string(),
    }
}

fn pick_weighted_encounter<'a>(
    entries: &'a [EncounterEntry],
    rng: &mut StdRng,
) -> Option<&'a EncounterEntry> {
    let total: u32 = entries.iter().map(|e| e.weight).sum();
    if total == 0 {
        return None;
    }
    let mut roll = rng.gen_range(0..total);
    for entry in entries {
        if roll < entry.weight {
            return Some(entry);
        }
        roll -= entry.weight;
    }
    entries.last()
}

fn encounters_for_depth(theme: &Theme, depth: u32) -> Option<&[EncounterEntry]> {
    theme
        .encounters
        .iter()
        .find(|t| depth >= t.depth[0] && depth <= t.depth[1])
        .map(|t| t.entries.as_slice())
}

fn loot_for_depth(theme: &Theme, depth: u32) -> Option<&[String]> {
    theme
        .loot
        .iter()
        .find(|t| depth >= t.depth[0] && depth <= t.depth[1])
        .map(|t| t.items.as_slice())
}

/// Generate a full dungeon: every zone in `zones`, in order, connected
/// linearly. Returns the entrance room's ref plus every `Intent` needed to
/// create it — the caller (the `generate_dungeon` Luau API) pushes these
/// into the same batch as everything else the calling script queued, so it
/// all applies-or-fails atomically together.
///
/// `dbref_start` is the first ref id to hand out (`"#{dbref_start}"`,
/// `"#{dbref_start + 1}"`, ...) — the caller is responsible for reserving
/// this range and advancing its own counter past whatever this returns.
///
/// `anchor_ref` must be a room that already exists in the world (typically
/// wherever the delving player currently stands). `Intent::Spawn` requires
/// its `location` to already exist, and a freshly generated dungeon has no
/// such object yet for its very first room — every other room instead
/// anchors to the room created immediately before it, which by then exists
/// within the same batch (intents apply in order — see
/// `softcode::apply_to`). `location_ref` carries no gameplay meaning for a
/// Room (navigation is exit-based, and `Kind::Room` is filtered out of
/// room-contents listings everywhere) — this is purely bookkeeping to
/// satisfy Spawn's invariant.
pub fn generate(
    seed: &str,
    zones: &[ZoneConfig],
    themes: &HashMap<String, Theme>,
    dbref_start: u64,
    anchor_ref: &str,
) -> Result<DungeonResult, String> {
    if zones.is_empty() {
        return Err("generate_dungeon: at least one zone is required".into());
    }
    for zone in zones {
        if !themes.contains_key(&zone.theme_name) {
            return Err(format!("generate_dungeon: unknown theme '{}'", zone.theme_name));
        }
        if zone.room_count_min == 0 {
            return Err("generate_dungeon: room_count_min must be at least 1".into());
        }
    }

    let mut intents = Vec::new();
    let mut next_ref = dbref_start;
    let alloc = |n: &mut u64| {
        let r = format!("#{}", *n);
        *n += 1;
        r
    };

    let mut entrance_ref: Option<String> = None;
    let mut prev_zone_last_room: Option<String> = None;
    let zone_count_total = zones.len();
    let mut room_positions: Vec<(String, i64, i64)> = Vec::new();

    for (zi, zone) in zones.iter().enumerate() {
        let theme = &themes[&zone.theme_name];
        let is_first_zone = zi == 0;
        let is_last_zone = zi == zone_count_total - 1;

        let zone_seed = hash_seed(&format!("{}{}", seed, zone.zone_number));
        let mut rng = StdRng::seed_from_u64(zone_seed);

        let room_count = (if zone.room_count_min >= zone.room_count_max {
            zone.room_count_min
        } else {
            rng.gen_range(zone.room_count_min..=zone.room_count_max)
        }) as usize;

        let (edges, room_rects) = bsp_layout(room_count, &mut rng);

        let room_refs: Vec<String> = (0..room_count).map(|_| alloc(&mut next_ref)).collect();
        let zone_entrance = room_refs[0].clone();
        let zone_last = room_refs[room_count - 1].clone();

        if is_first_zone {
            entrance_ref = Some(zone_entrance.clone());
        }

        for (i, room_ref) in room_refs.iter().enumerate() {
            let location = if i == 0 {
                prev_zone_last_room.clone().unwrap_or_else(|| anchor_ref.to_string())
            } else {
                room_refs[i - 1].clone()
            };

            let depth = (i + 1) as u32; // 1-based within the zone
            let is_global_entrance = is_first_zone && i == 0;
            let is_zone_exit = i == room_count - 1;
            let is_global_boss = is_last_zone && is_zone_exit;

            let (is_corridor, area) = roll_room_size(&mut rng);
            let (zone_count, zone_width) = combat_zones_for_area(area);
            let shape = if is_corridor { "corridor" } else { "chamber" };
            let text = pick_description(theme, shape, &mut rng);
            let noun = if is_corridor { "Passage" } else { "Chamber" };
            let title = format!("{} — {} {}", theme.title_prefix, noun, depth);

            intents.push(Intent::Spawn {
                ref_id: room_ref.clone(),
                key: format!("dungeon_z{}_r{}", zone.zone_number, depth),
                kind: Kind::Room,
                title: Some(title),
                description: Some(text),
                location,
                owner: None,
            });

            for (key, value) in [
                ("zone_count", serde_json::json!(zone_count)),
                ("zone_width", serde_json::json!(zone_width)),
                ("dungeon_seed", serde_json::json!(seed)),
                ("dungeon_zone", serde_json::json!(zone.zone_number)),
                ("dungeon_depth", serde_json::json!(depth)),
            ] {
                intents.push(Intent::SetAttr {
                    target: room_ref.clone(),
                    key: key.to_string(),
                    value,
                });
            }

            intents.push(Intent::SetTag {
                target: room_ref.clone(),
                tag: Tag {
                    category: "dungeon".into(),
                    key: if is_corridor { "corridor".into() } else { "room".into() },
                },
            });
            if is_global_entrance {
                intents.push(Intent::SetTag {
                    target: room_ref.clone(),
                    tag: Tag { category: "dungeon".into(), key: "entrance".into() },
                });
            }
            if is_zone_exit {
                intents.push(Intent::SetTag {
                    target: room_ref.clone(),
                    tag: Tag { category: "dungeon".into(), key: "exit".into() },
                });
            }
            if is_global_boss {
                intents.push(Intent::SetTag {
                    target: room_ref.clone(),
                    tag: Tag { category: "dungeon".into(), key: "boss".into() },
                });
            }

            // Encounter roll: skip the dungeon's very first room so players
            // aren't ambushed on arrival; the boss room always fights.
            let roll_encounter = if is_global_entrance {
                false
            } else if is_global_boss {
                true
            } else {
                rng.gen_range(0..100u32) < 60
            };
            if roll_encounter {
                if let Some(entries) = encounters_for_depth(theme, depth) {
                    if let Some(entry) = pick_weighted_encounter(entries, &mut rng) {
                        let count = if entry.count[0] >= entry.count[1] {
                            entry.count[0]
                        } else {
                            rng.gen_range(entry.count[0]..=entry.count[1])
                        };
                        intents.push(Intent::SetAttr {
                            target: room_ref.clone(),
                            key: "encounter_monsters".into(),
                            value: serde_json::json!([{ "monster": entry.monster, "count": count }]),
                        });
                    }
                }
            }

            // Loot roll — skip the entrance, favor non-corridors.
            if !is_global_entrance && !is_corridor && rng.gen_range(0..100u32) < 25 {
                if let Some(items) = loot_for_depth(theme, depth) {
                    if !items.is_empty() {
                        let pick = &items[rng.gen_range(0..items.len())];
                        intents.push(Intent::SetTag {
                            target: room_ref.clone(),
                            tag: Tag { category: "dungeon".into(), key: "loot".into() },
                        });
                        intents.push(Intent::SetAttr {
                            target: room_ref.clone(),
                            key: "loot_items".into(),
                            value: serde_json::json!([pick]),
                        });
                    }
                }
            }

            intents.push(Intent::SetProgram {
                target: room_ref.clone(),
                hook: "on_enter".into(),
                source: ON_ENTER_SOURCE.to_string(),
            });
        }

        // Track room positions for the layout grid.
        let zone_y_offset = zi as i64 * 1100;
        for (i, rect) in room_rects.iter().enumerate() {
            let cx = (rect.x1 + rect.x2) / 2;
            let cy = (rect.y1 + rect.y2) / 2 + zone_y_offset;
            room_positions.push((room_refs[i].clone(), cx, cy));
        }

        // Corridors within the zone, from the BSP spanning tree.
        for (a, b, dir) in edges {
            intents.push(Intent::CreateExit {
                ref_id: alloc(&mut next_ref),
                source: room_refs[a].clone(),
                direction: dir.to_string(),
                target: room_refs[b].clone(),
                aliases: Vec::new(),
            });
            intents.push(Intent::CreateExit {
                ref_id: alloc(&mut next_ref),
                source: room_refs[b].clone(),
                direction: opposite(dir).to_string(),
                target: room_refs[a].clone(),
                aliases: Vec::new(),
            });
        }

        // Connect from the previous zone's last room into this zone's
        // entrance.
        if let Some(prev_last) = prev_zone_last_room.take() {
            intents.push(Intent::CreateExit {
                ref_id: alloc(&mut next_ref),
                source: prev_last.clone(),
                direction: "deeper".into(),
                target: zone_entrance.clone(),
                aliases: Vec::new(),
            });
            intents.push(Intent::CreateExit {
                ref_id: alloc(&mut next_ref),
                source: zone_entrance,
                direction: "back".into(),
                target: prev_last,
                aliases: Vec::new(),
            });
        }

        prev_zone_last_room = Some(zone_last);
    }

    // Exit from dungeon entrance back to the overworld.
    if let Some(ref ent) = entrance_ref {
        intents.push(Intent::CreateExit {
            ref_id: alloc(&mut next_ref),
            source: ent.clone(),
            direction: "out".into(),
            target: anchor_ref.to_string(),
            aliases: vec!["leave".into(), "exit".into()],
        });
    }

    // Build a layout grid from room positions.
    let layout = build_layout_grid(&room_positions);

    Ok(DungeonResult {
        entrance_ref: entrance_ref.ok_or_else(|| "generate_dungeon: no rooms generated".to_string())?,
        intents,
        layout,
    })
}

fn build_layout_grid(positions: &[(String, i64, i64)]) -> crate::grid::Grid2D {
    if positions.is_empty() {
        return crate::grid::Grid2D::new(1, 1, serde_json::Value::Null);
    }

    let min_x = positions.iter().map(|(_, x, _)| *x).min().unwrap();
    let max_x = positions.iter().map(|(_, x, _)| *x).max().unwrap();
    let min_y = positions.iter().map(|(_, _, y)| *y).min().unwrap();
    let max_y = positions.iter().map(|(_, _, y)| *y).max().unwrap();

    let span_x = (max_x - min_x).max(1);
    let span_y = (max_y - min_y).max(1);

    let grid_w = (positions.len() as f64).sqrt().ceil().max(3.0) as usize * 2 + 1;
    let grid_h = ((positions.len() as f64 * span_y as f64 / span_x as f64).sqrt().ceil().max(3.0) as usize) * 2 + 1;

    let mut grid = crate::grid::Grid2D::new(grid_w, grid_h, serde_json::Value::Null);

    for (ref_id, px, py) in positions {
        let gx = (((*px - min_x) as f64 / span_x as f64) * (grid_w - 1) as f64).round() as usize + 1;
        let gy = (((*py - min_y) as f64 / span_y as f64) * (grid_h - 1) as f64).round() as usize + 1;
        grid.set_cell(gx, gy, serde_json::json!(ref_id));
    }

    grid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::softcode::{apply_batch, IntentBatch};
    use crate::theme::{EncounterTable, LootTable, RoomDescriptions};
    use crate::world::{GameObject, World};

    fn sample_theme() -> Theme {
        Theme {
            name: "test".into(),
            title_prefix: "The Test Halls of".into(),
            room_descriptions: vec![
                RoomDescriptions { shape: "chamber".into(), texts: vec!["A chamber.".into()] },
                RoomDescriptions { shape: "corridor".into(), texts: vec!["A corridor.".into()] },
            ],
            encounters: vec![EncounterTable {
                depth: [1, 20],
                entries: vec![
                    EncounterEntry { monster: "skeleton".into(), count: [1, 2], weight: 3 },
                    EncounterEntry { monster: "goblin".into(), count: [1, 1], weight: 1 },
                ],
            }],
            loot: vec![LootTable { depth: [1, 20], items: vec!["bone_charm".into()] }],
        }
    }

    fn sample_themes() -> HashMap<String, Theme> {
        let mut m = HashMap::new();
        m.insert("test".to_string(), sample_theme());
        m
    }

    fn sample_zones() -> Vec<ZoneConfig> {
        vec![
            ZoneConfig { theme_name: "test".into(), room_count_min: 3, room_count_max: 5, zone_number: 1 },
            ZoneConfig { theme_name: "test".into(), room_count_min: 2, room_count_max: 3, zone_number: 2 },
        ]
    }

    #[test]
    fn generation_is_deterministic_for_same_seed() {
        let themes = sample_themes();
        let zones = sample_zones();
        let a = generate("crypt-2026", &zones, &themes, 1, "#1").unwrap();
        let b = generate("crypt-2026", &zones, &themes, 1, "#1").unwrap();
        assert_eq!(a.entrance_ref, b.entrance_ref);
        // Intent doesn't derive PartialEq (kept minimal on purpose); Debug
        // output is deterministic and good enough to compare full layouts.
        assert_eq!(format!("{:?}", a.intents), format!("{:?}", b.intents));
    }

    #[test]
    fn different_seeds_diverge() {
        let themes = sample_themes();
        let zones = sample_zones();
        let a = generate("seed-one", &zones, &themes, 1, "#1").unwrap();
        let b = generate("seed-two", &zones, &themes, 1, "#1").unwrap();
        assert_ne!(format!("{:?}", a.intents), format!("{:?}", b.intents));
    }

    #[test]
    fn unknown_theme_is_rejected() {
        let themes = sample_themes();
        let zones = vec![ZoneConfig {
            theme_name: "nonexistent".into(),
            room_count_min: 2,
            room_count_max: 3,
            zone_number: 1,
        }];
        assert!(generate("seed", &zones, &themes, 1, "#1").is_err());
    }

    #[test]
    fn layout_applies_cleanly_and_is_fully_connected() {
        let themes = sample_themes();
        let zones = sample_zones();
        let result = generate("crypt-2026", &zones, &themes, 2, "#1").unwrap();

        let mut world = World::new();
        let anchor_ref = world.next_dbref(); // "#1"
        world.add_object(GameObject::new(&anchor_ref, "anchor", Kind::Room).with_title("Anchor"));

        let mut batch = IntentBatch::default();
        for intent in result.intents {
            batch.push(intent);
        }
        apply_batch(&mut world, &batch).expect("dungeon batch should apply");

        assert!(world.get(&result.entrance_ref).is_some());

        let dungeon_room_count = world
            .objects
            .values()
            .filter(|o| o.kind == Kind::Room && o.attrs.contains_key("dungeon_seed"))
            .count();
        // room_count_min sums to 5, room_count_max sums to 8 across the two
        // zones.
        assert!((5..=8).contains(&dungeon_room_count), "got {dungeon_room_count} rooms");

        // BFS from the entrance through the generated Exit objects — every
        // dungeon room must be reachable (the BSP tree is a spanning tree,
        // and zones are chained). The anchor room IS reachable via the
        // "out" exit on the entrance.
        let mut seen = std::collections::HashSet::new();
        let mut queue = vec![result.entrance_ref.clone()];
        seen.insert(result.entrance_ref.clone());
        while let Some(room) = queue.pop() {
            for exit in world.exits_from(&room) {
                if let Some(target) = exit.target_ref.clone() {
                    if seen.insert(target.clone()) {
                        queue.push(target);
                    }
                }
            }
        }

        assert!(seen.contains(&anchor_ref), "anchor should be reachable via the out exit");
        assert_eq!(
            seen.len(),
            dungeon_room_count + 1, // +1 for the anchor room reached via "out"
            "every generated room + anchor should be reachable from the entrance"
        );

        // The boss tag lands on exactly one room: the last room of the
        // last zone.
        let boss_tag = Tag { category: "dungeon".into(), key: "boss".into() };
        let boss_count = world.objects.values().filter(|o| o.tags.contains(&boss_tag)).count();
        assert_eq!(boss_count, 1);

        let entrance_tag = Tag { category: "dungeon".into(), key: "entrance".into() };
        assert!(world.get(&result.entrance_ref).unwrap().tags.contains(&entrance_tag));
    }
}
