use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::softcode::hooks::{self, ProgramOrigin};
use crate::world::{GameObject, Kind, Tag, World};

fn hash_content(content: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug)]
pub struct LoadResult {
    pub key_map: HashMap<String, String>,
    pub created: u32,
    pub updated: u32,
    pub skipped: u32,
    pub file_hashes: HashMap<PathBuf, u64>,
    pub changed_files: Vec<String>,
    /// Every `(obj_ref, hook, source)` a file program was actually written
    /// to during this load (excludes hooks skipped because an in-game edit
    /// shadows the file version — see `install_programs`). The caller (the
    /// `Engine`, which owns the `Database` handle) records each of these as
    /// a program version — see docs/plans/program-authoring.md Stage 3's
    /// "Which writes get versioned": the loader's file installs are an
    /// authoring path too, giving a recorded baseline for a future
    /// package-vs-local comparison. Content-addressed dedupe means
    /// repeated boot installs of unchanged files cost nothing beyond a
    /// lookup.
    pub installed_programs: Vec<(String, String, String)>,
}

const MANAGED_TAG: Tag = Tag {
    category: String::new(),
    key: String::new(),
};

fn managed_tag() -> Tag {
    Tag {
        category: "system".to_string(),
        key: "managed".to_string(),
    }
}

/// Attr key used to remember an object's `"<area>/<key>"` file identity, so
/// reloads can find the same managed object again without relying on a
/// stable ref_id (dbrefs are assigned once, at first creation).
///
/// `pub(crate)`: `@import`/`@export` (`src/import_export.rs`, see
/// docs/plans/program-authoring.md Stage 4) reuse this exact identity
/// mechanism rather than inventing a second one — "keep that mechanism and
/// drop only the ownership and reconcile semantics layered on top of it."
pub(crate) const FILE_KEY_ATTR: &str = "_file_key";

/// `pub(crate)` (with `Serialize` added below): `@export` builds these same
/// structs from live DB objects and serializes them back to TOML, so import
/// and export share one format definition by construction rather than two
/// that could drift apart — see docs/plans/program-authoring.md Stage 4.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct AreaFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) area: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) rooms: Vec<RoomDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) objects: Vec<ObjectDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) exits: Vec<ExitDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) scripts: Vec<ScriptDef>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct RoomDef {
    pub(crate) key: String,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tags: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) locks: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) programs: std::collections::HashMap<String, ProgramSource>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ObjectDef {
    pub(crate) key: String,
    #[serde(default = "default_kind")]
    pub(crate) kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) location: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tags: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) attrs: std::collections::HashMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) locks: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) programs: std::collections::HashMap<String, ProgramSource>,
}

fn default_kind() -> String {
    "item".into()
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ExitDef {
    pub(crate) from: String,
    pub(crate) direction: String,
    pub(crate) to: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) locks: std::collections::HashMap<String, String>,
}

/// A global tick script defined in an area file. Always runs at the
/// `on_tick` hook — see `install_programs` call site below, where every
/// `ScriptDef` becomes a `Kind::Code` object's `on_tick` Program.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ScriptDef {
    pub(crate) name: String,
    #[serde(default = "default_interval")]
    pub(crate) interval: u64,
    #[serde(flatten)]
    pub(crate) source: ProgramSource,
}

fn default_interval() -> u64 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum ProgramSource {
    File { file: String },
    Inline { source: String },
}

impl ProgramSource {
    pub(crate) fn resolve(&self, base_dir: &Path) -> Result<String, String> {
        match self {
            ProgramSource::File { file } => {
                let path = base_dir.join(file);
                std::fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read {}: {}", path.display(), e))
            }
            ProgramSource::Inline { source } => Ok(source.clone()),
        }
    }
}

pub(crate) struct ParsedArea {
    pub(crate) area_name: String,
    pub(crate) base_dir: PathBuf,
    pub(crate) file: AreaFile,
}

/// Parse every `.toml` area file under `dir` (recursively, same file
/// discovery `load_game_dir` uses) into `ParsedArea`s, with no world
/// interaction and no file-hash skip logic — always a fresh parse of
/// everything found. Shared by `@import`/`@export`
/// (`src/import_export.rs`), which read the same TOML+`.luau` format
/// `load_game_dir` does but apply it under different semantics (see
/// docs/plans/program-authoring.md Stage 4) — this keeps both readers
/// backed by the exact same struct definitions rather than two formats that
/// could drift.
pub(crate) fn parse_area_dir(dir: &Path) -> Result<Vec<ParsedArea>, String> {
    if !dir.exists() {
        return Err(format!("Directory not found: {}", dir.display()));
    }
    let mut area_files: Vec<PathBuf> = Vec::new();
    collect_toml_files(dir, &mut area_files);
    area_files.sort();

    let mut parsed = Vec::new();
    for path in &area_files {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let area_file: AreaFile = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
        let area_name = area_file.area.clone().unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        let base_dir = path.parent().unwrap_or(dir).to_path_buf();
        parsed.push(ParsedArea {
            area_name,
            base_dir,
            file: area_file,
        });
    }
    Ok(parsed)
}

