//! `@import` / `@export` — install a TOML+`.luau` bundle into the DB, and
//! emit DB-owned content back to the same format. See
//! docs/plans/program-authoring.md Stage 4.
//!
//! Both directions share `loader`'s TOML struct definitions
//! (`AreaFile`/`RoomDef`/`ObjectDef`/`ExitDef`/`ScriptDef`/`ProgramSource`,
//! all made `pub(crate)` for this purpose) so import and export are
//! provably the same format rather than two that could drift apart.
//! `FILE_KEY_ATTR` is the identity mechanism `load_game_dir` (the boot
//! loader) already uses; this module reuses it exactly, but drops the
//! `system:managed` tag and `ProgramOrigin` — the ownership/reconcile layer
//! Stage 4 supersedes with its own recorded/current/incoming hash
//! comparison (see `resolve_one_program` below).
//!
//! Deliberately separate from `loader.rs`'s boot-time reconciler: that code
//! path is unchanged by this plan (see the plan's "Explicitly out of
//! scope"), and keeping this module independent means nothing here can
//! alter boot behaviour.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::db::Database;
use crate::loader::{self, ProgramSource};
use crate::softcode::hooks;
use crate::world::{GameObject, Kind, Tag, World};

/// One (obj_ref, hook) program noted in an [`ImportReport`]'s `kept_local`
/// or `conflicts` bucket.
#[derive(Debug, Clone)]
pub struct ProgramNote {
    pub ref_id: String,
    pub hook: String,
}

/// What an `@import` did (or, for a dry run, would do) — the four buckets
/// from docs/plans/program-authoring.md Stage 4's "Upgrade: what re-import
/// does", plus the program-level three-way split of the "in both, edited
/// locally" case.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    /// Object identities (`"<file_key> (<kind>)"`) created — new to the DB.
    pub created: Vec<String>,
    /// Object identities that existed and had at least one field or program
    /// change applied.
    pub updated: Vec<String>,
    /// Object identities that existed and needed no change at all.
    pub unchanged: Vec<String>,
    /// Programs where a local edit was detected but upstream hadn't
    /// changed since the last import — kept as-is, nothing written.
    pub kept_local: Vec<ProgramNote>,
    /// Programs where both the local copy and the incoming bundle had
    /// changed since the last import — overwritten with the incoming
    /// source, with the local edit preserved as a version (see
    /// `resolve_one_program`) and reported here so the caller can point at
    /// `@program/history`.
    pub conflicts: Vec<ProgramNote>,
    /// File keys present in the DB (under one of this bundle's areas) that
    /// this bundle no longer defines. Reported, never removed — see the
    /// plan's "In the DB, gone from the file".
    pub missing: Vec<String>,
}

impl ImportReport {
    fn sort(&mut self) {
        self.created.sort();
        self.updated.sort();
        self.unchanged.sort();
        self.missing.sort();
        self.kept_local.sort_by(|a, b| (&a.ref_id, &a.hook).cmp(&(&b.ref_id, &b.hook)));
        self.conflicts.sort_by(|a, b| (&a.ref_id, &a.hook).cmp(&(&b.ref_id, &b.hook)));
    }
}

/// Render an [`ImportReport`] as the text `@import`, the REST `Import`
/// action, and `hearth import` all show — one implementation shared by all
/// three surfaces (see docs/plans/program-authoring.md Stage 4's "Surface
/// all of it three ways").
pub fn render_import_report(report: &ImportReport, dry_run: bool, bundle: &str) -> String {
    let mut out = String::new();
    if dry_run {
        out.push_str(&format!("Import (dry run) of {}:\r\n", bundle));
    } else {
        out.push_str(&format!("Import of {}:\r\n", bundle));
    }
    out.push_str(&format!(
        "  {} created, {} updated, {} unchanged\r\n",
        report.created.len(),
        report.updated.len(),
        report.unchanged.len()
    ));
    for c in &report.created {
        out.push_str(&format!("    + {}\r\n", c));
    }
    for u in &report.updated {
        out.push_str(&format!("    ~ {}\r\n", u));
    }
    if !report.kept_local.is_empty() {
        out.push_str(&format!(
            "  {} local edit(s) kept as-is (upstream unchanged since the last import):\r\n",
            report.kept_local.len()
        ));
        for n in &report.kept_local {
            out.push_str(&format!("    = {}/{}\r\n", n.ref_id, n.hook));
        }
    }
    if !report.conflicts.is_empty() {
        out.push_str(&format!(
            "  WARNING: {} local edit(s) were overwritten by this import. \
             Nothing was lost — your edits are preserved in the version log:\r\n",
            report.conflicts.len()
        ));
        for n in &report.conflicts {
            out.push_str(&format!(
                "    ! {}/{} — see @program/history {}/{}\r\n",
                n.ref_id, n.hook, n.ref_id, n.hook
            ));
        }
    }
    if !report.missing.is_empty() {
        out.push_str(&format!(
            "  {} object(s) in the database but missing from this bundle (NOT removed):\r\n",
            report.missing.len()
        ));
        for m in &report.missing {
            out.push_str(&format!("    ? {}\r\n", m));
        }
    }
    out
}

/// Install (or, for an upgrade, reconcile) a TOML+`.luau` bundle under
/// `path` into `world`/`db`. See the module docs and
/// docs/plans/program-authoring.md Stage 4.
///
/// Refuses — writing nothing at all, to either `world` or `db` — if the
/// bundle declares the same identity twice, or the same `cmd_` hook name on
/// two different objects (see `check_collisions`). `dry_run` computes and
/// returns the exact same report a real import would, against a scratch
/// clone of `world`, without touching the real `world` or writing to `db`.
pub fn import_bundle(
    path: &Path,
    world: &mut World,
    db: &Database,
    dry_run: bool,
    author: Option<&str>,
) -> Result<ImportReport, String> {
    let areas = loader::parse_area_dir(path)?;
    if areas.is_empty() {
        return Err(format!("No TOML files found under {}", path.display()));
    }
    check_collisions(&areas)?;

    let mut report = if dry_run {
        let mut scratch = world.clone();
        apply(&areas, &mut scratch, db, false, author)?
    } else {
        apply(&areas, world, db, true, author)?
    };
    report.sort();
    Ok(report)
}

/// Refuse (before any write) if this bundle declares the same object/exit/
/// script identity twice, or the same `cmd_` hook name on two different
/// identities — see the plan's "Key collisions refuse the import."
fn check_collisions(areas: &[loader::ParsedArea]) -> Result<(), String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut dupes: Vec<String> = Vec::new();
    let mut note_key = |key: String| {
        if !seen.insert(key.clone()) {
            dupes.push(key);
        }
    };

    for area in areas {
        for room in &area.file.rooms {
            note_key(format!("{}/{}", area.area_name, room.key));
        }
        for object in &area.file.objects {
            if Kind::parse(&object.kind).is_none() {
                return Err(format!(
                    "Unknown kind '{}' for '{}/{}' — nothing was written.",
                    object.kind, area.area_name, object.key
                ));
            }
            note_key(format!("{}/{}", area.area_name, object.key));
        }
        for exit in &area.file.exits {
            note_key(format!("{}/exit/{}/{}", area.area_name, exit.from, exit.direction));
        }
        for script in &area.file.scripts {
            note_key(format!("{}/script/{}", area.area_name, script.name));
        }
    }
    if !dupes.is_empty() {
        dupes.sort();
        dupes.dedup();
        return Err(format!(
            "Import refused: this bundle declares the same identity more than once: {}. \
             Nothing was written.",
            dupes.join(", ")
        ));
    }

    let mut cmd_owner: HashMap<String, String> = HashMap::new();
    let mut cmd_dupes: Vec<String> = Vec::new();
    let mut note_cmd = |hook: &str, file_key: &str| {
        if !hook.starts_with("cmd_") {
            return;
        }
        match cmd_owner.get(hook) {
            Some(owner) if owner != file_key => {
                cmd_dupes.push(format!("{} ({} vs {})", hook, owner, file_key));
            }
            Some(_) => {}
            None => {
                cmd_owner.insert(hook.to_string(), file_key.to_string());
            }
        }
    };
    for area in areas {
        for room in &area.file.rooms {
            let fk = format!("{}/{}", area.area_name, room.key);
            for hook in room.programs.keys() {
                note_cmd(hook, &fk);
            }
        }
        for object in &area.file.objects {
            let fk = format!("{}/{}", area.area_name, object.key);
            for hook in object.programs.keys() {
                note_cmd(hook, &fk);
            }
        }
    }
    if !cmd_dupes.is_empty() {
        cmd_dupes.sort();
        cmd_dupes.dedup();
        return Err(format!(
            "Import refused: the same command name is defined on more than one object \
             in this bundle: {}. Nothing was written.",
            cmd_dupes.join(", ")
        ));
    }

    Ok(())
}

