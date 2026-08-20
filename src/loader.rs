use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::softcode::hooks;
use crate::world::{GameObject, Kind, Script, Tag, World};

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
const FILE_KEY_ATTR: &str = "_file_key";

#[derive(Debug, Deserialize)]
struct AreaFile {
    #[serde(default)]
    area: Option<String>,
    #[serde(default)]
    rooms: Vec<RoomDef>,
    #[serde(default)]
    objects: Vec<ObjectDef>,
    #[serde(default)]
    exits: Vec<ExitDef>,
    #[serde(default)]
    scripts: Vec<ScriptDef>,
}

#[derive(Debug, Deserialize)]
struct RoomDef {
    key: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    locks: std::collections::HashMap<String, String>,
    #[serde(default)]
    programs: std::collections::HashMap<String, ProgramSource>,
}

#[derive(Debug, Deserialize)]
struct ObjectDef {
    key: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    attrs: std::collections::HashMap<String, serde_json::Value>,
    #[serde(default)]
    locks: std::collections::HashMap<String, String>,
    #[serde(default)]
    programs: std::collections::HashMap<String, ProgramSource>,
}

fn default_kind() -> String {
    "item".into()
}

#[derive(Debug, Deserialize)]
struct ExitDef {
    from: String,
    direction: String,
    to: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    locks: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ScriptDef {
    name: String,
    #[serde(default = "default_entry")]
    entry: String,
    #[serde(default = "default_interval")]
    interval: u64,
    #[serde(flatten)]
    source: ProgramSource,
}

fn default_entry() -> String {
    "on_tick".into()
}

fn default_interval() -> u64 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProgramSource {
    File { file: String },
    Inline { source: String },
}

impl ProgramSource {
    fn resolve(&self, base_dir: &Path) -> Result<String, String> {
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

struct ParsedArea {
    area_name: String,
    base_dir: PathBuf,
    file: AreaFile,
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
                    install_programs(existing, &room.programs, base_dir)?;
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
            install_programs(&mut obj, &room.programs, base_dir)?;
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
                    install_programs(existing, &object.programs, base_dir)?;
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
            install_programs(&mut obj, &object.programs, base_dir)?;
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

        for script in &area.file.scripts {
            let source = script.source.resolve(&area.base_dir)?;
            if let Some(existing) = world.scripts.get_mut(&script.name) {
                existing.source = source;
                existing.entry = script.entry.clone();
                existing.interval = script.interval;
                updated += 1;
                continue;
            }
            let mut s = Script::new(&script.name, &source);
            s.entry = script.entry.clone();
            s.interval = script.interval;
            world.scripts.insert(script.name.clone(), s);
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

fn install_programs(
    obj: &mut GameObject,
    programs: &std::collections::HashMap<String, ProgramSource>,
    base_dir: &Path,
) -> Result<(), String> {
    let stale: Vec<String> = obj
        .programs
        .keys()
        .filter(|hook| !programs.contains_key(*hook))
        .cloned()
        .collect();
    for hook in stale {
        hooks::remove_program(obj, &hook);
    }
    for (hook, source) in programs {
        let code = source.resolve(base_dir)?;
        hooks::set_program(obj, hook, code)?;
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

/// Resolve a file-level reference (a bare key like `"crossroads"`, or a
/// cross-area key like `"forest/edge"`) to the dbref it was assigned in
/// pass 1/2.
fn resolve_key(area: &str, reference: &str, key_map: &HashMap<String, String>) -> Result<String, String> {
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

fn collect_toml_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
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
