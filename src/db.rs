use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, params};

use crate::accounts::{Account, AccountStore, Scope};
use crate::softcode::hooks::ProgramRecord;
use crate::world::{GameObject, Kind, Tag, World};

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
                programs_json TEXT NOT NULL DEFAULT '{}',
                locks_json TEXT NOT NULL DEFAULT '{}',
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
            );",
        )?;

        // Migrations for DBs created before these columns existed
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN programs_json TEXT NOT NULL DEFAULT '{}'", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN locks_json TEXT NOT NULL DEFAULT '{}'", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN target_ref TEXT", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN aliases_json TEXT NOT NULL DEFAULT '[]'", []);
        let _ = self.conn.execute("ALTER TABLE accounts ADD COLUMN email TEXT", []);
        let _ = self.conn.execute("ALTER TABLE objects ADD COLUMN owner_ref TEXT", []);

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

        Ok(())
    }

    pub fn save_accounts(&self, accounts: &AccountStore) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM accounts", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO accounts (id, username, password_hash, character_ref, email, scopes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for account in accounts.all() {
                let scopes: Vec<&str> = account.scopes.iter().map(|s| s.label()).collect();
                let scopes_str = scopes.join(",");
                stmt.execute(params![
                    account.id,
                    account.username,
                    account.password_hash,
                    account.character_ref,
                    account.email,
                    scopes_str,
                ])?;
            }
        }
        tx.commit()
    }

    pub fn load_accounts(&self) -> rusqlite::Result<AccountStore> {
        let mut store = AccountStore::new();
        let mut stmt = self.conn.prepare(
            "SELECT id, username, password_hash, character_ref, email, scopes FROM accounts",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let username: String = row.get(1)?;
            let password_hash: String = row.get(2)?;
            let character_ref: Option<String> = row.get(3)?;
            let email: Option<String> = row.get(4)?;
            let scopes_str: String = row.get(5)?;
            Ok((id, username, password_hash, character_ref, email, scopes_str))
        })?;

        for row in rows {
            let (id, username, password_hash, character_ref, email, scopes_str) = row?;
            let scopes: HashSet<Scope> = scopes_str
                .split(',')
                .filter_map(|s| Scope::parse(s.trim()))
                .collect();
            let account = Account {
                id,
                username,
                password_hash,
                character_ref,
                email,
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
        tx.execute("DELETE FROM scripts", [])?;
        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('next_id', ?1)",
            params![world.next_id.to_string()],
        )?;

        {
            let mut obj_stmt = tx.prepare(
                "INSERT INTO objects (ref_id, key, kind, title, description, location_ref, target_ref, attrs_json, aliases_json, programs_json, locks_json, id, owner_ref) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            let mut tag_stmt = tx.prepare(
                "INSERT INTO tags (object_ref, category, key) VALUES (?1, ?2, ?3)",
            )?;

            for obj in world.objects.values() {
                let kind_str = obj.kind.to_string();
                let attrs_json = serde_json::to_string(&obj.attrs).unwrap_or_else(|_| "{}".into());
                let aliases: Vec<&String> = obj.aliases.iter().collect();
                let aliases_json = serde_json::to_string(&aliases).unwrap_or_else(|_| "[]".into());
                let programs_json =
                    serde_json::to_string(&obj.programs).unwrap_or_else(|_| "{}".into());
                let locks_json =
                    serde_json::to_string(&obj.locks).unwrap_or_else(|_| "{}".into());
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
                    programs_json,
                    locks_json,
                    obj.id,
                    obj.owner_ref,
                ])?;
                for tag in &obj.tags {
                    tag_stmt.execute(params![obj.ref_id, tag.category, tag.key])?;
                }
            }
        }

        {
            let mut script_stmt = tx.prepare(
                "INSERT INTO scripts (name, source, entry, interval, enabled, state_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for script in world.scripts.values() {
                let state_json =
                    serde_json::to_string(&script.state).unwrap_or_else(|_| "{}".into());
                script_stmt.execute(params![
                    script.name,
                    script.source,
                    script.entry,
                    script.interval,
                    script.enabled as i32,
                    state_json,
                ])?;
            }
        }

        tx.commit()
    }

    pub fn load_world(&self) -> rusqlite::Result<World> {
        let mut world = World::new();

        let mut obj_stmt = self.conn.prepare(
            "SELECT ref_id, key, kind, title, description, location_ref, target_ref, attrs_json, aliases_json, programs_json, locks_json, id, owner_ref FROM objects",
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
            let programs_json: String = row.get(9)?;
            let locks_json: String = row.get(10)?;
            let id: String = row.get(11)?;
            let owner_ref: Option<String> = row.get(12)?;
            Ok((
                ref_id, key, kind_str, title, description, location_ref,
                target_ref, attrs_json, aliases_json, programs_json, locks_json, id,
                owner_ref,
            ))
        })?;

        for row in obj_rows {
            let (ref_id, key, kind_str, title, description, location_ref,
                target_ref, attrs_json, aliases_json, programs_json, locks_json, id,
                owner_ref) = row?;
            let kind = match kind_str.as_str() {
                "room" => Kind::Room,
                "item" => Kind::Item,
                "npc" => Kind::Npc,
                "player" => Kind::Player,
                "exit" => Kind::Exit,
                _ => Kind::Item,
            };
            let attrs: HashMap<String, serde_json::Value> =
                serde_json::from_str(&attrs_json).unwrap_or_default();
            let aliases_vec: Vec<String> =
                serde_json::from_str(&aliases_json).unwrap_or_default();
            let aliases: HashSet<String> = aliases_vec.into_iter().collect();
            let programs: HashMap<String, ProgramRecord> =
                serde_json::from_str(&programs_json).unwrap_or_default();
            let locks: HashMap<String, String> =
                serde_json::from_str(&locks_json).unwrap_or_default();
            let obj = GameObject {
                ref_id: ref_id.clone(),
                key,
                kind,
                title,
                description,
                location_ref,
                owner_ref,
                target_ref,
                attrs,
                tags: HashSet::new(),
                aliases,
                programs,
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

        // Load global scripts
        let mut script_stmt = self.conn.prepare(
            "SELECT name, source, entry, interval, enabled, state_json FROM scripts",
        )?;
        let script_rows = script_stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let source: String = row.get(1)?;
            let entry: String = row.get(2)?;
            let interval: u64 = row.get(3)?;
            let enabled: bool = row.get(4)?;
            let state_json: String = row.get(5)?;
            Ok((name, source, entry, interval, enabled, state_json))
        })?;
        for row in script_rows {
            let (name, source, entry, interval, enabled, state_json) = row?;
            let state: HashMap<String, serde_json::Value> =
                serde_json::from_str(&state_json).unwrap_or_default();
            use crate::world::Script;
            let script = Script {
                name: name.clone(),
                source,
                entry,
                interval,
                enabled,
                state,
            };
            world.scripts.insert(name, script);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::Scope;
    use crate::softcode::hooks::ProgramRecord;
    use crate::world::Script;
    use std::path::Path;

    fn temp_db() -> Database {
        Database::open(Path::new(":memory:")).unwrap()
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
        crate::softcode::hooks::set_program(
            &mut obj,
            "on_get",
            "function on_get(this, actor, room) emit(actor, \"Sparkle!\") end".into(),
        )
        .unwrap();
        world.add_object(obj);

        db.save_world(&world).unwrap();
        let loaded = db.load_world().unwrap();

        let gem = loaded.get(&gem_ref).unwrap();
        assert!(gem.programs.contains_key("on_get"));
        assert!(gem.programs["on_get"].source.contains("Sparkle!"));
    }

    #[test]
    fn world_round_trip_scripts() {
        let db = temp_db();
        let mut world = World::new();

        let mut script = Script::new("weather", "function on_tick(state) end");
        script.interval = 60;
        script.state.insert("weather".into(), serde_json::json!("clear"));
        world.scripts.insert("weather".into(), script);

        db.save_world(&world).unwrap();
        let loaded = db.load_world().unwrap();

        assert!(loaded.scripts.contains_key("weather"));
        let s = &loaded.scripts["weather"];
        assert_eq!(s.interval, 60);
        assert_eq!(s.state.get("weather").unwrap(), "clear");
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
}