/// The outcome of comparing one program hook against the recorded/current/
/// incoming three-way hash — dpkg's conffile algorithm, per the plan's
/// "Upgrade: what re-import does" table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgOutcome {
    /// The object had no program at this hook yet.
    New,
    /// Recorded == current (nobody touched it since the last import) and
    /// incoming differs — safe to overwrite.
    Overwritten,
    /// Recorded == current == incoming — nothing to do.
    Unchanged,
    /// Recorded != current (a local edit happened) but incoming == recorded
    /// (upstream hasn't changed) — keep the local edit.
    KeptLocal,
    /// Both the local copy and incoming changed since the last import — or
    /// there is no recorded baseline at all, so "nobody touched it" cannot
    /// be proven. Overwritten, with the local source preserved as a
    /// version first.
    Conflict,
}

fn blake3_hex(s: &str) -> String {
    blake3::hash(s.as_bytes()).to_hex().to_string()
}

/// Resolve one `(ref_id, hook)` against `incoming_source`, mutating
/// `world`'s Program and (when `record_db`) `db`'s version log and import
/// baseline as needed. `object_is_new` short-circuits straight to `New` —
/// a freshly created object has nothing to compare against.
fn resolve_one_program(
    ref_id: &str,
    hook: &str,
    incoming_source: &str,
    world: &mut World,
    db: &Database,
    record_db: bool,
    author: Option<&str>,
    object_is_new: bool,
) -> Result<ProgOutcome, String> {
    let incoming_hash = blake3_hex(incoming_source);

    let current_source = if object_is_new {
        None
    } else {
        world.get(ref_id).and_then(|o| o.programs.get(hook)).map(|p| p.source.clone())
    };

    let Some(current_source) = current_source else {
        let obj = world
            .get_mut(ref_id)
            .ok_or_else(|| format!("No object with ref '{}'", ref_id))?;
        hooks::set_program(obj, hook, incoming_source.to_string())?;
        if record_db {
            db.record_program_version(ref_id, hook, incoming_source, author)
                .map_err(|e| e.to_string())?;
            db.set_import_hash(ref_id, hook, &incoming_hash).map_err(|e| e.to_string())?;
        }
        return Ok(ProgOutcome::New);
    };

    let current_hash = blake3_hex(&current_source);
    if current_hash == incoming_hash {
        // Already matches — but if this hook has never gone through an
        // import before, stamp a baseline now so a *future* local edit has
        // something to be compared against.
        if record_db {
            let recorded = db.get_import_hash(ref_id, hook).map_err(|e| e.to_string())?;
            if recorded.as_deref() != Some(incoming_hash.as_str()) {
                db.set_import_hash(ref_id, hook, &incoming_hash).map_err(|e| e.to_string())?;
            }
        }
        return Ok(ProgOutcome::Unchanged);
    }

    let recorded = db.get_import_hash(ref_id, hook).map_err(|e| e.to_string())?;
    let outcome = match recorded {
        None => ProgOutcome::Conflict,
        Some(ref rec) if rec == &current_hash => ProgOutcome::Overwritten,
        Some(ref rec) if rec == &incoming_hash => ProgOutcome::KeptLocal,
        Some(_) => ProgOutcome::Conflict,
    };

    match outcome {
        ProgOutcome::Overwritten | ProgOutcome::Conflict => {
            if record_db {
                if outcome == ProgOutcome::Conflict {
                    // Guarantee the local edit about to be replaced is in
                    // the version log, regardless of how it got there — see
                    // the plan's "the local version is preserved in the
                    // stage 3 version log."
                    db.record_program_version(ref_id, hook, &current_source, author)
                        .map_err(|e| e.to_string())?;
                }
                db.record_program_version(ref_id, hook, incoming_source, author)
                    .map_err(|e| e.to_string())?;
                db.set_import_hash(ref_id, hook, &incoming_hash).map_err(|e| e.to_string())?;
            }
            let obj = world
                .get_mut(ref_id)
                .ok_or_else(|| format!("No object with ref '{}'", ref_id))?;
            hooks::set_program(obj, hook, incoming_source.to_string())?;
        }
        ProgOutcome::KeptLocal | ProgOutcome::Unchanged | ProgOutcome::New => {}
    }
    Ok(outcome)
}

/// Resolve every hook in `programs` against `ref_id`, appending
/// [`ProgramNote`]s to `report` for the informational buckets. Returns
/// whether anything actually changed, so the caller can decide whether the
/// owning object counts as `updated` or `unchanged`.
#[allow(clippy::too_many_arguments)]
fn resolve_programs(
    ref_id: &str,
    programs: &HashMap<String, ProgramSource>,
    base_dir: &Path,
    world: &mut World,
    db: &Database,
    record_db: bool,
    author: Option<&str>,
    object_is_new: bool,
    report: &mut ImportReport,
) -> Result<bool, String> {
    let mut any_changed = false;
    for (hook, source_def) in programs {
        let source = source_def.resolve(base_dir)?;
        let outcome = resolve_one_program(
            ref_id, hook, &source, world, db, record_db, author, object_is_new,
        )?;
        match outcome {
            ProgOutcome::New | ProgOutcome::Overwritten => any_changed = true,
            ProgOutcome::Conflict => {
                any_changed = true;
                report.conflicts.push(ProgramNote { ref_id: ref_id.to_string(), hook: hook.clone() });
            }
            ProgOutcome::KeptLocal => {
                report.kept_local.push(ProgramNote { ref_id: ref_id.to_string(), hook: hook.clone() });
            }
            ProgOutcome::Unchanged => {}
        }
    }
    Ok(any_changed)
}

