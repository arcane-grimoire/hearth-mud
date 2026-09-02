use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::softcode::hooks::{self, ProgramOrigin};
use crate::world::{GameObject, Kind, Tag, World};

/// blake3 rather than `DefaultHasher`: these hashes are persisted across
/// restarts so that boot can skip unchanged files, and `DefaultHasher` is
/// explicitly not stable between Rust versions — every file would look changed
/// after a toolchain bump. Same reasoning as the program version log.
fn hash_content(content: &str) -> String {
    blake3::hash(content.as_bytes()).to_hex().to_string()
}

#[derive(Debug)]
pub struct LoadResult {
    pub key_map: HashMap<String, String>,
    pub created: u32,
    pub updated: u32,
    pub skipped: u32,
    pub file_hashes: HashMap<PathBuf, String>,
    pub changed_files: Vec<String>,
    /// Objects whose file-defined script was shadowed by an in-game edit on
    /// this load, by file key — the file change was NOT applied. Surfaced
    /// loudly (never silently dropped) in the reload report. See
    /// `install_script`.
    pub diverged: Vec<String>,
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

fn locked_tag() -> Tag {
    Tag {
        category: "system".to_string(),
        key: "locked".to_string(),
    }
}

/// Whether a file key (`"<area>/<key>"`) falls under a configured `locked`
/// prefix. A prefix matches the whole area (`"std"` covers `"std/monster"`)
/// or an exact key, but never a partial segment (`"std"` must not match
/// `"standard/x"`).
fn key_is_locked(file_key: &str, prefixes: &[String]) -> bool {
    prefixes.iter().any(|p| {
        file_key == p || file_key.starts_with(&format!("{p}/"))
    })
}

/// Reconcile the `system:locked` own-tag on managed objects against the
/// configured `locked` prefixes: a managed object is locked **iff** its file
/// key matches a prefix (see `Config::locked` and `docs/plans/archetypes.md`).
/// The config is the single source of truth for which managed objects are
/// locked, so this both *adds* the tag where a key now matches and *removes*
/// it where it no longer does — dropping a prefix (or clearing `locked`
/// entirely) unlocks those objects on the next boot rather than leaving them
/// read-only forever.
///
/// Called by the engine after `load_game_dir` (and on `@reload-world`), and
/// also when `load_world_files = false` — the DB is authoritative for content
/// then, but the config still decides which keys lock, so a changed prefix
/// takes effect on the next boot without re-importing. Only `system:managed`
/// (file-authoritative) objects are touched; player-created objects are never
/// reconciled (a hand-set lock on one stands). Idempotent: a pass that changes
/// nothing is a no-op. `system:locked` is an OWN tag and is never resolved up
/// the archetype chain, so a locked base does not lock its subtypes/instances.
///
/// Returns the number of objects whose lock state changed this pass.
pub fn stamp_locked(world: &mut World, prefixes: &[String]) -> u32 {
    let managed = managed_tag();
    let locked = locked_tag();
    // Decide per managed object whether its lock state must flip.
    let changes: Vec<(String, bool)> = world
        .objects
        .values()
        .filter(|o| o.tags.contains(&managed))
        .filter_map(|o| {
            let fk = o.attrs.get(FILE_KEY_ATTR).and_then(|v| v.as_str())?;
            let should_lock = key_is_locked(fk, prefixes);
            let is_locked = o.tags.contains(&locked);
            (should_lock != is_locked).then(|| (o.ref_id.clone(), should_lock))
        })
        .collect();
    let mut added = 0u32;
    let mut removed = 0u32;
    for (ref_id, should_lock) in &changes {
        if let Some(obj) = world.get_mut(ref_id) {
            if *should_lock {
                obj.tags.insert(locked.clone());
                added += 1;
            } else {
                obj.tags.remove(&locked);
                removed += 1;
            }
        }
    }
    if added > 0 || removed > 0 {
        tracing::info!(added, removed, "Reconciled system:locked on file-authoritative objects");
    }
    added + removed
}

/// Warn-only load-time validation: a declared attribute's actual value should
/// match its declared type. Loud but **non-fatal** — a schema that lies (or an
/// attr edited to the wrong shape) is a builder mistake worth surfacing, never
/// a reason to refuse boot. See `docs/plans/attribute-schema.md`. Called at boot
/// and on `@reload-world`.
pub fn validate_attr_schemas(world: &World) {
    for issue in collect_attr_schema_issues(world) {
        tracing::warn!("{}", issue);
    }
}

/// Testable core of [`validate_attr_schemas`]: one issue string per (object,
/// attr) mismatch. Checks each object's **own** attrs (`obj.attrs`) against its
/// resolved schema, so a bad value is reported once — at the object that holds
/// it — not re-warned on every inheriting instance.
fn collect_attr_schema_issues(world: &World) -> Vec<String> {
    use crate::attr_schema::AttrType;
    let mut issues = Vec::new();
    for obj in world.objects.values() {
        for (desc, _src) in world.resolved_attr_schema(obj) {
            let Some(value) = obj.attrs.get(&desc.key) else {
                continue;
            };
            let who = obj
                .attrs
                .get(FILE_KEY_ATTR)
                .and_then(|v| v.as_str())
                .unwrap_or(obj.ref_id.as_str());
            let base_ok = |ty: &AttrType, v: &serde_json::Value| match ty {
                AttrType::Int => v.is_i64() || v.is_u64(),
                AttrType::Float => v.is_number(),
                AttrType::Bool => v.is_boolean(),
                AttrType::String | AttrType::Text | AttrType::Color | AttrType::Ref | AttrType::Enum => {
                    v.is_string()
                }
                AttrType::List => v.is_array(),
                AttrType::Unknown(_) => true,
            };
            if !base_ok(&desc.ty, value) {
                issues.push(format!(
                    "attr schema: {who} attr '{}' = {value} — expected {}",
                    desc.key,
                    desc.ty.tag()
                ));
                continue;
            }
            // Enum: the (string) value must be one of the declared options.
            if matches!(desc.ty, AttrType::Enum)
                && !desc.values.is_empty()
                && let Some(s) = value.as_str()
                && !desc.values.iter().any(|v| v == s)
            {
                issues.push(format!(
                    "attr schema: {who} attr '{}' = {s:?} — not one of {:?}",
                    desc.key, desc.values
                ));
            }
            // List: each element should match the declared item type.
            if let (AttrType::List, Some(item), Some(arr)) =
                (&desc.ty, &desc.item_type, value.as_array())
            {
                for (i, el) in arr.iter().enumerate() {
                    if !base_ok(item, el) {
                        issues.push(format!(
                            "attr schema: {who} attr '{}'[{i}] = {el} — expected {}",
                            desc.key,
                            item.tag()
                        ));
                    }
                }
            }
        }
    }
    issues
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

/// Every file path an area references for code, across rooms, objects, and
/// scripts — used by change-detection to hash file-referenced sources.
///
/// Scripts matter here: the old change-detection walked rooms and objects
/// only, so a `[[scripts]]` program file was invisible in the same way a
/// room's was. Exits matter for the same reason — an exit carries its own
/// script (a door's `can_traverse` gate), and leaving it unhashed meant
/// editing that file and restarting silently kept running the old gate.
fn referenced_program_files(area: &AreaFile) -> impl Iterator<Item = &str> {
    area.rooms
        .iter()
        .flat_map(|r| r.script.iter().flat_map(|s| s.paths()))
        .chain(
            area.objects
                .iter()
                .flat_map(|o| o.script.iter().flat_map(|s| s.paths())),
        )
        .chain(
            area.rooms
                .iter()
                .flat_map(|r| r.libs.values().filter_map(|p| p.file_path())),
        )
        .chain(
            area.objects
                .iter()
                .flat_map(|o| o.libs.values().filter_map(|p| p.file_path())),
        )
        .chain(
            area.exits
                .iter()
                .flat_map(|e| e.script.iter().flat_map(|s| s.paths())),
        )
        .chain(area.scripts.iter().filter_map(|s| s.source.file_path()))
}

/// Rebuild the `<area>/<key>` → dbref map from a world, without reading any
/// files.
///
/// The map is normally a by-product of loading the game directory, but with
/// `load_world_files = false` that never runs — and then every file-key
/// lookup fails, starting with `spawn_room`, which falls back to creating a
/// duplicate empty room. The identity lives on each object in
/// [`FILE_KEY_ATTR`] and persists with it, so a world loaded straight from
/// the database already carries everything the map needs.
///
/// Objects built in-game carry no file identity and are simply absent.
pub fn key_map_from_world(world: &World) -> HashMap<String, String> {
    world
        .objects
        .values()
        .filter_map(|obj| {
            let key = obj.attrs.get(FILE_KEY_ATTR)?.as_str()?;
            Some((key.to_string(), obj.ref_id.clone()))
        })
        .collect()
}

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
    /// `Option` (like `ObjectDef::title`) so an archetyped room with no own
    /// title inherits it from the archetype instead of exporting `title = ""`
    /// and re-importing that empty string as an override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tags: Vec<String>,
    /// Attributes on the room. Rooms carry state like any other object —
    /// `climate` for a weather system, `sector` for terrain, the per-phase
    /// descriptions a day/night cycle swaps between — so a file must be able
    /// to declare them.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) attrs: std::collections::HashMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) locks: std::collections::HashMap<String, String>,
    /// The archetype this room delegates to, as a file key ("area/key"), if
    /// any — see `docs/plans/archetypes.md`. Resolved to a dbref at load time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) archetype: Option<String>,
    /// The room's behavior script (hooks as functions in one shared scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) script: Option<ScriptSource>,
    /// `require`able lib modules, keyed by bare `<name>` (loaded as
    /// `require("<name>")`). Rare on rooms; usually on `Kind::Code` objects.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) libs: std::collections::HashMap<String, ProgramSource>,
    /// Declared attribute schema — inline array of typed descriptors driving
    /// the builder's form. See `docs/plans/attribute-schema.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) attr_schema: Vec<crate::attr_schema::AttrDescriptor>,
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
    /// The archetype this object delegates to, as a file key ("area/key"), if
    /// any — see `docs/plans/archetypes.md`. Resolved to a dbref at load time;
    /// the instance inherits the archetype's script/title/description/attrs/
    /// tags, keeping its own overrides and state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) archetype: Option<String>,
    /// The object's behavior script (hooks as functions in one shared scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) script: Option<ScriptSource>,
    /// `require`able lib modules, keyed by bare `<name>` (loaded as
    /// `require("<name>")`). Authored on `Kind::Code` objects.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) libs: std::collections::HashMap<String, ProgramSource>,
    /// Declared attribute schema — inline array of typed descriptors driving
    /// the builder's form. See `docs/plans/attribute-schema.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) attr_schema: Vec<crate::attr_schema::AttrDescriptor>,
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
    /// Attributes on the exit itself. An exit is an ordinary object and the
    /// engine already reads attrs off it (`_dest_x`/`_dest_y` for coordinate
    /// exits, `muffle`/`blocked_sound` for sound propagation, door state for a
    /// `can_traverse` gate), so a file must be able to declare them — otherwise
    /// the only authoring paths are `@set` and softcode.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub(crate) attrs: std::collections::HashMap<String, serde_json::Value>,
    /// The exit's behavior script. `can_traverse` is the exit's own hook, so
    /// gating a passage from a file needs this. (No `libs`: those are authored
    /// on `Kind::Code` objects.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) script: Option<ScriptSource>,
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

    /// The referenced file path, if this is a `File` source.
    pub(crate) fn file_path(&self) -> Option<&str> {
        match self {
            ProgramSource::File { file } => Some(file.as_str()),
            ProgramSource::Inline { .. } => None,
        }
    }
}

/// How an object's single behavior script is specified in a TOML area file.
///
/// A script may be one file (`script = "barkeep.luau"`), several files
/// concatenated into one chunk so an object's methods can be split across
/// files while still sharing scope (`script = ["a.luau", "b.luau"]`), or a
/// detailed form (`script = { file = "x.luau" }` / `{ source = "..." }`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum ScriptSource {
    Path(String),
    Paths(Vec<String>),
    Detailed(ProgramSource),
}

