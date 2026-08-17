use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, params};

use crate::accounts::{Account, AccountStore, Scope};
use crate::softcode::hooks::ProgramRecord;
use crate::world::{Exit, GameObject, Kind, Tag, World};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
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
                scopes TEXT NOT NULL DEFAULT 'player'
            );

            CREATE TABLE IF NOT EXISTS objects (
                ref_id TEXT PRIMARY KEY,
                key TEXT NOT NULL,
                kind TEXT NOT NULL,
                title TEXT,
                description TEXT NOT NULL DEFAULT '',
                location_ref TEXT,
                attrs_json TEXT NOT NULL DEFAULT '{}',
                programs_json TEXT NOT NULL DEFAULT '{}',
                id TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tags (
                object_ref TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT '',
                key TEXT NOT NULL,
                PRIMARY KEY (object_ref, category, key)
            );

            CREATE TABLE IF NOT EXISTS exits (
                ref_id TEXT PRIMARY KEY,
                source_ref TEXT NOT NULL,
                target_ref TEXT NOT NULL,
                key TEXT NOT NULL,
                aliases_json TEXT NOT NULL DEFAULT '[]',
                id TEXT NOT NULL
            );",
        )?;

        // Databases created before Programs existed won't have this column;
        // CREATE TABLE IF NOT EXISTS above is a no-op for them.
        let _ = self
            .conn
            .execute("ALTER TABLE objects ADD COLUMN programs_json TEXT NOT NULL DEFAULT '{}'", []);

        Ok(())
    }

    pub fn save_accounts(&self, accounts: &AccountStore) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM accounts", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO accounts (id, username, password_hash, character_ref, scopes) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for account in accounts.all() {
                let scopes: Vec<&str> = account.scopes.iter().map(|s| s.label()).collect();
                let scopes_str = scopes.join(",");
                stmt.execute(params![
                    account.id,
                    account.username,
                    account.password_hash,
                    account.character_ref,
                    scopes_str,
                ])?;
            }
        }
        tx.commit()
    }

    pub fn load_accounts(&self) -> rusqlite::Result<AccountStore> {
        let mut store = AccountStore::new();
        let mut stmt = self.conn.prepare(
            "SELECT id, username, password_hash, character_ref, scopes FROM accounts",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let username: String = row.get(1)?;
            let password_hash: String = row.get(2)?;
            let character_ref: Option<String> = row.get(3)?;
            let scopes_str: String = row.get(4)?;
            Ok((id, username, password_hash, character_ref, scopes_str))
        })?;

        for row in rows {
            let (id, username, password_hash, character_ref, scopes_str) = row?;
            let scopes: HashSet<Scope> = scopes_str
                .split(',')
                .filter_map(|s| Scope::parse(s.trim()))
                .collect();
            let account = Account {
                id,
                username,
                password_hash,
                character_ref,
                scopes,
            };
            store.insert(account);
        }
        Ok(store)
    }

    pub fn save_world(&self, world: &World) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM objects", [])?;
        tx.execute("DELETE FROM tags", [])?;
        tx.execute("DELETE FROM exits", [])?;

        {
            let mut obj_stmt = tx.prepare(
                "INSERT INTO objects (ref_id, key, kind, title, description, location_ref, attrs_json, programs_json, id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            let mut tag_stmt = tx.prepare(
                "INSERT INTO tags (object_ref, category, key) VALUES (?1, ?2, ?3)",
            )?;

            for obj in world.objects.values() {
                let kind_str = obj.kind.to_string();
                let attrs_json = serde_json::to_string(&obj.attrs).unwrap_or_else(|_| "{}".into());
                let programs_json =
                    serde_json::to_string(&obj.programs).unwrap_or_else(|_| "{}".into());
                obj_stmt.execute(params![
                    obj.ref_id,
                    obj.key,
                    kind_str,
                    obj.title,
                    obj.description,
                    obj.location_ref,
                    attrs_json,
                    programs_json,
                    obj.id,
                ])?;
                for tag in &obj.tags {
                    tag_stmt.execute(params![obj.ref_id, tag.category, tag.key])?;
                }
            }
        }

        {
            let mut exit_stmt = tx.prepare(
                "INSERT INTO exits (ref_id, source_ref, target_ref, key, aliases_json, id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for exit in &world.exits {
                let aliases_json = serde_json::to_string(&exit.aliases).unwrap_or_else(|_| "[]".into());
                exit_stmt.execute(params![
                    exit.ref_id,
                    exit.source_ref,
                    exit.target_ref,
                    exit.key,
                    aliases_json,
                    exit.id,
                ])?;
            }
        }

        tx.commit()
    }

    pub fn load_world(&self) -> rusqlite::Result<World> {
        let mut world = World::new();

        let mut obj_stmt = self.conn.prepare(
            "SELECT ref_id, key, kind, title, description, location_ref, attrs_json, programs_json, id FROM objects",
        )?;
        let obj_rows = obj_stmt.query_map([], |row| {
            let ref_id: String = row.get(0)?;
            let key: String = row.get(1)?;
            let kind_str: String = row.get(2)?;
            let title: Option<String> = row.get(3)?;
            let description: String = row.get(4)?;
            let location_ref: Option<String> = row.get(5)?;
            let attrs_json: String = row.get(6)?;
            let programs_json: String = row.get(7)?;
            let id: String = row.get(8)?;
            Ok((
                ref_id,
                key,
                kind_str,
                title,
                description,
                location_ref,
                attrs_json,
                programs_json,
                id,
            ))
        })?;

        for row in obj_rows {
            let (ref_id, key, kind_str, title, description, location_ref, attrs_json, programs_json, id) =
                row?;
            let kind = match kind_str.as_str() {
                "room" => Kind::Room,
                "item" => Kind::Item,
                "npc" => Kind::Npc,
                "player" => Kind::Player,
                _ => Kind::Item,
            };
            let attrs: HashMap<String, serde_json::Value> =
                serde_json::from_str(&attrs_json).unwrap_or_default();
            let programs: HashMap<String, ProgramRecord> =
                serde_json::from_str(&programs_json).unwrap_or_default();
            let obj = GameObject {
                ref_id: ref_id.clone(),
                key,
                kind,
                title,
                description,
                location_ref,
                attrs,
                tags: HashSet::new(),
                programs,
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

        // Load exits
        let mut exit_stmt = self.conn.prepare(
            "SELECT ref_id, source_ref, target_ref, key, aliases_json, id FROM exits",
        )?;
        let exit_rows = exit_stmt.query_map([], |row| {
            let ref_id: String = row.get(0)?;
            let source_ref: String = row.get(1)?;
            let target_ref: String = row.get(2)?;
            let key: String = row.get(3)?;
            let aliases_json: String = row.get(4)?;
            let id: String = row.get(5)?;
            Ok((ref_id, source_ref, target_ref, key, aliases_json, id))
        })?;
        for row in exit_rows {
            let (ref_id, source_ref, target_ref, key, aliases_json, id) = row?;
            let aliases: Vec<String> = serde_json::from_str(&aliases_json).unwrap_or_default();
            let exit = Exit {
                ref_id,
                source_ref,
                target_ref,
                key,
                aliases,
                id,
            };
            world.add_exit(exit);
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
}