/// The create/update pass. When `record_db` is `false` this is exactly the
/// dry-run computation — `world` is a scratch clone in that case (see
/// `import_bundle`), so every mutation here is safe to always perform;
/// only the `db` writes are gated.
fn apply(
    areas: &[loader::ParsedArea],
    world: &mut World,
    db: &Database,
    record_db: bool,
    author: Option<&str>,
) -> Result<ImportReport, String> {
    let mut report = ImportReport::default();

    let mut key_map: HashMap<String, String> = world
        .objects
        .values()
        .filter_map(|o| {
            o.attrs
                .get(loader::FILE_KEY_ATTR)
                .and_then(|v| v.as_str())
                .map(|k| (k.to_string(), o.ref_id.clone()))
        })
        .collect();

    let bundle_areas: HashSet<String> = areas.iter().map(|a| a.area_name.clone()).collect();
    let mut incoming_keys: HashSet<String> = HashSet::new();

    // Pass 1: reserve dbrefs for every room/object identity up front, so
    // pass 2 can resolve forward and cross-area references regardless of
    // file order — same reasoning as `load_game_dir`.
    for area in areas {
        for room in &area.file.rooms {
            key_map
                .entry(format!("{}/{}", area.area_name, room.key))
                .or_insert_with(|| world.next_dbref());
        }
        for object in &area.file.objects {
            key_map
                .entry(format!("{}/{}", area.area_name, object.key))
                .or_insert_with(|| world.next_dbref());
        }
    }

    // Pass 2: rooms and objects.
    for area in areas {
        for room in &area.file.rooms {
            let fk = format!("{}/{}", area.area_name, room.key);
            incoming_keys.insert(fk.clone());
            let ref_id = key_map[&fk].clone();
            let is_new = world.get(&ref_id).is_none();
            let mut changed = is_new;

            if is_new {
                let mut obj = GameObject::new(&ref_id, &room.key, Kind::Room).with_title(&room.title);
                obj.description = room.description.clone();
                for t in &room.tags {
                    if let Ok(tag) = Tag::parse(t) {
                        obj.tags.insert(tag);
                    }
                }
                obj.locks = room.locks.clone();
                obj.attrs.insert(loader::FILE_KEY_ATTR.into(), serde_json::json!(fk));
                world.add_object(obj);
            } else {
                let obj = world.get_mut(&ref_id).unwrap();
                if obj.title.as_deref() != Some(room.title.as_str()) {
                    obj.title = Some(room.title.clone());
                    changed = true;
                }
                if obj.description != room.description {
                    obj.description = room.description.clone();
                    changed = true;
                }
                if obj.locks != room.locks {
                    obj.locks = room.locks.clone();
                    changed = true;
                }
                for t in &room.tags {
                    if let Ok(tag) = Tag::parse(t)
                        && obj.tags.insert(tag)
                    {
                        changed = true;
                    }
                }
            }

            let prog_changed = resolve_programs(
                &ref_id, &room.programs, &area.base_dir, world, db, record_db, author, is_new, &mut report,
            )?;
            changed |= prog_changed;

            let label = format!("{} (room)", fk);
            if is_new {
                report.created.push(label);
            } else if changed {
                report.updated.push(label);
            } else {
                report.unchanged.push(label);
            }
        }

        for object in &area.file.objects {
            let fk = format!("{}/{}", area.area_name, object.key);
            incoming_keys.insert(fk.clone());
            let ref_id = key_map[&fk].clone();
            let kind = Kind::parse(&object.kind)
                .ok_or_else(|| format!("Unknown kind '{}' for '{}'", object.kind, fk))?;
            let location_ref = match &object.location {
                Some(loc) => Some(loader::resolve_key(&area.area_name, loc, &key_map)?),
                None => None,
            };
            let is_new = world.get(&ref_id).is_none();
            let mut changed = is_new;

            if is_new {
                let mut obj = GameObject::new(&ref_id, &object.key, kind);
                if let Some(title) = &object.title {
                    obj = obj.with_title(title);
                }
                obj.description = object.description.clone();
                if let Some(loc) = &location_ref {
                    obj = obj.with_location(loc.clone());
                }
                for t in &object.tags {
                    if let Ok(tag) = Tag::parse(t) {
                        obj.tags.insert(tag);
                    }
                }
                obj.attrs = object.attrs.clone();
                obj.attrs.insert(loader::FILE_KEY_ATTR.into(), serde_json::json!(fk));
                obj.locks = object.locks.clone();
                world.add_object(obj);
            } else {
                let obj = world.get_mut(&ref_id).unwrap();
                if let Some(title) = &object.title
                    && obj.title.as_deref() != Some(title.as_str())
                {
                    obj.title = Some(title.clone());
                    changed = true;
                }
                if obj.description != object.description {
                    obj.description = object.description.clone();
                    changed = true;
                }
                if let Some(loc) = &location_ref
                    && obj.location_ref.as_deref() != Some(loc.as_str())
                {
                    obj.location_ref = Some(loc.clone());
                    changed = true;
                }
                for (k, v) in &object.attrs {
                    if obj.attrs.get(k) != Some(v) {
                        obj.attrs.insert(k.clone(), v.clone());
                        changed = true;
                    }
                }
                if obj.locks != object.locks {
                    obj.locks = object.locks.clone();
                    changed = true;
                }
                for t in &object.tags {
                    if let Ok(tag) = Tag::parse(t)
                        && obj.tags.insert(tag)
                    {
                        changed = true;
                    }
                }
            }

            let prog_changed = resolve_programs(
                &ref_id, &object.programs, &area.base_dir, world, db, record_db, author, is_new, &mut report,
            )?;
            changed |= prog_changed;

            let label = format!("{} ({})", fk, object.kind);
            if is_new {
                report.created.push(label);
            } else if changed {
                report.updated.push(label);
            } else {
                report.unchanged.push(label);
            }
        }
    }

    // Pass 3: exits and scripts — both may reference rooms/objects, so wait
    // until every room/object identity exists.
    for area in areas {
        for exit in &area.file.exits {
            let from = loader::resolve_key(&area.area_name, &exit.from, &key_map)?;
            let to = loader::resolve_key(&area.area_name, &exit.to, &key_map)?;
            let fk = format!("{}/exit/{}/{}", area.area_name, exit.from, exit.direction);
            incoming_keys.insert(fk.clone());

            let (ref_id, is_new) = match key_map.get(&fk).cloned() {
                Some(r) if world.get(&r).is_some() => (r, false),
                _ => {
                    let r = world.next_dbref();
                    key_map.insert(fk.clone(), r.clone());
                    (r, true)
                }
            };
            let mut changed = is_new;

            if is_new {
                let mut obj = GameObject::new(&ref_id, &exit.direction, Kind::Exit)
                    .with_location(&from)
                    .with_target(&to);
                obj.aliases = exit.aliases.iter().cloned().collect();
                obj.locks = exit.locks.clone();
                obj.attrs.insert(loader::FILE_KEY_ATTR.into(), serde_json::json!(fk));
                world.add_object(obj);
            } else {
                let obj = world.get_mut(&ref_id).unwrap();
                if obj.location_ref.as_deref() != Some(from.as_str()) {
                    obj.location_ref = Some(from.clone());
                    changed = true;
                }
                if obj.target_ref.as_deref() != Some(to.as_str()) {
                    obj.target_ref = Some(to.clone());
                    changed = true;
                }
                let new_aliases: HashSet<String> = exit.aliases.iter().cloned().collect();
                if obj.aliases != new_aliases {
                    obj.aliases = new_aliases;
                    changed = true;
                }
                if obj.locks != exit.locks {
                    obj.locks = exit.locks.clone();
                    changed = true;
                }
            }

            let label = format!("{} (exit)", fk);
            if is_new {
                report.created.push(label);
            } else if changed {
                report.updated.push(label);
            } else {
                report.unchanged.push(label);
            }
        }

        for script in &area.file.scripts {
            let fk = format!("{}/script/{}", area.area_name, script.name);
            incoming_keys.insert(fk.clone());

            let (ref_id, is_new) = match key_map.get(&fk).cloned() {
                Some(r) if world.get(&r).is_some() => (r, false),
                _ => {
                    let r = world.next_dbref();
                    key_map.insert(fk.clone(), r.clone());
                    (r, true)
                }
            };
            let mut changed = is_new;

            if is_new {
                let mut obj = GameObject::new(&ref_id, &script.name, Kind::Code);
                obj.attrs.insert("tick_interval".into(), serde_json::json!(script.interval));
                obj.attrs.insert(loader::FILE_KEY_ATTR.into(), serde_json::json!(fk));
                world.add_object(obj);
            } else {
                let obj = world.get_mut(&ref_id).unwrap();
                let interval_json = serde_json::json!(script.interval);
                if obj.attrs.get("tick_interval") != Some(&interval_json) {
                    obj.attrs.insert("tick_interval".into(), interval_json);
                    changed = true;
                }
            }

            let mut programs = HashMap::new();
            programs.insert("on_tick".to_string(), script.source.clone());
            let prog_changed = resolve_programs(
                &ref_id, &programs, &area.base_dir, world, db, record_db, author, is_new, &mut report,
            )?;
            changed |= prog_changed;

            let label = format!("{} (script)", fk);
            if is_new {
                report.created.push(label);
            } else if changed {
                report.updated.push(label);
            } else {
                report.unchanged.push(label);
            }
        }
    }

    // Case 4: in the DB (under one of this bundle's areas), gone from the
    // bundle. Reported, never removed.
    for obj in world.objects.values() {
        if let Some(fk) = obj.attrs.get(loader::FILE_KEY_ATTR).and_then(|v| v.as_str()) {
            let area_prefix = fk.split('/').next().unwrap_or("");
            if bundle_areas.contains(area_prefix) && !incoming_keys.contains(fk) {
                report.missing.push(fk.to_string());
            }
        }
    }

    Ok(report)
}

// -- Export --

/// What `@export` did.
#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    pub written_areas: Vec<String>,
    pub objects_written: usize,
    /// Objects that could not be represented — see the module's export
    /// limitations. `"<file_key> (<reason>)"`.
    pub skipped: Vec<String>,
}

pub fn render_export_report(report: &ExportReport, path: &str) -> String {
    let mut out = format!("Export to {}:\r\n", path);
    let mut areas = report.written_areas.clone();
    areas.sort();
    out.push_str(&format!(
        "  {} area(s) written: {}\r\n",
        areas.len(),
        if areas.is_empty() { "(none)".to_string() } else { areas.join(", ") }
    ));
    out.push_str(&format!("  {} object(s) written\r\n", report.objects_written));
    if !report.skipped.is_empty() {
        out.push_str(&format!(
            "  {} object(s) skipped (cannot be represented as files):\r\n",
            report.skipped.len()
        ));
        for s in &report.skipped {
            out.push_str(&format!("    ? {}\r\n", s));
        }
    }
    out
}

/// Replace any character that isn't safe in a filename with `_`, so an
/// object key can be used to build a `.luau` filename without escaping.
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