impl ScriptSource {
    /// Resolve to the full Luau source, reading and concatenating files as
    /// needed (in listed order, separated by blank lines).
    pub(crate) fn resolve(&self, base_dir: &Path) -> Result<String, String> {
        match self {
            ScriptSource::Path(file) => ProgramSource::File { file: file.clone() }.resolve(base_dir),
            ScriptSource::Paths(files) => {
                let mut parts = Vec::with_capacity(files.len());
                for file in files {
                    parts.push(ProgramSource::File { file: file.clone() }.resolve(base_dir)?);
                }
                Ok(parts.join("\n\n"))
            }
            ScriptSource::Detailed(src) => src.resolve(base_dir),
        }
    }

    /// The referenced file paths (empty for an inline `Detailed` source).
    pub(crate) fn paths(&self) -> Vec<&str> {
        match self {
            ScriptSource::Path(file) => vec![file.as_str()],
            ScriptSource::Paths(files) => files.iter().map(String::as_str).collect(),
            ScriptSource::Detailed(src) => src.file_path().into_iter().collect(),
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
/// Map + terrain sources (`terrain.toml`, `maps/<name>.toml`) share the game
/// directory with area files but are owned by `map_template` / the
/// `file_sources` table, not world content. The area walk skips them: they
/// would otherwise parse as empty `AreaFile`s (no `deny_unknown_fields`), and
/// worse, one malformed map file would abort the *entire* world-content load
/// (`load_game_dir` propagates the first parse error) — a real risk now that
/// `@export`/edit/`@import` makes hand-editing map files a supported workflow.
pub(crate) fn is_map_source_path(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "terrain.toml")
        || path.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "maps")
}

pub(crate) fn parse_area_dir(dir: &Path) -> Result<Vec<ParsedArea>, String> {
    if !dir.exists() {
        return Err(format!("Directory not found: {}", dir.display()));
    }
    let mut area_files: Vec<PathBuf> = Vec::new();
    collect_toml_files(dir, &mut area_files);
    area_files.sort();

    let mut parsed = Vec::new();
    for path in &area_files {
        if is_map_source_path(path) {
            continue;
        }
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
    prev_hashes: &HashMap<PathBuf, String>,
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
            diverged: Vec::new(),
        });
    }

    let mut new_hashes: HashMap<PathBuf, String> = HashMap::new();
    let mut changed_files: Vec<String> = Vec::new();
    let mut diverged: Vec<String> = Vec::new();

    // Parse every area file up front — later passes need to see all of them
    // together to resolve cross-file/cross-area references.
    let mut parsed: Vec<ParsedArea> = Vec::new();
    let mut skipped_files: Vec<PathBuf> = Vec::new();
    for path in &area_files {
        // Map/terrain sources ride the same directory but are owned by the
        // `file_sources` table — never world content, and never allowed to
        // abort the world-content load. See `is_map_source_path`.
        if is_map_source_path(path) {
            continue;
        }
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let toml_hash = hash_content(&contents);

        // Parsed unconditionally, even for an area that turns out unchanged.
        // Whether it can be skipped depends on the program files it
        // references, and there is no way to know what those are without
        // reading it. Parsing is cheap; installing into the world is not, and
        // installing is what the skip actually saves.
        let area_file: AreaFile = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
        let area_name = area_file.area.clone().unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        let base_dir = path.parent().unwrap_or(game_dir).to_path_buf();

        // Hash every program file this area references, whether or not the
        // TOML changed. Hashing them only for already-changed areas is what
        // made a bare `.luau` edit invisible — the most common edit there is —
        // and what dropped those hashes from the persisted set on a warm load.
        let mut program_hashes: Vec<(PathBuf, String)> = Vec::new();
        for file in referenced_program_files(&area_file) {
            let program_path = base_dir.join(file);
            if program_path.exists()
                && let Ok(program_contents) = std::fs::read_to_string(&program_path)
            {
                program_hashes.push((program_path, hash_content(&program_contents)));
            }
        }

        let toml_unchanged = prev_hashes.get(path) == Some(&toml_hash);
        let programs_unchanged = program_hashes
            .iter()
            .all(|(p, h)| prev_hashes.get(p) == Some(h));

        if !toml_unchanged {
            let relative = path.strip_prefix(game_dir).unwrap_or(path);
            changed_files.push(relative.display().to_string());
        }
        for (p, h) in &program_hashes {
            if prev_hashes.get(p) != Some(h) {
                let relative = p.strip_prefix(game_dir).unwrap_or(p);
                let name = relative.display().to_string();
                if !changed_files.contains(&name) {
                    changed_files.push(name);
                }
            }
        }

        // Recorded before the skip, so a skipped area still carries its
        // hashes forward instead of shrinking the persisted set each boot.
        new_hashes.insert(path.clone(), toml_hash);
        for (p, h) in program_hashes {
            new_hashes.insert(p, h);
        }

        if toml_unchanged && programs_unchanged {
            skipped_files.push(path.clone());
            continue;
        }

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
            // Resolve before the mutable borrow below (immutable borrow of
            // `world` for the cycle check ends here); used in whichever branch
            // runs.
            let archetype_ref = resolve_archetype(&ref_id, area_name, &room.archetype, &key_map);
            if let Some(existing) = world.get_mut(&ref_id) {
                if existing.tags.contains(&managed) {
                    if let Some(title) = &room.title {
                        existing.title = Some(title.clone());
                    }
                    existing.description = room.description.clone();
                    // Merge, not replace: a room accumulates runtime state the
                    // file knows nothing about. Same rule as objects and exits.
                    existing.attrs.extend(room.attrs.clone());
                    existing.locks = room.locks.clone();
                    existing.archetype_ref = archetype_ref;
                    sync_managed_tags(existing, &room.tags);
                    install_script(existing, &room.script, &room.libs, base_dir, &mut diverged)?;
                    install_attr_schema(existing, &room.attr_schema, &file_key);
                    updated += 1;
                }
                continue;
            }
            let mut obj = GameObject::new(&ref_id, &room.key, Kind::Room);
            if let Some(title) = &room.title {
                obj = obj.with_title(title);
            }
            obj.description = room.description.clone();
            obj.tags.insert(managed.clone());
            for tag_spec in &room.tags {
                if let Ok(tag) = Tag::parse(tag_spec) {
                    obj.tags.insert(tag);
                }
            }
            obj.locks = room.locks.clone();
            obj.archetype_ref = archetype_ref;
            obj.attrs = room.attrs.clone();
            obj.attrs.insert(FILE_KEY_ATTR.into(), serde_json::json!(file_key));
            install_script(&mut obj, &room.script, &room.libs, base_dir, &mut diverged)?;
            install_attr_schema(&mut obj, &room.attr_schema, &file_key);
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
            let archetype_ref = resolve_archetype(&ref_id, area_name, &object.archetype, &key_map);
            if let Some(existing) = world.get_mut(&ref_id) {
                if existing.tags.contains(&managed) {
                    // Kind is part of the file's *definition*, so a managed
                    // object adopts it on reload like it adopts title, tags,
                    // and script. Without this, changing `kind` in a file did
                    // nothing to an object that already existed — only a fresh
                    // create picked it up — so converting content (e.g. a
                    // command object from a hidden `item` to `code`) meant
                    // destroying and recreating it, or wiping the database.
                    // In-game-created objects aren't managed, so they're
                    // untouched. Note a kind change can change visibility:
                    // `World::objects_in` excludes Code and Exit, so flipping
                    // a container-ish object to `code` stops listing whatever
                    // sits inside it.
                    existing.kind = kind;
                    if let Some(title) = &object.title {
                        existing.title = Some(title.clone());
                    }
                    existing.description = object.description.clone();
                    if let Some(loc) = &location_ref {
                        existing.location_ref = Some(loc.clone());
                    }
                    existing.archetype_ref = archetype_ref;
                    existing.attrs.extend(object.attrs.clone());
                    existing.locks = object.locks.clone();
                    sync_managed_tags(existing, &object.tags);
                    install_script(existing, &object.script, &object.libs, base_dir, &mut diverged)?;
                    install_attr_schema(existing, &object.attr_schema, &file_key);
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
            obj.archetype_ref = archetype_ref;
            install_script(&mut obj, &object.script, &object.libs, base_dir, &mut diverged)?;
            install_attr_schema(&mut obj, &object.attr_schema, &file_key);
            world.add_object(obj);
            created += 1;
        }
    }

    // -- Pass 3: exits. These reference rooms/objects by key, never the
    //    other way around, so they can wait until everything else exists.
    // Exits carry no lib modules (those are authored on Kind::Code objects),
    // but install_script takes a map, so hand it an empty one.
    let empty_libs: std::collections::HashMap<String, ProgramSource> =
        std::collections::HashMap::new();
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
                        // Merge, don't replace: an exit accumulates runtime
                        // state the file knows nothing about (a door's
                        // `closed`, whatever softcode has stamped on it), and
                        // a reload must not wipe it. Same rule as objects.
                        existing.attrs.extend(exit.attrs.clone());
                        install_script(
                            existing,
                            &exit.script,
                            &empty_libs,
                            &area.base_dir,
                            &mut diverged,
                        )?;
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
            obj.attrs = exit.attrs.clone();
            obj.attrs.insert(FILE_KEY_ATTR.into(), serde_json::json!(file_key));
            install_script(&mut obj, &exit.script, &empty_libs, &area.base_dir, &mut diverged)?;
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
            // A `[[scripts]]` entry is a `Kind::Code` object whose script must
            // define `function on_tick(this, state, room)`.
            let script_src = Some(ScriptSource::Detailed(ProgramSource::Inline {
                source: script.source.resolve(&area.base_dir)?,
            }));
            let empty_libs = std::collections::HashMap::new();

            if let Some(existing_ref) = key_map.get(&file_key).cloned()
                && let Some(existing) = world.get_mut(&existing_ref) {
                    if existing.tags.contains(&managed) {
                        existing.attrs.insert(
                            "tick_interval".into(),
                            serde_json::json!(script.interval),
                        );
                        install_script(existing, &script_src, &empty_libs, &area.base_dir, &mut diverged)?;
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
            install_script(&mut obj, &script_src, &empty_libs, &area.base_dir, &mut diverged)?;
            world.add_object(obj);
            created += 1;
        }
    }

    // Validate archetype delegation against the FINAL graph, now that every
    // file-declared `archetype_ref` is set (order-independent — see
    // `break_archetype_cycles`).
    break_archetype_cycles(world);

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
        diverged,
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

/// Reconcile the file-owned script and lib modules on `obj` against the file.
///
/// Only [`ProgramOrigin::File`] code is touched. A script or lib written
/// in-game is database-owned: the loader never clobbers it, and if the file no
/// longer names it, it is treated as a builder's addition rather than a stale
/// file program. Reconciling those away would destroy them on every
/// `@reload-world` — and, because startup loads with no previous file hashes,
/// on every restart.
/// Assign a declared attribute schema onto an object, warning (non-fatally)
/// about any unknown descriptor type so it's visible without breaking the load
/// — the builder degrades an unknown type to a raw field. See
/// `docs/plans/attribute-schema.md`.
fn install_attr_schema(
    obj: &mut GameObject,
    schema: &[crate::attr_schema::AttrDescriptor],
    file_key: &str,
) {
    for d in schema {
        if d.ty.is_unknown() {
            tracing::warn!(
                file_key,
                key = %d.key,
                ty = d.ty.tag(),
                "unknown attribute type in schema — builder falls back to a raw field"
            );
        }
    }
    obj.attr_schema = schema.to_vec();
}

fn install_script(
    obj: &mut GameObject,
    script: &Option<ScriptSource>,
    libs: &std::collections::HashMap<String, ProgramSource>,
    base_dir: &Path,
    diverged: &mut Vec<String>,
) -> Result<(), String> {
    // The object's single script.
    let script_is_ingame = obj
        .script
        .as_ref()
        .is_some_and(|s| s.origin == ProgramOrigin::InGame);
    // Vocal divergence: an in-game edit shadows the file, so the file's script
    // state — whether it (re)defines a script OR removes one — will NOT be
    // applied. An in-game script always differs from the file (that's what
    // gives it `InGame` origin), so report whenever one shadows a reconciled
    // file, including the case where the file drops its `script` entry (which
    // would otherwise vanish silently — exactly what this report guards).
    // Never silent. See `docs/plans/archetypes.md`; the file remains the
    // distribution source, so a maintainer needs to know the running copy has
    // drifted from it.
    if script_is_ingame {
        let label = obj
            .attrs
            .get(FILE_KEY_ATTR)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| obj.ref_id.clone());
        tracing::warn!(
            object = %obj.ref_id,
            file_key = %label,
            "file script shadowed by an in-game edit — file change NOT applied"
        );
        diverged.push(label);
    }
    if !script_is_ingame {
        match script {
            Some(src) => {
                let code = src.resolve(base_dir)?;
                hooks::set_script_with_origin(obj, code, ProgramOrigin::File);
            }
            None => {
                // The file no longer defines a script — drop a stale
                // File-origin one (an in-game one is handled above).
                hooks::clear_script(obj);
            }
        }
    }

    // Lib modules.
    let stale: Vec<String> = obj
        .libs
        .iter()
        .filter(|(name, m)| m.origin == ProgramOrigin::File && !libs.contains_key(name.as_str()))
        .map(|(name, _)| name.clone())
        .collect();
    for name in stale {
        hooks::remove_lib(obj, &name);
    }
    for (name, source) in libs {
        if obj
            .libs
            .get(name)
            .is_some_and(|m| m.origin == ProgramOrigin::InGame)
        {
            continue;
        }
        let code = source.resolve(base_dir)?;
        hooks::set_lib(obj, name, code, ProgramOrigin::File);
    }
    Ok(())
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

/// Resolve a room/object's declared `archetype` file key to the dbref it was
/// assigned in pass 1, guarding against cycles. Returns `None` (with a warning)
/// when the key can't be resolved or would create an archetype cycle, so a bad
/// declaration disables delegation for that object rather than failing the
/// whole load. See `docs/plans/archetypes.md`.
fn resolve_archetype(
    ref_id: &str,
    area_name: &str,
    archetype: &Option<String>,
    key_map: &HashMap<String, String>,
) -> Option<String> {
    let key = archetype.as_ref()?;
    match resolve_key(area_name, key, key_map) {
        Ok(arch_ref) => Some(arch_ref),
        Err(e) => {
            tracing::warn!(object = %ref_id, archetype = %key, error = %e, "archetype could not be resolved — ignored");
            None
        }
    }
}

/// Break any archetype cycles in the FINAL graph, after all `archetype_ref`s
/// have been set from a batch of files.
///
/// Cycle-checking each edge as it's set (against the partially-updated graph)
/// is order-dependent — a reload that *rewires* an existing chain can see a
/// stale edge and wrongly drop a valid new one. So resolution just sets the
/// declared edges, and this pass validates the whole graph at once: any object
/// that appears in its own ancestor chain has its `archetype_ref` cleared
/// (loudly), which is deterministic regardless of file order.
pub(crate) fn break_archetype_cycles(world: &mut World) {
    let cyclic: Vec<String> = world
        .objects
        .values()
        .filter_map(|o| {
            let arch = o.archetype_ref.as_deref()?;
            // `o` is in a cycle iff `o` is reachable from its own parent.
            world
                .would_cycle_archetype(&o.ref_id, arch)
                .then(|| o.ref_id.clone())
        })
        .collect();
    for ref_id in cyclic {
        tracing::warn!(object = %ref_id, "archetype forms a cycle — delegation cleared");
        if let Some(o) = world.get_mut(&ref_id) {
            o.archetype_ref = None;
        }
    }
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
        } else if path.extension().and_then(|e| e.to_str()) == Some("ink")
            && let Ok(source) = std::fs::read_to_string(&path) {
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
    fn file_declared_archetype_resolves_and_delegates() {
        let dir = TempGameDir::new();
        dir.write_area(
            "bestiary",
            "bestiary.toml",
            r#"
area = "bestiary"

[[objects]]
key = "goblin"
kind = "npc"
title = "Goblin"
[objects.attrs]
max_hp = 5
[objects.script]
source = "function on_death(this, actor, room) end"

[[objects]]
key = "grunt"
kind = "npc"
archetype = "bestiary/goblin"
"#,
        );

        let mut world = World::new();
        let result = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();
        let goblin_ref = result.key_map.get("bestiary/goblin").unwrap();
        let grunt_ref = result.key_map.get("bestiary/grunt").unwrap();

        let grunt = world.get(grunt_ref).unwrap();
        // The `archetype = "bestiary/goblin"` key resolved to the goblin's dbref.
        assert_eq!(grunt.archetype_ref.as_deref(), Some(goblin_ref.as_str()));
        // And the instance delegates: title/attr/hook all resolve up the chain
        // even though the grunt declares none of its own.
        assert_eq!(world.resolved_title(grunt).as_deref(), Some("Goblin"));
        assert_eq!(world.resolved_attr(grunt, "max_hp"), Some(&serde_json::json!(5)));
        assert!(crate::softcode::hooks::object_responds(&world, grunt, "on_death"));
    }

    #[test]
    fn file_declared_archetype_cycle_is_ignored() {
        let dir = TempGameDir::new();
        // a -> b and b -> a would cycle; the loader must not hang, and must
        // drop one side of the cycle rather than fail the load.
        dir.write_area(
            "loop",
            "loop.toml",
            r#"
area = "loop"

[[objects]]
key = "a"
kind = "item"
archetype = "loop/b"

[[objects]]
key = "b"
kind = "item"
archetype = "loop/a"
"#,
        );
        let mut world = World::new();
        let result = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();
        let a = world.get(result.key_map.get("loop/a").unwrap()).unwrap();
        let b = world.get(result.key_map.get("loop/b").unwrap()).unwrap();
        // At most one of the two edges survives — never both (that's the cycle).
        assert!(
            !(a.archetype_ref.is_some() && b.archetype_ref.is_some()),
            "a cyclic archetype pair must not both resolve"
        );
    }

    #[test]
    fn area_walk_skips_maps_and_terrain_and_survives_a_bad_map_file() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            "area = \"town\"\n[[rooms]]\nkey = \"square\"\ntitle = \"Square\"\n",
        );
        // A malformed map file must NOT abort the whole world-content load —
        // it isn't world content at all. Owned by the file_sources table.
        dir.write_area("maps", "iron_hills.toml", "this is not valid toml [[[");
        dir.write_area("", "terrain.toml", "[terrain.f]\ntheme = \"forest\"\n");

        let parsed = parse_area_dir(&dir.path)
            .expect("a malformed map file must not abort the area walk");
        let areas: Vec<&str> = parsed.iter().map(|a| a.area_name.as_str()).collect();
        assert!(areas.contains(&"town"));
        assert!(!areas.contains(&"iron_hills"), "maps/*.toml is not an area");
        assert!(!areas.contains(&"terrain"), "terrain.toml is not an area");
    }

    /// Rooms carry state like any other object — `climate` for a weather
    /// system, `sector` for terrain, the per-phase descriptions a day/night
    /// cycle swaps between — but `RoomDef` had no `attrs` field at all. With no
    /// `deny_unknown_fields`, a `[rooms.attrs]` block parsed, loaded without
    /// error, and was silently dropped.
    #[test]
    fn rooms_carry_declared_attrs() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"

                [[rooms]]
                key = "square"
                title = "The Square"

                [rooms.attrs]
                climate = "coastal"
                desc_night = "The square is empty and dark."
                light = 1
            "#,
        );

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new())
            .expect("load should succeed")
            .key_map;
        let square = world.get(key_map.get("town/square").unwrap()).unwrap();

        assert_eq!(square.attrs.get("climate"), Some(&serde_json::json!("coastal")));
        assert_eq!(square.attrs.get("light"), Some(&serde_json::json!(1)));
        assert_eq!(
            square.attrs.get("desc_night"),
            Some(&serde_json::json!("The square is empty and dark."))
        );
    }

    /// As for objects and exits, a reload merges a room's declared attrs over
    /// whatever it accumulated at runtime rather than replacing the lot.
    #[test]
    fn reloading_a_room_merges_attrs_rather_than_replacing() {
        let dir = TempGameDir::new();
        let area = r#"
            area = "town"

            [[rooms]]
            key = "square"
            title = "The Square"

            [rooms.attrs]
            climate = "coastal"
        "#;
        dir.write_area("town", "town.toml", area);

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let square_ref = key_map.get("town/square").unwrap().clone();

        // Runtime state the file knows nothing about.
        world
            .get_mut(&square_ref)
            .unwrap()
            .attrs
            .insert("current_weather".into(), serde_json::json!("storm"));

        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let square = world.get(&square_ref).unwrap();
        assert_eq!(
            square.attrs.get("current_weather"),
            Some(&serde_json::json!("storm")),
            "a reload must not drop runtime state"
        );
        assert_eq!(square.attrs.get("climate"), Some(&serde_json::json!("coastal")));
    }

    /// An exit is an ordinary object and the engine reads attrs and hooks off
    /// it — `_dest_x`/`_dest_y` for coordinate exits, `can_traverse` for a
    /// gate — but `ExitDef` used to accept only from/direction/to/aliases/
    /// locks. With no `deny_unknown_fields`, an `[exits.attrs]` block parsed,
    /// loaded without error, and was silently dropped, so the coordinate-exit
    /// feature's documented TOML authoring path did not exist.
    #[test]
    fn exits_carry_declared_attrs_and_scripts() {
        let dir = TempGameDir::new();
        dir.write_area("town", "gate.luau", "function can_traverse(this, actor, room)\n  return false\nend\n");
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"

                [[rooms]]
                key = "crossroads"
                title = "The Crossroads"

                [[rooms]]
                key = "wilds"
                title = "The Moor"

                [[exits]]
                from = "crossroads"
                direction = "north"
                to = "wilds"
                script = "gate.luau"

                [exits.attrs]
                _dest_x = 5
                _dest_y = 12
                is_door = true
            "#,
        );

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new())
            .expect("load should succeed")
            .key_map;
        let crossroads = key_map.get("town/crossroads").unwrap();

        let exits = world.exits_from(crossroads);
        assert_eq!(exits.len(), 1);
        let exit = exits[0];

        assert_eq!(exit.attrs.get("_dest_x"), Some(&serde_json::json!(5)),
            "an exit must carry its declared coordinate attrs");
        assert_eq!(exit.attrs.get("_dest_y"), Some(&serde_json::json!(12)));
        assert_eq!(exit.attrs.get("is_door"), Some(&serde_json::json!(true)));
        assert!(exit.script.is_some(), "an exit must carry its declared script");
        assert!(
            crate::softcode::hooks::object_responds(&world, exit, "can_traverse"),
            "the declared script's hooks must be derived on the exit"
        );
    }

    /// A reload must not wipe state an exit accumulated at runtime — a door's
    /// `closed`, or anything softcode stamped on it. Same merge rule managed
    /// objects already follow.
    #[test]
    fn reloading_an_exit_merges_attrs_rather_than_replacing() {
        let dir = TempGameDir::new();
        let area = r#"
            area = "town"

            [[rooms]]
            key = "a"
            title = "A"

            [[rooms]]
            key = "b"
            title = "B"

            [[exits]]
            from = "a"
            direction = "north"
            to = "b"

            [exits.attrs]
            is_door = true
        "#;
        dir.write_area("town", "town.toml", area);

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let a = key_map.get("town/a").unwrap().clone();

        // Runtime state: someone shut the door.
        let exit_ref = world.exits_from(&a)[0].ref_id.clone();
        world.get_mut(&exit_ref).unwrap().attrs.insert("closed".into(), serde_json::json!(true));

        // Reload the same files.
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let exit = world.get(&exit_ref).unwrap();
        assert_eq!(exit.attrs.get("closed"), Some(&serde_json::json!(true)),
            "a reload must not drop runtime state the file doesn't declare");
        assert_eq!(exit.attrs.get("is_door"), Some(&serde_json::json!(true)),
            "the file's declared attrs must still be applied");
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

    /// Kind is part of a managed object's file-owned definition: changing it
    /// in the file and redeploying converts the existing object in place,
    /// rather than requiring a destroy-and-recreate (or a database wipe).
    /// The motivating case is a `system:global` command object moving from the
    /// hidden-`item` pattern to `code`.
    #[test]
    fn reload_adopts_a_changed_kind_on_a_managed_object() {
        let dir = TempGameDir::new();
        let area = |kind: &str| {
            format!(
                r#"
                area = "std"
                [[rooms]]
                key = "void"
                title = "Void"
                [[objects]]
                key = "cmd_hero"
                kind = "{kind}"
                title = "Command: hero"
                tags = ["system:global", "system:hidden"]
            "#
            )
        };

        let mut world = World::new();
        dir.write_area("std", "rules.toml", &area("item"));
        let key_map1 = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let ref_id = key_map1.get("std/cmd_hero").unwrap().clone();
        assert_eq!(world.get(&ref_id).unwrap().kind, Kind::Item);

        dir.write_area("std", "rules.toml", &area("code"));
        let key_map2 = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;

        // Converted in place: same dbref, new kind.
        assert_eq!(key_map2.get("std/cmd_hero"), Some(&ref_id));
        assert_eq!(world.get(&ref_id).unwrap().kind, Kind::Code);
    }

    /// The flip side: an object built in-game carries no `system:managed` tag,
    /// so a file that happens to share its dbref never rewrites its kind.
    #[test]
    fn reload_leaves_an_unmanaged_objects_kind_alone() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"
                [[rooms]]
                key = "crossroads"
                title = "The Crossroads"
            "#,
        );
        let mut world = World::new();
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        // A builder's own item, never file-managed.
        let ref_id = world.next_dbref();
        world.add_object(GameObject::new(&ref_id, "lantern", Kind::Item));

        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();
        assert_eq!(world.get(&ref_id).unwrap().kind, Kind::Item);
        assert!(!world.get(&ref_id).unwrap().tags.contains(&managed_tag()));
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
        hooks::set_script(obj, "function cmd_wave() end".into());

        dir.write_area("town", "town.toml", &area_with_program("New.", ""));
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let obj = world.get(&ref_id).unwrap();
        assert_eq!(obj.description, "New.", "file-owned fields still reconcile");
        let script = obj
            .script
            .as_ref()
            .expect("in-game script must survive a reload of its object");
        assert!(script.defines("cmd_wave"));
        assert_eq!(script.origin, ProgramOrigin::InGame);
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
                r#"script = { source = "function on_enter() end" }"#,
            ),
        );

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let ref_id = key_map.get("town/crossroads").unwrap().clone();
        assert_eq!(
            world.get(&ref_id).unwrap().script.as_ref().unwrap().origin,
            ProgramOrigin::File,
        );

        dir.write_area("town", "town.toml", &area_with_program("Old.", ""));
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        assert!(world.get(&ref_id).unwrap().script.is_none());
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
        let obj_script = script.script.as_ref().expect("on_tick script");
        assert!(obj_script.defines("on_tick"));
        assert_eq!(obj_script.origin, ProgramOrigin::File);
        assert!(obj_script.source.contains("function on_tick"));
    }

    /// A load installs a room script, an object script, and a `[[scripts]]`
    /// tick script, each landing on the right object with the right hooks.
    #[test]
    fn load_installs_room_object_and_tick_scripts() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
                area = "town"

                [[rooms]]
                key = "square"
                title = "Square"
                script = { source = "function on_look() end" }

                [[objects]]
                key = "gem"
                kind = "item"
                location = "square"
                script = { source = "function on_get() end" }

                [[scripts]]
                name = "weather"
                source = "function on_tick(this, state, room) end"
            "#,
        );

        let mut world = World::new();
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let square = world.objects.values().find(|o| o.key == "square").unwrap();
        assert!(hooks::object_defines_hook(square, "on_look"));
        let gem = world.objects.values().find(|o| o.key == "gem").unwrap();
        assert!(hooks::object_defines_hook(gem, "on_get"));
        let weather = world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "weather")
            .unwrap();
        assert!(hooks::object_defines_hook(weather, "on_tick"));
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
        hooks::set_script(obj, "function on_tick(this, state, room) state.x = 1 end".into());

        dir.write_area("town", "town.toml", &area(20));
        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let obj = world.get(&ref_id).unwrap();
        assert_eq!(
            obj.attrs["tick_interval"],
            serde_json::json!(20),
            "file-owned tick_interval still reconciles"
        );
        let script = obj.script.as_ref().unwrap();
        assert_eq!(script.origin, ProgramOrigin::InGame);
        assert!(script.source.contains("state.x = 1"), "in-game script survives reload");
    }

    /// An in-game edit to a hook the files also define shadows the file
    /// version rather than being overwritten by it.
    #[test]
    fn in_game_program_shadows_the_file_version() {
        let dir = TempGameDir::new();
        let toml = area_with_program(
            "Old.",
            r#"script = { source = "function on_enter() return 'from file' end" }"#,
        );
        dir.write_area("town", "town.toml", &toml);

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let ref_id = key_map.get("town/crossroads").unwrap().clone();

        let obj = world.get_mut(&ref_id).unwrap();
        hooks::set_script(obj, "function on_enter() return 'edited' end".into());

        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let script = world.get(&ref_id).unwrap().script.as_ref().unwrap().clone();
        assert!(script.source.contains("edited"), "file load clobbered the override");
        assert_eq!(script.origin, ProgramOrigin::InGame);
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
                r#"script = { source = "function on_tick() end" }"#,
            ),
        );

        let mut world = World::new();
        let key_map = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap().key_map;
        let ref_id = key_map.get("town/crossroads").unwrap().clone();

        world
            .get_mut(&ref_id)
            .unwrap()
            .script
            .as_mut()
            .unwrap()
            .state
            .insert("visits".into(), serde_json::json!(7));

        load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let script = world.get(&ref_id).unwrap().script.as_ref().unwrap();
        assert_eq!(script.state.get("visits"), Some(&serde_json::json!(7)));
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

    /// With `load_world_files = false` the boot path never ran the loader, so
    /// the `<area>/<key>` map was empty and every file-key lookup failed —
    /// including `spawn_room`, which then created a duplicate empty room and
    /// dropped players into it instead of the real one. The identity is
    /// already on each object and persists with it, so the map can be
    /// rebuilt from a world loaded straight from the database.
    /// Editing a program file without touching its area TOML is the single
    /// most common thing a developer does, and it was invisible to change
    /// detection: `.luau` files were only hashed for areas that had already
    /// been marked changed, so the edit was never seen and stale code kept
    /// running. Harmless while boot always reloaded everything; a silent
    /// correctness bug once boot started honouring the skip.
    #[test]
    fn editing_only_a_program_file_is_detected() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
            area = "town"
            [[rooms]]
            key = "square"
            title = "The Square"
            script = "greet.luau"
            "#,
        );
        std::fs::write(
            dir.path.join("town").join("greet.luau"),
            "function on_enter(this, actor, room) end",
        )
        .unwrap();

        let mut world = World::new();
        let first = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        // Only the program file changes. The area TOML is untouched.
        std::fs::write(
            dir.path.join("town").join("greet.luau"),
            "function on_enter(this, actor, room) emit(actor, \"hello\") end",
        )
        .unwrap();

        let second = load_game_dir(&dir.path, &mut world, &first.file_hashes).unwrap();

        let room = world
            .objects
            .values()
            .find(|o| o.key == "square")
            .expect("room should exist");
        let script = crate::softcode::hooks::get_script(room)
            .expect("room should still have its script");
        assert!(
            script.source.contains("hello"),
            "the edited program should have been reinstalled, got: {}",
            script.source
        );
        assert_eq!(second.skipped, 0, "a changed program must de-skip its area");
    }

    /// A warm load must carry every hash forward, not just the ones belonging
    /// to areas it happened to reinstall — otherwise the persisted set shrinks
    /// on each boot and the dropped files look changed next time.
    #[test]
    /// Editing an exit's script file and reloading must actually reinstall it.
    ///
    /// `referenced_program_files` drives change detection, and it walked rooms,
    /// objects, their libs, and `[[scripts]]` — but not exits. So a door's
    /// `.luau` was unhashed: nothing the loader tracked had changed, the area
    /// was skipped, and the DB's old gate kept running. Exactly the bug the
    /// function's own comment says was fixed for rooms and `[[scripts]]`.
    #[test]
    fn editing_an_exit_script_is_detected_on_reload() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
            area = "town"
            [[rooms]]
            key = "a"
            title = "A"
            [[rooms]]
            key = "b"
            title = "B"
            [[exits]]
            from = "a"
            direction = "north"
            to = "b"
            script = "gate.luau"
            "#,
        );
        let gate = dir.path.join("town").join("gate.luau");
        std::fs::write(&gate, "function can_traverse(this, actor, room) return false end").unwrap();

        let mut world = World::new();
        let first = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();
        let exit_ref = world
            .objects
            .values()
            .find(|o| o.kind == Kind::Room && o.key == "a")
            .map(|r| world.exits_from(&r.ref_id)[0].ref_id.clone())
            .expect("exit loaded");
        assert!(world.get(&exit_ref).unwrap().script.is_some());

        // Edit the gate — the area TOML is untouched, so only the .luau's own
        // hash can reveal the change.
        std::fs::write(
            &gate,
            "function can_traverse(this, actor, room) return true end
             function on_enter(this, actor, room) end",
        )
        .unwrap();

        load_game_dir(&dir.path, &mut world, &first.file_hashes).unwrap();

        let script = world.get(&exit_ref).unwrap().script.as_ref().expect("still scripted");
        assert!(
            script.source.contains("return true"),
            "an edited exit script must be reinstalled, got: {}",
            script.source
        );
        assert!(
            crate::softcode::hooks::object_responds(&world, world.get(&exit_ref).unwrap(), "on_enter"),
            "and its newly-added hooks must be derived"
        );
    }

    #[test]
    fn skipped_areas_still_carry_their_program_hashes_forward() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
            area = "town"
            [[rooms]]
            key = "square"
            title = "The Square"
            script = "greet.luau"
            "#,
        );
        std::fs::write(
            dir.path.join("town").join("greet.luau"),
            "function on_enter(this, actor, room) end",
        )
        .unwrap();

        let mut world = World::new();
        let first = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();
        let second = load_game_dir(&dir.path, &mut world, &first.file_hashes).unwrap();

        assert_eq!(
            second.file_hashes, first.file_hashes,
            "an unchanged reload must not drop hashes it skipped over"
        );
    }

    /// The in-process skip was already tested; this is the part that was
    /// missing — carrying it across a restart. Boot used to pass an empty
    /// previous-hash map, so every startup re-read and reinstalled the whole
    /// game directory no matter what had changed.
    #[test]
    fn unchanged_files_stay_skipped_across_a_restart() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            r#"
            area = "town"
            [[rooms]]
            key = "square"
            title = "The Square"
            "#,
        );

        let db_path = dir.path.join("scratch.db");

        // First boot: nothing known, so the file is read and installed.
        let mut world = World::new();
        let first = {
            let db = crate::db::Database::open(&db_path).unwrap();
            let result = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();
            db.save_file_hashes(&result.file_hashes).unwrap();
            result
        };
        assert_eq!(first.skipped, 0, "first load has nothing to skip");
        assert!(first.created > 0);

        // Second boot: a fresh connection, as a restart would have, reading
        // the hashes back out of the database.
        let db = crate::db::Database::open(&db_path).unwrap();
        let restored = db.load_file_hashes().unwrap();
        assert_eq!(
            restored, first.file_hashes,
            "hashes must survive the connection being dropped"
        );

        let second = load_game_dir(&dir.path, &mut world, &restored).unwrap();
        assert!(
            second.skipped > 0,
            "an unchanged file should be skipped on the next boot"
        );
        assert_eq!(second.created, 0, "nothing new should be created");
    }

    #[test]
    fn key_map_can_be_rebuilt_from_a_world_without_reading_files() {
        let mut world = World::new();

        let crossroads = world.next_dbref();
        let mut room = GameObject::new(&crossroads, "crossroads", Kind::Room);
        room.attrs
            .insert(FILE_KEY_ATTR.into(), serde_json::json!("town/crossroads"));
        world.add_object(room);

        // An object built in-game carries no file identity and must simply
        // be absent from the map rather than breaking the rebuild.
        let adhoc = world.next_dbref();
        world.add_object(GameObject::new(&adhoc, "barrel", Kind::Item));

        let key_map = key_map_from_world(&world);

        assert_eq!(key_map.get("town/crossroads"), Some(&crossroads));
        assert_eq!(key_map.len(), 1, "only file-identified objects belong in the map");
    }

    #[test]
    fn key_is_locked_matches_area_and_exact_key_but_not_partial_segment() {
        let prefixes = vec!["std".to_string()];
        assert!(key_is_locked("std/monster", &prefixes), "area prefix covers keys under it");
        assert!(key_is_locked("std", &prefixes), "exact key matches");
        assert!(!key_is_locked("standard/foo", &prefixes), "must not match a partial segment");
        assert!(!key_is_locked("town/square", &prefixes));
        assert!(!key_is_locked("std/monster", &[]), "no prefixes locks nothing");
    }

    /// A configured `locked` prefix stamps `system:locked` (own tag) onto its
    /// managed objects at load, and nothing else. Idempotent on re-stamp.
    #[test]
    fn config_locked_prefix_stamps_system_locked() {
        let dir = TempGameDir::new();
        dir.write_area(
            "std",
            "monster.toml",
            "area = \"std\"\n[[objects]]\nkey = \"monster\"\nkind = \"npc\"\ntitle = \"Monster\"\n",
        );
        dir.write_area(
            "town",
            "town.toml",
            "area = \"town\"\n[[rooms]]\nkey = \"square\"\ntitle = \"Square\"\n",
        );

        let mut world = World::new();
        let result = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();

        let stamped = stamp_locked(&mut world, &["std".to_string()]);
        assert_eq!(stamped, 1, "only the std/* object is stamped");

        let locked = locked_tag();
        let monster = world.get(result.key_map.get("std/monster").unwrap()).unwrap();
        assert!(monster.tags.contains(&locked), "std/* object is locked");
        let square = world.get(result.key_map.get("town/square").unwrap()).unwrap();
        assert!(!square.tags.contains(&locked), "town/* object is not locked");

        // Idempotent: re-stamping stamps nothing new.
        assert_eq!(stamp_locked(&mut world, &["std".to_string()]), 0);

        // Dropping the prefix reconciles the stale lock away — a removed
        // `locked` config must not leave objects read-only forever.
        assert_eq!(stamp_locked(&mut world, &[]), 1, "clearing prefixes unlocks");
        let monster = world.get(result.key_map.get("std/monster").unwrap()).unwrap();
        assert!(!monster.tags.contains(&locked), "std/* object is unlocked after prefix removal");
        assert_eq!(stamp_locked(&mut world, &[]), 0, "already unlocked — no further change");
    }

    /// Attr-schema validation flags a value whose type (or enum membership)
    /// doesn't match its descriptor, and stays quiet for a correct value.
    #[test]
    fn attr_schema_validation_flags_type_mismatches() {
        use crate::attr_schema::{AttrDescriptor, AttrType};
        let mut world = World::new();

        let bad = world.next_dbref();
        let mut o = GameObject::new(&bad, "goblin", Kind::Npc);
        let mut biome = AttrDescriptor::new("biome", AttrType::Enum);
        biome.values = vec!["arid".into(), "alpine".into()];
        o.attr_schema = vec![AttrDescriptor::new("hp", AttrType::Int), biome];
        o.attrs.insert("hp".into(), serde_json::json!("three")); // int declared, string given
        o.attrs.insert("biome".into(), serde_json::json!("swamp")); // not in the set
        world.add_object(o);

        let issues = collect_attr_schema_issues(&world);
        assert!(
            issues.iter().any(|i| i.contains("'hp'") && i.contains("expected int")),
            "type mismatch flagged: {issues:?}"
        );
        assert!(
            issues.iter().any(|i| i.contains("'biome'") && i.contains("not one of")),
            "enum-out-of-set flagged: {issues:?}"
        );

        // A correct value produces no issue for that object.
        let good = world.next_dbref();
        let mut g = GameObject::new(&good, "ok", Kind::Npc);
        g.attr_schema = vec![AttrDescriptor::new("hp", AttrType::Int)];
        g.attrs.insert("hp".into(), serde_json::json!(9));
        world.add_object(g);
        assert!(
            !collect_attr_schema_issues(&world).iter().any(|i| i.contains(&good)),
            "a correct value must not be flagged"
        );
    }

    /// A file change that an in-game edit shadows must be surfaced (never
    /// silently dropped): the in-game script survives, and the load reports the
    /// divergence by file key.
    #[test]
    fn shadowed_file_script_reports_divergence() {
        let dir = TempGameDir::new();
        dir.write_area(
            "town",
            "town.toml",
            &area_with_program(
                "Old.",
                r#"script = { source = "function on_enter() return 'file' end" }"#,
            ),
        );

        let mut world = World::new();
        let first = load_game_dir(&dir.path, &mut world, &HashMap::new()).unwrap();
        let ref_id = first.key_map.get("town/crossroads").unwrap().clone();
        assert!(first.diverged.is_empty(), "no divergence on the first clean load");

        // An in-game edit shadows the file version.
        hooks::set_script(
            world.get_mut(&ref_id).unwrap(),
            "function on_enter() return 'edited' end".into(),
        );

        // Change the file and reload — the file change cannot be applied.
        dir.write_area(
            "town",
            "town.toml",
            &area_with_program(
                "New.",
                r#"script = { source = "function on_enter() return 'file2' end" }"#,
            ),
        );
        let second = load_game_dir(&dir.path, &mut world, &first.file_hashes).unwrap();

        assert!(
            second.diverged.iter().any(|k| k.contains("crossroads")),
            "a shadowed file change must be surfaced, got {:?}",
            second.diverged
        );
        // The in-game edit survived; the file change was NOT applied.
        assert!(
            world.get(&ref_id).unwrap().script.as_ref().unwrap().source.contains("edited"),
        );
    }
}
