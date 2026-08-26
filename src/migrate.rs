//! Content migrations — forward-only, declarative rename/remove operations
//! that fix object *identity* in the database before the loader reconciles
//! file content against it.
//!
//! # Why this exists
//!
//! The loader keys a managed object's identity off its [`FILE_KEY_ATTR`]
//! (`_file_key`): on boot it matches each incoming `<area>/<key>` file against
//! the object carrying that same `_file_key` (see `loader::key_map_from_world`)
//! and updates it in place; anything with no match is treated as brand new.
//! That means **any rename or move of a file-key silently orphans the old
//! object and builds a duplicate** — there is no signal that "town/crossroads"
//! and "world/town/crossroads" are the same room. A restructure that renames
//! every key duplicates the entire world.
//!
//! Migrations declare that intent. A revision says "content under `town/` moved
//! to `world/town/`"; applying it restamps the existing objects' `_file_key` in
//! place — keeping their dbref, attrs, contents, and occupants — so the next
//! boot matches them instead of duplicating them.
//!
//! # Model (Alembic's bones, content-shaped)
//!
//! - A `migrations` DB table records every applied revision, so `hearth
//!   migrate` runs only the pending ones and is safe to re-run / redeploy.
//! - Revisions are ordered by their string id (zero-padded ordinals sort
//!   lexically); pending revisions apply in that order.
//! - **Forward-only** — content renames don't reverse meaningfully, so there is
//!   no `down`.
//! - **Explicit** — applied by the `hearth migrate` deploy step, never as a
//!   silent boot mutation (the failure mode CLAUDE.md warns about).
//!
//! # File format (`<game_root>/migrations/<revision>_<slug>.toml`)
//!
//! ```toml
//! revision = "0001"
//! description = "world content moved under world/"
//!
//! # `remove` ops apply BEFORE `rename` ops within one migration, so a rename
//! # can move onto a key a stale duplicate previously occupied. Need finer
//! # ordering? Split into two numbered migration files.
//! [[remove]]
//! prefix = "world/"          # or:  key = "system/rules"
//!
//! [[rename]]
//! from_prefix = "town/"      # or:  from = "exact/old"
//! to_prefix = "world/town/"  #      to   = "exact/new"
//! ```
//!
//! A rename onto a key another object already holds is a hard error (the
//! migration is refused, nothing is written): declare a `remove` to clear it
//! first. Only objects that carry a `_file_key` are ever touched, so
//! player-created content is untouchable by construction.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::db::Database;
use crate::loader::FILE_KEY_ATTR;
use crate::world::World;

/// One parsed migration file.
#[derive(Debug, Deserialize)]
pub struct MigrationFile {
    pub revision: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub rename: Vec<RenameOp>,
    #[serde(default)]
    pub remove: Vec<RemoveOp>,
}