/// Load or reload game content from files.
///
/// - New objects are created, assigned a fresh dbref, and tagged
///   `system:managed`.
/// - Existing managed objects get their description, title, programs,
///   tags, locks, and attrs updated from files — their dbref never changes.
/// - Existing non-managed objects (player-created) are never touched.
///
/// Returns a map of `"<area>/<key>"` file keys to the dbrefs they resolved
/// to, so callers (e.g. spawn-room resolution) can look up file-defined
/// objects by their human-readable key.
pub fn load_game_dir(
    game_dir: &Path,
    world: &mut World,
    prev_hashes: &HashMap<PathBuf, u64>,
) -> Result<LoadResult, String> {
    if !game_dir.exists() {
        return Err(format!("Game directory not found: {}", game_dir.display()));
    }

    let mut area_files: Vec<PathBuf> = Vec::new();
    collect_toml_files(game_dir, &mut area_files);
    area_files.sort();

    if area_files.is_empty() {
        tracing::info!(?game_dir, "No TOML files found in game directory");
        return Ok(LoadResult {
            key_map: HashMap::new(),
            created: 0,
            updated: 0,
            skipped: 0,
            file_hashes: HashMap::new(),
            changed_files: Vec::new(),
            installed_programs: Vec::new(),
        });
    }

    let mut new_hashes: HashMap<PathBuf, u64> = HashMap::new();
    let mut changed_files: Vec<String> = Vec::new();

    // Parse every area file up front — later passes need to see all of them
    // together to resolve cross-file/cross-area references.
    let mut parsed: Vec<ParsedArea> = Vec::new();
    let mut skipped_files: Vec<PathBuf> = Vec::new();
    for path in &area_files {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let hash = hash_content(&contents);
        new_hashes.insert(path.clone(), hash);

        if prev_hashes.get(path) == Some(&hash) {
            skipped_files.push(path.clone());
            continue;
        }

        let relative = path.strip_prefix(game_dir).unwrap_or(path);
        changed_files.push(relative.display().to_string());

        let area_file: AreaFile = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
        let area_name = area_file.area.clone().unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        let base_dir = path.parent().unwrap_or(game_dir).to_path_buf();
        parsed.push(ParsedArea {
            area_name,
            base_dir,
            file: area_file,
        });
    }

    let skipped = skipped_files.len() as u32;

    let managed = managed_tag();
    let mut created = 0u32;
    let mut updated = 0u32;
    let mut installed_programs: Vec<(String, String, String)> = Vec::new();

    // -- Pass 0: seed the key map from already-loaded managed objects, so
    //    reload doesn't hand out new dbrefs for content that already has one.
    let mut key_map: HashMap<String, String> = HashMap::new();
    for obj in world.objects.values() {
        if obj.tags.contains(&managed)
            && let Some(fk) = obj.attrs.get(FILE_KEY_ATTR).and_then(|v| v.as_str()) {
                key_map.insert(fk.to_string(), obj.ref_id.clone());
            }
    }

    // -- Pass 1: assign dbrefs to every room and object across all files
    //    first, so pass 2 can resolve forward references (including
    //    cross-area ones) no matter what order files/entries appear in.
    for area in &parsed {
        for room in &area.file.rooms {
            let file_key = format!("{}/{}", area.area_name, room.key);
            key_map.entry(file_key).or_insert_with(|| world.next_dbref());
        }
        for object in &area.file.objects {
            let file_key = format!("{}/{}", area.area_name, object.key);
            key_map.entry(file_key).or_insert_with(|| world.next_dbref());
        }
    }

    // -- Pass 2: create/update rooms and objects, now that every key
    //    resolves to a dbref.
    for area in &parsed {
        let area_name = &area.area_name;
        let base_dir = &area.base_dir;

        for room in &area.file.rooms {
            let file_key = format!("{}/{}", area_name, room.key);
            let ref_id = key_map
                .get(&file_key)
                .cloned()
                .expect("dbref assigned in pass 1");
            if let Some(existing) = world.get_mut(&ref_id) {
                if existing.tags.contains(&managed) {
                    existing.title = Some(room.title.clone());
                    existing.description = room.description.clone();
                    existing.locks = room.locks.clone();
                    sync_managed_tags(existing, &room.tags);
                    installed_programs.extend(install_programs(existing, &room.programs, base_dir)?);
                    updated += 1;
                }
                continue;
            }
            let mut obj = GameObject::new(&ref_id, &room.key, Kind::Room).with_title(&room.title);
            obj.description = room.description.clone();
            obj.tags.insert(managed.clone());
            for tag_spec in &room.tags {
                if let Ok(tag) = Tag::parse(tag_spec) {
                    obj.tags.insert(tag);
                }
            }
            obj.locks = room.locks.clone();
            obj.attrs.insert(FILE_KEY_ATTR.into(), serde_json::json!(file_key));
            installed_programs.extend(install_programs(&mut obj, &room.programs, base_dir)?);
            world.add_object(obj);
            created += 1;
        }

        for object in &area.file.objects {
            let kind = Kind::parse(&object.kind).ok_or_else(|| {
                format!("Unknown kind '{}' in area '{}'", object.kind, area_name)
            })?;
            let file_key = format!("{}/{}", area_name, object.key);
            let ref_id = key_map
                .get(&file_key)
                .cloned()
                .expect("dbref assigned in pass 1");
            let location_ref = match &object.location {
                Some(loc) => Some(resolve_key(area_name, loc, &key_map)?),
                None => None,
            };
            if let Some(existing) = world.get_mut(&ref_id) {
                if existing.tags.contains(&managed) {
                    if let Some(title) = &object.title {
                        existing.title = Some(title.clone());
                    }
                    existing.description = object.description.clone();
                    if let Some(loc) = &location_ref {
                        existing.location_ref = Some(loc.clone());
                    }
                    existing.attrs.extend(object.attrs.clone());
                    existing.locks = object.locks.clone();
                    sync_managed_tags(existing, &object.tags);
                    installed_programs.extend(install_programs(existing, &object.programs, base_dir)?);
                    updated += 1;
                }
                continue;
            }
            let mut obj = GameObject::new(&ref_id, &object.key, kind);
            if let Some(title) = &object.title {
                obj = obj.with_title(title);
            }
            obj.description = object.description.clone();
            if let Some(loc) = &location_ref {
                obj = obj.with_location(loc.clone());
            }
            obj.tags.insert(managed.clone());
            for tag_spec in &object.tags {
                if let Ok(tag) = Tag::parse(tag_spec) {
                    obj.tags.insert(tag);
                }
            }
            obj.attrs = object.attrs.clone();
            obj.attrs.insert(FILE_KEY_ATTR.into(), serde_json::json!(file_key));
            obj.locks = object.locks.clone();
            installed_programs.extend(install_programs(&mut obj, &object.programs, base_dir)?);
            world.add_object(obj);
            created += 1;
        }
    }

    // -- Pass 3: exits. These reference rooms/objects by key, never the
    //    other way around, so they can wait until everything else exists.
    for area in &parsed {
        let area_name = &area.area_name;

        for exit in &area.file.exits {
            let from = resolve_key(area_name, &exit.from, &key_map)?;
            let to = resolve_key(area_name, &exit.to, &key_map)?;
            let file_key = format!("{}/exit/{}/{}", area_name, exit.from, exit.direction);

            if let Some(existing_ref) = key_map.get(&file_key).cloned()
                && let Some(existing) = world.get_mut(&existing_ref) {
                    if existing.tags.contains(&managed) {
                        existing.location_ref = Some(from.clone());
                        existing.target_ref = Some(to.clone());
                        existing.aliases = exit.aliases.iter().cloned().collect();
                        existing.locks = exit.locks.clone();
                        updated += 1;
                    }
                    continue;
                }
                // key_map had a stale entry (object was deleted) — fall
                // through and recreate it with a fresh dbref.

            let ref_id = world.next_dbref();
            key_map.insert(file_key.clone(), ref_id.clone());
            let mut obj = GameObject::new(&ref_id, &exit.direction, Kind::Exit)
                .with_location(&from)
                .with_target(&to);
            obj.aliases = exit.aliases.iter().cloned().collect();
            obj.locks = exit.locks.clone();
            obj.tags.insert(managed.clone());
            obj.attrs.insert(FILE_KEY_ATTR.into(), serde_json::json!(file_key));
            world.add_object(obj);
            created += 1;
        }

        // A file-defined global script is a `Kind::Code` object carrying an
        // `on_tick` Program plus a `tick_interval` attr — the same shape
        // `@script` produces at runtime (see docs/plans/program-authoring.md
        // Stage 2). Identified and reconciled the same way rooms/objects/
        // exits are: a `FILE_KEY_ATTR` identity plus `system:managed`, so
        // reload updates it in place and an in-game edit to its Program
        // survives (`install_programs` only touches `ProgramOrigin::File`
        // programs).
        for script in &area.file.scripts {
            let file_key = format!("{}/script/{}", area_name, script.name);
            let source = ProgramSource::Inline {
                source: script.source.resolve(&area.base_dir)?,
            };
            let mut programs = std::collections::HashMap::new();
            programs.insert("on_tick".to_string(), source);

            if let Some(existing_ref) = key_map.get(&file_key).cloned()
                && let Some(existing) = world.get_mut(&existing_ref) {
                    if existing.tags.contains(&managed) {
                        existing.attrs.insert(
                            "tick_interval".into(),
                            serde_json::json!(script.interval),
                        );
                        installed_programs.extend(install_programs(existing, &programs, &area.base_dir)?);
                        updated += 1;
                    }
                    continue;
                }

            let ref_id = world.next_dbref();
            key_map.insert(file_key.clone(), ref_id.clone());
            let mut obj = GameObject::new(&ref_id, &script.name, Kind::Code);
            obj.attrs.insert("tick_interval".into(), serde_json::json!(script.interval));
            obj.tags.insert(managed.clone());
            obj.attrs.insert(FILE_KEY_ATTR.into(), serde_json::json!(file_key));
            installed_programs.extend(install_programs(&mut obj, &programs, &area.base_dir)?);
            world.add_object(obj);
            created += 1;
        }
    }

    // Also hash .luau files referenced by programs in changed areas
    for area in &parsed {
        for programs in area.file.rooms.iter().flat_map(|r| std::iter::once(&r.programs))
            .chain(area.file.objects.iter().map(|o| &o.programs))
        {
            for source in programs.values() {
                if let ProgramSource::File { file } = source {
                    let path = area.base_dir.join(file);
                    if path.exists()
                        && let Ok(contents) = std::fs::read_to_string(&path) {
                            let hash = hash_content(&contents);
                            if prev_hashes.get(&path) != Some(&hash) {
                                let relative = path.strip_prefix(game_dir).unwrap_or(&path);
                                let name = relative.display().to_string();
                                if !changed_files.contains(&name) {
                                    changed_files.push(name);
                                }
                            }
                            new_hashes.insert(path, hash);
                        }
                }
            }
        }
    }

    tracing::info!(
        ?game_dir,
        created,
        updated,
        skipped,
        "Game content loaded from files"
    );

    Ok(LoadResult {
        key_map,
        created,
        updated,
        skipped,
        file_hashes: new_hashes,
        changed_files,
        installed_programs,
    })
}