/// Resolve a dbref to the file-key-relative reference `@import` would parse
/// it back from — bare (`"crossroads"`) if `dbref` belongs to `exporting_area`,
/// fully qualified (`"forest/edge"`) otherwise. Mirrors `loader::resolve_key`
/// in reverse.
///
/// Known limitation: this only recovers the *canonical* bare-or-qualified
/// form. If a bundle's author wrote a redundant same-area qualified
/// reference (e.g. `to = "town/crossroads"` from within the `"town"` area,
/// where a bare `"crossroads"` would have resolved identically), export
/// normalizes it to the bare form — round-tripping still produces a
/// *semantically* identical bundle, but the exit's own file-key identity
/// would then differ from what a hand-written original might have used.
fn resolve_ref_to_key(dbref: &str, exporting_area: &str, ref_to_key: &HashMap<&str, &str>) -> Option<String> {
    let fk = *ref_to_key.get(dbref)?;
    let (area, key) = fk.split_once('/')?;
    if area == exporting_area {
        Some(key.to_string())
    } else {
        Some(fk.to_string())
    }
}

/// Area an object lands in when export can't derive a real one for it — a
/// freshly `@dig`ged room, a `@script`/`@lib` object (never has a
/// location), or a container chain that bottoms out with no room
/// underneath. Reported via `written_areas`, never silently dropped — see
/// `resolve_export_area` and the plan's "Export is not optional."
const FALLBACK_AREA: &str = "unfiled";

/// Turn arbitrary text into a lowercase, filesystem/TOML-safe slug: ASCII
/// alphanumerics survive, every other run of characters becomes a single
/// `-` (leading/trailing dashes trim away). Empty input, or input with no
/// alphanumeric characters at all, falls back to `"obj"` so a stamped key
/// is never an empty path segment.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = true; // suppresses a leading '-'
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "obj".to_string()
    } else {
        out
    }
}

/// Pick the first `"{area}/{sub}/{base}"` (`sub` may be empty, giving
/// `"{area}/{base}"`) not already in `used`, trying `base`, `base-2`,
/// `base-3`, ... and reserving the winner in `used` before returning its
/// bare (non-area-qualified) form. Deterministic for a given `used` and
/// processing order, which is what makes re-exporting an unchanged world
/// assign the same disambiguated keys every time — see
/// `stamp_missing_identities`'s fixed kind ordering.
fn dedupe_key(area: &str, sub: &str, base: &str, used: &mut HashSet<String>) -> String {
    let mut n = 1u32;
    loop {
        let candidate = if n == 1 { base.to_string() } else { format!("{}-{}", base, n) };
        let full = if sub.is_empty() {
            format!("{}/{}", area, candidate)
        } else {
            format!("{}/{}/{}", area, sub, candidate)
        };
        if used.insert(full) {
            return candidate;
        }
        n += 1;
    }
}

/// Walk `location_ref` from `ref_id` up to the nearest ancestor that
/// already carries a `FILE_KEY_ATTR` (its area is taken directly), or that
/// is a `Room`/`Code` object with none (which always resolves to
/// [`FALLBACK_AREA`] — neither kind has anywhere else to inherit an area
/// from). Passes straight through a carrying player (players are not
/// terminal — an item a player is holding still belongs to whatever room
/// the player is standing in) and through any number of nested
/// containers. A dangling reference or a `location_ref` cycle (guarded by
/// a depth cap) also resolves to [`FALLBACK_AREA`] rather than failing.
fn resolve_export_area(world: &World, ref_id: &str) -> String {
    let mut current = ref_id.to_string();
    for _ in 0..64 {
        let Some(obj) = world.get(&current) else {
            return FALLBACK_AREA.to_string();
        };
        if let Some(fk) = obj.attrs.get(loader::FILE_KEY_ATTR).and_then(|v| v.as_str()) {
            return fk.split('/').next().unwrap_or(FALLBACK_AREA).to_string();
        }
        if matches!(obj.kind, Kind::Room | Kind::Code) {
            return FALLBACK_AREA.to_string();
        }
        match &obj.location_ref {
            Some(loc) => current = loc.clone(),
            None => return FALLBACK_AREA.to_string(),
        }
    }
    FALLBACK_AREA.to_string()
}