/// Rename operation: either a prefix rewrite (`from_prefix` → `to_prefix`) or
/// an exact-key rewrite (`from` → `to`). Exactly one pair must be set.
#[derive(Debug, Deserialize)]
pub struct RenameOp {
    pub from_prefix: Option<String>,
    pub to_prefix: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Remove operation: delete the object with this exact `key`, or every object
/// whose key starts with `prefix`. Exactly one must be set.
#[derive(Debug, Deserialize)]
pub struct RemoveOp {
    pub key: Option<String>,
    pub prefix: Option<String>,
}

/// What one migration did (or would do, on a dry run).
#[derive(Debug, Default)]
pub struct RevisionReport {
    pub revision: String,
    pub description: String,
    /// `(old_key, new_key)` for each restamped object.
    pub renamed: Vec<(String, String)>,
    /// The `_file_key` of each removed object.
    pub removed: Vec<String>,
}

/// The outcome of a `hearth migrate` run.
#[derive(Debug, Default)]
pub struct MigrateReport {
    pub applied: Vec<RevisionReport>,
    /// Revisions already recorded as applied, skipped this run.
    pub already_applied: usize,
    pub dry_run: bool,
}

impl MigrateReport {
    pub fn nothing_to_do(&self) -> bool {
        self.applied.is_empty()
    }
}

/// The `_file_key` of an object, if it has one (i.e. it is file-managed).
fn file_key(obj: &crate::world::GameObject) -> Option<&str> {
    obj.attrs.get(FILE_KEY_ATTR).and_then(|v| v.as_str())
}

/// Load and validate every migration file under `dir`, sorted by revision.
/// Returns an error on a parse failure or a duplicate revision id (the same
/// duplicate-revision guard a CI check would enforce).
pub fn load_migrations(dir: &Path) -> Result<Vec<MigrationFile>, String> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<(PathBuf, MigrationFile)> = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("reading migrations dir {}: {}", dir.display(), e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    entries.sort();

    for path in entries {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {}", path.display(), e))?;
        let mf: MigrationFile = toml::from_str(&text)
            .map_err(|e| format!("parsing {}: {}", path.display(), e))?;
        if mf.revision.trim().is_empty() {
            return Err(format!("{}: empty revision id", path.display()));
        }
        validate(&mf).map_err(|e| format!("{}: {}", path.display(), e))?;
        files.push((path, mf));
    }

    // Reject duplicate revisions across files.
    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    for (path, mf) in &files {
        if let Some(prev) = seen.insert(mf.revision.clone(), path.clone()) {
            return Err(format!(
                "duplicate revision '{}' in {} and {}",
                mf.revision,
                prev.display(),
                path.display()
            ));
        }
    }

    files.sort_by(|a, b| a.1.revision.cmp(&b.1.revision));
    Ok(files.into_iter().map(|(_, mf)| mf).collect())
}

fn validate(mf: &MigrationFile) -> Result<(), String> {
    for r in &mf.rename {
        let prefix_pair = r.from_prefix.is_some() || r.to_prefix.is_some();
        let exact_pair = r.from.is_some() || r.to.is_some();
        match (prefix_pair, exact_pair) {
            (true, true) => {
                return Err("rename: set either from_prefix/to_prefix or from/to, not both".into());
            }
            (false, false) => return Err("rename: needs from_prefix/to_prefix or from/to".into()),
            (true, false) => {
                if r.from_prefix.is_none() || r.to_prefix.is_none() {
                    return Err("rename: from_prefix requires to_prefix (and vice versa)".into());
                }
            }
            (false, true) => {
                if r.from.is_none() || r.to.is_none() {
                    return Err("rename: from requires to (and vice versa)".into());
                }
            }
        }
    }
    for rm in &mf.remove {
        match (rm.key.is_some(), rm.prefix.is_some()) {
            (true, true) => return Err("remove: set either key or prefix, not both".into()),
            (false, false) => return Err("remove: needs key or prefix".into()),
            _ => {}
        }
    }
    Ok(())
}

/// The new `_file_key` this rename produces for `key`, or `None` if it doesn't
/// match. Prefix renames rewrite the matching leading segment; exact renames
/// match the whole key.
fn rename_target(op: &RenameOp, key: &str) -> Option<String> {
    if let (Some(fp), Some(tp)) = (&op.from_prefix, &op.to_prefix) {
        return key.strip_prefix(fp.as_str()).map(|rest| format!("{tp}{rest}"));
    }
    if let (Some(f), Some(t)) = (&op.from, &op.to) {
        return (key == f).then(|| t.clone());
    }
    None
}

/// Plan one migration against `world` without mutating it: the refs to remove
/// and the `(ref, old_key, new_key)` restamps, with collision checks. A rename
/// onto a key still held by another object (after removals) is an error.
fn plan(
    world: &World,
    mf: &MigrationFile,
) -> Result<(Vec<String>, Vec<(String, String, String)>), String> {
    // Removals first — by exact key or prefix, managed objects only.
    let mut remove_refs: Vec<String> = Vec::new();
    let mut removed_keys: HashSet<String> = HashSet::new();
    for rm in &mf.remove {
        for obj in world.objects.values() {
            let Some(fk) = file_key(obj) else { continue };
            let hit = match (&rm.key, &rm.prefix) {
                (Some(k), _) => fk == k,
                (_, Some(p)) => fk.starts_with(p.as_str()),
                _ => false,
            };
            if hit {
                remove_refs.push(obj.ref_id.clone());
                removed_keys.insert(fk.to_string());
            }
        }
    }

    // The key set that will exist after removals — the collision baseline.
    let mut occupied: HashMap<String, String> = HashMap::new(); // key -> ref
    for obj in world.objects.values() {
        if let Some(fk) = file_key(obj)
            && !removed_keys.contains(fk)
        {
            occupied.insert(fk.to_string(), obj.ref_id.clone());
        }
    }

    // Renames, applied over the post-removal key set. Each restamp updates the
    // occupancy map so two renames can't collide onto the same target either.
    let mut restamps: Vec<(String, String, String)> = Vec::new();
    for op in &mf.rename {
        // Snapshot the sources first: renaming mutates `occupied`, and we must
        // not let one rename's output feed another's input within a single op.
        let sources: Vec<(String, String, String)> = world
            .objects
            .values()
            .filter_map(|obj| {
                let fk = file_key(obj)?;
                if removed_keys.contains(fk) {
                    return None;
                }
                let new_key = rename_target(op, fk)?;
                Some((obj.ref_id.clone(), fk.to_string(), new_key))
            })
            .collect();
        for (ref_id, old_key, new_key) in sources {
            if let Some(holder) = occupied.get(&new_key)
                && holder != &ref_id
            {
                return Err(format!(
                    "rename target '{}' is already held by {} — clear it with a `remove` first",
                    new_key, holder
                ));
            }
            occupied.remove(&old_key);
            occupied.insert(new_key.clone(), ref_id.clone());
            restamps.push((ref_id, old_key, new_key));
        }
    }

    Ok((remove_refs, restamps))
}

/// Apply one migration to `world` in memory, returning what it did. Plans (with
/// collision checks) before mutating, so a migration either applies fully or
/// leaves the world untouched.
pub fn apply_one(world: &mut World, mf: &MigrationFile) -> Result<RevisionReport, String> {
    let (remove_refs, restamps) = plan(world, mf)?;

    let mut report = RevisionReport {
        revision: mf.revision.clone(),
        description: mf.description.clone(),
        ..Default::default()
    };

    for ref_id in &remove_refs {
        if let Some(obj) = world.remove_object(ref_id)
            && let Some(fk) = obj.attrs.get(FILE_KEY_ATTR).and_then(|v| v.as_str())
        {
            report.removed.push(fk.to_string());
        }
    }
    for (ref_id, old_key, new_key) in restamps {
        if let Some(obj) = world.get_mut(&ref_id) {
            obj.attrs
                .insert(FILE_KEY_ATTR.into(), serde_json::Value::String(new_key.clone()));
            report.renamed.push((old_key, new_key));
        }
    }
    report.removed.sort();
    report.renamed.sort();
    Ok(report)
}

/// Directory holding a game's migration files: `<game_root>/migrations`, where
/// the game root is the parent of `game_dir` (the same convention
/// `game_web_dir` uses). `None` if `game_dir` has no parent.
pub fn migrations_dir(game_dir: &Path) -> Option<PathBuf> {
    game_dir.parent().map(|root| root.join("migrations"))
}

/// Run every pending migration against the DB world, in revision order,
/// persisting and recording each as it succeeds. On `dry_run`, applies to the
/// in-memory world for collision checks and reporting but writes nothing.
///
/// Each migration is its own commit: if revision N fails, revisions before it
/// stay applied and recorded, N is not, and a re-run resumes at N.
pub fn run(
    db: &Database,
    world: &mut World,
    dir: &Path,
    dry_run: bool,
    now: u64,
) -> Result<MigrateReport, String> {
    let all = load_migrations(dir)?;
    let applied: HashSet<String> = db
        .load_applied_migrations()
        .map_err(|e| format!("reading applied migrations: {}", e))?;

    let mut report = MigrateReport {
        dry_run,
        already_applied: all.iter().filter(|m| applied.contains(&m.revision)).count(),
        ..Default::default()
    };

    for mf in &all {
        if applied.contains(&mf.revision) {
            continue;
        }
        let rev = apply_one(world, mf)?;
        if !dry_run {
            let changes = world.drain_dirty();
            db.save_world_delta(world, &changes)
                .map_err(|e| format!("persisting revision {}: {}", mf.revision, e))?;
            db.record_migration(&mf.revision, &mf.description, now)
                .map_err(|e| format!("recording revision {}: {}", mf.revision, e))?;
        }
        report.applied.push(rev);
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{GameObject, Kind};

    fn managed(world: &mut World, key: &str, kind: Kind) -> String {
        let ref_id = world.next_dbref();
        let mut obj = GameObject::new(&ref_id, key, kind);
        obj.attrs
            .insert(FILE_KEY_ATTR.into(), serde_json::Value::String(key.into()));
        world.add_object(obj);
        ref_id
    }

    fn parse(src: &str) -> MigrationFile {
        let mf: MigrationFile = toml::from_str(src).unwrap();
        validate(&mf).unwrap();
        mf
    }

    #[test]
    fn prefix_rename_restamps_in_place_keeping_ref_and_contents() {
        let mut world = World::new();
        let room = managed(&mut world, "town/crossroads", Kind::Room);
        // A player standing in that room — located by ref, not by file-key.
        let player = world.next_dbref();
        world.add_object(
            GameObject::new(&player, "admin", Kind::Player).with_location(&room),
        );

        let mf = parse(
            "revision = \"0001\"\n\
             [[rename]]\n\
             from_prefix = \"town/\"\n\
             to_prefix = \"world/town/\"\n",
        );
        let rep = apply_one(&mut world, &mf).unwrap();

        // Same dbref, new file-key, occupant undisturbed.
        assert_eq!(
            file_key(world.get(&room).unwrap()),
            Some("world/town/crossroads")
        );
        assert_eq!(
            world.get(&player).unwrap().location_ref.as_deref(),
            Some(room.as_str())
        );
        assert_eq!(rep.renamed, vec![("town/crossroads".into(), "world/town/crossroads".into())]);
    }

    #[test]
    fn remove_runs_before_rename_so_a_rename_can_reclaim_a_duplicated_key() {
        // The Last Stag incident in miniature: an old original plus a stale
        // duplicate at the new key. Remove the duplicate, then rename the
        // original onto the freed key.
        let mut world = World::new();
        let original = managed(&mut world, "town/crossroads", Kind::Room);
        let _duplicate = managed(&mut world, "world/town/crossroads", Kind::Room);

        let mf = parse(
            "revision = \"0001\"\n\
             [[remove]]\n\
             prefix = \"world/\"\n\
             [[rename]]\n\
             from_prefix = \"town/\"\n\
             to_prefix = \"world/town/\"\n",
        );
        let rep = apply_one(&mut world, &mf).unwrap();

        // The duplicate is gone; the original now holds the new key.
        assert_eq!(rep.removed, vec!["world/town/crossroads".to_string()]);
        assert_eq!(
            file_key(world.get(&original).unwrap()),
            Some("world/town/crossroads")
        );
        // Exactly one object holds the new key now.
        let holders: Vec<_> = world
            .objects
            .values()
            .filter(|o| file_key(o) == Some("world/town/crossroads"))
            .collect();
        assert_eq!(holders.len(), 1);
    }

    #[test]
    fn rename_onto_an_occupied_key_is_refused_and_world_is_untouched() {
        let mut world = World::new();
        let original = managed(&mut world, "town/crossroads", Kind::Room);
        managed(&mut world, "world/town/crossroads", Kind::Room);

        // No `remove` to clear the target — must error, and (planning happens
        // before mutation) leave the original's key untouched.
        let mf = parse(
            "revision = \"0001\"\n\
             [[rename]]\n\
             from_prefix = \"town/\"\n\
             to_prefix = \"world/town/\"\n",
        );
        let err = apply_one(&mut world, &mf).unwrap_err();
        assert!(err.contains("already held"), "got: {err}");
        assert_eq!(file_key(world.get(&original).unwrap()), Some("town/crossroads"));
    }

    #[test]
    fn exact_rename_and_exact_remove() {
        let mut world = World::new();
        let rules = managed(&mut world, "system/rules", Kind::Item);
        let sign = managed(&mut world, "town/sign", Kind::Item);

        let mf = parse(
            "revision = \"0002\"\n\
             [[remove]]\n\
             key = \"system/rules\"\n\
             [[rename]]\n\
             from = \"town/sign\"\n\
             to = \"world/town/notice\"\n",
        );
        let rep = apply_one(&mut world, &mf).unwrap();
        assert!(world.get(&rules).is_none());
        assert_eq!(file_key(world.get(&sign).unwrap()), Some("world/town/notice"));
        assert_eq!(rep.removed, vec!["system/rules".to_string()]);
    }

    #[test]
    fn player_created_objects_without_a_file_key_are_never_touched() {
        let mut world = World::new();
        // No _file_key attr => not managed.
        let hand_built = world.next_dbref();
        world.add_object(GameObject::new(&hand_built, "town/relic", Kind::Item));

        let mf = parse(
            "revision = \"0003\"\n\
             [[remove]]\n\
             prefix = \"town/\"\n\
             [[rename]]\n\
             from_prefix = \"town/\"\n\
             to_prefix = \"world/town/\"\n",
        );
        let rep = apply_one(&mut world, &mf).unwrap();
        assert!(world.get(&hand_built).is_some(), "unmanaged object survived");
        assert!(rep.removed.is_empty());
        assert!(rep.renamed.is_empty());
    }

    #[test]
    fn run_persists_records_and_is_idempotent() {
        // Full path: files on disk -> DB world -> apply -> persist -> reload,
        // then a second run that must be a no-op.
        let db = Database::open(std::path::Path::new(":memory:")).unwrap();

        // Baseline world: the incident shape — original + stale duplicate.
        let mut base = World::new();
        managed(&mut base, "town/crossroads", Kind::Room);
        managed(&mut base, "world/town/crossroads", Kind::Room);
        managed(&mut base, "system/rules", Kind::Item);
        db.save_world(&base).unwrap();

        // Migration file on disk.
        let dir = std::env::temp_dir().join("hearth_migrate_run_e2e");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("0001_restructure.toml"),
            "revision = \"0001\"\n\
             description = \"world content moved under world/\"\n\
             [[remove]]\n\
             prefix = \"world/\"\n\
             [[remove]]\n\
             key = \"system/rules\"\n\
             [[rename]]\n\
             from_prefix = \"town/\"\n\
             to_prefix = \"world/town/\"\n",
        )
        .unwrap();

        // First run applies + persists + records.
        let mut world = db.load_world().unwrap();
        let report = run(&db, &mut world, &dir, false, 42).unwrap();
        assert_eq!(report.applied.len(), 1);
        assert_eq!(report.applied[0].revision, "0001");

        // Reload from the DB: the rename persisted, the duplicate and the
        // removed key are gone, exactly one object holds the new key.
        let reloaded = db.load_world().unwrap();
        let holders: Vec<_> = reloaded
            .objects
            .values()
            .filter(|o| file_key(o) == Some("world/town/crossroads"))
            .collect();
        assert_eq!(holders.len(), 1, "one object holds the renamed key");
        assert!(
            !reloaded.objects.values().any(|o| file_key(o) == Some("system/rules")),
            "system/rules was removed"
        );

        // Second run: 0001 is recorded, so nothing pending.
        let mut world2 = db.load_world().unwrap();
        let again = run(&db, &mut world2, &dir, false, 43).unwrap();
        assert!(again.nothing_to_do(), "re-running applies nothing");
        assert_eq!(again.already_applied, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let db = Database::open(std::path::Path::new(":memory:")).unwrap();
        let mut base = World::new();
        managed(&mut base, "town/sq", Kind::Room);
        db.save_world(&base).unwrap();

        let dir = std::env::temp_dir().join("hearth_migrate_dryrun_e2e");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("0001_x.toml"),
            "revision = \"0001\"\n\
             [[rename]]\n\
             from_prefix = \"town/\"\n\
             to_prefix = \"world/town/\"\n",
        )
        .unwrap();

        let mut world = db.load_world().unwrap();
        let report = run(&db, &mut world, &dir, true, 0).unwrap();
        assert_eq!(report.applied.len(), 1, "dry run still reports the plan");
        assert!(report.dry_run);

        // Nothing was recorded or persisted.
        assert!(db.load_applied_migrations().unwrap().is_empty());
        let reloaded = db.load_world().unwrap();
        assert_eq!(
            file_key(reloaded.objects.values().next().unwrap()),
            Some("town/sq"),
            "dry run left the DB untouched"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_rejects_mixed_and_empty_ops() {
        let bad_mixed: MigrationFile = toml::from_str(
            "revision = \"0001\"\n\
             [[rename]]\n\
             from_prefix = \"a/\"\n\
             to = \"b\"\n",
        )
        .unwrap();
        assert!(validate(&bad_mixed).is_err());

        let bad_empty: MigrationFile = toml::from_str(
            "revision = \"0001\"\n\
             [[remove]]\n",
        )
        .unwrap();
        assert!(validate(&bad_empty).is_err());
    }
}