fn sync_managed_tags(obj: &mut GameObject, file_tags: &[String]) {
    let managed = managed_tag();
    for tag_spec in file_tags {
        if let Ok(tag) = Tag::parse(tag_spec) {
            obj.tags.insert(tag);
        }
    }
    obj.tags.insert(managed);
}

/// Reconcile the file-owned Programs on `obj` against `programs`.
///
/// Only [`ProgramOrigin::File`] programs are touched. A Program written
/// in-game is database-owned: if the files don't name its hook it is a
/// builder's addition rather than a stale file program, and if they do it is
/// an override that shadows the file version until the builder removes it.
/// Reconciling those away would destroy them on every `@reload-world` — and,
/// because startup loads with no previous file hashes, on every restart.
/// Returns every `(obj_ref, hook, source)` this call actually wrote to a
/// File-origin Program — the caller threads these into `LoadResult::installed_programs`
/// so the `Engine` can record a version for each (see docs/plans/program-authoring.md
/// Stage 3). Hooks skipped because an in-game edit shadows the file version
/// are not included — they aren't a write.
fn install_programs(
    obj: &mut GameObject,
    programs: &std::collections::HashMap<String, ProgramSource>,
    base_dir: &Path,
) -> Result<Vec<(String, String, String)>, String> {
    let stale: Vec<String> = obj
        .programs
        .iter()
        .filter(|(hook, record)| {
            record.origin == ProgramOrigin::File && !programs.contains_key(hook.as_str())
        })
        .map(|(hook, _)| hook.clone())
        .collect();
    for hook in stale {
        hooks::remove_program(obj, &hook);
    }
    let mut installed = Vec::new();
    for (hook, source) in programs {
        if obj
            .programs
            .get(hook)
            .is_some_and(|record| record.origin == ProgramOrigin::InGame)
        {
            continue;
        }
        let code = source.resolve(base_dir)?;
        hooks::set_program_with_origin(obj, hook, code.clone(), ProgramOrigin::File)?;
        installed.push((obj.ref_id.clone(), hook.clone(), code));
    }
    Ok(installed)
}