/// Assign a `FILE_KEY_ATTR` identity to every exportable object that lacks
/// one — see the module docs, `export_bundle`'s doc comment, and the
/// plan's "Export is not optional." `Kind::Player` is never touched here;
/// `export_bundle` excludes it before this even matters.
///
/// This is the one place `@export` is not purely read-only: it mutates
/// `world` directly so the assigned identity is stable rather than
/// re-derived (and potentially re-disambiguated differently) on every
/// export. Processing order is fixed — Room, then Item/Npc, then Code,
/// then Exit last — because an Exit's key embeds its source room's key
/// text and so needs that room already resolved; nothing else depends on
/// processing order (see `resolve_export_area`, which reads ancestor
/// identity rather than an in-progress assignment).
fn stamp_missing_identities(world: &mut World) {
    let mut used_keys: HashSet<String> = world
        .objects
        .values()
        .filter_map(|o| o.attrs.get(loader::FILE_KEY_ATTR).and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .collect();

    // Deterministic order: numeric dbref, so re-running export against an
    // unchanged world always disambiguates collisions the same way.
    let mut refs: Vec<String> = world
        .objects
        .values()
        .filter(|o| o.kind != Kind::Player && !o.attrs.contains_key(loader::FILE_KEY_ATTR))
        .map(|o| o.ref_id.clone())
        .collect();
    refs.sort_by_key(|r| r.trim_start_matches('#').parse::<u64>().unwrap_or(u64::MAX));

    for ref_id in &refs {
        if world.get(ref_id).map(|o| o.kind.clone()) != Some(Kind::Room) {
            continue;
        }
        let (title, key_field) = {
            let obj = world.get(ref_id).unwrap();
            (obj.title.clone(), obj.key.clone())
        };
        let base = slugify(title.as_deref().unwrap_or(&key_field));
        let key = dedupe_key(FALLBACK_AREA, "", &base, &mut used_keys);
        let fk = format!("{}/{}", FALLBACK_AREA, key);
        world.get_mut(ref_id).unwrap().attrs.insert(loader::FILE_KEY_ATTR.into(), serde_json::json!(fk));
    }

    for ref_id in &refs {
        let kind = world.get(ref_id).map(|o| o.kind.clone());
        if !matches!(kind, Some(Kind::Item) | Some(Kind::Npc)) {
            continue;
        }
        let area = resolve_export_area(world, ref_id);
        let (title, key_field) = {
            let obj = world.get(ref_id).unwrap();
            (obj.title.clone(), obj.key.clone())
        };
        let base = slugify(title.as_deref().unwrap_or(&key_field));
        let key = dedupe_key(&area, "", &base, &mut used_keys);
        let fk = format!("{}/{}", area, key);
        world.get_mut(ref_id).unwrap().attrs.insert(loader::FILE_KEY_ATTR.into(), serde_json::json!(fk));
    }

    for ref_id in &refs {
        if world.get(ref_id).map(|o| o.kind.clone()) != Some(Kind::Code) {
            continue;
        }
        let key_field = world.get(ref_id).unwrap().key.clone();
        let base = slugify(&key_field);
        let key = dedupe_key(FALLBACK_AREA, "script", &base, &mut used_keys);
        let fk = format!("{}/script/{}", FALLBACK_AREA, key);
        world.get_mut(ref_id).unwrap().attrs.insert(loader::FILE_KEY_ATTR.into(), serde_json::json!(fk));
    }

    // Exits last: their key embeds the source room's key, which must
    // already be resolved (either pre-existing, or just stamped above).
    for ref_id in &refs {
        if world.get(ref_id).map(|o| o.kind.clone()) != Some(Kind::Exit) {
            continue;
        }
        let (direction, room_ref) = {
            let obj = world.get(ref_id).unwrap();
            (obj.key.clone(), obj.location_ref.clone())
        };
        let Some(room_ref) = room_ref else {
            // No source room at all — nothing to key this against.
            // export_bundle reports it in `skipped` rather than silently
            // dropping it.
            continue;
        };
        let room_fk = world
            .get(&room_ref)
            .and_then(|r| r.attrs.get(loader::FILE_KEY_ATTR))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let (area, room_key) = match &room_fk {
            Some(fk) => {
                let (a, k) = fk.split_once('/').unwrap_or((FALLBACK_AREA, fk.as_str()));
                (a.to_string(), k.to_string())
            }
            // The source room isn't itself exportable (e.g. it was
            // destroyed but the exit lingers) — key against its raw
            // dbref so the exit still gets *something* rather than being
            // silently skipped by this pass (export_bundle's own
            // location resolution will separately catch and report it).
            None => (FALLBACK_AREA.to_string(), room_ref.clone()),
        };
        let candidate = format!("{}/exit/{}/{}", area, room_key, direction);
        if used_keys.insert(candidate.clone()) {
            world.get_mut(ref_id).unwrap().attrs.insert(loader::FILE_KEY_ATTR.into(), serde_json::json!(candidate));
        }
        // else: another exit already claimed this exact room+direction
        // (two exits with the same direction from the same room) — leave
        // unstamped. export_bundle reports it in `skipped` rather than
        // guessing at a disambiguated direction, which would change what
        // players type to use it.
    }
}

/// Emit every non-`Player` object in `world` back to
/// `<path>/<area>/<area>.toml` plus sibling `.luau` files, one area per
/// directory — the mirror of `@import`. Unlike a pure read, this can
/// mutate `world` first: see [`stamp_missing_identities`], called at the
/// top of this function, which gives any object lacking a `FILE_KEY_ATTR`
/// identity (anything created in-game via `@create`, `@dig`, `@script`,
/// `@lib`, etc.) a stable, deterministically-derived one — because once
/// `load_world_files` is off, `@export` is the only way such an object
/// survives a lost database. See the module docs and the plan's "Export
/// is not optional."
///
/// `Kind::Player` is excluded explicitly, not incidentally: player
/// objects are account-linked runtime state, not world content, and are
/// never exported regardless of whether they carry an identity.
///
/// An object's area is its own `FILE_KEY_ATTR` area if it has one,
/// otherwise the containing room found by walking `location_ref` — see
/// `resolve_export_area` for the exact rule, including nested containers
/// and the [`FALLBACK_AREA`] catch-all.
pub fn export_bundle(path: &Path, world: &mut World) -> Result<ExportReport, String> {
    stamp_missing_identities(world);

    let mut report = ExportReport::default();
    let mut by_area: HashMap<String, Vec<&GameObject>> = HashMap::new();
    for obj in world.objects.values() {
        if obj.kind == Kind::Player {
            // Account-linked runtime state, not world content — see this
            // function's doc comment. Excluded here explicitly rather
            // than by the incidental fact that nothing ever gave a
            // player a FILE_KEY_ATTR.
            continue;
        }
        match obj.attrs.get(loader::FILE_KEY_ATTR).and_then(|v| v.as_str()) {
            Some(fk) => {
                let area = fk.split('/').next().unwrap_or(FALLBACK_AREA).to_string();
                by_area.entry(area).or_default().push(obj);
            }
            None => {
                // Only reachable for an Exit that collided with another
                // exit's derived key during stamping (see
                // `stamp_missing_identities`) — two exits with the same
                // direction from the same room. Reported, not silently
                // dropped.
                report.skipped.push(format!(
                    "{} (exit key collision — another exit already uses this room and direction)",
                    obj.ref_id
                ));
            }
        }
    }

    let ref_to_key: HashMap<&str, &str> = world
        .objects
        .values()
        .filter_map(|o| {
            o.attrs
                .get(loader::FILE_KEY_ATTR)
                .and_then(|v| v.as_str())
                .map(|k| (o.ref_id.as_str(), k))
        })
        .collect();

    std::fs::create_dir_all(path).map_err(|e| format!("Failed to create {}: {}", path.display(), e))?;

    let mut area_names: Vec<&String> = by_area.keys().collect();
    area_names.sort();

    for area_name in area_names {
        let objs = &by_area[area_name];
        let area_dir = path.join(area_name);
        std::fs::create_dir_all(&area_dir)
            .map_err(|e| format!("Failed to create {}: {}", area_dir.display(), e))?;

        let mut area_file = loader::AreaFile {
            area: Some(area_name.clone()),
            rooms: Vec::new(),
            objects: Vec::new(),
            exits: Vec::new(),
            scripts: Vec::new(),
        };

        let mut sorted = objs.clone();
        sorted.sort_by_key(|o| {
            o.attrs.get(loader::FILE_KEY_ATTR).and_then(|v| v.as_str()).unwrap_or("").to_string()
        });

        for obj in sorted {
            let fk = obj
                .attrs
                .get(loader::FILE_KEY_ATTR)
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string();

            match obj.kind {
                Kind::Exit => {
                    let from_key = obj
                        .location_ref
                        .as_deref()
                        .and_then(|r| resolve_ref_to_key(r, area_name, &ref_to_key));
                    let to_key = obj
                        .target_ref
                        .as_deref()
                        .and_then(|r| resolve_ref_to_key(r, area_name, &ref_to_key));
                    let (Some(from_key), Some(to_key)) = (from_key, to_key) else {
                        report.skipped.push(format!("{} (unresolvable from/to)", fk));
                        continue;
                    };
                    let mut aliases: Vec<String> = obj.aliases.iter().cloned().collect();
                    aliases.sort();
                    area_file.exits.push(loader::ExitDef {
                        from: from_key,
                        direction: obj.key.clone(),
                        to: to_key,
                        aliases,
                        locks: obj.locks.clone(),
                    });
                    report.objects_written += 1;
                }
                Kind::Code => {
                    let interval = obj.attrs.get("tick_interval").and_then(|v| v.as_u64()).unwrap_or(1);
                    let name = fk.rsplit('/').next().unwrap_or(&obj.key).to_string();
                    let source = obj.programs.get("on_tick").map(|p| p.source.clone()).unwrap_or_default();
                    let file_name = format!("{}__on_tick.luau", sanitize_filename(&name));
                    std::fs::write(area_dir.join(&file_name), &source)
                        .map_err(|e| format!("Failed to write {}: {}", file_name, e))?;
                    area_file.scripts.push(loader::ScriptDef {
                        name,
                        interval,
                        source: ProgramSource::File { file: file_name },
                    });
                    report.objects_written += 1;
                }
                Kind::Room => {
                    // Last segment of the (possibly freshly-stamped) file
                    // key, not `obj.key` directly — the two only diverge
                    // for an object `stamp_missing_identities` just gave
                    // an identity to, and using the stamped slug here
                    // (rather than mutating `obj.key`, which is what
                    // players type to reference the object in `get`/
                    // `drop`/`examine`) keeps stamping from having any
                    // live gameplay side effect.
                    let key = fk.rsplit('/').next().unwrap_or(&obj.key).to_string();
                    let programs = write_programs(obj, &key, &area_dir)?;
                    let mut tags: Vec<String> = obj.tags.iter().map(|t| t.as_spec()).collect();
                    tags.sort();
                    area_file.rooms.push(loader::RoomDef {
                        key,
                        title: obj.title.clone().unwrap_or_default(),
                        description: obj.description.clone(),
                        tags,
                        locks: obj.locks.clone(),
                        programs,
                    });
                    report.objects_written += 1;
                }
                Kind::Item | Kind::Npc => {
                    let location = match &obj.location_ref {
                        Some(r) => match resolve_ref_to_key(r, area_name, &ref_to_key) {
                            Some(k) => Some(k),
                            None => {
                                let reason = if world
                                    .get(r)
                                    .map(|o| o.kind == Kind::Player)
                                    .unwrap_or(false)
                                {
                                    "carried by a player, not exportable"
                                } else {
                                    "unresolvable location"
                                };
                                report.skipped.push(format!("{} ({})", fk, reason));
                                continue;
                            }
                        },
                        None => None,
                    };
                    // See the note on the `Kind::Room` arm above.
                    let key = fk.rsplit('/').next().unwrap_or(&obj.key).to_string();
                    let programs = write_programs(obj, &key, &area_dir)?;
                    let mut tags: Vec<String> = obj.tags.iter().map(|t| t.as_spec()).collect();
                    tags.sort();
                    let mut attrs = obj.attrs.clone();
                    attrs.remove(loader::FILE_KEY_ATTR);
                    area_file.objects.push(loader::ObjectDef {
                        key,
                        kind: obj.kind.to_string(),
                        title: obj.title.clone(),
                        description: obj.description.clone(),
                        location,
                        tags,
                        attrs,
                        locks: obj.locks.clone(),
                        programs,
                    });
                    report.objects_written += 1;
                }
                Kind::Player => unreachable!(
                    "Kind::Player is filtered out before `by_area` is built, above"
                ),
            }
        }

        let toml_text = toml::to_string_pretty(&area_file)
            .map_err(|e| format!("Failed to serialize area '{}': {}", area_name, e))?;
        let toml_path = area_dir.join(format!("{}.toml", area_name));
        std::fs::write(&toml_path, toml_text)
            .map_err(|e| format!("Failed to write {}: {}", toml_path.display(), e))?;
        report.written_areas.push(area_name.clone());
    }

    Ok(report)
}

/// Write every Program on `obj` to a sibling `.luau` file under `area_dir`
/// and return the `{hook: ProgramSource::File}` map to embed in its
/// `RoomDef`/`ObjectDef`. Always emits file references, never inline
/// source — matches the predominant style game authors already use (see
/// `the-last-stag-mud`) and avoids TOML string-escaping edge cases for
/// multi-line Luau.
///
/// `export_key` (the last segment of the object's `FILE_KEY_ATTR`, not
/// `obj.key`) names the file: `obj.key` is only guaranteed unique for an
/// object that came from a file import (`check_collisions` enforces that
/// at import time) — `@create` sets it from a lowercased title with no
/// uniqueness check at all, so two ad-hoc objects can easily share one
/// (two things both named "Crate"). Using `obj.key` here would let a
/// second object's Program silently overwrite the first's `.luau` file on
/// disk the moment both exported into the same area; `export_key` is
/// already unique within the area by construction (see `dedupe_key`).
fn write_programs(
    obj: &GameObject,
    export_key: &str,
    area_dir: &Path,
) -> Result<HashMap<String, ProgramSource>, String> {
    let mut programs = HashMap::new();
    for (hook, prog) in &obj.programs {
        let file_name = format!("{}__{}.luau", sanitize_filename(export_key), hook);
        std::fs::write(area_dir.join(&file_name), &prog.source)
            .map_err(|e| format!("Failed to write {}: {}", file_name, e))?;
        programs.insert(hook.clone(), ProgramSource::File { file: file_name });
    }
    Ok(programs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("hearth-import-test-{}-{}", std::process::id(), n));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn write(&self, subdir: &str, filename: &str, contents: &str) {
            let dir = self.path.join(subdir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(filename), contents).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn temp_db() -> Database {
        Database::open(Path::new(":memory:")).unwrap()
    }

    fn town_toml(crossroads_desc: &str) -> String {
        format!(
            r#"
                area = "town"

                [[rooms]]
                key = "crossroads"
                title = "The Crossroads"
                description = "{crossroads_desc}"

                [[rooms]]
                key = "square"
                title = "Town Square"
                description = "A square."

                [[objects]]
                key = "sign"
                kind = "item"
                title = "a sign"
                location = "crossroads"
                [objects.programs]
                on_look = {{ source = "function on_look() return 'a sign' end" }}

                [[exits]]
                from = "crossroads"
                direction = "north"
                to = "square"
                aliases = ["n"]
            "#
        )
    }

    #[test]
    fn import_creates_objects() {
        let dir = TempDir::new();
        dir.write("town", "town.toml", &town_toml("A dusty crossroads."));
        let db = temp_db();
        let mut world = World::new();

        let report = import_bundle(&dir.path, &mut world, &db, false, Some("#1")).unwrap();

        assert_eq!(report.created.len(), 4, "{:?}", report);
        assert!(report.updated.is_empty());
        assert!(report.unchanged.is_empty());

        let crossroads = world
            .objects
            .values()
            .find(|o| o.attrs.get("_file_key").and_then(|v| v.as_str()) == Some("town/crossroads"))
            .expect("crossroads should exist");
        assert_eq!(crossroads.title.as_deref(), Some("The Crossroads"));
        assert_eq!(crossroads.kind, Kind::Room);

        let sign = world
            .objects
            .values()
            .find(|o| o.key == "sign")
            .expect("sign should exist");
        assert!(sign.programs.contains_key("on_look"));
        assert_eq!(sign.location_ref.as_deref(), Some(crossroads.ref_id.as_str()));

        let exit = world.exits_from(&crossroads.ref_id);
        assert_eq!(exit.len(), 1);
        assert!(exit[0].aliases.contains("n"));
    }

    #[test]
    fn reimport_is_idempotent() {
        let dir = TempDir::new();
        dir.write("town", "town.toml", &town_toml("A dusty crossroads."));
        let db = temp_db();
        let mut world = World::new();

        import_bundle(&dir.path, &mut world, &db, false, None).unwrap();
        let object_count = world.objects.len();
        let next_id = world.next_id;

        let report = import_bundle(&dir.path, &mut world, &db, false, None).unwrap();

        assert!(report.created.is_empty(), "{:?}", report);
        assert!(report.updated.is_empty(), "{:?}", report);
        assert_eq!(report.unchanged.len(), 4);
        assert_eq!(world.objects.len(), object_count, "reimport must not duplicate objects");
        assert_eq!(world.next_id, next_id, "reimport of unchanged content must not mint new dbrefs");
    }

    #[test]
    fn three_way_case_upstream_changed_local_untouched_overwrites() {
        let dir = TempDir::new();
        dir.write("town", "town.toml", &town_toml("Old description."));
        let db = temp_db();
        let mut world = World::new();
        import_bundle(&dir.path, &mut world, &db, false, None).unwrap();

        let sign_ref = world.objects.values().find(|o| o.key == "sign").unwrap().ref_id.clone();

        // Upstream changes the sign's on_look program; nobody touched it locally.
        dir.write(
            "town",
            "town.toml",
            &town_toml("Old description.").replace(
                "function on_look() return 'a sign' end",
                "function on_look() return 'an updated sign' end",
            ),
        );

        let report = import_bundle(&dir.path, &mut world, &db, false, None).unwrap();
        assert!(report.conflicts.is_empty(), "{:?}", report);
        assert!(report.kept_local.is_empty(), "{:?}", report);
        let sign = world.get(&sign_ref).unwrap();
        assert!(sign.programs["on_look"].source.contains("an updated sign"));
    }

    #[test]
    fn three_way_case_local_changed_upstream_untouched_keeps_local() {
        let dir = TempDir::new();
        dir.write("town", "town.toml", &town_toml("Old description."));
        let db = temp_db();
        let mut world = World::new();
        import_bundle(&dir.path, &mut world, &db, false, None).unwrap();

        let sign_ref = world.objects.values().find(|o| o.key == "sign").unwrap().ref_id.clone();
        {
            let obj = world.get_mut(&sign_ref).unwrap();
            hooks::set_program(obj, "on_look", "function on_look() return 'builder edit' end".into()).unwrap();
        }

        // Re-import the exact same bundle — upstream unchanged.
        let report = import_bundle(&dir.path, &mut world, &db, false, None).unwrap();

        assert_eq!(report.kept_local.len(), 1, "{:?}", report);
        assert!(report.conflicts.is_empty(), "{:?}", report);
        let sign = world.get(&sign_ref).unwrap();
        assert!(
            sign.programs["on_look"].source.contains("builder edit"),
            "local edit must survive when upstream did not change"
        );
    }

    #[test]
    fn three_way_case_both_changed_is_a_conflict_and_preserves_history() {
        let dir = TempDir::new();
        dir.write("town", "town.toml", &town_toml("Old description."));
        let db = temp_db();
        let mut world = World::new();
        import_bundle(&dir.path, &mut world, &db, false, Some("#7")).unwrap();

        let sign_ref = world.objects.values().find(|o| o.key == "sign").unwrap().ref_id.clone();
        {
            let obj = world.get_mut(&sign_ref).unwrap();
            hooks::set_program(obj, "on_look", "function on_look() return 'builder edit' end".into()).unwrap();
        }

        // Upstream *also* changes it.
        dir.write(
            "town",
            "town.toml",
            &town_toml("Old description.").replace(
                "function on_look() return 'a sign' end",
                "function on_look() return 'upstream edit' end",
            ),
        );

        let report = import_bundle(&dir.path, &mut world, &db, false, Some("#7")).unwrap();

        assert_eq!(report.conflicts.len(), 1, "{:?}", report);
        assert_eq!(report.conflicts[0].ref_id, sign_ref);
        assert_eq!(report.conflicts[0].hook, "on_look");

        let sign = world.get(&sign_ref).unwrap();
        assert!(
            sign.programs["on_look"].source.contains("upstream edit"),
            "conflict resolves by overwriting with the incoming source"
        );

        // The local edit must be preserved in the version log.
        let history = db.list_program_versions(&sign_ref, "on_look").unwrap();
        assert!(
            history.iter().any(|v| v.source.contains("builder edit")),
            "local edit must be preserved in program history: {:?}",
            history
        );
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = TempDir::new();
        dir.write("town", "town.toml", &town_toml("Old description."));
        let db = temp_db();
        let mut world = World::new();
        import_bundle(&dir.path, &mut world, &db, false, None).unwrap();

        let object_count_before = world.objects.len();
        let next_id_before = world.next_id;
        let sign_ref = world.objects.values().find(|o| o.key == "sign").unwrap().ref_id.clone();
        let versions_before_dry_run = db.list_program_versions(&sign_ref, "on_look").unwrap().len();

        // Change upstream (both a plain field and the sign's program), then
        // dry-run import it.
        dir.write(
            "town",
            "town.toml",
            &town_toml("New description.").replace(
                "function on_look() return 'a sign' end",
                "function on_look() return 'a new sign' end",
            ),
        );
        let report = import_bundle(&dir.path, &mut world, &db, true, None).unwrap();

        assert_eq!(report.updated.len(), 2, "{:?}", report);
        assert_eq!(world.objects.len(), object_count_before, "dry run must not create objects");
        assert_eq!(world.next_id, next_id_before, "dry run must not mint dbrefs");
        let crossroads = world.objects.values().find(|o| o.key == "crossroads").unwrap();
        assert_eq!(crossroads.description, "Old description.", "dry run must not mutate the real world");
        let sign = world.get(&sign_ref).unwrap();
        assert!(
            sign.programs["on_look"].source.contains("'a sign'"),
            "dry run must not mutate the real world's Program either"
        );

        let versions_after_dry_run = db.list_program_versions(&sign_ref, "on_look").unwrap().len();
        assert_eq!(
            versions_after_dry_run, versions_before_dry_run,
            "dry run must not write to the program version log"
        );
    }

    #[test]
    fn dry_run_report_matches_a_real_run() {
        let dir = TempDir::new();
        dir.write("town", "town.toml", &town_toml("Old description."));
        let db = temp_db();
        let mut world = World::new();
        import_bundle(&dir.path, &mut world, &db, false, None).unwrap();
        dir.write("town", "town.toml", &town_toml("New description."));

        let mut world_for_dry_run = world.clone();
        let dry = import_bundle(&dir.path, &mut world_for_dry_run, &db, true, None).unwrap();
        let real = import_bundle(&dir.path, &mut world, &db, false, None).unwrap();

        assert_eq!(dry.created, real.created);
        assert_eq!(dry.updated, real.updated);
        assert_eq!(dry.unchanged, real.unchanged);
    }

    #[test]
    fn duplicate_identity_refuses_and_writes_nothing() {
        let dir = TempDir::new();
        dir.write(
            "town",
            "a.toml",
            r#"
                area = "town"
                [[rooms]]
                key = "crossroads"
                title = "A"
            "#,
        );
        dir.write(
            "town",
            "b.toml",
            r#"
                area = "town"
                [[rooms]]
                key = "crossroads"
                title = "B"
            "#,
        );
        let db = temp_db();
        let mut world = World::new();

        let err = import_bundle(&dir.path, &mut world, &db, false, None).unwrap_err();
        assert!(err.contains("crossroads"), "unexpected error: {}", err);
        assert!(world.objects.is_empty(), "nothing should be written on collision");
    }

    #[test]
    fn duplicate_cmd_hook_on_different_objects_refuses() {
        let dir = TempDir::new();
        dir.write(
            "town",
            "town.toml",
            r#"
                area = "town"

                [[objects]]
                key = "a"
                kind = "item"
                [objects.programs]
                cmd_attack = { source = "function cmd_attack() end" }

                [[objects]]
                key = "b"
                kind = "item"
                [objects.programs]
                cmd_attack = { source = "function cmd_attack() end" }
            "#,
        );
        let db = temp_db();
        let mut world = World::new();

        let err = import_bundle(&dir.path, &mut world, &db, false, None).unwrap_err();
        assert!(err.contains("cmd_attack"), "unexpected error: {}", err);
        assert!(world.objects.is_empty(), "nothing should be written on collision");
    }

    #[test]
    fn missing_from_bundle_is_reported_not_removed() {
        let dir = TempDir::new();
        dir.write("town", "town.toml", &town_toml("Old description."));
        let db = temp_db();
        let mut world = World::new();
        import_bundle(&dir.path, &mut world, &db, false, None).unwrap();

        // A new bundle for the same area drops the "square" room and the exit.
        dir.write(
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

        let report = import_bundle(&dir.path, &mut world, &db, false, None).unwrap();
        assert!(report.missing.contains(&"town/square".to_string()), "{:?}", report);
        assert!(
            world.objects.values().any(|o| o.key == "square"),
            "missing objects must not be removed"
        );
    }

    #[test]
    fn export_then_import_round_trips_to_a_no_op() {
        let import_dir = TempDir::new();
        import_dir.write("town", "town.toml", &town_toml("A dusty crossroads."));
        let db = temp_db();
        let mut world = World::new();
        import_bundle(&import_dir.path, &mut world, &db, false, Some("#1")).unwrap();

        let export_dir = TempDir::new();
        let export_report = export_bundle(&export_dir.path, &mut world).unwrap();
        assert!(export_report.skipped.is_empty(), "{:?}", export_report);
        assert_eq!(export_report.objects_written, 4);

        let object_count_before = world.objects.len();
        let report = import_bundle(&export_dir.path, &mut world, &db, false, Some("#1")).unwrap();

        assert!(report.created.is_empty(), "export->import round trip must be a no-op: {:?}", report);
        assert!(report.conflicts.is_empty(), "{:?}", report);
        assert!(report.kept_local.is_empty(), "{:?}", report);
        assert_eq!(
            report.unchanged.len(),
            4,
            "everything should already match: {:?}",
            report
        );
        assert_eq!(world.objects.len(), object_count_before);
    }

    /// Build an ad-hoc item the way `@create` does — `Kind::Item`, title,
    /// `location_ref` set, an owner, and critically *no* `FILE_KEY_ATTR` —
    /// rather than hand-constructing one with a key already stamped. See
    /// `src/engine/mod.rs`'s `cmd_create`.
    fn adhoc_item(world: &mut World, title: &str, location: &str, owner: &str) -> String {
        let ref_id = world.next_dbref();
        let key = title.to_lowercase().replace(' ', "_");
        let item = GameObject::new(&ref_id, &key, Kind::Item)
            .with_title(title)
            .with_location(location)
            .with_owner(owner);
        world.add_object(item);
        ref_id
    }

    /// Build an ad-hoc room the way `@dig` does — `Kind::Room`, title, an
    /// owner, and no location and no `FILE_KEY_ATTR`. See `cmd_dig`.
    fn adhoc_room(world: &mut World, title: &str, owner: &str) -> String {
        let ref_id = world.next_dbref();
        let key = title.to_lowercase().replace(' ', "_");
        let room = GameObject::new(&ref_id, &key, Kind::Room).with_title(title).with_owner(owner);
        world.add_object(room);
        ref_id
    }

    /// Objects created entirely in-game (no `@import` involved, no
    /// `FILE_KEY_ATTR` ever set by hand) must still round-trip: `@export`
    /// stamps identity, and re-importing the exported bundle must report
    /// zero created and zero updated. Covers a room with no natural area
    /// (nothing imported it, so it has no location to derive one from) and
    /// a nested container — an item inside an item inside that room —
    /// which only has a *location*, no area of its own, either.
    #[test]
    fn adhoc_objects_round_trip_through_export_including_nested_containment() {
        let db = temp_db();
        let mut world = World::new();
        let owner = "#1";

        let room_ref = adhoc_room(&mut world, "The Vault", owner);
        let bag_ref = adhoc_item(&mut world, "Leather Bag", &room_ref, owner);
        let coin_ref = adhoc_item(&mut world, "Gold Coin", &bag_ref, owner);

        let export_dir = TempDir::new();
        let report = export_bundle(&export_dir.path, &mut world).unwrap();
        assert!(report.skipped.is_empty(), "nothing should be unrepresentable: {:?}", report);
        assert_eq!(report.objects_written, 3);
        assert!(report.written_areas.contains(&FALLBACK_AREA.to_string()), "{:?}", report);

        // Identity was stamped in place — same objects, now with a key.
        assert!(world.get(&room_ref).unwrap().attrs.contains_key(loader::FILE_KEY_ATTR));
        assert!(world.get(&bag_ref).unwrap().attrs.contains_key(loader::FILE_KEY_ATTR));
        let coin_fk = world.get(&coin_ref).unwrap().attrs[loader::FILE_KEY_ATTR].as_str().unwrap().to_string();
        assert!(coin_fk.starts_with(&format!("{}/", FALLBACK_AREA)), "unexpected key: {}", coin_fk);

        // Gameplay-facing `.key` must be untouched by stamping — export
        // must never change what a player types to refer to an object.
        assert_eq!(world.get(&coin_ref).unwrap().key, "gold_coin");

        let object_count_before = world.objects.len();
        let import_report = import_bundle(&export_dir.path, &mut world, &db, false, Some(owner)).unwrap();
        assert!(import_report.created.is_empty(), "export->import must be a no-op: {:?}", import_report);
        assert!(import_report.updated.is_empty(), "{:?}", import_report);
        assert_eq!(import_report.unchanged.len(), 3, "{:?}", import_report);
        assert_eq!(world.objects.len(), object_count_before);

        // The nested relationship must have actually round-tripped, not
        // just avoided an error.
        assert_eq!(world.get(&coin_ref).unwrap().location_ref.as_deref(), Some(bag_ref.as_str()));
    }

    /// Re-exporting an unchanged world must derive the exact same keys
    /// every time — otherwise a second `@export` (with nothing having
    /// changed in between) would look like a diff in git for no reason,
    /// and a re-import could fail to match what an earlier export already
    /// installed.
    #[test]
    fn stamped_keys_are_stable_across_repeated_exports() {
        let mut world = World::new();
        let owner = "#1";
        let room_ref = adhoc_room(&mut world, "Greenhouse", owner);
        adhoc_item(&mut world, "Watering Can", &room_ref, owner);

        let dir_a = TempDir::new();
        export_bundle(&dir_a.path, &mut world).unwrap();
        let keys_after_first: std::collections::BTreeMap<String, String> = world
            .objects
            .values()
            .filter_map(|o| {
                o.attrs
                    .get(loader::FILE_KEY_ATTR)
                    .and_then(|v| v.as_str())
                    .map(|k| (o.ref_id.clone(), k.to_string()))
            })
            .collect();

        // Export again — identity is already stamped, so this must be a
        // pure no-op on the keys (nothing left to stamp).
        let dir_b = TempDir::new();
        export_bundle(&dir_b.path, &mut world).unwrap();
        let keys_after_second: std::collections::BTreeMap<String, String> = world
            .objects
            .values()
            .filter_map(|o| {
                o.attrs
                    .get(loader::FILE_KEY_ATTR)
                    .and_then(|v| v.as_str())
                    .map(|k| (o.ref_id.clone(), k.to_string()))
            })
            .collect();

        assert_eq!(keys_after_first, keys_after_second);
    }

    /// Two ad-hoc objects with the same title in the same (fallback) area
    /// must not collide — the second gets a disambiguated key rather than
    /// silently overwriting or erroring.
    #[test]
    fn colliding_titles_get_disambiguated_keys() {
        let mut world = World::new();
        let owner = "#1";
        let room_ref = adhoc_room(&mut world, "Storage", owner);
        let a = adhoc_item(&mut world, "Crate", &room_ref, owner);
        let b = adhoc_item(&mut world, "Crate", &room_ref, owner);

        let dir = TempDir::new();
        let report = export_bundle(&dir.path, &mut world).unwrap();
        assert!(report.skipped.is_empty(), "{:?}", report);

        let key_a = world.get(&a).unwrap().attrs[loader::FILE_KEY_ATTR].as_str().unwrap().to_string();
        let key_b = world.get(&b).unwrap().attrs[loader::FILE_KEY_ATTR].as_str().unwrap().to_string();
        assert_ne!(key_a, key_b, "colliding titles must not stamp the same key");
    }

    /// An ad-hoc exit (the way `@open` creates one — no `FILE_KEY_ATTR`,
    /// `location_ref`/`target_ref` set, `.key` is the direction) must round
    /// trip too: its stamped key embeds the source room's key, which the
    /// stamping pass has to resolve before the exit's own key can be built.
    #[test]
    fn adhoc_exit_round_trips_through_export() {
        let db = temp_db();
        let mut world = World::new();
        let owner = "#1";

        let room_a = adhoc_room(&mut world, "Alpha Room", owner);
        let room_b = adhoc_room(&mut world, "Beta Room", owner);
        let exit_ref = world.next_dbref();
        let exit = GameObject::new(&exit_ref, "north", Kind::Exit)
            .with_location(&room_a)
            .with_target(&room_b)
            .with_owner(owner);
        world.add_object(exit);

        let export_dir = TempDir::new();
        let report = export_bundle(&export_dir.path, &mut world).unwrap();
        assert!(report.skipped.is_empty(), "{:?}", report);
        assert_eq!(report.objects_written, 3);

        let exit_fk = world.get(&exit_ref).unwrap().attrs[loader::FILE_KEY_ATTR].as_str().unwrap().to_string();
        assert!(exit_fk.contains("/exit/"), "unexpected exit key: {}", exit_fk);

        let object_count_before = world.objects.len();
        let import_report = import_bundle(&export_dir.path, &mut world, &db, false, Some(owner)).unwrap();
        assert!(import_report.created.is_empty(), "{:?}", import_report);
        assert!(import_report.updated.is_empty(), "{:?}", import_report);
        assert_eq!(world.objects.len(), object_count_before);
        assert_eq!(world.find_exit(&room_a, "north").map(|e| e.ref_id.clone()), Some(exit_ref));
    }

    /// `@create` derives `.key` from a lowercased title with no uniqueness
    /// check at all (unlike a file import, where `check_collisions`
    /// enforces it) — so two ad-hoc objects can easily share one, e.g. two
    /// things both named "Crate". If `write_programs` used `obj.key` (the
    /// gameplay key) to name the `.luau` file it writes, the second
    /// object's Program would silently overwrite the first's file on disk
    /// the moment both exported into the same area. It must use the
    /// disambiguated export key instead.
    #[test]
    fn colliding_gameplay_keys_do_not_clobber_each_others_program_files() {
        let mut world = World::new();
        let owner = "#1";
        let room_ref = adhoc_room(&mut world, "Storage", owner);

        let a = world.next_dbref();
        let mut obj_a = GameObject::new(&a, "crate", Kind::Item).with_title("Crate").with_location(&room_ref);
        hooks::set_program(&mut obj_a, "on_look", "return 'crate A'".into()).unwrap();
        world.add_object(obj_a);

        let b = world.next_dbref();
        let mut obj_b = GameObject::new(&b, "crate", Kind::Item).with_title("Crate").with_location(&room_ref);
        hooks::set_program(&mut obj_b, "on_look", "return 'crate B'".into()).unwrap();
        world.add_object(obj_b);

        assert_eq!(world.get(&a).unwrap().key, world.get(&b).unwrap().key, "both share the same gameplay key");

        let dir = TempDir::new();
        let report = export_bundle(&dir.path, &mut world).unwrap();
        assert!(report.skipped.is_empty(), "{:?}", report);

        let source_a = world.get(&a).unwrap().programs["on_look"].source.clone();
        let source_b = world.get(&b).unwrap().programs["on_look"].source.clone();

        // Read back both `.luau` files that were actually written and
        // confirm neither one clobbered the other.
        let toml_text =
            std::fs::read_to_string(dir.path.join(FALLBACK_AREA).join(format!("{}.toml", FALLBACK_AREA))).unwrap();
        let file_names: Vec<&str> = toml_text
            .lines()
            .filter_map(|l| l.trim().strip_prefix("file = \"").and_then(|s| s.strip_suffix('"')))
            .collect();
        assert_eq!(file_names.len(), 2, "expected a distinct .luau file per object:\n{}", toml_text);
        assert_ne!(file_names[0], file_names[1], "both objects wrote to the same file: {:?}", file_names);

        let written: Vec<String> =
            file_names.iter().map(|f| std::fs::read_to_string(dir.path.join(FALLBACK_AREA).join(f)).unwrap()).collect();
        assert!(written.contains(&source_a));
        assert!(written.contains(&source_b));
    }

    /// `Kind::Player` is account-linked runtime state, not world content —
    /// it must never be exported, identity or no identity.
    #[test]
    fn players_are_never_exported() {
        let mut world = World::new();
        let room_ref = adhoc_room(&mut world, "Lobby", "#1");
        let player_ref = world.next_dbref();
        let player = GameObject::new(&player_ref, "wanderer", Kind::Player)
            .with_title("A Wanderer")
            .with_location(&room_ref);
        world.add_object(player);

        let dir = TempDir::new();
        let report = export_bundle(&dir.path, &mut world).unwrap();

        assert!(
            !world.get(&player_ref).unwrap().attrs.contains_key(loader::FILE_KEY_ATTR),
            "a player must never be stamped with a file identity"
        );
        // Only the room should have been written — the player must not
        // appear anywhere in the emitted bundle.
        assert_eq!(report.objects_written, 1, "{:?}", report);
        let toml_text = std::fs::read_to_string(
            dir.path.join(FALLBACK_AREA).join(format!("{}.toml", FALLBACK_AREA)),
        )
        .unwrap();
        assert!(!toml_text.contains("wanderer"), "player must not appear in exported TOML:\n{}", toml_text);
    }
}
