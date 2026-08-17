use std::path::Path;

use serde::Deserialize;

use crate::softcode::hooks;
use crate::world::{GameObject, Kind, Script, Tag, World};

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

/// Load or reload game content from files.
///
/// - New objects are created and tagged `system:managed`.
/// - Existing managed objects get their description, title, programs,
///   tags, locks, and attrs updated from files.
/// - Existing non-managed objects (player-created) are never touched.
pub fn load_game_dir(game_dir: &Path, world: &mut World) -> Result<(), String> {
    if !game_dir.exists() {
        return Err(format!("Game directory not found: {}", game_dir.display()));
    }

    let mut area_files: Vec<std::path::PathBuf> = Vec::new();
    collect_toml_files(game_dir, &mut area_files);
    area_files.sort();

    if area_files.is_empty() {
        tracing::info!(?game_dir, "No TOML files found in game directory");
        return Ok(());
    }

    let managed = managed_tag();
    let mut created = 0u32;
    let mut updated = 0u32;

    for path in &area_files {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let area_file: AreaFile = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;

        let area_name = area_file
            .area
            .clone()
            .unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            });

        let base_dir = path.parent().unwrap_or(game_dir);

        for room in &area_file.rooms {
            let ref_id = format!("area/{}/room/{}", area_name, room.key);
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
            let mut obj = GameObject::new(&ref_id, &room.key, Kind::Room)
                .with_title(&room.title);
            obj.description = room.description.clone();
            obj.tags.insert(managed.clone());
            for tag_spec in &room.tags {
                if let Ok(tag) = Tag::parse(tag_spec) {
                    obj.tags.insert(tag);
                }
            }
            obj.locks = room.locks.clone();
            install_programs(&mut obj, &room.programs, base_dir)?;
            world.add_object(obj);
            created += 1;
        }

        for object in &area_file.objects {
            let kind = Kind::parse(&object.kind)
                .ok_or_else(|| format!("Unknown kind '{}' in {}", object.kind, path.display()))?;
            let ref_id = format!("area/{}/{}/{}", area_name, object.kind, object.key);
            if let Some(existing) = world.get_mut(&ref_id) {
                if existing.tags.contains(&managed) {
                    if let Some(title) = &object.title {
                        existing.title = Some(title.clone());
                    }
                    existing.description = object.description.clone();
                    if let Some(loc) = &object.location {
                        existing.location_ref = Some(resolve_ref(&area_name, loc));
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
            if let Some(loc) = &object.location {
                let resolved = resolve_ref(&area_name, loc);
                obj = obj.with_location(resolved);
            }
            obj.tags.insert(managed.clone());
            for tag_spec in &object.tags {
                if let Ok(tag) = Tag::parse(tag_spec) {
                    obj.tags.insert(tag);
                }
            }
            obj.attrs = object.attrs.clone();
            obj.locks = object.locks.clone();
            install_programs(&mut obj, &object.programs, base_dir)?;
            world.add_object(obj);
            created += 1;
        }

        for exit in &area_file.exits {
            let from = resolve_ref(&area_name, &exit.from);
            let to = resolve_ref(&area_name, &exit.to);
            let from_key = from.rsplit('/').next().unwrap_or("x");
            let to_key = to.rsplit('/').next().unwrap_or("y");
            let ref_id = format!("area/{}/exit/{}_to_{}", area_name, from_key, to_key);
            if let Some(existing) = world.get_mut(&ref_id) {
                if existing.tags.contains(&managed) {
                    existing.location_ref = Some(from.clone());
                    existing.target_ref = Some(to.clone());
                    existing.aliases = exit.aliases.iter().cloned().collect();
                    existing.locks = exit.locks.clone();
                    updated += 1;
                }
                continue;
            }
            let mut obj = GameObject::new(&ref_id, &exit.direction, Kind::Exit)
                .with_location(&from)
                .with_target(&to);
            obj.aliases = exit.aliases.iter().cloned().collect();
            obj.locks = exit.locks.clone();
            obj.tags.insert(managed.clone());
            world.add_object(obj);
            created += 1;
        }

        for script in &area_file.scripts {
            let source = script.source.resolve(base_dir)?;
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

    tracing::info!(
        ?game_dir,
        created,
        updated,
        "Game content loaded from files"
    );

    Ok(())
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
    for (hook, source) in programs {
        let code = source.resolve(base_dir)?;
        hooks::set_program(obj, hook, code)?;
    }
    Ok(())
}

fn resolve_ref(area: &str, reference: &str) -> String {
    if reference.starts_with("area/") {
        reference.to_string()
    } else {
        format!("area/{}/room/{}", area, reference)
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