/// Resolve a `"<area>/<key>"` file identity to the dbref the managed object
/// carrying it currently occupies, by scanning for the `_file_key` attr the
/// loader stamps onto every managed object (see [`FILE_KEY_ATTR`]).
///
/// Used by `map_template`'s `fixed_room` cell override, which links a map
/// grid cell to an existing statically-defined room rather than spawning a
/// new one. Unlike `resolve_key`, this always takes a fully-qualified
/// `"<area>/<key>"` string — there's no "current area" to resolve a bare key
/// against outside of area-file loading.
pub fn resolve_file_key(world: &World, file_key: &str) -> Option<String> {
    world
        .objects
        .values()
        .find(|o| o.attrs.get(FILE_KEY_ATTR).and_then(|v| v.as_str()) == Some(file_key))
        .map(|o| o.ref_id.clone())
}

/// Resolve a file-level reference (a bare key like `"crossroads"`, or a
/// cross-area key like `"forest/edge"`) to the dbref it was assigned in
/// pass 1/2.
pub(crate) fn resolve_key(area: &str, reference: &str, key_map: &HashMap<String, String>) -> Result<String, String> {
    let file_key = if reference.contains('/') {
        reference.to_string()
    } else {
        format!("{}/{}", area, reference)
    };
    key_map.get(&file_key).cloned()
        .ok_or_else(|| format!("Unresolved reference: '{}'", file_key))
}

