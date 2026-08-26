use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::accounts::{Account, AccountStore, Scope};
use crate::softcode::hooks::{self, LibModule, ObjectScript};
use crate::world::{GameObject, Kind, Tag, World};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        // WAL + `synchronous=NORMAL` is the right durability point for a
        // checkpoint-only store whose live world is in memory: under WAL,
        // NORMAL fsyncs at checkpoint rather than on every commit, so a save
        // no longer blocks the single-writer engine task on a per-commit fsync
        // (the writer runs every save synchronously — see `Engine::do_save`).
        // The tradeoff is that an OS/power crash can lose the last few
        // committed transactions (never corrupt the DB); with a 5-minute
        // autosave of an in-memory world that is an acceptable window.
        // `busy_timeout` makes any second connection wait for the write lock
        // instead of erroring out immediately with SQLITE_BUSY.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; \
             PRAGMA synchronous=NORMAL; \
             PRAGMA busy_timeout=5000; \
             PRAGMA foreign_keys=ON;",
        )?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS accounts (
                id TEXT PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                character_ref TEXT,
                email TEXT,
                scopes TEXT NOT NULL DEFAULT 'player'
            );

            CREATE TABLE IF NOT EXISTS objects (
                ref_id TEXT PRIMARY KEY,
                key TEXT NOT NULL,
                kind TEXT NOT NULL,
                title TEXT,
                description TEXT NOT NULL DEFAULT '',
                location_ref TEXT,
                target_ref TEXT,
                attrs_json TEXT NOT NULL DEFAULT '{}',
                aliases_json TEXT NOT NULL DEFAULT '[]',
                script_json TEXT NOT NULL DEFAULT 'null',
                libs_json TEXT NOT NULL DEFAULT '{}',
                locks_json TEXT NOT NULL DEFAULT '{}',
                attr_schema_json TEXT NOT NULL DEFAULT '[]',
                id TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tags (
                object_ref TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT '',
                key TEXT NOT NULL,
                PRIMARY KEY (object_ref, category, key)
            );

            CREATE TABLE IF NOT EXISTS scripts (
                name TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                entry TEXT NOT NULL DEFAULT 'on_tick',
                interval INTEGER NOT NULL DEFAULT 1,
                enabled INTEGER NOT NULL DEFAULT 1,
                state_json TEXT NOT NULL DEFAULT '{}'
            );

            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS file_hashes (
                path TEXT PRIMARY KEY,
                hash TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS file_sources (
                path TEXT PRIMARY KEY,
                toml TEXT NOT NULL
            );",
        )?;

        // Migrations for DBs created before these columns existed
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN script_json TEXT NOT NULL DEFAULT 'null'", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN libs_json TEXT NOT NULL DEFAULT '{}'", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN locks_json TEXT NOT NULL DEFAULT '{}'", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN target_ref TEXT", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN aliases_json TEXT NOT NULL DEFAULT '[]'", []);
        let _ = self.conn.execute("ALTER TABLE accounts ADD COLUMN email TEXT", []);
        let _ = self.conn.execute("ALTER TABLE accounts ADD COLUMN characters_json TEXT NOT NULL DEFAULT '[]'", []);
        let _ = self.conn.execute("ALTER TABLE accounts ADD COLUMN active_character TEXT", []);
        let _ = self.conn.execute("ALTER TABLE accounts ADD COLUMN max_characters INTEGER", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN owner_ref TEXT", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN archetype_ref TEXT", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN attr_schema_json TEXT NOT NULL DEFAULT '[]'", []);

        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scheduled_hooks (
                id TEXT PRIMARY KEY,
                fire_at_tick INTEGER NOT NULL,
                target TEXT NOT NULL,
                hook TEXT NOT NULL,
                data_json TEXT
            );

            CREATE TABLE IF NOT EXISTS api_tokens (
                token_hash TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                label TEXT NOT NULL,
                expires_at INTEGER
            );"
        )?;

        let _ = self.conn.execute("ALTER TABLE api_tokens ADD COLUMN expires_at INTEGER", []);

        // `@import`'s upgrade path needs a baseline: the hash of the source an
        // import last installed for a `(obj_ref, hook)` key, so a later
        // re-import can tell "recorded vs current vs incoming" apart — the
        // three-way comparison dpkg uses for conffiles. This backs the
        // maps/terrain (`file_sources`) reconciliation; the `hook` column is a
        // generic sub-key (e.g. the map path under `FILE_SOURCE_REF`).
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS import_hashes (
                obj_ref TEXT NOT NULL,
                hook    TEXT NOT NULL,
                hash    TEXT NOT NULL,
                PRIMARY KEY (obj_ref, hook)
            );",
        )?;

        // Applied content migrations (≈ Alembic's alembic_version, but a full
        // ledger rather than a single head): one row per revision that has run,
        // so `hearth migrate` applies only the pending ones and is safe to
        // re-run / redeploy. See `crate::migrate`.
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS migrations (
                revision    TEXT PRIMARY KEY,
                description TEXT NOT NULL DEFAULT '',
                applied_at  INTEGER
            );",
        )?;
        // Program version history (program_blobs / program_versions) was
        // removed with the move to one script per object — drop the tables so
        // old DBs don't carry dead weight.
        let _ = self.conn.execute("DROP TABLE IF EXISTS program_versions", []);
        let _ = self.conn.execute("DROP TABLE IF EXISTS program_blobs", []);

        Ok(())
    }

    /// Record the blake3 hash of the source `@import` most recently
    /// installed for `(obj_ref, hook)` — the "recorded" side of the
    /// recorded/current/incoming comparison in
    /// docs/plans/program-authoring.md Stage 4. Overwrites any previous
    /// value: only the *last* import's hash matters for the next one.
    pub fn set_import_hash(&self, obj_ref: &str, hook: &str, hash: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO import_hashes (obj_ref, hook, hash) VALUES (?1, ?2, ?3)
             ON CONFLICT(obj_ref, hook) DO UPDATE SET hash = excluded.hash",
            params![obj_ref, hook, hash],
        )?;
        Ok(())
    }

    /// The hash the last `@import` recorded for `(obj_ref, hook)`, if any.
    /// `None` means this hook has never been written by an import — either
    /// it's brand new, or it was authored in-game with no import baseline to
    /// compare against.
    pub fn get_import_hash(&self, obj_ref: &str, hook: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT hash FROM import_hashes WHERE obj_ref = ?1 AND hook = ?2",
                params![obj_ref, hook],
                |row| row.get(0),
            )
            .optional()
    }

    /// The set of content-migration revisions already applied to this DB.
    /// `hearth migrate` skips these and runs only the pending ones in order.
    pub fn load_applied_migrations(&self) -> rusqlite::Result<HashSet<String>> {
        let mut stmt = self.conn.prepare("SELECT revision FROM migrations")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut set = HashSet::new();
        for r in rows {
            set.insert(r?);
        }
        Ok(set)
    }

    /// Record a migration revision as applied. `applied_at` is a Unix
    /// timestamp (seconds); pass 0 if a clock isn't available (dry runs never
    /// call this). Idempotent: re-recording the same revision is a no-op.
    pub fn record_migration(
        &self,
        revision: &str,
        description: &str,
        applied_at: u64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO migrations (revision, description, applied_at) \
             VALUES (?1, ?2, ?3)",
            params![revision, description, applied_at as i64],
        )?;
        Ok(())
    }

    /// Persist the game-clock minute counter in the `meta` table (alongside
    /// `next_id`), so in-world time survives a restart.
    pub fn save_game_minute(&self, minute: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('game_minute', ?1)",
            [minute.to_string()],
        )?;
        Ok(())
    }

    /// The saved game-clock minute counter, or 0 if none was stored yet.
    pub fn load_game_minute(&self) -> rusqlite::Result<u64> {
        let v: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'game_minute'", [], |r| r.get(0))
            .optional()?;
        Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    pub fn save_accounts(&self, accounts: &AccountStore) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM accounts", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO accounts (id, username, password_hash, character_ref, email, scopes, characters_json, active_character, max_characters) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for account in accounts.all() {
                let scopes: Vec<&str> = account.scopes.iter().map(|s| s.label()).collect();
                let scopes_str = scopes.join(",");
                let characters_json = serde_json::to_string(&account.characters).unwrap_or_else(|_| "[]".into());
                stmt.execute(params![
                    account.id,
                    account.username,
                    account.password_hash,
                    account.active_character,
                    account.email,
                    scopes_str,
                    characters_json,
                    account.active_character,
                    account.max_characters,
                ])?;
            }
        }
        tx.commit()
    }

    pub fn load_accounts(&self) -> rusqlite::Result<AccountStore> {
        let mut store = AccountStore::new();
        let mut stmt = self.conn.prepare(
            "SELECT id, username, password_hash, character_ref, email, scopes, characters_json, active_character, max_characters FROM accounts",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let username: String = row.get(1)?;
            let password_hash: String = row.get(2)?;
            let character_ref: Option<String> = row.get(3)?;
            let email: Option<String> = row.get(4)?;
            let scopes_str: String = row.get(5)?;
            let characters_json: Option<String> = row.get(6).unwrap_or(None);
            let active_character: Option<String> = row.get(7).unwrap_or(None);
            let max_characters: Option<u8> = row.get(8).unwrap_or(None);
            Ok((id, username, password_hash, character_ref, email, scopes_str, characters_json, active_character, max_characters))
        })?;

        for row in rows {
            let (id, username, password_hash, character_ref, email, scopes_str, characters_json, active_character, max_characters) = row?;
            let scopes: HashSet<Scope> = scopes_str
                .split(',')
                .filter_map(|s| Scope::parse(s.trim()))
                .collect();

            let characters: Vec<String> = characters_json
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok())
                .unwrap_or_default();

            // Migration: if characters is empty but character_ref exists, migrate
            let (characters, active_character) = if characters.is_empty() {
                if let Some(ref cr) = character_ref {
                    if !cr.is_empty() {
                        (vec![cr.clone()], Some(cr.clone()))
                    } else {
                        (Vec::new(), None)
                    }
                } else {
                    (Vec::new(), active_character)
                }
            } else {
                (characters, active_character)
            };

            let account = Account {
                id,
                username,
                password_hash,
                characters,
                active_character,
                max_characters,
                email,
                scopes,
            };
            store.insert(account);
        }
        Ok(store)
    }

    /// Persist the content hash of every game file seen at the last load.
    ///
    /// `load_game_dir` already skips files whose hash is unchanged, which is
    /// why `@reload-world` is cheap — but `Engine::new` had nowhere to get a
    /// previous set from, so every boot re-read and reinstalled the whole game
    /// directory. Storing them is what carries the skip across a restart.
    ///
    /// Replaces the stored set rather than merging, so a file deleted from the
    /// game directory stops being remembered.
    pub fn save_file_hashes(
        &self,
        hashes: &HashMap<std::path::PathBuf, String>,
    ) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM file_hashes", [])?;
        {
            let mut stmt =
                tx.prepare("INSERT INTO file_hashes (path, hash) VALUES (?1, ?2)")?;
            for (path, hash) in hashes {
                stmt.execute(rusqlite::params![path.to_string_lossy(), hash])?;
            }
        }
        tx.commit()
    }

    /// The counterpart to [`Database::save_file_hashes`]. An empty map means
    /// "nothing known", which correctly makes the next load treat every file
    /// as changed.
    pub fn load_file_hashes(&self) -> rusqlite::Result<HashMap<std::path::PathBuf, String>> {
        let mut stmt = self.conn.prepare("SELECT path, hash FROM file_hashes")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                std::path::PathBuf::from(row.get::<_, String>(0)?),
                row.get::<_, String>(1)?,
            ))
        })?;
        rows.collect()
    }

    /// Map + terrain sources (TOML), keyed by game-dir-relative path
    /// (`"terrain.toml"`, `"maps/<name>.toml"`). The DB owns these once
    /// seeded — see [`Database::seed_file_source`] / [`Database::save_file_source`].
    ///
    /// Seed a source only if `path` is absent: on-disk files provide the
    /// defaults for a fresh database, but never overwrite content the DB
    /// already owns (builder edits, imports), so map edits survive restart
    /// and redeploy the way world content does.
    pub fn seed_file_source(&self, path: &str, toml: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO file_sources (path, toml) VALUES (?1, ?2)",
            rusqlite::params![path, toml],
        )?;
        Ok(())
    }

    /// Upsert a map source — the builder's write path. The DB now owns it.
    pub fn save_file_source(&self, path: &str, toml: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO file_sources (path, toml) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET toml = excluded.toml",
            rusqlite::params![path, toml],
        )?;
        Ok(())
    }

    pub fn load_file_sources(&self) -> rusqlite::Result<HashMap<String, String>> {
        let mut stmt = self.conn.prepare("SELECT path, toml FROM file_sources")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect()
    }

    pub fn save_world(&self, world: &World) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM objects", [])?;
        tx.execute("DELETE FROM tags", [])?;
        tx.execute("DELETE FROM scripts", [])?;
        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('next_id', ?1)",
            params![world.next_id.to_string()],
        )?;

        {
            let mut obj_stmt = tx.prepare(
                "INSERT INTO objects (ref_id, key, kind, title, description, location_ref, target_ref, attrs_json, aliases_json, script_json, libs_json, locks_json, id, owner_ref, archetype_ref, attr_schema_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            )?;
            let mut tag_stmt = tx.prepare(
                "INSERT INTO tags (object_ref, category, key) VALUES (?1, ?2, ?3)",
            )?;

            for obj in world.objects.values() {
                let kind_str = obj.kind.to_string();
                let attrs_json = serde_json::to_string(&obj.attrs).unwrap_or_else(|_| "{}".into());
                let aliases: Vec<&String> = obj.aliases.iter().collect();
                let aliases_json = serde_json::to_string(&aliases).unwrap_or_else(|_| "[]".into());
                let script_json =
                    serde_json::to_string(&obj.script).unwrap_or_else(|_| "null".into());
                let libs_json =
                    serde_json::to_string(&obj.libs).unwrap_or_else(|_| "{}".into());
                let locks_json =
                    serde_json::to_string(&obj.locks).unwrap_or_else(|_| "{}".into());
                let attr_schema_json =
                    serde_json::to_string(&obj.attr_schema).unwrap_or_else(|_| "[]".into());
                obj_stmt.execute(params![
                    obj.ref_id,
                    obj.key,
                    kind_str,
                    obj.title,
                    obj.description,
                    obj.location_ref,
                    obj.target_ref,
                    attrs_json,
                    aliases_json,
                    script_json,
                    libs_json,
                    locks_json,
                    obj.id,
                    obj.owner_ref,
                    obj.archetype_ref,
                    attr_schema_json,
                ])?;
                for tag in &obj.tags {
                    tag_stmt.execute(params![obj.ref_id, tag.category, tag.key])?;
                }
            }
        }

        tx.commit()
    }

    /// Persist only the objects changed since the last drain of
    /// `World::dirty` — upserts for writes/creations, deletes for removals.
    /// Avoids the full-world DELETE+re-serialize of [`Self::save_world`] on
    /// the periodic autosave. Falls back to semantics identical to a full
    /// save when the caller passes an empty change set (no-op).
    pub fn save_world_delta(
        &self,
        world: &World,
        changes: &HashMap<String, bool>,
    ) -> rusqlite::Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('next_id', ?1)",
            params![world.next_id.to_string()],
        )?;

        {
            let mut obj_stmt = tx.prepare(
                "INSERT OR REPLACE INTO objects (ref_id, key, kind, title, description, location_ref, target_ref, attrs_json, aliases_json, script_json, libs_json, locks_json, id, owner_ref, archetype_ref, attr_schema_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            )?;
            let mut tag_del = tx.prepare("DELETE FROM tags WHERE object_ref = ?1")?;
            let mut tag_stmt = tx.prepare(
                "INSERT INTO tags (object_ref, category, key) VALUES (?1, ?2, ?3)",
            )?;
            let mut obj_del = tx.prepare("DELETE FROM objects WHERE ref_id = ?1")?;

            for (ref_id, exists) in changes {
                if !exists {
                    tag_del.execute(params![ref_id])?;
                    obj_del.execute(params![ref_id])?;
                    continue;
                }
                let Some(obj) = world.get(ref_id) else { continue };
                let kind_str = obj.kind.to_string();
                let attrs_json = serde_json::to_string(&obj.attrs).unwrap_or_else(|_| "{}".into());
                let aliases: Vec<&String> = obj.aliases.iter().collect();
                let aliases_json = serde_json::to_string(&aliases).unwrap_or_else(|_| "[]".into());
                let script_json =
                    serde_json::to_string(&obj.script).unwrap_or_else(|_| "null".into());
                let libs_json =
                    serde_json::to_string(&obj.libs).unwrap_or_else(|_| "{}".into());
                let locks_json =
                    serde_json::to_string(&obj.locks).unwrap_or_else(|_| "{}".into());
                let attr_schema_json =
                    serde_json::to_string(&obj.attr_schema).unwrap_or_else(|_| "[]".into());
                // Tags are cheap to rewrite wholesale per object.
                tag_del.execute(params![ref_id])?;
                obj_stmt.execute(params![
                    obj.ref_id,
                    obj.key,
                    kind_str,
                    obj.title,
                    obj.description,
                    obj.location_ref,
                    obj.target_ref,
                    attrs_json,
                    aliases_json,
                    script_json,
                    libs_json,
                    locks_json,
                    obj.id,
                    obj.owner_ref,
                    obj.archetype_ref,
                    attr_schema_json,
                ])?;
                for tag in &obj.tags {
                    tag_stmt.execute(params![obj.ref_id, tag.category, tag.key])?;
                }
            }
        }

        tx.commit()
    }

    pub fn load_world(&self) -> rusqlite::Result<World> {
        let mut world = World::new();

        let mut obj_stmt = self.conn.prepare(
            "SELECT ref_id, key, kind, title, description, location_ref, target_ref, attrs_json, aliases_json, script_json, libs_json, locks_json, id, owner_ref, archetype_ref, attr_schema_json FROM objects",
        )?;
        let obj_rows = obj_stmt.query_map([], |row| {
            let ref_id: String = row.get(0)?;
            let key: String = row.get(1)?;
            let kind_str: String = row.get(2)?;
            let title: Option<String> = row.get(3)?;
            let description: String = row.get(4)?;
            let location_ref: Option<String> = row.get(5)?;
            let target_ref: Option<String> = row.get(6)?;
            let attrs_json: String = row.get(7)?;
            let aliases_json: String = row.get(8)?;
            let script_json: String = row.get(9)?;
            let libs_json: String = row.get(10)?;
            let locks_json: String = row.get(11)?;
            let id: String = row.get(12)?;
            let owner_ref: Option<String> = row.get(13)?;
            let archetype_ref: Option<String> = row.get(14)?;
            let attr_schema_json: String = row.get(15)?;
            Ok((
                ref_id, key, kind_str, title, description, location_ref,
                target_ref, attrs_json, aliases_json, script_json, libs_json, locks_json, id,
                owner_ref, archetype_ref, attr_schema_json,
            ))
        })?;

        for row in obj_rows {
            let (ref_id, key, kind_str, title, description, location_ref,
                target_ref, attrs_json, aliases_json, script_json, libs_json, locks_json, id,
                owner_ref, archetype_ref, attr_schema_json) = row?;
            let kind = match kind_str.as_str() {
                "room" => Kind::Room,
                "item" => Kind::Item,
                "npc" => Kind::Npc,
                "player" => Kind::Player,
                "exit" => Kind::Exit,
                "code" => Kind::Code,
                _ => Kind::Item,
            };
            let attrs: HashMap<String, serde_json::Value> =
                serde_json::from_str(&attrs_json).unwrap_or_default();
            let aliases_vec: Vec<String> =
                serde_json::from_str(&aliases_json).unwrap_or_default();
            let aliases: HashSet<String> = aliases_vec.into_iter().collect();
            let script: Option<ObjectScript> =
                serde_json::from_str(&script_json).unwrap_or_default();
            let libs: HashMap<String, LibModule> =
                serde_json::from_str(&libs_json).unwrap_or_default();
            let locks: HashMap<String, String> =
                serde_json::from_str(&locks_json).unwrap_or_default();
            let attr_schema: Vec<crate::attr_schema::AttrDescriptor> =
                serde_json::from_str(&attr_schema_json).unwrap_or_default();
            let obj = GameObject {
                ref_id: ref_id.clone(),
                key,
                kind,
                title,
                description,
                location_ref,
                owner_ref,
                target_ref,
                archetype_ref,
                attrs,
                attr_schema,
                tags: HashSet::new(),
                aliases,
                script,
                libs,
                locks,
                id,
            };
            world.add_object(obj);
        }

        // Load tags
        let mut tag_stmt = self.conn.prepare(
            "SELECT object_ref, category, key FROM tags",
        )?;
        let tag_rows = tag_stmt.query_map([], |row| {
            let object_ref: String = row.get(0)?;
            let category: String = row.get(1)?;
            let key: String = row.get(2)?;
            Ok((object_ref, category, key))
        })?;
        for row in tag_rows {
            let (object_ref, category, key) = row?;
            if let Some(obj) = world.get_mut(&object_ref) {
                obj.tags.insert(Tag { category, key });
            }
        }

        let next_id: u64 = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = 'next_id'", [], |row| {
                row.get::<_, String>(0)
            })
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        world.next_id = next_id;

        // Migrate legacy `scripts` rows (pre-`Kind::Code`, see
        // docs/plans/program-authoring.md Stage 2) into `Kind::Code`
        // objects carrying an `on_tick` Program — the same object shape a
        // freshly-authored `@script` produces. This is a Rust-side
        // migration, not an `@eval` job: softcode has no access to this
        // table. `next_id` is read above so the dbrefs handed out here
        // continue the existing counter rather than colliding with it.
        //
        // `save_world` never writes to `scripts` anymore (it only deletes
        // from it, to keep old rows from resurrecting), so after the first
        // save following this migration the table is empty and this loop
        // is a no-op on every subsequent boot.
        let mut legacy_stmt = self
            .conn
            .prepare("SELECT name, source, entry, interval, enabled, state_json FROM scripts")?;
        let legacy_rows = legacy_stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let source: String = row.get(1)?;
            let entry: String = row.get(2)?;
            let interval: u64 = row.get(3)?;
            let enabled: bool = row.get(4)?;
            let state_json: String = row.get(5)?;
            Ok((name, source, entry, interval, enabled, state_json))
        })?;
        for row in legacy_rows {
            let (name, source, entry, interval, enabled, state_json) = row?;
            if entry != "on_tick" {
                tracing::warn!(
                    script = %name, entry = %entry,
                    "Migrating legacy global script with non-on_tick entry; only on_tick runs on Kind::Code objects"
                );
            }
            let state: HashMap<String, serde_json::Value> =
                serde_json::from_str(&state_json).unwrap_or_default();
            let ref_id = world.next_dbref();
            let mut obj = GameObject::new(&ref_id, &name, Kind::Code);
            obj.attrs.insert("tick_interval".into(), serde_json::json!(interval));
            hooks::set_script_with_origin(&mut obj, source, hooks::ProgramOrigin::InGame);
            if let Some(script) = obj.script.as_mut() {
                script.enabled = enabled;
                script.state = state;
            }
            world.add_object(obj);
            tracing::info!(script = %name, ref_id = %ref_id, "Migrated legacy global script to Kind::Code object");
        }

        Ok(world)
    }

    pub fn has_world_data(&self) -> bool {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM objects", [], |row| row.get(0))
            .unwrap_or(0);
        count > 0
    }

    pub fn save_scheduled_hooks(
        &self,
        hooks: &[crate::softcode::ScheduledHook],
    ) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM scheduled_hooks", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO scheduled_hooks (id, fire_at_tick, target, hook, data_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for h in hooks {
                let data_json = h
                    .data
                    .as_ref()
                    .map(|d| serde_json::to_string(d).unwrap_or_default());
                stmt.execute(params![h.id, h.fire_at_tick, h.target, h.hook, data_json])?;
            }
        }
        tx.commit()
    }

    pub fn load_scheduled_hooks(&self) -> rusqlite::Result<Vec<crate::softcode::ScheduledHook>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, fire_at_tick, target, hook, data_json FROM scheduled_hooks",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let fire_at_tick: u64 = row.get(1)?;
            let target: String = row.get(2)?;
            let hook: String = row.get(3)?;
            let data_json: Option<String> = row.get(4)?;
            Ok((id, fire_at_tick, target, hook, data_json))
        })?;

        let mut hooks = Vec::new();
        for row in rows {
            let (id, fire_at_tick, target, hook, data_json) = row?;
            let data = data_json.and_then(|s| serde_json::from_str(&s).ok());
            hooks.push(crate::softcode::ScheduledHook {
                id,
                fire_at_tick,
                target,
                hook,
                data,
            });
        }
        Ok(hooks)
    }

    pub fn save_tokens(&self, tokens: &[(String, String, String, Option<u64>)]) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM api_tokens", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO api_tokens (token_hash, account_id, label, expires_at) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (hash, account_id, label, expires_at) in tokens {
                stmt.execute(params![hash, account_id, label, expires_at])?;
            }
        }
        tx.commit()
    }

    pub fn load_tokens(&self) -> rusqlite::Result<Vec<(String, String, String, Option<u64>)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT token_hash, account_id, label, expires_at FROM api_tokens")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<u64>>(3)?,
            ))
        })?;
        rows.collect()
    }


    fn now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::Scope;
    use std::path::Path;

    fn temp_db() -> Database {
        Database::open(Path::new(":memory:")).unwrap()
    }

    /// Delta saves must compose with a prior full/delta save: upserts
    /// update in place, removals delete rows, and unchanged objects are
    /// left alone but still load.
    #[test]
    fn world_delta_round_trip() {
        let db = temp_db();
        let mut world = World::new();

        let room_ref = world.next_dbref();
        world.add_object(GameObject::new(&room_ref, "hall", Kind::Room));
        let item_ref = world.next_dbref();
        world.add_object(
            GameObject::new(&item_ref, "sword", Kind::Item).with_location(&room_ref),
        );

        // First save: everything is dirty (loader-style fresh start).
        let changes = world.drain_dirty();
        assert_eq!(changes.len(), 2);
        db.save_world_delta(&world, &changes).unwrap();

        // Mutate one object, delete the other.
        world.get_mut(&room_ref).unwrap().title = Some("Renamed".into());
        world.remove_object(&item_ref);
        let changes = world.drain_dirty();
        assert_eq!(changes.len(), 2);
        db.save_world_delta(&world, &changes).unwrap();

        let loaded = db.load_world().unwrap();
        assert!(loaded.get(&room_ref).is_some());
        assert_eq!(loaded.get(&room_ref).unwrap().title.as_deref(), Some("Renamed"));
        assert!(loaded.get(&item_ref).is_none());
    }

    #[test]
    fn world_round_trip_objects() {
        let db = temp_db();
        let mut world = World::new();

        let hall_ref = world.next_dbref(); // "#1"
        let mut room = GameObject::new(&hall_ref, "hall", Kind::Room)
            .with_title("Great Hall");
        room.description = "A grand hall.".into();
        room.attrs.insert("mood".into(), serde_json::json!("eerie"));
        room.tags.insert(Tag::parse("zone:castle").unwrap());
        room.locks.insert("enter".into(), "perm(builder)".into());
        world.add_object(room);

        let sword_ref = world.next_dbref(); // "#2"
        let item = GameObject::new(&sword_ref, "sword", Kind::Item)
            .with_title("a sharp sword")
            .with_location(&hall_ref);
        world.add_object(item);

        db.save_world(&world).unwrap();
        let loaded = db.load_world().unwrap();

        assert_eq!(loaded.objects.len(), 2);

        let room = loaded.get(&hall_ref).unwrap();
        assert_eq!(room.title.as_deref(), Some("Great Hall"));
        assert_eq!(room.description, "A grand hall.");
        assert_eq!(room.attrs.get("mood").unwrap(), "eerie");
        assert!(room.tags.contains(&Tag::parse("zone:castle").unwrap()));
        assert_eq!(room.locks.get("enter").unwrap(), "perm(builder)");

        let item = loaded.get(&sword_ref).unwrap();
        assert_eq!(item.location_ref.as_deref(), Some(hall_ref.as_str()));
    }

    #[test]
    fn world_round_trip_exits() {
        let db = temp_db();
        let mut world = World::new();

        let a_ref = world.next_dbref(); // "#1"
        let b_ref = world.next_dbref(); // "#2"
        let room_a = GameObject::new(&a_ref, "a", Kind::Room).with_title("Room A");
        let room_b = GameObject::new(&b_ref, "b", Kind::Room).with_title("Room B");
        world.add_object(room_a);
        world.add_object(room_b);

        let exit_ref = world.next_dbref(); // "#3"
        let exit = GameObject::new(&exit_ref, "north", Kind::Exit)
            .with_location(&a_ref)
            .with_target(&b_ref)
            .with_aliases(vec!["n"]);
        world.add_object(exit);

        db.save_world(&world).unwrap();
        let loaded = db.load_world().unwrap();

        let exits = loaded.exits_from(&a_ref);
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].key, "north");
        assert_eq!(exits[0].target_ref.as_deref(), Some(b_ref.as_str()));
        assert!(exits[0].aliases.contains("n"));
    }

    #[test]
    fn world_round_trip_programs() {
        let db = temp_db();
        let mut world = World::new();

        let gem_ref = world.next_dbref(); // "#1"
        let mut obj = GameObject::new(&gem_ref, "gem", Kind::Item);
        hooks::set_script(
            &mut obj,
            "function on_get(this, actor, room) emit(actor, \"Sparkle!\") end".into(),
        );
        world.add_object(obj);

        db.save_world(&world).unwrap();
        let loaded = db.load_world().unwrap();

        let gem = loaded.get(&gem_ref).unwrap();
        assert!(hooks::object_defines_hook(gem, "on_get"));
        assert!(gem.script.as_ref().unwrap().source.contains("Sparkle!"));
    }

    #[test]
    fn world_round_trip_archetype_ref() {
        // An instance's archetype_ref must survive a checkpoint — otherwise
        // every instance becomes standalone (losing inherited title/attrs/
        // hooks) on the next restart. Regression for the persistence gap.
        let db = temp_db();
        let mut world = World::new();

        let arch_ref = world.next_dbref();
        world.add_object(GameObject::new(&arch_ref, "goblin", Kind::Npc).with_title("Goblin"));
        let inst_ref = world.next_dbref();
        let mut inst = GameObject::new(&inst_ref, "grunt", Kind::Npc);
        inst.archetype_ref = Some(arch_ref.clone());
        world.add_object(inst);

        db.save_world(&world).unwrap();
        let loaded = db.load_world().unwrap();

        assert_eq!(
            loaded.get(&inst_ref).unwrap().archetype_ref.as_deref(),
            Some(arch_ref.as_str()),
            "archetype_ref must round-trip through save/load"
        );
        // And the archetype with none stays None.
        assert_eq!(loaded.get(&arch_ref).unwrap().archetype_ref, None);
    }

    #[test]
    fn world_round_trip_attr_schema() {
        use crate::attr_schema::{AttrDescriptor, AttrType};
        let db = temp_db();
        let mut world = World::new();

        let ref_id = world.next_dbref();
        let mut obj = GameObject::new(&ref_id, "monster", Kind::Npc);
        let mut hp = AttrDescriptor::new("hp", AttrType::Int);
        hp.label = Some("Hit points".into());
        hp.min = Some(0.0);
        hp.default = Some(serde_json::json!(1));
        obj.attr_schema = vec![hp, AttrDescriptor::new("biome", AttrType::Enum)];
        world.add_object(obj);

        db.save_world(&world).unwrap();
        let loaded = db.load_world().unwrap();

        let schema = &loaded.get(&ref_id).unwrap().attr_schema;
        assert_eq!(schema.len(), 2, "attr_schema must round-trip through save/load");
        assert_eq!(schema[0].key, "hp");
        assert_eq!(schema[0].ty, AttrType::Int);
        assert_eq!(schema[0].label.as_deref(), Some("Hit points"));
        assert_eq!(schema[0].min, Some(0.0));
        assert_eq!(schema[0].default, Some(serde_json::json!(1)));
        assert_eq!(schema[1].ty, AttrType::Enum);
    }

    #[test]
    fn world_round_trip_code_object_script() {
        let db = temp_db();
        let mut world = World::new();

        let ref_id = world.next_dbref();
        let mut obj = GameObject::new(&ref_id, "weather", Kind::Code);
        obj.attrs.insert("tick_interval".into(), serde_json::json!(60));
        hooks::set_script(&mut obj, "function on_tick(this, state, room) end".into());
        obj.script.as_mut().unwrap().state.insert("weather".into(), serde_json::json!("clear"));
        world.add_object(obj);

        db.save_world(&world).unwrap();
        let loaded = db.load_world().unwrap();

        let s = loaded.get(&ref_id).unwrap();
        assert_eq!(s.kind, Kind::Code);
        assert_eq!(s.attrs["tick_interval"], serde_json::json!(60));
        let script = s.script.as_ref().unwrap();
        assert_eq!(script.state.get("weather").unwrap(), "clear");
    }

    /// A world saved by a pre-`Kind::Code` build of Hearth has its global
    /// scripts sitting in the legacy `scripts` table. `load_world` must
    /// migrate those rows into `Kind::Code` objects in memory — softcode
    /// has no access to that table, so this has to be a Rust-side
    /// migration (see docs/plans/program-authoring.md Stage 2) — and the
    /// migrated object must still be tickable through the normal
    /// per-object `on_tick` path.
    #[test]
    fn load_world_migrates_legacy_scripts_table_to_code_objects() {
        let db = temp_db();
        // Simulate a pre-migration database: write directly to the legacy
        // `scripts` table, bypassing `save_world` (which no longer writes
        // to it at all).
        db.conn
            .execute(
                "INSERT INTO scripts (name, source, entry, interval, enabled, state_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "weather",
                    "function on_tick(state) state.ticks = (state.ticks or 0) + 1 end",
                    "on_tick",
                    5,
                    1,
                    r#"{"ticks": 3}"#,
                ],
            )
            .unwrap();

        let world = db.load_world().unwrap();

        let migrated = world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "weather")
            .expect("legacy script should migrate to a Kind::Code object");
        assert_eq!(migrated.attrs["tick_interval"], serde_json::json!(5));
        let script = migrated
            .script
            .as_ref()
            .expect("migrated object should carry a script");
        assert!(script.enabled);
        assert!(script.defines("on_tick"));
        assert_eq!(script.state.get("ticks").unwrap(), &serde_json::json!(3));
        assert!(script.source.contains("state.ticks"));

        // Saving and reloading should not re-migrate (the legacy table is
        // emptied on save) or lose the object.
        db.save_world(&world).unwrap();
        let reloaded = db.load_world().unwrap();
        let count = reloaded
            .objects
            .values()
            .filter(|o| o.kind == Kind::Code && o.key == "weather")
            .count();
        assert_eq!(count, 1, "reload after save should not duplicate the migrated object");
    }

    #[test]
    fn accounts_round_trip() {
        let db = temp_db();
        let mut store = AccountStore::new();
        store.create("admin", "password123").unwrap();
        store.create("player2", "password456").unwrap();

        db.save_accounts(&store).unwrap();
        let loaded = db.load_accounts().unwrap();

        let admin = loaded.get_by_username("admin").unwrap();
        assert!(admin.scopes.contains(&Scope::Admin));

        let player = loaded.get_by_username("player2").unwrap();
        assert!(player.scopes.contains(&Scope::Player));
        assert!(!player.scopes.contains(&Scope::Admin));
    }

    #[test]
    fn empty_db_has_no_world_data() {
        let db = temp_db();
        assert!(!db.has_world_data());
    }

    #[test]
    fn save_then_has_world_data() {
        let db = temp_db();
        let mut world = World::new();
        let ref_id = world.next_dbref();
        world.add_object(GameObject::new(&ref_id, "x", Kind::Room));
        db.save_world(&world).unwrap();
        assert!(db.has_world_data());
    }

    #[test]
    fn next_id_round_trips() {
        let db = temp_db();
        let mut world = World::new();
        let ref1 = world.next_dbref();
        let ref2 = world.next_dbref();
        world.add_object(GameObject::new(&ref1, "a", Kind::Room));
        world.add_object(GameObject::new(&ref2, "b", Kind::Room));
        assert_eq!(world.next_id, 2);

        db.save_world(&world).unwrap();
        let loaded = db.load_world().unwrap();
        assert_eq!(loaded.next_id, 2);

        // Next dbref allocated after reload should continue the sequence.
        let mut loaded = loaded;
        let ref3 = loaded.next_dbref();
        assert_eq!(ref3, "#3");
    }

    // -- Program version history (Stage 3) --


    // -- Import hashes (Stage 4) --

    #[test]
    fn import_hash_round_trips_and_updates() {
        let db = temp_db();
        assert_eq!(db.get_import_hash("#1", "on_look").unwrap(), None);

        db.set_import_hash("#1", "on_look", "abc123").unwrap();
        assert_eq!(db.get_import_hash("#1", "on_look").unwrap().as_deref(), Some("abc123"));

        // A second import overwrites, it does not accumulate a history.
        db.set_import_hash("#1", "on_look", "def456").unwrap();
        assert_eq!(db.get_import_hash("#1", "on_look").unwrap().as_deref(), Some("def456"));
    }


    /// `load_game_dir` already skips files whose content hash is unchanged,
    /// and `@reload-world` benefits — but `Engine::new` starts from an empty
    /// map, so every boot re-reads and reinstalls the whole game directory.
    /// Persisting the hashes is what carries that skip across a restart.
    #[test]
    fn file_hashes_round_trip() {
        let db = Database::open(Path::new(":memory:")).unwrap();

        use std::path::PathBuf;

        let mut hashes = HashMap::new();
        hashes.insert(PathBuf::from("town/town.toml"), "abc123".to_string());
        hashes.insert(PathBuf::from("system/cmd_fight.luau"), "def456".to_string());
        db.save_file_hashes(&hashes).unwrap();

        let loaded = db.load_file_hashes().unwrap();
        assert_eq!(loaded, hashes);

        // A later save replaces the set rather than merging, so a file
        // deleted from the game directory stops being remembered.
        let mut fewer = HashMap::new();
        fewer.insert(PathBuf::from("town/town.toml"), "abc123".to_string());
        db.save_file_hashes(&fewer).unwrap();
        assert_eq!(db.load_file_hashes().unwrap(), fewer);
    }
}