/// Built-in stdlib modules, embedded at compile time.
const STDLIB_MODULES: &[(&str, &str)] = &[
    ("str", include_str!("../lib/str.luau")),
    ("collections", include_str!("../lib/collections.luau")),
    ("random", include_str!("../lib/random.luau")),
    ("signal", include_str!("../lib/signal.luau")),
    ("state_machine", include_str!("../lib/state_machine.luau")),
    ("text", include_str!("../lib/text.luau")),
    ("Grid3D", include_str!("../lib/Grid3D.luau")),
    ("grids", include_str!("../lib/grids.luau")),
];

/// Load lib modules: embedded stdlib first, then `<game_dir>/lib/` which
/// can override any stdlib module by name.
pub fn load_modules(game_dir: &Path) -> HashMap<String, String> {
    let mut modules: HashMap<String, String> = STDLIB_MODULES
        .iter()
        .map(|(name, src)| (name.to_string(), src.to_string()))
        .collect();

    let lib_dir = game_dir.join("lib");
    let entries = match std::fs::read_dir(&lib_dir) {
        Ok(e) => e,
        Err(_) => {
            tracing::info!(count = modules.len(), "Loaded lib modules (stdlib only)");
            return modules;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("luau")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if stem.ends_with(".test") {
                    continue;
                }
                if let Ok(source) = std::fs::read_to_string(&path) {
                    modules.insert(stem.to_string(), source);
                }
            }
    }
    tracing::info!(count = modules.len(), "Loaded lib modules");
    modules
}

/// Recursively scan `game_dir` for `.ink` files. Returns a map of
/// relative path (without extension) to source code.
pub fn load_ink_files(game_dir: &Path) -> HashMap<String, String> {
    let mut files = HashMap::new();
    collect_ink_files(game_dir, game_dir, &mut files);
    if !files.is_empty() {
        tracing::info!(count = files.len(), "Loaded ink files");
    }
    files
}

fn collect_ink_files(base: &Path, dir: &Path, out: &mut HashMap<String, String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_ink_files(base, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("ink") {
            if let Ok(source) = std::fs::read_to_string(&path) {
                let relative = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(relative, source);
            }
        }
    }
}

pub(crate) fn collect_toml_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_toml_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("toml")
            && path.file_name().and_then(|f| f.to_str()) != Some("hearth.toml")
        {
            out.push(path);
        }
    }
}

pub struct TestFile {
    pub path: std::path::PathBuf,
    pub relative: String,
    pub source: String,
    pub is_lib: bool,
}

pub fn discover_test_files(game_dir: &Path) -> Vec<TestFile> {
    let mut files = Vec::new();
    collect_test_files(game_dir, game_dir, &mut files);
    files.sort_by(|a, b| a.relative.cmp(&b.relative));
    files
}

fn collect_test_files(base: &Path, dir: &Path, out: &mut Vec<TestFile>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_test_files(base, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("luau")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && stem.ends_with(".test")
                    && let Ok(source) = std::fs::read_to_string(&path) {
                        let relative = path
                            .strip_prefix(base)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .to_string();
                        let is_lib = path.starts_with(base.join("lib"));
                        out.push(TestFile {
                            path,
                            relative,
                            source,
                            is_lib,
                        });
                    }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A scratch game_dir under the target directory, cleaned up on drop.
    struct TempGameDir {
        path: PathBuf,
    }

    impl TempGameDir {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("hearth-loader-test-{}-{}", std::process::id(), n));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn write_area(&self, subdir: &str, filename: &str, contents: &str) {
            let dir = self.path.join(subdir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(filename), contents).unwrap();
        }
    }

    impl Drop for TempGameDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn assigns_sequential_dbrefs_and_resolves_same_area_exits() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"

                [[rooms]]
                key = "crossroads"
                title = "The Crossroads"

                [[rooms]]
                key = "square"
                title = "Town Square"

                [[exits]]
                from = "crossroads"
                direction = "north"
                to = "square"
                aliases = ["n"]
            "#,
        );

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).expect("load should succeed").key_map;

        let crossroads_ref = key_map.get("town/crossroads").expect("crossroads in key_map");
        let square_ref = key_map.get("town/square").expect("square in key_map");
        assert_ne!(crossroads_ref, square_ref);

        let crossroads = world.get(crossroads_ref).unwrap();
        assert_eq!(crossroads.kind, Kind::Room);
        assert_eq!(crossroads.title.as_deref(), Some("The Crossroads"));

        let exits = world.exits_from(crossroads_ref);
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].target_ref.as_deref(), Some(square_ref.as_str()));
        assert!(exits[0].aliases.contains("n"));
    }

    #[test]
    fn resolves_new_style_cross_area_refs() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"
                [[rooms]]
                key = "crossroads"
                title = "The Crossroads"
                [[exits]]
                from = "crossroads"
                direction = "south"
                to = "forest/edge"
            "#,
        );
        dir.write_area(
            "forest",
            "forest.toml",
            r#"
                area = "forest"
                [[rooms]]
                key = "edge"
                title = "Forest Edge"
            "#,
        );

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).expect("load should succeed").key_map;

        let crossroads_ref = key_map.get("town/crossroads").unwrap();
        let edge_ref = key_map.get("forest/edge").unwrap();
        let exits = world.exits_from(crossroads_ref);
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].target_ref.as_deref(), Some(edge_ref.as_str()));
    }

    #[test]
    fn reload_is_idempotent_and_preserves_dbrefs() {
        let dir = TempGameDir::new();
        let source = r#"
            area = "town"
            [[rooms]]
            key = "crossroads"
            title = "The Crossroads"
            [[objects]]
            key = "sign"
            kind = "item"
            title = "a sign"
            location = "crossroads"
        "#;
        dir.write_area("town", "town.toml", source);

        let mut world = World::new();
        let key_map1 = load_game_dir(&dir.path, &mut world, &HashMap::new()).expect("first load").key_map;
        let next_id_after_first = world.next_id;
        let object_count_after_first = world.objects.len();

        let key_map2 = load_game_dir(&dir.path, &mut world, &HashMap::new()).expect("reload").key_map;

        assert_eq!(world.next_id, next_id_after_first, "reload must not mint new dbrefs");
        assert_eq!(world.objects.len(), object_count_after_first, "reload must not duplicate objects");
        assert_eq!(key_map1.get("town/crossroads"), key_map2.get("town/crossroads"));
        assert_eq!(key_map1.get("town/sign"), key_map2.get("town/sign"));
    }

    #[test]
    fn reload_updates_managed_object_fields_without_touching_dbref() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"
                [[rooms]]
                key = "crossroads"
                title = "The Crossroads"
                description = "Old description."
            "#,
        );

        let mut world = World::new();
        let key_map1 = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let ref_id = key_map1.get("town/crossroads").unwrap().clone();

        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"
                [[rooms]]
                key = "crossroads"
                title = "The Crossroads"
                description = "New description."
            "#,
        );
        let key_map2 = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;

        assert_eq!(key_map2.get("town/crossroads"), Some(&ref_id));
        assert_eq!(world.get(&ref_id).unwrap().description, "New description.");
    }

    /// A room with one inline program, so tests can vary just the parts they
    /// care about.
    fn area_with_program(description: &str, programs: &str) -> String {
        format!(
            r#"
                area = "town"
                [[rooms]]
                key = "crossroads"
                title = "The Crossroads"
                description = "{description}"
                {programs}
            "#
        )
    }

    /// The regression that motivated [`ProgramOrigin`]: a builder's in-game
    /// program on a managed object used to be reconciled away by any load that
    /// touched that object — including every startup, which loads with no
    /// previous file hashes.
    #[test]
    fn reload_preserves_in_game_programs_on_managed_objects() {
        let dir = TempGameDir::new();
        dir.write_area("town", "town.toml", &area_with_program("Old.", ""));

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let ref_id = key_map.get("town/crossroads").unwrap().clone();

        let obj = world.get_mut(&ref_id).unwrap();
        hooks::set_program(obj, "cmd_wave", "function cmd_wave() end".into()).unwrap();

        dir.write_area("town", "town.toml", &area_with_program("New.", ""));
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let obj = world.get(&ref_id).unwrap();
        assert_eq!(obj.description, "New.", "file-owned fields still reconcile");
        let program = obj
            .programs
            .get("cmd_wave")
            .expect("in-game program must survive a reload of its object");
        assert_eq!(program.origin, ProgramOrigin::InGame);
    }

    /// The other half of the contract — dropping a program from the files must
    /// still remove it, or the fix would just leak stale programs forever.
    #[test]
    fn reload_removes_file_programs_dropped_from_the_files() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            &area_with_program(
                "Old.",
                r#"[rooms.programs.on_enter]
                   source = "function on_enter() end""#,
            ),
        );

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let ref_id = key_map.get("town/crossroads").unwrap().clone();
        assert_eq!(
            world.get(&ref_id).unwrap().programs.get("on_enter").unwrap().origin,
            ProgramOrigin::File,
        );

        dir.write_area("town", "town.toml", &area_with_program("Old.", ""));
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        assert!(!world.get(&ref_id).unwrap().programs.contains_key("on_enter"));
    }

    /// `[[scripts]]` in an area file becomes a `Kind::Code` object carrying
    /// an `on_tick` Program and a `tick_interval` attr — the same shape
    /// `@script` produces at runtime.
    #[test]
    fn file_defined_script_becomes_code_object() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"

                [[scripts]]
                name = "weather"
                interval = 10
                source = "function on_tick(this, state, room) end"
            "#,
        );

        let mut world = World::new();
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let script = world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "weather")
            .expect("file-defined script should become a Kind::Code object");
        assert_eq!(script.attrs["tick_interval"], serde_json::json!(10));
        let program = script.programs.get("on_tick").expect("on_tick Program");
        assert_eq!(program.origin, ProgramOrigin::File);
        assert!(program.source.contains("function on_tick"));
    }

    /// `LoadResult::installed_programs` reports every `(obj_ref, hook,
    /// source)` a file program was actually written to — the caller (the
    /// `Engine`) threads these into `Database::record_program_version` so
    /// file installs get a recorded baseline too (see
    /// docs/plans/program-authoring.md Stage 3's "Which writes get
    /// versioned"). Covers a room, an object, and a script in one load.
    #[test]
    fn load_result_reports_every_installed_program() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"

                [[rooms]]
                key = "square"
                title = "Square"
                [rooms.programs]
                on_look = { source = "function on_look() end" }

                [[objects]]
                key = "gem"
                kind = "item"
                location = "square"
                [objects.programs]
                on_get = { source = "function on_get() end" }

                [[scripts]]
                name = "weather"
                source = "function on_tick(this, state, room) end"
            "#,
        );

        let mut world = World::new();
        let result = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        assert_eq!(result.installed_programs.len(), 3);
        let hooks: std::collections::HashSet<&str> = result
            .installed_programs
            .iter()
            .map(|(_, hook, _)| hook.as_str())
            .collect();
        assert!(hooks.contains("on_look"));
        assert!(hooks.contains("on_get"));
        assert!(hooks.contains("on_tick"));

        // Every reported obj_ref should resolve to a real object, and the
        // reported source should match what actually landed on it.
        for (obj_ref, hook, source) in &result.installed_programs {
            let obj = world.get(obj_ref).expect("installed_programs obj_ref should resolve");
            assert_eq!(&obj.programs[hook].source, source);
        }
    }

    /// Reloading with unchanged files skips them entirely (existing
    /// content-hash behavior) — so nothing already installed shows up in
    /// `installed_programs` again, and repeated boot installs stay free at
    /// the loader level too, not just via DB-side dedupe.
    #[test]
    fn load_result_installed_programs_empty_on_unchanged_reload() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"

                [[rooms]]
                key = "square"
                title = "Square"
                [rooms.programs]
                on_look = { source = "function on_look() end" }
            "#,
        );

        let mut world = World::new();
        let first = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();
        assert_eq!(first.installed_programs.len(), 1);

        let second = load_game_dir(&dir.path, &mut world, &first.file_hashes).unwrap();
        assert!(second.installed_programs.is_empty());
    }

    /// Reloading a file-defined script updates its source and interval in
    /// place (same dbref) and preserves an in-game edit to its Program,
    /// exactly like `reload_preserves_in_game_programs_on_managed_objects`
    /// does for ordinary objects.
    #[test]
    fn reload_updates_file_script_and_preserves_in_game_override() {
        let dir = TempGameDir::new();
        let area = |interval: u64| {
            format!(
                r#"
                area = "town"

                [[scripts]]
                name = "weather"
                interval = {interval}
                source = "function on_tick(this, state, room) end"
                "#
            )
        };
        dir.write_area("town", "town.toml", &area(10));

        let mut world = World::new();
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();
        let ref_id = world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "weather")
            .unwrap()
            .ref_id
            .clone();

        // An in-game edit shadows the file version on reload.
        let obj = world.get_mut(&ref_id).unwrap();
        hooks::set_program(obj, "on_tick", "function on_tick(this, state, room) state.x = 1 end".into()).unwrap();

        dir.write_area("town", "town.toml", &area(20));
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let obj = world.get(&ref_id).unwrap();
        assert_eq!(
            obj.attrs["tick_interval"],
            serde_json::json!(20),
            "file-owned tick_interval still reconciles"
        );
        let program = obj.programs.get("on_tick").unwrap();
        assert_eq!(program.origin, ProgramOrigin::InGame);
        assert!(program.source.contains("state.x = 1"), "in-game Program survives reload");
    }

    /// An in-game edit to a hook the files also define shadows the file
    /// version rather than being overwritten by it.
    #[test]
    fn in_game_program_shadows_the_file_version() {
        let dir = TempGameDir::new();
        let toml = area_with_program(
            "Old.",
            r#"[rooms.programs.on_enter]
               source = "function on_enter() return 'from file' end""#,
        );
        dir.write_area("town", "town.toml", &toml);

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let ref_id = key_map.get("town/crossroads").unwrap().clone();

        let obj = world.get_mut(&ref_id).unwrap();
        hooks::set_program(obj, "on_enter", "function on_enter() return 'edited' end".into())
            .unwrap();

        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let program = world.get(&ref_id).unwrap().programs.get("on_enter").unwrap();
        assert!(program.source.contains("edited"), "file load clobbered the override");
        assert_eq!(program.origin, ProgramOrigin::InGame);
    }

    /// Reinstalling a file program on startup must not reset what its
    /// `on_tick` state has accumulated.
    #[test]
    fn reload_preserves_accumulated_program_state() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            &area_with_program(
                "Old.",
                r#"[rooms.programs.on_tick]
                   source = "function on_tick() end""#,
            ),
        );

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let ref_id = key_map.get("town/crossroads").unwrap().clone();

        world
            .get_mut(&ref_id)
            .unwrap()
            .programs
            .get_mut("on_tick")
            .unwrap()
            .state
            .insert("visits".into(), serde_json::json!(7));

        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let program = world.get(&ref_id).unwrap().programs.get("on_tick").unwrap();
        assert_eq!(program.state.get("visits"), Some(&serde_json::json!(7)));
    }

    #[test]
    fn unresolved_reference_errors() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"
                [[rooms]]
                key = "crossroads"
                title = "The Crossroads"
                [[exits]]
                from = "crossroads"
                direction = "south"
                to = "nowhere"
            "#,
        );

        let mut world = World::new();
        let err = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap_err();
        assert!(err.contains("Unresolved reference"), "unexpected error: {}", err);
    }

    #[test]
    fn load_modules_scans_lib_dir() {
        let dir = TempGameDir::new();
        let lib_dir = dir.path.join("lib");
        std::fs::create_dir_all(&lib_dir).unwrap();
        std::fs::write(
            lib_dir.join("utils.luau"),
            r#"local M = {} function M.add(a, b) return a + b end return M"#,
        )
        .unwrap();
        std::fs::write(
            lib_dir.join("helpers.luau"),
            r#"return { greet = function(n) return "hi " .. n end }"#,
        )
        .unwrap();
        // non-.luau files should be ignored
        std::fs::write(lib_dir.join("notes.txt"), "not a module").unwrap();

        let modules = load_modules(&dir.path);
        assert_eq!(modules.len(), STDLIB_MODULES.len() + 2);
        assert!(modules.contains_key("utils"));
        assert!(modules.contains_key("helpers"));
        assert!(!modules.contains_key("notes"));
        // stdlib modules are present
        assert!(modules.contains_key("str"));
        assert!(modules.contains_key("collections"));
    }
}
