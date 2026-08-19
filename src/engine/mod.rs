mod commands;

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use tokio::sync::mpsc;

use crate::accounts::{AccountStore, Scope};
use crate::config::Config;
use crate::db::Database;
use crate::locks::{self, AccessContext};
use crate::softcode::hooks::{self, ProgramRecord};
use crate::softcode::{self, Budget, Effect, SoftcodeRuntime};
use crate::world::{GameObject, Kind, Script, Tag, World};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Text { text: String },
    Prompt { echo: bool },
    Room {
        name: String,
        description: String,
        exits: Vec<ExitData>,
        contents: Vec<EntityData>,
    },
    Inventory { items: Vec<ItemData> },
    Game { channel: String, data: serde_json::Value },
    Auth { token: String, scopes: Vec<String> },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExitData {
    pub dir: String,
    pub name: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EntityData {
    pub name: String,
    pub kind: String,
    pub ref_id: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub owned: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ItemData {
    pub name: String,
    pub ref_id: String,
}

#[derive(Debug)]
pub enum EngineMessage {
    PlayerConnected {
        session_id: String,
        tx: mpsc::UnboundedSender<ClientMessage>,
    },
    PlayerDisconnected {
        session_id: String,
    },
    PlayerInput {
        session_id: String,
        input: String,
    },
    ApiRequest {
        request: ApiRequest,
        token: Option<String>,
        reply: tokio::sync::oneshot::Sender<ApiResponse>,
    },
    Shutdown,
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ApiRequest {
    ListRooms,
    ListObjects { location: Option<String> },
    CreateRoom { area: String, key: String, title: String, description: Option<String> },
    CreateObject { area: String, key: String, kind: String, title: Option<String>, description: Option<String>, location: Option<String> },
    CreateExit { source: String, direction: String, target: String, aliases: Option<Vec<String>> },
    Examine { ref_id: String },
    SetAttribute { ref_id: String, key: String, value: serde_json::Value },
    SetDescription { ref_id: String, description: String },
    SetTitle { ref_id: String, title: String },
    AddTag { ref_id: String, tag: String },
    RemoveTag { ref_id: String, tag: String },
    DeleteObject { ref_id: String },
    SetProgram { ref_id: String, hook: String, source: String },
    RemoveProgram { ref_id: String, hook: String },
    ListPrograms { ref_id: String },
    ListExits { room_ref: String },
    SaveWorld,
}

#[derive(Debug, serde::Serialize)]
pub struct ApiResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ApiResponse {
    fn success(data: serde_json::Value) -> Self {
        Self { ok: true, data: Some(data), error: None }
    }
    fn error(msg: impl Into<String>) -> Self {
        Self { ok: false, data: None, error: Some(msg.into()) }
    }
    fn ok() -> Self {
        Self { ok: true, data: None, error: None }
    }
}

enum SessionState {
    PromptUsername,
    PromptPassword { username: String },
    CreateUsername,
    CreatePassword { username: String },
    ConfirmPassword { username: String, password: String },
    Playing { actor_ref: String, account_id: String },
}

struct Session {
    tx: mpsc::UnboundedSender<ClientMessage>,
    state: SessionState,
}

struct TokenInfo {
    account_id: String,
    label: String,
    persistent: bool,
    expires_at: Option<u64>,
}

/// The outcome of [`Engine::fire_hook`].
struct HookRun {
    /// Whether the Program's hook function explicitly returned `false`.
    denied: bool,
    /// Whether the Program's own batch sent a message directly to the
    /// actor — used to decide whether a `can_` veto needs a fallback
    /// "you can't do that" message or the script already said something.
    emitted_to_actor: bool,
}

pub struct Engine {
    world: World,
    accounts: AccountStore,
    sessions: HashMap<String, Session>,
    db: Database,
    softcode: SoftcodeRuntime,
    rx: mpsc::UnboundedReceiver<EngineMessage>,
    tick_count: u64,
    tick_secs: u64,
    autosave_secs: u64,
    /// The configured spawn room *key* (e.g. `"town/crossroads"`), used to
    /// re-resolve `spawn_room_ref` after `@reload-world`.
    spawn_room: String,
    /// The dbref the spawn room key currently resolves to.
    spawn_room_ref: String,
    game_dir: Option<String>,
    /// Dungeon theme data loaded from `<game_dir>/themes/*.toml`. See
    /// `crate::theme` and `docs/dungeon-generation-design.md` in the game
    /// repo.
    themes: HashMap<String, crate::theme::Theme>,
    /// Hand-designed map templates loaded from `<game_dir>/maps/*.toml`. See
    /// `crate::map_template`.
    map_templates: HashMap<String, crate::map_template::MapTemplateFile>,
    /// Hooks scheduled to fire in the future via `after(ticks, target, hook)`.
    scheduled_hooks: Vec<ScheduledHook>,
    /// API tokens: hash → info. Both session (ephemeral) and persistent tokens.
    api_tokens: HashMap<String, TokenInfo>,
    /// Content hashes from the last load/reload, used to skip unchanged files.
    file_hashes: HashMap<std::path::PathBuf, u64>,
}

use crate::softcode::ScheduledHook;

impl Engine {
    pub fn new(rx: mpsc::UnboundedReceiver<EngineMessage>, db: Database, config: &Config) -> Self {
        let (mut world, accounts) = if db.has_world_data() {
            let world = db.load_world().expect("Failed to load world from DB");
            let accounts = db.load_accounts().expect("Failed to load accounts from DB");
            tracing::info!(
                objects = world.objects.len(),
                "World loaded from database"
            );
            (world, accounts)
        } else {
            tracing::info!("Fresh world initialized");
            (World::new(), AccountStore::new())
        };

        // Always load/reload game files — new content is created,
        // managed content is updated, non-managed content is untouched.
        let mut key_map: HashMap<String, String> = HashMap::new();
        let mut file_hashes: HashMap<std::path::PathBuf, u64> = HashMap::new();
        let softcode = SoftcodeRuntime::new();
        if let Some(game_dir) = &config.game_dir {
            let game_path = std::path::Path::new(game_dir);
            match crate::loader::load_game_dir(game_path, &mut world, &HashMap::new()) {
                Ok(result) => {
                    key_map = result.key_map;
                    file_hashes = result.file_hashes;
                }
                Err(e) => tracing::error!(error = %e, "Failed to load game content"),
            }
            softcode.load_modules(crate::loader::load_modules(game_path));
        }

        // Resolve the spawn room to a dbref, creating a fallback room if
        // the configured key wasn't found anywhere.
        let spawn_room_ref = match key_map.get(&config.spawn_room) {
            Some(ref_id) => ref_id.clone(),
            None => {
                let ref_id = world.next_dbref();
                let key = config.spawn_room.rsplit('/').next().unwrap_or("spawn");
                let room = GameObject::new(&ref_id, key, Kind::Room)
                    .with_title("Spawn")
                    .with_description("An empty room. Build your world from here.");
                world.add_object(room);
                ref_id
            }
        };

        let themes = config
            .game_dir
            .as_deref()
            .map(|dir| crate::theme::load_themes(std::path::Path::new(dir)))
            .unwrap_or_default();

        let map_templates = config
            .game_dir
            .as_deref()
            .map(|dir| crate::map_template::load_map_templates(std::path::Path::new(dir)))
            .unwrap_or_default();

        let scheduled_hooks = db.load_scheduled_hooks().unwrap_or_else(|e| {
            tracing::error!(error = %e, "Failed to load scheduled hooks");
            Vec::new()
        });

        let mut api_tokens = HashMap::new();
        if let Ok(tokens) = db.load_tokens() {
            for (hash, account_id, label, expires_at) in tokens {
                api_tokens.insert(hash, TokenInfo { account_id, label, persistent: true, expires_at });
            }
        }

        Self {
            world,
            accounts,
            sessions: HashMap::new(),
            db,
            softcode,
            rx,
            tick_count: 0,
            tick_secs: config.tick_secs,
            autosave_secs: config.autosave_secs,
            spawn_room: config.spawn_room.clone(),
            spawn_room_ref,
            game_dir: config.game_dir.clone(),
            themes,
            map_templates,
            scheduled_hooks,
            api_tokens,
            file_hashes,
        }
    }

    pub async fn run(mut self) {
        tracing::info!("Engine started");

        self.fire_lifecycle_hook("on_startup");

        let mut tick_interval =
            tokio::time::interval(std::time::Duration::from_secs(self.tick_secs));
        let mut autosave_interval =
            tokio::time::interval(std::time::Duration::from_secs(self.autosave_secs));

        // Don't fire immediately on start
        tick_interval.tick().await;
        autosave_interval.tick().await;

        loop {
            tokio::select! {
                msg = self.rx.recv() => {
                    match msg {
                        Some(EngineMessage::PlayerConnected { session_id, tx }) => {
                            self.handle_connect(session_id, tx);
                        }
                        Some(EngineMessage::PlayerDisconnected { session_id }) => {
                            self.handle_disconnect(&session_id);
                        }
                        Some(EngineMessage::PlayerInput { session_id, input }) => {
                            self.handle_input(&session_id, &input);
                        }
                        Some(EngineMessage::ApiRequest { request, token, reply }) => {
                            let response = self.handle_api_request(request, token);
                            let _ = reply.send(response);
                        }
                        Some(EngineMessage::Shutdown) | None => {
                            break;
                        }
                    }
                }
                _ = tick_interval.tick() => {
                    self.do_tick();
                }
                _ = autosave_interval.tick() => {
                    tracing::info!("Autosave triggered");
                    self.do_save();
                }
            }
        }

        tracing::info!("Engine shutting down...");
        self.fire_lifecycle_hook("on_shutdown");
        self.do_save();
    }

    fn do_tick(&mut self) {
        self.tick_count += 1;
        let tick = self.tick_count;
        let start = std::time::Instant::now();
        let tick_budget = std::time::Duration::from_millis(500);
        let mut ran = 0u32;

        // -- Per-object on_tick hooks --
        let mut tickable: Vec<(String, u64)> = Vec::new();
        for obj in self.world.objects.values() {
            if let Some(program) = hooks::get_program(obj, "on_tick") {
                if !program.enabled {
                    continue;
                }
                let interval = obj
                    .attrs
                    .get("tick_interval")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1);
                tickable.push((obj.ref_id.clone(), interval));
            }
        }
        tickable.sort_by(|a, b| a.0.cmp(&b.0));

        for (ref_id, interval) in &tickable {
            if start.elapsed() > tick_budget {
                tracing::warn!(tick, ran, "Tick budget exceeded");
                break;
            }
            if *interval == 0 || tick % interval != 0 {
                continue;
            }
            match self.fire_tick_hook(ref_id) {
                Ok(_) => ran += 1,
                Err(e) => {
                    tracing::warn!(hook = "on_tick", target = %ref_id, error = %e, "Tick script error");
                }
            }
        }

        // -- Global scripts --
        let mut script_names: Vec<(String, u64)> = self
            .world
            .scripts
            .values()
            .filter(|s| s.enabled)
            .map(|s| (s.name.clone(), s.interval))
            .collect();
        script_names.sort_by(|a, b| a.0.cmp(&b.0));

        for (name, interval) in &script_names {
            if start.elapsed() > tick_budget {
                tracing::warn!(tick, ran, "Tick budget exceeded (global scripts)");
                break;
            }
            if *interval == 0 || tick % interval != 0 {
                continue;
            }
            match self.fire_global_script(name) {
                Ok(_) => ran += 1,
                Err(e) => {
                    tracing::warn!(script = %name, error = %e, "Global script error");
                }
            }
        }

        // -- Scheduled hooks (from `after()`) --
        let (due, remaining): (Vec<_>, Vec<_>) = std::mem::take(&mut self.scheduled_hooks)
            .into_iter()
            .partition(|s| s.fire_at_tick <= tick);
        self.scheduled_hooks = remaining;
        for scheduled in due {
            if start.elapsed() > tick_budget {
                self.scheduled_hooks.push(scheduled);
                tracing::warn!(tick, ran, "Tick budget exceeded (scheduled hooks)");
                break;
            }
            let args_json = scheduled
                .data
                .as_ref()
                .and_then(|d| serde_json::to_string(d).ok());
            match self.fire_tick_hook_with_args(
                &scheduled.target,
                &scheduled.hook,
                args_json.as_deref(),
            ) {
                Ok(()) => ran += 1,
                Err(e) => {
                    tracing::warn!(
                        hook = %scheduled.hook, target = %scheduled.target,
                        error = %e, "Scheduled hook error"
                    );
                }
            }
        }

        // Clean up expired tokens every 60 ticks (~1 minute)
        if tick % 60 == 0 {
            let before = self.api_tokens.len();
            self.api_tokens.retain(|_, t| !Self::is_token_expired(t));
            if self.api_tokens.len() < before {
                self.save_tokens();
            }
        }

        if ran > 0 {
            tracing::debug!(tick, ran, elapsed_ms = start.elapsed().as_millis(), "Tick complete");
        }
    }

    fn fire_tick_hook(&mut self, this_ref: &str) -> Result<(), String> {
        self.fire_tick_hook_named(this_ref, "on_tick")
    }

    fn fire_global_script(&mut self, name: &str) -> Result<(), String> {
        let script = match self.world.scripts.get(name) {
            Some(s) => s.clone(),
            None => return Ok(()),
        };

        let dbref_counter = Rc::new(Cell::new(self.world.next_id));
        let result = self
            .softcode
            .run_global_script(
                &self.world,
                &script.source,
                &script.entry,
                &script.state,
                Budget::default(),
                Rc::clone(&dbref_counter),
                &self.themes,
                &self.map_templates,
                &self.scheduled_hooks,
            )
            .map_err(|e| e.to_string())?;

        let effects = softcode::apply_batch(&mut self.world, &result.batch)?;
        self.world.next_id = dbref_counter.get();
        self.deliver_effects(&effects, name);

        // Write state back
        if let Some(s) = self.world.scripts.get_mut(name) {
            s.state = result.state;
        }

        Ok(())
    }

    fn fire_lifecycle_hook(&mut self, hook_name: &str) {
        let refs: Vec<String> = self
            .world
            .objects
            .values()
            .filter(|obj| hooks::get_program(obj, hook_name).is_some())
            .map(|obj| obj.ref_id.clone())
            .collect();
        let mut ran = 0u32;
        for ref_id in &refs {
            match self.fire_tick_hook_named(ref_id, hook_name) {
                Ok(()) => ran += 1,
                Err(e) => {
                    tracing::warn!(hook = hook_name, target = %ref_id, error = %e, "Lifecycle hook error");
                }
            }
        }
        if ran > 0 {
            tracing::info!(hook = hook_name, ran, "Lifecycle hooks fired");
        }
    }

    fn fire_on_create(&mut self, ref_id: &str) {
        if self
            .world
            .get(ref_id)
            .and_then(|o| hooks::get_program(o, "on_create"))
            .is_none()
        {
            return;
        }
        if let Err(e) = self.fire_tick_hook_named(ref_id, "on_create") {
            tracing::warn!(hook = "on_create", target = %ref_id, error = %e, "on_create hook error");
        }
    }

    fn fire_tick_hook_with_args(
        &mut self,
        this_ref: &str,
        hook_name: &str,
        args: Option<&str>,
    ) -> Result<(), String> {
        let program = match self
            .world
            .get(this_ref)
            .and_then(|o| hooks::get_program(o, hook_name))
        {
            Some(p) => p.clone(),
            None => return Ok(()),
        };

        let room_ref = self
            .world
            .get(this_ref)
            .and_then(|o| o.location_ref.clone());

        let dbref_counter = Rc::new(Cell::new(self.world.next_id));
        let result = self
            .softcode
            .run_hook(
                &self.world,
                &program,
                this_ref,
                this_ref,
                room_ref.as_deref(),
                args,
                Budget::default(),
                Rc::clone(&dbref_counter),
                &self.themes,
                &self.map_templates,
                &self.scheduled_hooks,
            )
            .map_err(|e| e.to_string())?;

        let effects = softcode::apply_batch(&mut self.world, &result.batch)?;
        self.world.next_id = dbref_counter.get();
        self.deliver_effects(&effects, this_ref);

        if !result.state.is_empty() {
            if let Some(obj) = self.world.get_mut(this_ref) {
                if let Some(prog) = obj.programs.get_mut(hook_name) {
                    prog.state = result.state;
                }
            }
        }

        Ok(())
    }

    fn fire_tick_hook_named(&mut self, this_ref: &str, hook_name: &str) -> Result<(), String> {
        let program = match self
            .world
            .get(this_ref)
            .and_then(|o| hooks::get_program(o, hook_name))
        {
            Some(p) => p.clone(),
            None => return Ok(()),
        };

        let room_ref = self
            .world
            .get(this_ref)
            .and_then(|o| o.location_ref.clone());

        let dbref_counter = Rc::new(Cell::new(self.world.next_id));
        let result = self
            .softcode
            .run_hook(
                &self.world,
                &program,
                this_ref,
                this_ref,
                room_ref.as_deref(),
                None,
                Budget::default(),
                Rc::clone(&dbref_counter),
                &self.themes,
                &self.map_templates,
                &self.scheduled_hooks,
            )
            .map_err(|e| e.to_string())?;

        let effects = softcode::apply_batch(&mut self.world, &result.batch)?;
        self.world.next_id = dbref_counter.get();
        self.deliver_effects(&effects, this_ref);

        if !result.state.is_empty() {
            if let Some(obj) = self.world.get_mut(this_ref) {
                if let Some(prog) = obj.programs.get_mut(hook_name) {
                    prog.state = result.state;
                }
            }
        }

        Ok(())
    }

    fn do_save(&mut self) {
        self.fire_lifecycle_hook("on_save");
        match self.db.save_world(&self.world) {
            Ok(()) => tracing::info!(
                objects = self.world.objects.len(),
                "World saved"
            ),
            Err(e) => tracing::error!(error = %e, "Failed to save world"),
        }
        match self.db.save_accounts(&self.accounts) {
            Ok(()) => tracing::info!("Accounts saved"),
            Err(e) => tracing::error!(error = %e, "Failed to save accounts"),
        }
        match self.db.save_scheduled_hooks(&self.scheduled_hooks) {
            Ok(()) => tracing::debug!(
                count = self.scheduled_hooks.len(),
                "Scheduled hooks saved"
            ),
            Err(e) => tracing::error!(error = %e, "Failed to save scheduled hooks"),
        }
    }

    fn hash_token(token: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        token.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn is_token_expired(info: &TokenInfo) -> bool {
        info.expires_at.map_or(false, |exp| Self::now_secs() > exp)
    }

    fn handle_api_request(&mut self, req: ApiRequest, token: Option<String>) -> ApiResponse {
        let is_read = matches!(
            &req,
            ApiRequest::ListRooms
                | ApiRequest::ListObjects { .. }
                | ApiRequest::Examine { .. }
                | ApiRequest::ListPrograms { .. }
                | ApiRequest::ListExits { .. }
        );

        if !is_read {
            let account_id = token
                .as_deref()
                .map(Self::hash_token)
                .and_then(|hash| self.api_tokens.get(&hash))
                .filter(|info| !Self::is_token_expired(info))
                .map(|info| info.account_id.clone());

            let account_id = match account_id {
                Some(id) => id,
                None => return ApiResponse::error("Authentication required"),
            };

            let has_builder = self
                .accounts
                .get(&account_id)
                .map(|a| a.has_scope(Scope::Builder))
                .unwrap_or(false);

            if !has_builder {
                return ApiResponse::error("Builder scope required");
            }

            if matches!(&req, ApiRequest::SaveWorld) {
                let has_admin = self
                    .accounts
                    .get(&account_id)
                    .map(|a| a.has_scope(Scope::Admin))
                    .unwrap_or(false);
                if !has_admin {
                    return ApiResponse::error("Admin scope required");
                }
            }
        }

        match req {
            ApiRequest::ListRooms => {
                let rooms: Vec<serde_json::Value> = self
                    .world
                    .objects
                    .values()
                    .filter(|o| o.kind == Kind::Room)
                    .map(|o| serde_json::json!({
                        "ref_id": o.ref_id,
                        "key": o.key,
                        "title": o.title,
                        "description": o.description,
                    }))
                    .collect();
                ApiResponse::success(serde_json::json!(rooms))
            }
            ApiRequest::ListObjects { location } => {
                let objs: Vec<serde_json::Value> = self
                    .world
                    .objects
                    .values()
                    .filter(|o| match &location {
                        Some(loc) => o.location_ref.as_deref() == Some(loc.as_str()),
                        None => true,
                    })
                    .filter(|o| o.kind != Kind::Exit)
                    .map(|o| serde_json::json!({
                        "ref_id": o.ref_id,
                        "key": o.key,
                        "kind": o.kind.to_string(),
                        "title": o.title,
                        "location_ref": o.location_ref,
                    }))
                    .collect();
                ApiResponse::success(serde_json::json!(objs))
            }
            ApiRequest::CreateRoom { area: _area, key, title, description } => {
                let ref_id = self.world.next_dbref();
                let mut room = GameObject::new(&ref_id, &key, Kind::Room).with_title(&title);
                if let Some(desc) = description {
                    room.description = desc;
                }
                self.world.add_object(room);
                self.fire_on_create(&ref_id);
                ApiResponse::success(serde_json::json!({ "ref_id": ref_id }))
            }
            ApiRequest::CreateObject { area: _area, key, kind, title, description, location } => {
                let kind_enum = match Kind::parse(&kind) {
                    Some(k) => k,
                    None => return ApiResponse::error(format!("Unknown kind: '{}'", kind)),
                };
                let ref_id = self.world.next_dbref();
                let mut obj = GameObject::new(&ref_id, &key, kind_enum);
                if let Some(t) = title { obj = obj.with_title(t); }
                if let Some(d) = description { obj.description = d; }
                if let Some(loc) = location { obj = obj.with_location(loc); }
                self.world.add_object(obj);
                self.fire_on_create(&ref_id);
                ApiResponse::success(serde_json::json!({ "ref_id": ref_id }))
            }
            ApiRequest::CreateExit { source, direction, target, aliases } => {
                if self.world.get(&source).is_none() {
                    return ApiResponse::error(format!("Source room '{}' not found", source));
                }
                if self.world.get(&target).is_none() {
                    return ApiResponse::error(format!("Target room '{}' not found", target));
                }
                let ref_id = self.world.next_dbref();
                let mut exit = GameObject::new(&ref_id, &direction, Kind::Exit)
                    .with_location(&source)
                    .with_target(&target);
                if let Some(al) = aliases {
                    exit.aliases = al.into_iter().collect();
                }
                self.world.add_object(exit);
                self.fire_on_create(&ref_id);
                ApiResponse::success(serde_json::json!({ "ref_id": ref_id }))
            }
            ApiRequest::Examine { ref_id } => {
                match self.world.get(&ref_id) {
                    Some(obj) => {
                        let tags: Vec<String> = obj.tags.iter().map(|t| t.as_spec()).collect();
                        let programs: Vec<String> = obj.programs.keys().cloned().collect();
                        let locks: &HashMap<String, String> = &obj.locks;
                        ApiResponse::success(serde_json::json!({
                            "ref_id": obj.ref_id,
                            "key": obj.key,
                            "kind": obj.kind.to_string(),
                            "title": obj.title,
                            "description": obj.description,
                            "location_ref": obj.location_ref,
                            "target_ref": obj.target_ref,
                            "attrs": obj.attrs,
                            "tags": tags,
                            "programs": programs,
                            "locks": locks,
                            "aliases": obj.aliases.iter().collect::<Vec<_>>(),
                        }))
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::SetAttribute { ref_id, key, value } => {
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        if value.is_null() {
                            obj.attrs.remove(&key);
                        } else {
                            obj.attrs.insert(key.clone(), value);
                        }
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::SetDescription { ref_id, description } => {
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        obj.description = description;
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::SetTitle { ref_id, title } => {
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        obj.title = Some(title);
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::AddTag { ref_id, tag } => {
                let parsed = match crate::world::Tag::parse(&tag) {
                    Ok(t) => t,
                    Err(e) => return ApiResponse::error(e),
                };
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        obj.tags.insert(parsed);
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::RemoveTag { ref_id, tag } => {
                let parsed = match crate::world::Tag::parse(&tag) {
                    Ok(t) => t,
                    Err(e) => return ApiResponse::error(e),
                };
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        obj.tags.remove(&parsed);
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::DeleteObject { ref_id } => {
                if self.world.get(&ref_id).map(|o| o.kind == Kind::Player).unwrap_or(false) {
                    return ApiResponse::error("Cannot delete player objects");
                }
                if self.world.objects.remove(&ref_id).is_some() {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("No object with ref '{}'", ref_id))
                }
            }
            ApiRequest::SetProgram { ref_id, hook, source } => {
                if let Err(e) = self.softcode.check_syntax(&source) {
                    return ApiResponse::error(format!("Syntax error: {}", e));
                }
                match self.world.get_mut(&ref_id) {
                    Some(obj) => match hooks::set_program(obj, &hook, source) {
                        Ok(()) => ApiResponse::ok(),
                        Err(e) => ApiResponse::error(e),
                    },
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::RemoveProgram { ref_id, hook } => {
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        hooks::remove_program(obj, &hook);
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::ListPrograms { ref_id } => {
                match self.world.get(&ref_id) {
                    Some(obj) => {
                        let programs: Vec<serde_json::Value> = hooks::list_programs(obj)
                            .iter()
                            .map(|p| serde_json::json!({
                                "hook": p.hook,
                                "enabled": p.enabled,
                                "source": p.source,
                            }))
                            .collect();
                        ApiResponse::success(serde_json::json!(programs))
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::ListExits { room_ref } => {
                let exits: Vec<serde_json::Value> = self
                    .world
                    .exits_from(&room_ref)
                    .iter()
                    .map(|e| serde_json::json!({
                        "ref_id": e.ref_id,
                        "direction": e.key,
                        "target_ref": e.target_ref,
                        "aliases": e.aliases.iter().collect::<Vec<_>>(),
                    }))
                    .collect();
                ApiResponse::success(serde_json::json!(exits))
            }
            ApiRequest::SaveWorld => {
                self.do_save();
                ApiResponse::ok()
            }
        }
    }

    fn handle_connect(&mut self, session_id: String, tx: mpsc::UnboundedSender<ClientMessage>) {
        let session = Session {
            tx,
            state: SessionState::PromptUsername,
        };
        self.sessions.insert(session_id.clone(), session);

        self.send(
            &session_id,
            "\r\nWelcome to Hearth.\r\n\r\nEnter your username, or type 'create' for a new account: ",
        );
    }

    fn handle_disconnect(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.remove(session_id) {
            if let SessionState::Playing { actor_ref, .. } = &session.state {
                let name = self
                    .world
                    .get(actor_ref)
                    .map(|o| o.display_name().to_string())
                    .unwrap_or_default();
                // Mark player as offline but keep them in the world.
                // Attrs, tags, inventory, location all persist.
                // Fire on_disconnect on the player and room before marking offline
                let room = self.world.get(actor_ref).and_then(|o| o.location_ref.clone());
                let _ = self.fire_hook(actor_ref, "on_disconnect", actor_ref, room.as_deref(), None);
                if let Some(room_ref) = &room {
                    let _ = self.fire_hook(room_ref, "on_disconnect", actor_ref, Some(room_ref), None);
                }
                self.fire_global_hooks("on_disconnect", actor_ref, room.as_deref(), None);

                if let Some(obj) = self.world.get_mut(actor_ref) {
                    obj.tags.insert(crate::world::Tag {
                        category: "system".to_string(),
                        key: "offline".to_string(),
                    });
                }
                if !name.is_empty() {
                    self.broadcast_to_all(&format!("{} has disconnected.\r\n", name), session_id);
                }
            }
            tracing::info!(session_id, "Player disconnected");
        }

        let session_label = format!("session-{}", &session_id[..8]);
        self.api_tokens
            .retain(|_, t| !(t.label == session_label && !t.persistent));
    }

    fn handle_input(&mut self, session_id: &str, input: &str) {
        let input = input.trim();
        let state = match self.sessions.get(session_id) {
            Some(s) => match &s.state {
                SessionState::PromptUsername => "prompt_username",
                SessionState::PromptPassword { .. } => "prompt_password",
                SessionState::CreateUsername => "create_username",
                SessionState::CreatePassword { .. } => "create_password",
                SessionState::ConfirmPassword { .. } => "confirm_password",
                SessionState::Playing { .. } => "playing",
            },
            None => return,
        };

        match state {
            "prompt_username" => self.handle_login_username(session_id, input),
            "prompt_password" => self.handle_login_password(session_id, input),
            "create_username" => self.handle_create_username(session_id, input),
            "create_password" => self.handle_create_password(session_id, input),
            "confirm_password" => self.handle_confirm_password(session_id, input),
            "playing" => self.handle_game_input(session_id, input),
            _ => {}
        }
    }

    fn handle_token_reconnect(&mut self, session_id: &str, token: &str) {
        let token_hash = Self::hash_token(token);

        let (account_id, expired) = match self.api_tokens.get(&token_hash) {
            Some(info) => (info.account_id.clone(), Self::is_token_expired(info)),
            None => {
                self.send(session_id, "\r\nSession expired. Please log in.\r\nUsername: ");
                return;
            }
        };

        if expired {
            self.api_tokens.remove(&token_hash);
            self.save_tokens();
            self.send(session_id, "\r\nSession expired. Please log in.\r\nUsername: ");
            return;
        }

        let account = match self.accounts.get(&account_id) {
            Some(a) => a,
            None => {
                self.send(session_id, "\r\nAccount not found. Please log in.\r\nUsername: ");
                return;
            }
        };

        let username = account.username.clone();
        let character_ref = account.character_ref.clone().unwrap_or_default();

        // If already logged in from another session (e.g. stale tab), kick the old one
        let stale: Vec<String> = self
            .sessions
            .iter()
            .filter(|(sid, s)| {
                *sid != session_id
                    && matches!(&s.state, SessionState::Playing { account_id: aid, .. } if *aid == account_id)
            })
            .map(|(sid, _)| sid.clone())
            .collect();
        for old_sid in stale {
            self.send(&old_sid, "\r\nReconnected from another session.\r\n");
            self.sessions.remove(&old_sid);
        }

        self.enter_world(session_id, &username, &character_ref, &account_id);
    }

    fn handle_login_username(&mut self, session_id: &str, input: &str) {
        if let Some(token) = input.strip_prefix("reconnect ") {
            self.handle_token_reconnect(session_id, token.trim());
            return;
        }

        if input.eq_ignore_ascii_case("create") {
            if let Some(session) = self.sessions.get_mut(session_id) {
                session.state = SessionState::CreateUsername;
            }
            self.send(session_id, "Choose a username: ");
            return;
        }

        if input.is_empty() {
            self.send(session_id, "Enter your username, or type 'create': ");
            return;
        }

        let username = input.to_string();
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.state = SessionState::PromptPassword { username };
        }
        self.send_echo_off(session_id);
        self.send(session_id, "Password: ");
    }

    fn handle_login_password(&mut self, session_id: &str, input: &str) {
        let username = match self.sessions.get(session_id) {
            Some(Session {
                state: SessionState::PromptPassword { username },
                ..
            }) => username.clone(),
            _ => return,
        };

        self.send_echo_on(session_id);

        match self.accounts.authenticate(&username, input) {
            Ok(account) => {
                let account_id = account.id.clone();
                let character_ref = account.character_ref.clone().unwrap_or_default();

                // Kick any existing sessions for this account
                let stale: Vec<String> = self
                    .sessions
                    .iter()
                    .filter(|(sid, s)| {
                        *sid != session_id
                            && matches!(&s.state, SessionState::Playing { account_id: aid, .. } if *aid == account_id)
                    })
                    .map(|(sid, _)| sid.clone())
                    .collect();
                for old_sid in stale {
                    self.send(&old_sid, "\r\nLogged in from another session.\r\n");
                    self.sessions.remove(&old_sid);
                }

                self.enter_world(session_id, &username, &character_ref, &account_id);
            }
            Err(msg) => {
                self.send(session_id, &format!("{}\r\nUsername: ", msg));
                if let Some(session) = self.sessions.get_mut(session_id) {
                    session.state = SessionState::PromptUsername;
                }
            }
        }
    }

    fn handle_create_username(&mut self, session_id: &str, input: &str) {
        if input.is_empty() {
            self.send(session_id, "Choose a username: ");
            return;
        }

        if self.accounts.get_by_username(input).is_some() {
            self.send(session_id, "That username is already taken.\r\nChoose a username: ");
            return;
        }

        // Validate early
        if input.len() < 3 {
            self.send(session_id, "Username must be at least 3 characters.\r\nChoose a username: ");
            return;
        }
        if input.len() > 20 {
            self.send(session_id, "Username must be 20 characters or fewer.\r\nChoose a username: ");
            return;
        }
        if !input.chars().all(|c| c.is_alphanumeric() || c == '_') {
            self.send(
                session_id,
                "Username may only contain letters, numbers, and underscores.\r\nChoose a username: ",
            );
            return;
        }

        let username = input.to_string();
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.state = SessionState::CreatePassword { username };
        }
        self.send_echo_off(session_id);
        self.send(session_id, "Choose a password (6+ characters): ");
    }

    fn handle_create_password(&mut self, session_id: &str, input: &str) {
        let username = match self.sessions.get(session_id) {
            Some(Session {
                state: SessionState::CreatePassword { username },
                ..
            }) => username.clone(),
            _ => return,
        };

        if input.len() < 6 {
            self.send(session_id, "Password must be at least 6 characters.\r\nChoose a password: ");
            return;
        }

        let password = input.to_string();
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.state = SessionState::ConfirmPassword { username, password };
        }
        self.send(session_id, "Confirm password: ");
    }

    fn handle_confirm_password(&mut self, session_id: &str, input: &str) {
        let (username, password) = match self.sessions.get(session_id) {
            Some(Session {
                state: SessionState::ConfirmPassword { username, password },
                ..
            }) => (username.clone(), password.clone()),
            _ => return,
        };

        if input != password {
            self.send(
                session_id,
                "\r\nPasswords don't match.\r\nChoose a password (6+ characters): ",
            );
            if let Some(session) = self.sessions.get_mut(session_id) {
                session.state = SessionState::CreatePassword { username };
            }
            return;
        }

        self.send_echo_on(session_id);

        match self.accounts.create(&username, &password) {
            Ok(account) => {
                let account_id = account.id.clone();
                let character_ref = account.character_ref.clone().unwrap_or_default();
                self.send(session_id, &format!("\r\nAccount created! Welcome, {}.\r\n", username));
                self.enter_world(session_id, &username, &character_ref, &account_id);
            }
            Err(msg) => {
                self.send(session_id, &format!("{}\r\nChoose a username: ", msg));
                if let Some(session) = self.sessions.get_mut(session_id) {
                    session.state = SessionState::CreateUsername;
                }
            }
        }
    }

    fn enter_world(
        &mut self,
        session_id: &str,
        username: &str,
        character_ref: &str,
        account_id: &str,
    ) {
        let spawn_room_ref = self.spawn_room_ref.clone();

        // A character_ref is only usable if it's non-empty *and* still
        // resolves to a live object — accounts created before this login,
        // or whose player object was somehow lost, get a fresh one below.
        let existing_needs_fix: Option<bool> = if character_ref.is_empty() {
            None
        } else {
            self.world.get(character_ref).map(|obj| {
                obj.location_ref
                    .as_ref()
                    .map(|r| !self.world.objects.contains_key(r.as_str()))
                    .unwrap_or(true)
            })
        };

        let player_ref = if let Some(needs_fix) = existing_needs_fix {
            // Returning player.
            let existing = self.world.get_mut(character_ref).unwrap();
            existing.tags.remove(&crate::world::Tag {
                category: "system".to_string(),
                key: "offline".to_string(),
            });
            if needs_fix {
                existing.location_ref = Some(spawn_room_ref.clone());
            }
            character_ref.to_string()
        } else {
            // First login, or the account's stored dbref no longer resolves
            // to an object — create the character and assign it a dbref.
            let ref_id = self.world.next_dbref();
            let player = GameObject::new(&ref_id, username, Kind::Player)
                .with_title(username)
                .with_description("A traveler.")
                .with_location(&spawn_room_ref);
            self.world.add_object(player);
            self.fire_on_create(&ref_id);
            if let Some(account) = self.accounts.get_mut(account_id) {
                account.character_ref = Some(ref_id.clone());
            }
            self.db.save_accounts(&self.accounts).ok();
            ref_id
        };

        if let Some(session) = self.sessions.get_mut(session_id) {
            session.state = SessionState::Playing {
                actor_ref: player_ref.clone(),
                account_id: account_id.to_string(),
            };
        }

        let scope_msg = if let Some(acct) = self.accounts.get(account_id) {
            let labels = acct.scope_labels();
            if labels.len() > 1 || labels.first() != Some(&"player") {
                format!(" [{}]", labels.join(", "))
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let token = uuid::Uuid::new_v4().to_string();
        let token_hash = Self::hash_token(&token);
        self.api_tokens.insert(
            token_hash,
            TokenInfo {
                account_id: account_id.to_string(),
                label: format!("session-{}", &session_id[..8]),
                persistent: true,
                expires_at: Some(Self::now_secs() + 30 * 24 * 60 * 60),
            },
        );
        self.save_tokens();

        let scopes: Vec<String> = self
            .accounts
            .get(account_id)
            .map(|a| a.scopes.iter().map(|s| s.label().to_string()).collect())
            .unwrap_or_default();

        if let Some(session) = self.sessions.get(session_id) {
            let _ = session.tx.send(ClientMessage::Auth {
                token,
                scopes,
            });
        }

        self.send(
            session_id,
            &format!("\r\nWelcome back, {}.{}\r\n\r\n", username, scope_msg),
        );
        let look_output = self.look_with_visibility(&player_ref);
        self.send(session_id, &look_output);
        self.send_room_data(session_id, &player_ref);

        self.broadcast_to_all(
            &format!("{} has connected.\r\n", username),
            session_id,
        );

        // Fire on_connect on the player and on the room
        let room = self.world.get(&player_ref).and_then(|o| o.location_ref.clone());
        let _ = self.fire_hook(&player_ref, "on_connect", &player_ref, room.as_deref(), None);
        if let Some(room_ref) = &room {
            let _ = self.fire_hook(room_ref, "on_connect", &player_ref, Some(room_ref), None);
        }
        self.fire_global_hooks("on_connect", &player_ref, room.as_deref(), None);
    }

    fn session_account_id(&self, session_id: &str) -> Option<String> {
        match self.sessions.get(session_id) {
            Some(Session {
                state: SessionState::Playing { account_id, .. },
                ..
            }) => Some(account_id.clone()),
            _ => None,
        }
    }

    fn session_has_scope(&self, session_id: &str, scope: Scope) -> bool {
        self.session_account_id(session_id)
            .and_then(|id| self.accounts.get(&id))
            .map(|a| a.has_scope(scope))
            .unwrap_or(false)
    }

    fn handle_game_input(&mut self, session_id: &str, input: &str) {
        if input.is_empty() {
            return;
        }

        let actor_ref = match self.sessions.get(session_id) {
            Some(Session {
                state: SessionState::Playing { actor_ref, .. },
                ..
            }) => actor_ref.clone(),
            _ => return,
        };

        // Check for pending prompt — intercept input before command dispatch
        if let Some(actor) = self.world.get(&actor_ref) {
            let prompt_obj = actor.attrs.get("_prompt_object").and_then(|v| v.as_str()).map(String::from);
            let prompt_hook = actor.attrs.get("_prompt_hook").and_then(|v| v.as_str()).map(String::from);
            if let (Some(obj_ref), Some(hook)) = (prompt_obj, prompt_hook) {
                // Clear the prompt attrs before firing the hook
                if let Some(actor) = self.world.get_mut(&actor_ref) {
                    actor.attrs.remove("_prompt_object");
                    actor.attrs.remove("_prompt_hook");
                }
                let room_ref = self.world.get(&actor_ref).and_then(|o| o.location_ref.clone());
                let output = match self.fire_hook(&obj_ref, &hook, &actor_ref, room_ref.as_deref(), Some(input)) {
                    Ok(_) => String::new(),
                    Err(e) => {
                        tracing::warn!(hook = %hook, target = %obj_ref, error = %e, "prompt callback error");
                        "Something goes wrong.\r\n".to_string()
                    }
                };
                if !output.is_empty() {
                    self.send(session_id, &output);
                }
                return;
            }
        }

        let (cmd, args) = match input.split_once(' ') {
            Some((c, a)) => (c.to_lowercase(), a.trim().to_string()),
            None => (input.to_lowercase(), String::new()),
        };

        let room_before = self.world.get(&actor_ref).and_then(|a| a.location_ref.clone());

        let output = match cmd.as_str() {
            // Player commands
            "look" | "l" => self.cmd_look(&actor_ref, &args),
            "say" | "\"" => {
                let msg = if cmd == "\"" {
                    input[1..].trim().to_string()
                } else {
                    args.clone()
                };
                self.cmd_say(session_id, &actor_ref, &msg)
            }
            "go" => self.cmd_go(&actor_ref, &args),
            "quit" | "q" => {
                self.send(session_id, "Farewell.\r\n");
                self.handle_disconnect(session_id);
                return;
            }
            "inventory" | "inv" | "i" => commands::do_inventory(&self.world, &actor_ref),
            "get" | "take" => self.cmd_get(&actor_ref, &args),
            "put" | "place" => self.cmd_put(&actor_ref, &args),
            "drop" => self.cmd_drop(&actor_ref, &args),
            "use" => self.cmd_use(&actor_ref, &args),
            "examine" | "ex" => commands::do_examine(&self.world, &actor_ref, &args),
            "who" => self.do_who(session_id),
            "@password" => self.cmd_password(session_id, &args),
            "@email" => self.cmd_email(session_id, &args),
            "whisper" => self.cmd_whisper(session_id, &actor_ref, &args),
            "emote" | "pose" | ":" => {
                let msg = if cmd == ":" {
                    input[1..].trim().to_string()
                } else {
                    args.clone()
                };
                self.do_emote(session_id, &actor_ref, &msg)
            }

            // Builder commands (@ prefix)
            "@dig" => self.cmd_dig(session_id, &actor_ref, &args),
            "@open" => self.cmd_open(session_id, &actor_ref, &args),
            "@describe" | "@desc" => self.cmd_describe(session_id, &actor_ref, &args),
            "@create" => self.cmd_create(session_id, &actor_ref, &args),
            "@destroy" => self.cmd_destroy(session_id, &actor_ref, &args),
            "@set" => self.cmd_set(session_id, &actor_ref, &args),
            "@teleport" | "@tel" => self.cmd_teleport(session_id, &actor_ref, &args),
            "@name" => self.cmd_name(session_id, &actor_ref, &args),
            "@program" => self.cmd_program(session_id, &actor_ref, &args),
            "@programs" => self.cmd_programs(session_id, &actor_ref, &args),
            "@rmprogram" => self.cmd_rmprogram(session_id, &actor_ref, &args),
            "@tag" => self.cmd_tag(session_id, &actor_ref, &args),
            "@untag" => self.cmd_untag(session_id, &actor_ref, &args),
            "@script" => self.cmd_script(session_id, &args),
            "@scripts" => self.cmd_scripts(session_id),
            "@rmscript" => self.cmd_rmscript(session_id, &args),
            "@script-interval" => self.cmd_script_interval(session_id, &args),
            "@lock" => self.cmd_lock(session_id, &actor_ref, &args),
            "@unlock" => self.cmd_unlock(session_id, &actor_ref, &args),
            "@locks" => self.cmd_locks(session_id, &actor_ref, &args),

            "@chown" => self.cmd_chown(session_id, &args),

            // Admin commands
            "@grant" => self.cmd_grant(session_id, &args),
            "@revoke" => self.cmd_revoke(session_id, &args),
            "@scopes" => self.cmd_scopes(session_id, &args),
            "@wall" => self.cmd_wall(session_id, &args),
            "@boot" => self.cmd_boot(session_id, &args),
            "@save" => self.cmd_save(session_id),
            "@shutdown" => self.cmd_shutdown(session_id),
            "@reload-world" => self.cmd_reload_world(session_id),
            "@test" => self.cmd_test(session_id, &args),
            "@reload" => self.cmd_reload(session_id, &actor_ref, &args),

            "@token" | "@tokens" => self.cmd_token(session_id, &args),
            "@display" => self.cmd_display(&actor_ref, &args),

            "help" | "?" => {
                let is_builder = self.session_has_scope(session_id, Scope::Builder);
                let is_admin = self.session_has_scope(session_id, Scope::Admin);
                commands::do_help_with_roles(is_builder, is_admin)
            }
            _ => self.dispatch_fallback(&actor_ref, &cmd, &args),
        };

        self.send(session_id, &output);

        let room_after = self.world.get(&actor_ref).and_then(|a| a.location_ref.clone());
        if matches!(cmd.as_str(), "look" | "l") || room_before != room_after {
            self.send_room_data(session_id, &actor_ref);
        }
    }

    // -- Builder commands --

    fn cmd_dig(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @dig <title>
        let title = args.trim();
        if title.is_empty() {
            return "Usage: @dig <title>\r\n".to_string();
        }
        let key = title.to_lowercase().replace(' ', "_");
        let ref_id = self.world.next_dbref();
        let room = GameObject::new(&ref_id, &key, Kind::Room)
            .with_title(title)
            .with_owner(actor_ref);
        self.world.add_object(room);
        self.fire_on_create(&ref_id);
        format!("Room created: {} ({})\r\n", title, ref_id)
    }

    fn cmd_open(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @open <direction> = <target_ref>  (from current room)
        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return "You're nowhere.\r\n".to_string(),
        };
        let (direction, target) = match args.split_once('=') {
            Some((d, t)) => (d.trim().to_string(), t.trim().to_string()),
            None => return "Usage: @open <direction> = <target_ref>\r\n".to_string(),
        };
        if direction.is_empty() || target.is_empty() {
            return "Usage: @open <direction> = <target_ref>\r\n".to_string();
        }
        if self.world.get(&target).is_none() {
            return format!("No room found with ref '{}'.\r\n", target);
        }
        let exit_ref = self.world.next_dbref();
        let exit = GameObject::new(&exit_ref, &direction, Kind::Exit)
            .with_location(&room_ref)
            .with_target(&target)
            .with_owner(actor_ref);
        self.world.add_object(exit);
        self.fire_on_create(&exit_ref);
        format!("Exit '{}' created from {} to {}.\r\n", direction, room_ref, target)
    }

    fn cmd_describe(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @describe [<ref> =] <description>  — defaults to current room
        let (target_ref, desc) = if let Some((ref_part, desc_part)) = args.split_once('=') {
            let ref_part = ref_part.trim();
            if self.world.get(ref_part).is_some() {
                (ref_part.to_string(), desc_part.trim().to_string())
            } else {
                // No valid ref, treat entire args as description for current room
                (
                    self.world
                        .get(actor_ref)
                        .and_then(|a| a.location_ref.clone())
                        .unwrap_or_default(),
                    args.to_string(),
                )
            }
        } else {
            (
                self.world
                    .get(actor_ref)
                    .and_then(|a| a.location_ref.clone())
                    .unwrap_or_default(),
                args.to_string(),
            )
        };

        if desc.is_empty() {
            return "Usage: @describe [<ref> =] <description>\r\n".to_string();
        }

        if let Some(obj) = self.world.get_mut(&target_ref) {
            obj.description = desc;
            format!("Description set on {}.\r\n", target_ref)
        } else {
            "Target not found.\r\n".to_string()
        }
    }

    fn cmd_create(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @create <title>
        let title = args.trim();
        if title.is_empty() {
            return "Usage: @create <title>\r\n".to_string();
        }
        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return "You're nowhere.\r\n".to_string(),
        };
        let key = title.to_lowercase().replace(' ', "_");
        let ref_id = self.world.next_dbref();
        let item = GameObject::new(&ref_id, &key, Kind::Item)
            .with_title(title)
            .with_location(&room_ref)
            .with_owner(actor_ref);
        self.world.add_object(item);
        self.fire_on_create(&ref_id);
        format!("Created {} ({}).\r\n", title, ref_id)
    }

    fn cmd_destroy(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        if args.is_empty() {
            return "Usage: @destroy <ref>\r\n".to_string();
        }
        let target_ref = args.trim();
        if !self.can_modify_object(session_id, actor_ref, target_ref) {
            return "Permission denied (not owner).\r\n".to_string();
        }
        if self.world.get(target_ref).map(|o| o.kind == Kind::Player).unwrap_or(false) {
            return "Cannot destroy player objects.\r\n".to_string();
        }
        if let Some(obj) = self.world.get(target_ref) {
            if obj.kind == Kind::Room {
                let occupants = self.world.objects_in(target_ref);
                if !occupants.is_empty() {
                    return "Cannot destroy a room with objects in it.\r\n".to_string();
                }
            }
            let name = obj.display_name().to_string();
            // If it's a room, also remove exits that source from or target it
            if obj.kind == Kind::Room {
                let exit_refs: Vec<String> = self
                    .world
                    .objects
                    .values()
                    .filter(|o| {
                        o.kind == Kind::Exit
                            && (o.location_ref.as_deref() == Some(target_ref)
                                || o.target_ref.as_deref() == Some(target_ref))
                    })
                    .map(|o| o.ref_id.clone())
                    .collect();
                for r in exit_refs {
                    self.world.objects.remove(&r);
                }
            }
            let _ = self.fire_hook(target_ref, "on_destroy", actor_ref, None, None);
            self.world.objects.remove(target_ref);
            format!("Destroyed {} ({}).\r\n", name, target_ref)
        } else {
            format!("No object with ref '{}'.\r\n", target_ref)
        }
    }

    fn cmd_set(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @set <ref>/<attr> = <value>
        let (path, value) = match args.split_once('=') {
            Some((p, v)) => (p.trim(), v.trim()),
            None => return "Usage: @set <ref>/<attr> = <value>\r\n".to_string(),
        };
        let (target_ref, attr_key) = match path.rsplit_once('/') {
            Some((r, a)) => (r.trim(), a.trim()),
            None => return "Usage: @set <ref>/<attr> = <value>\r\n".to_string(),
        };
        // Allow "here" as shortcut for current room
        let resolved_ref = if target_ref == "here" {
            match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
                Some(r) => r,
                None => return "You're nowhere.\r\n".to_string(),
            }
        } else {
            target_ref.to_string()
        };

        if !self.can_modify_object(session_id, actor_ref, &resolved_ref) {
            return "Permission denied (not owner).\r\n".to_string();
        }

        let json_val: serde_json::Value = match serde_json::from_str(value) {
            Ok(v) => v,
            Err(_) => serde_json::Value::String(value.to_string()),
        };

        if let Some(obj) = self.world.get_mut(&resolved_ref) {
            obj.attrs.insert(attr_key.to_string(), json_val);
            format!("Set {}/{} on {}.\r\n", resolved_ref, attr_key, obj.display_name())
        } else {
            format!("No object with ref '{}'.\r\n", resolved_ref)
        }
    }

    fn cmd_teleport(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        if args.is_empty() {
            return "Usage: @teleport <room_ref>\r\n".to_string();
        }
        let target = args.trim();
        match self.world.get(target) {
            Some(obj) if obj.kind == Kind::Room => {
                commands::move_player(&mut self.world, actor_ref, target)
            }
            Some(_) => "That's not a room.\r\n".to_string(),
            None => format!("No room with ref '{}'.\r\n", target),
        }
    }

    fn cmd_name(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @name [<ref> =] <new name>  — defaults to current room
        let (target_ref, new_name) = if let Some((ref_part, name_part)) = args.split_once('=') {
            let ref_part = ref_part.trim();
            if self.world.get(ref_part).is_some() {
                (ref_part.to_string(), name_part.trim().to_string())
            } else {
                (
                    self.world
                        .get(actor_ref)
                        .and_then(|a| a.location_ref.clone())
                        .unwrap_or_default(),
                    args.to_string(),
                )
            }
        } else {
            (
                self.world
                    .get(actor_ref)
                    .and_then(|a| a.location_ref.clone())
                    .unwrap_or_default(),
                args.to_string(),
            )
        };
        if new_name.is_empty() {
            return "Usage: @name [<ref> =] <new name>\r\n".to_string();
        }
        if let Some(obj) = self.world.get_mut(&target_ref) {
            obj.title = Some(new_name.clone());
            format!("Renamed {} to '{}'.\r\n", target_ref, new_name)
        } else {
            "Target not found.\r\n".to_string()
        }
    }

    /// Resolve a `<ref>/<hook>` path, allowing `here` as a shortcut for the
    /// actor's current room — same convention as `@set`.
    fn resolve_ref_hook_path(&self, actor_ref: &str, path: &str) -> Result<(String, String), String> {
        let (target_ref, hook) = path
            .rsplit_once('/')
            .ok_or_else(|| "Expected <ref>/<hook>".to_string())?;
        let target_ref = target_ref.trim();
        let hook = hook.trim();
        if hook.is_empty() {
            return Err("Expected <ref>/<hook>".to_string());
        }
        let resolved = if target_ref == "here" {
            self.world
                .get(actor_ref)
                .and_then(|a| a.location_ref.clone())
                .ok_or_else(|| "You're nowhere.".to_string())?
        } else {
            target_ref.to_string()
        };
        Ok((resolved, hook.to_string()))
    }

    fn cmd_program(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @program <ref>/<hook> = <luau source>
        let (path, source) = match args.split_once('=') {
            Some((p, s)) => (p.trim(), s.trim()),
            None => return "Usage: @program <ref>/<hook> = <luau source>\r\n".to_string(),
        };
        if source.is_empty() {
            return "Usage: @program <ref>/<hook> = <luau source>\r\n".to_string();
        }
        let (target_ref, hook) = match self.resolve_ref_hook_path(actor_ref, path) {
            Ok(v) => v,
            Err(e) => return format!("{}\r\n", e),
        };
        if self.world.get(&target_ref).is_none() {
            return format!("No object with ref '{}'.\r\n", target_ref);
        }
        if !self.can_modify_object(session_id, actor_ref, &target_ref) {
            return "Permission denied (not owner).\r\n".to_string();
        }
        if !hooks::is_valid_hook_name(&hook) {
            return format!(
                "Unknown hook '{}'. Known hooks: {}, or cmd_<name>.\r\n",
                hook,
                hooks::KNOWN_HOOKS.join(", ")
            );
        }
        if let Err(e) = self.softcode.check_syntax(source) {
            return format!("Syntax error in program: {}\r\n", e);
        }
        let obj = self.world.get_mut(&target_ref).unwrap();
        if let Err(e) = hooks::set_program(obj, &hook, source.to_string()) {
            return format!("{}\r\n", e);
        }
        format!(
            "Program installed: {}/{} ({})\r\n",
            target_ref,
            hook,
            hooks::describe_hook(&hook)
        )
    }

    fn cmd_programs(&self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @programs [<ref>] — defaults to current room
        let target_ref = if args.trim().is_empty() {
            match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
                Some(r) => r,
                None => return "You're nowhere.\r\n".to_string(),
            }
        } else if args.trim() == "here" {
            match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
                Some(r) => r,
                None => return "You're nowhere.\r\n".to_string(),
            }
        } else {
            args.trim().to_string()
        };
        let obj = match self.world.get(&target_ref) {
            Some(o) => o,
            None => return format!("No object with ref '{}'.\r\n", target_ref),
        };
        let programs = hooks::list_programs(obj);
        if programs.is_empty() {
            return format!("{} has no programs.\r\n", target_ref);
        }
        let mut out = format!("Programs on {}:\r\n", target_ref);
        for p in programs {
            let preview: String = p.source.chars().take(40).collect();
            let ellipsis = if p.source.chars().count() > 40 { "..." } else { "" };
            out.push_str(&format!(
                "  {}{}  {}\r\n      {}{}\r\n",
                p.hook,
                if p.enabled { "" } else { " (disabled)" },
                hooks::describe_hook(&p.hook),
                preview.replace('\n', " "),
                ellipsis
            ));
        }
        out
    }

    fn cmd_rmprogram(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @rmprogram <ref>/<hook>
        if args.trim().is_empty() {
            return "Usage: @rmprogram <ref>/<hook>\r\n".to_string();
        }
        let (target_ref, hook) = match self.resolve_ref_hook_path(actor_ref, args.trim()) {
            Ok(v) => v,
            Err(e) => return format!("{}\r\n", e),
        };
        match self.world.get_mut(&target_ref) {
            Some(obj) => {
                if hooks::remove_program(obj, &hook) {
                    format!("Removed program {}/{}.\r\n", target_ref, hook)
                } else {
                    format!("{} has no '{}' program.\r\n", target_ref, hook)
                }
            }
            None => format!("No object with ref '{}'.\r\n", target_ref),
        }
    }

    // -- Softcode hook execution --

    /// Look up an enabled Program at `hook_name` on `this_ref`, run it if
    /// present, and apply/deliver any Intents it queued.
    fn fire_hook(
        &mut self,
        this_ref: &str,
        hook_name: &str,
        actor_ref: &str,
        room_ref: Option<&str>,
        args: Option<&str>,
    ) -> Result<HookRun, String> {
        let program: &ProgramRecord = match self
            .world
            .get(this_ref)
            .and_then(|o| hooks::get_program(o, hook_name))
        {
            Some(p) => p,
            None => {
                return Ok(HookRun {
                    denied: false,
                    emitted_to_actor: false,
                });
            }
        };

        let dbref_counter = Rc::new(Cell::new(self.world.next_id));
        let result = self
            .softcode
            .run_hook(
                &self.world,
                program,
                this_ref,
                actor_ref,
                room_ref,
                args,
                Budget::default(),
                Rc::clone(&dbref_counter),
                &self.themes,
                &self.map_templates,
                &self.scheduled_hooks,
            )
            .map_err(|e| e.to_string())?;

        let denied = result.denied;
        let effects = softcode::apply_batch(&mut self.world, &result.batch)?;
        self.world.next_id = dbref_counter.get();
        let emitted_to_actor = effects
            .iter()
            .any(|e| matches!(e, Effect::ToActor { target, .. } if target == actor_ref));
        self.deliver_effects(&effects, actor_ref);

        Ok(HookRun {
            denied,
            emitted_to_actor,
        })
    }

    fn fire_global_hooks(
        &mut self,
        hook_name: &str,
        actor_ref: &str,
        room_ref: Option<&str>,
        args: Option<&str>,
    ) {
        let global_tag = crate::world::Tag {
            category: "system".into(),
            key: "global".into(),
        };
        let refs: Vec<String> = self
            .world
            .objects
            .values()
            .filter(|o| o.tags.contains(&global_tag))
            .filter(|o| hooks::get_program(o, hook_name).is_some())
            .map(|o| o.ref_id.clone())
            .collect();
        for ref_id in refs {
            let _ = self.fire_hook(&ref_id, hook_name, actor_ref, room_ref, args);
        }
    }

    fn deliver_effects(&mut self, effects: &[Effect], actor_ref: &str) {
        let mut triggers = Vec::new();
        for effect in effects {
            match effect {
                Effect::ToActor { target, message } => self.send_to_actor_ref(target, message),
                Effect::ToRoom {
                    room,
                    message,
                    exclude,
                } => self.send_to_room(room, message, exclude),
                Effect::ScheduleHook { target, hook, ticks, data } => {
                    self.scheduled_hooks.push(ScheduledHook {
                        id: uuid::Uuid::new_v4().to_string(),
                        fire_at_tick: self.tick_count + ticks,
                        target: target.clone(),
                        hook: hook.clone(),
                        data: data.clone(),
                    });
                }
                Effect::CancelScheduledHook { target, hook } => {
                    self.scheduled_hooks.retain(|s| !(s.target == *target && s.hook == *hook));
                }
                Effect::TriggerHook { target, hook, data } => {
                    triggers.push((target.clone(), hook.clone(), data.clone()));
                }
                Effect::EmitNearby { room, x, y, radius, message, exclude } => {
                    let r2 = radius * radius;
                    for (_, session) in &self.sessions {
                        if let SessionState::Playing { actor_ref: ar, .. } = &session.state {
                            if exclude.contains(ar) {
                                continue;
                            }
                            if let Some(actor) = self.world.get(ar) {
                                if actor.location_ref.as_deref() != Some(room) {
                                    continue;
                                }
                                let ax = actor.attrs.get("_x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let ay = actor.attrs.get("_y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let dx = ax - x;
                                let dy = ay - y;
                                if dx * dx + dy * dy <= r2 {
                                    let _ = session.tx.send(ClientMessage::Text { text: message.clone() });
                                }
                            }
                        }
                    }
                }
                Effect::EmitData { target, channel, data } => {
                    self.send_emit_data(target, channel, data);
                }
            }
        }
        for (target, hook, data) in triggers {
            let room_ref = self
                .world
                .get(&target)
                .and_then(|o| o.location_ref.clone());
            if data.is_some() {
                if let Some(obj) = self.world.get_mut(&target) {
                    obj.attrs.insert("_trigger_data".into(), data.clone().unwrap());
                }
            }
            if let Err(e) = self.fire_hook(&target, &hook, actor_ref, room_ref.as_deref(), None) {
                tracing::warn!(hook = %hook, target = %target, error = %e, "Triggered hook error");
            }
            if data.is_some() {
                if let Some(obj) = self.world.get_mut(&target) {
                    obj.attrs.remove("_trigger_data");
                }
            }
        }
    }

    fn send_to_actor_ref(&self, actor_ref: &str, message: &str) {
        for session in self.sessions.values() {
            if let SessionState::Playing { actor_ref: ar, .. } = &session.state
                && ar == actor_ref
            {
                let _ = session.tx.send(ClientMessage::Text { text: format!("{}\r\n", message) });
            }
        }
    }

    fn send_emit_data(&self, actor_ref: &str, channel: &str, data: &serde_json::Value) {
        for session in self.sessions.values() {
            if let SessionState::Playing { actor_ref: ar, .. } = &session.state
                && ar == actor_ref
            {
                let _ = session.tx.send(ClientMessage::Game {
                    channel: channel.to_string(),
                    data: data.clone(),
                });
            }
        }
    }

    fn cmd_token(&mut self, session_id: &str, args: &str) -> String {
        let account_id = match self.session_account_id(session_id) {
            Some(id) => id,
            None => return "Not logged in.\r\n".to_string(),
        };

        let (sub, rest) = match args.split_once(' ') {
            Some((s, r)) => (s.trim(), r.trim()),
            None => (args.trim(), ""),
        };

        match sub {
            "create" => {
                if rest.is_empty() {
                    return "Usage: @token create <label>\r\n".to_string();
                }
                if self
                    .api_tokens
                    .values()
                    .any(|t| t.account_id == account_id && t.label == rest && t.persistent)
                {
                    return format!("Token '{}' already exists. Revoke it first.\r\n", rest);
                }
                let token = uuid::Uuid::new_v4().to_string();
                let token_hash = Self::hash_token(&token);
                self.api_tokens.insert(
                    token_hash,
                    TokenInfo {
                        account_id,
                        label: rest.to_string(),
                        persistent: true,
                        expires_at: None,
                    },
                );
                self.save_tokens();
                format!(
                    "Token created: {}\r\nSave this — it won't be shown again.\r\n",
                    token
                )
            }
            "list" => {
                let tokens: Vec<&TokenInfo> = self
                    .api_tokens
                    .values()
                    .filter(|t| t.account_id == account_id && t.persistent)
                    .collect();
                if tokens.is_empty() {
                    "No API tokens.\r\n".to_string()
                } else {
                    let mut out = "API tokens:\r\n".to_string();
                    for t in tokens {
                        out.push_str(&format!("  {}\r\n", t.label));
                    }
                    out
                }
            }
            "revoke" => {
                if rest.is_empty() {
                    return "Usage: @token revoke <label>\r\n".to_string();
                }
                let before = self.api_tokens.len();
                self.api_tokens.retain(|_, t| {
                    !(t.account_id == account_id && t.label == rest && t.persistent)
                });
                if self.api_tokens.len() < before {
                    self.save_tokens();
                    format!("Token '{}' revoked.\r\n", rest)
                } else {
                    format!("No token named '{}'.\r\n", rest)
                }
            }
            _ => "Usage: @token create|list|revoke\r\n".to_string(),
        }
    }

    fn save_tokens(&self) {
        let tokens: Vec<(String, String, String, Option<u64>)> = self
            .api_tokens
            .iter()
            .filter(|(_, t)| t.persistent)
            .map(|(hash, t)| (hash.clone(), t.account_id.clone(), t.label.clone(), t.expires_at))
            .collect();
        self.db.save_tokens(&tokens).ok();
    }

    fn send_to_room(&self, room_ref: &str, message: &str, exclude: &[String]) {
        for session in self.sessions.values() {
            if let SessionState::Playing { actor_ref, .. } = &session.state {
                if exclude.iter().any(|e| e == actor_ref) {
                    continue;
                }
                if let Some(actor) = self.world.get(actor_ref)
                    && actor.location_ref.as_deref() == Some(room_ref)
                {
                    let _ = session.tx.send(ClientMessage::Text { text: format!("{}\r\n", message) });
                }
            }
        }
    }

    /// Hook-aware `get`: fires `can_get` (which may veto the pickup) then
    /// the built-in pickup, then `on_get`.
    fn cmd_get(&mut self, actor_ref: &str, args: &str) -> String {
        if args.is_empty() {
            return "Get what?\r\n".to_string();
        }

        if let Some((item_name, container_name)) = commands::split_on_preposition(args, "from") {
            return self.cmd_get_from_container(actor_ref, item_name, container_name);
        }

        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return "You're nowhere.\r\n".to_string(),
        };
        let item_ref = match commands::find_item_ref(&self.world, &room_ref, args) {
            Some(r) => r,
            None => return format!("You don't see '{}' here.\r\n", args),
        };

        // Check get lock (DSL)
        let item_locks = self
            .world
            .get(&item_ref)
            .map(|o| o.locks.clone())
            .unwrap_or_default();
        if let Some(false) = self.check_lock("get", &item_locks, actor_ref, Some(&item_ref)) {
            return "You can't pick that up.\r\n".to_string();
        }

        // Check can_get hook (Luau)
        match self.fire_hook(&item_ref, "can_get", actor_ref, Some(&room_ref), None) {
            Ok(run) if run.denied => {
                return if run.emitted_to_actor {
                    String::new()
                } else {
                    "You can't get that.\r\n".to_string()
                };
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(hook = "can_get", target = %item_ref, error = %e, "softcode error");
                return "Something in the world resists, then breaks.\r\n".to_string();
            }
        }

        let name = self.world.get(&item_ref).unwrap().display_name().to_string();
        if let Some(obj) = self.world.get_mut(&item_ref) {
            obj.location_ref = Some(actor_ref.to_string());
        }

        let _ = self.fire_hook(&item_ref, "on_move", actor_ref, Some(&room_ref), None);
        let _ = self.fire_hook(actor_ref, "on_receive", actor_ref, Some(&room_ref), None);

        if let Err(e) = self.fire_hook(&item_ref, "on_get", actor_ref, Some(&room_ref), None) {
            tracing::warn!(hook = "on_get", target = %item_ref, error = %e, "softcode error");
        }

        format!("You pick up {}.\r\n", name)
    }

    fn cmd_get_from_container(
        &mut self,
        actor_ref: &str,
        item_name: &str,
        container_name: &str,
    ) -> String {
        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return "You're nowhere.\r\n".to_string(),
        };

        let container_ref =
            match commands::find_item_in_inventory_or_room(&self.world, actor_ref, &room_ref, container_name) {
                Some(r) => r,
                None => return format!("You don't see '{}' here.\r\n", container_name),
            };

        let item_ref = match commands::find_item_ref(&self.world, &container_ref, item_name) {
            Some(r) => r,
            None => return format!("You don't see '{}' in that.\r\n", item_name),
        };

        let item_locks = self
            .world
            .get(&item_ref)
            .map(|o| o.locks.clone())
            .unwrap_or_default();
        if let Some(false) = self.check_lock("get", &item_locks, actor_ref, Some(&item_ref)) {
            return "You can't take that.\r\n".to_string();
        }

        match self.fire_hook(&item_ref, "can_get", actor_ref, Some(&room_ref), None) {
            Ok(run) if run.denied => {
                return if run.emitted_to_actor {
                    String::new()
                } else {
                    "You can't get that.\r\n".to_string()
                };
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(hook = "can_get", target = %item_ref, error = %e, "softcode error");
                return "Something in the world resists.\r\n".to_string();
            }
        }

        let item_display = self.world.get(&item_ref).unwrap().display_name().to_string();
        let container_display = self.world.get(&container_ref).unwrap().display_name().to_string();
        if let Some(obj) = self.world.get_mut(&item_ref) {
            obj.location_ref = Some(actor_ref.to_string());
        }

        let _ = self.fire_hook(&item_ref, "on_move", actor_ref, Some(&room_ref), None);
        let _ = self.fire_hook(actor_ref, "on_receive", actor_ref, Some(&room_ref), None);
        let _ = self.fire_hook(&item_ref, "on_get", actor_ref, Some(&room_ref), None);

        format!("You get {} from {}.\r\n", item_display, container_display)
    }

    fn cmd_put(&mut self, actor_ref: &str, args: &str) -> String {
        if args.is_empty() {
            return "Usage: put <item> in <container>\r\n".to_string();
        }

        let (item_name, container_name) = match commands::split_on_preposition(args, "in") {
            Some(pair) => pair,
            None => return "Usage: put <item> in <container>\r\n".to_string(),
        };

        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return "You're nowhere.\r\n".to_string(),
        };

        let item_ref =
            match commands::find_item_in_inventory_or_room(&self.world, actor_ref, &room_ref, item_name) {
                Some(r) => r,
                None => return format!("You don't see '{}' here.\r\n", item_name),
            };

        let container_ref =
            match commands::find_item_in_inventory_or_room(&self.world, actor_ref, &room_ref, container_name) {
                Some(r) => r,
                None => return format!("You don't see '{}' here.\r\n", container_name),
            };

        let container_tag = Tag {
            category: "item".into(),
            key: "container".into(),
        };
        match self.world.get(&container_ref) {
            Some(c) if !c.tags.contains(&container_tag) => {
                return format!("{} is not a container.\r\n", c.display_name());
            }
            None => return "Container not found.\r\n".to_string(),
            _ => {}
        }

        if item_ref == container_ref {
            return "You can't put something inside itself.\r\n".to_string();
        }

        // Check capacity
        if let Some(capacity) = self
            .world
            .get(&container_ref)
            .and_then(|c| c.attrs.get("container_capacity"))
            .and_then(|v| v.as_u64())
        {
            let current = self
                .world
                .objects_in(&container_ref)
                .into_iter()
                .filter(|o| o.kind == Kind::Item)
                .count() as u64;
            if current >= capacity {
                return "That container is full.\r\n".to_string();
            }
        }

        // Check put lock on the container
        let container_locks = self
            .world
            .get(&container_ref)
            .map(|o| o.locks.clone())
            .unwrap_or_default();
        if let Some(false) =
            self.check_lock("put", &container_locks, actor_ref, Some(&container_ref))
        {
            return "You can't put things in there.\r\n".to_string();
        }

        // Check can_put hook on the container
        match self.fire_hook(&container_ref, "can_put", actor_ref, Some(&room_ref), None) {
            Ok(run) if run.denied => {
                return if run.emitted_to_actor {
                    String::new()
                } else {
                    "You can't put things in there.\r\n".to_string()
                };
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(hook = "can_put", target = %container_ref, error = %e, "softcode error");
                return "Something in the world resists.\r\n".to_string();
            }
        }

        let item_display = self.world.get(&item_ref).unwrap().display_name().to_string();
        let container_display = self
            .world
            .get(&container_ref)
            .unwrap()
            .display_name()
            .to_string();
        if let Some(obj) = self.world.get_mut(&item_ref) {
            obj.location_ref = Some(container_ref.clone());
        }

        let _ = self.fire_hook(&item_ref, "on_move", actor_ref, Some(&room_ref), None);
        let _ = self.fire_hook(&container_ref, "on_receive", actor_ref, Some(&room_ref), None);
        let _ = self.fire_hook(&container_ref, "on_put", actor_ref, Some(&room_ref), None);

        format!("You put {} in {}.\r\n", item_display, container_display)
    }

    /// When no builtin command matched, look for a `cmd_<name>` Program on
    /// an object in the room or the actor's inventory (room first) before
    /// falling through to exit matching — see ADR 0004.
    fn dispatch_fallback(&mut self, actor_ref: &str, cmd: &str, args: &str) -> String {
        let room_ref = self.world.get(actor_ref).and_then(|a| a.location_ref.clone());

        if let Some(room_ref) = &room_ref {
            let hook_name = format!("cmd_{}", cmd);
            let global_tag = crate::world::Tag {
                category: "system".into(),
                key: "global".into(),
            };
            let target_ref = {
                // Candidates, in resolution order: the room itself,
                // objects in the room, actor's inventory, then global objects.
                let room_itself = self.world.get(room_ref).into_iter();
                let room_objs = self
                    .world
                    .objects_in(room_ref)
                    .into_iter()
                    .filter(|o| o.ref_id != actor_ref);
                let inv_objs = self.world.objects_in(actor_ref).into_iter();
                let global_objs = self
                    .world
                    .objects
                    .values()
                    .filter(|o| o.tags.contains(&global_tag));
                hooks::find_cmd_hook(
                    room_itself.chain(room_objs).chain(inv_objs).chain(global_objs),
                    cmd,
                )
                .map(|(obj, _)| obj.ref_id.clone())
            };

            if let Some(target_ref) = target_ref {
                return match self.fire_hook(
                    &target_ref,
                    &hook_name,
                    actor_ref,
                    Some(room_ref),
                    Some(args),
                ) {
                    Ok(_) => String::new(),
                    Err(e) => {
                        tracing::warn!(hook = %hook_name, target = %target_ref, error = %e, "softcode error");
                        "Something goes wrong.\r\n".to_string()
                    }
                };
            }
        }

        if let Some(exit) = room_ref.as_deref().and_then(|r| self.world.find_exit(r, cmd)) {
            let exit_ref = exit.ref_id.clone();
            let target = exit.target_ref.clone().unwrap_or_default();
            self.do_move(actor_ref, &exit_ref, &target)
        } else {
            "Huh? Type 'help' for commands.\r\n".to_string()
        }
    }

    // -- Admin commands --

    fn cmd_grant(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        // @grant <username> <scope>
        let (username, scope_str) = match args.split_once(' ') {
            Some((u, s)) => (u.trim(), s.trim()),
            None => return "Usage: @grant <username> <scope>\r\nScopes: player, builder, admin\r\n".to_string(),
        };
        let scope = match Scope::parse(scope_str) {
            Some(s) => s,
            None => return format!("Unknown scope '{}'. Valid: player, builder, admin\r\n", scope_str),
        };
        let account_id = match self.accounts.get_id_by_username(username) {
            Some(id) => id,
            None => return format!("No account named '{}'.\r\n", username),
        };
        self.accounts.grant_scope(&account_id, scope);
        format!("Granted '{}' scope to {}.\r\n", scope, username)
    }

    fn cmd_revoke(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        let (username, scope_str) = match args.split_once(' ') {
            Some((u, s)) => (u.trim(), s.trim()),
            None => return "Usage: @revoke <username> <scope>\r\n".to_string(),
        };
        let scope = match Scope::parse(scope_str) {
            Some(s) => s,
            None => return format!("Unknown scope '{}'. Valid: player, builder, admin\r\n", scope_str),
        };
        // Prevent revoking your own admin
        let my_account_id = self.session_account_id(session_id).unwrap_or_default();
        if let Some(target_id) = self.accounts.get_id_by_username(username) {
            if target_id == my_account_id && scope == Scope::Admin {
                return "You can't revoke your own admin scope.\r\n".to_string();
            }
            self.accounts.revoke_scope(&target_id, scope);
            format!("Revoked '{}' scope from {}.\r\n", scope, username)
        } else {
            format!("No account named '{}'.\r\n", username)
        }
    }

    fn cmd_scopes(&self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        let username = args.trim();
        if username.is_empty() {
            // Show own scopes
            if let Some(account_id) = self.session_account_id(session_id) {
                if let Some(acct) = self.accounts.get(&account_id) {
                    return format!("Your scopes: {}\r\n", acct.scope_labels().join(", "));
                }
            }
            return "Could not find your account.\r\n".to_string();
        }
        match self.accounts.get_by_username(username) {
            Some(acct) => format!("{}'s scopes: {}\r\n", acct.username, acct.scope_labels().join(", ")),
            None => format!("No account named '{}'.\r\n", username),
        }
    }

    fn cmd_wall(&self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        if args.is_empty() {
            return "Usage: @wall <message>\r\n".to_string();
        }
        let msg = ClientMessage::Text { text: format!("\r\n[ADMIN] {}\r\n", args) };
        for (_, session) in &self.sessions {
            if matches!(&session.state, SessionState::Playing { .. }) {
                let _ = session.tx.send(msg.clone());
            }
        }
        "Message sent to all players.\r\n".to_string()
    }

    fn cmd_save(&mut self, session_id: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        self.do_save();
        format!(
            "World saved ({} objects).\r\n",
            self.world.objects.len()
        )
    }

    fn cmd_boot(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        let username = args.trim();
        if username.is_empty() {
            return "Usage: @boot <username>\r\n".to_string();
        }
        let target_session = self
            .sessions
            .iter()
            .find(|(sid, s)| {
                if *sid == session_id {
                    return false;
                }
                if let SessionState::Playing { account_id, .. } = &s.state {
                    self.accounts
                        .get(account_id)
                        .map(|a| a.username.eq_ignore_ascii_case(username))
                        .unwrap_or(false)
                } else {
                    false
                }
            })
            .map(|(sid, _)| sid.clone());

        match target_session {
            Some(target_sid) => {
                self.send(&target_sid, "\r\nYou have been disconnected by an admin.\r\n");
                self.handle_disconnect(&target_sid);
                format!("Booted {}.\r\n", username)
            }
            None => format!("'{}' is not online.\r\n", username),
        }
    }

    fn cmd_password(&mut self, session_id: &str, args: &str) -> String {
        // @password <old> <new>
        let account_id = match self.session_account_id(session_id) {
            Some(id) => id,
            None => return "You're not logged in.\r\n".to_string(),
        };
        let (old_pw, new_pw) = match args.split_once(' ') {
            Some((o, n)) => (o.trim(), n.trim()),
            None => return "Usage: @password <old_password> <new_password>\r\n".to_string(),
        };
        if old_pw.is_empty() || new_pw.is_empty() {
            return "Usage: @password <old_password> <new_password>\r\n".to_string();
        }
        match self.accounts.change_password(&account_id, old_pw, new_pw) {
            Ok(()) => "Password changed.\r\n".to_string(),
            Err(e) => format!("{}\r\n", e),
        }
    }

    fn cmd_email(&mut self, session_id: &str, args: &str) -> String {
        let account_id = match self.session_account_id(session_id) {
            Some(id) => id,
            None => return "You're not logged in.\r\n".to_string(),
        };
        let email = args.trim();
        if email.is_empty() {
            // Show current email
            let current = self
                .accounts
                .get(&account_id)
                .and_then(|a| a.email.as_deref())
                .unwrap_or("not set");
            return format!("Email: {}\r\nUsage: @email <address> or @email clear\r\n", current);
        }
        if email.eq_ignore_ascii_case("clear") {
            match self.accounts.set_email(&account_id, None) {
                Ok(()) => return "Email cleared.\r\n".to_string(),
                Err(e) => return format!("{}\r\n", e),
            }
        }
        match self.accounts.set_email(&account_id, Some(email.to_string())) {
            Ok(()) => format!("Email set to {}.\r\n", email),
            Err(e) => format!("{}\r\n", e),
        }
    }

    fn do_who(&self, _session_id: &str) -> String {
        let mut out = "\r\nOnline players:\r\n".to_string();
        let mut count = 0;
        for session in self.sessions.values() {
            if let SessionState::Playing { actor_ref, .. } = &session.state {
                if let Some(actor) = self.world.get(actor_ref) {
                    out.push_str(&format!("  {}\r\n", actor.display_name()));
                    count += 1;
                }
            }
        }
        out.push_str(&format!("{} player(s) online.\r\n", count));
        out
    }

    fn broadcast_to_all(&self, msg: &str, exclude_session: &str) {
        for (sid, session) in &self.sessions {
            if sid == exclude_session {
                continue;
            }
            if matches!(&session.state, SessionState::Playing { .. }) {
                let _ = session.tx.send(ClientMessage::Text { text: msg.to_string() });
            }
        }
    }

    fn send(&self, session_id: &str, msg: &str) {
        if let Some(session) = self.sessions.get(session_id) {
            let _ = session.tx.send(ClientMessage::Text { text: msg.to_string() });
        }
    }

    fn send_echo_off(&self, session_id: &str) {
        if let Some(session) = self.sessions.get(session_id) {
            let _ = session.tx.send(ClientMessage::Prompt { echo: false });
        }
    }

    fn send_echo_on(&self, session_id: &str) {
        if let Some(session) = self.sessions.get(session_id) {
            let _ = session.tx.send(ClientMessage::Prompt { echo: true });
        }
    }

    fn send_room_data(&mut self, session_id: &str, actor_ref: &str) {
        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return,
        };

        let hidden_tag = Tag { category: "system".into(), key: "hidden".into() };
        let offline_tag = Tag { category: "system".into(), key: "offline".into() };

        // Resolve can_see for hidden objects (same logic as look_with_visibility)
        let hidden_candidates: Vec<(String, bool)> = self
            .world
            .objects_in(&room_ref)
            .iter()
            .filter(|o| o.tags.contains(&hidden_tag) && o.ref_id != actor_ref)
            .map(|o| (o.ref_id.clone(), o.programs.contains_key("can_see")))
            .collect();
        let mut hidden_refs: Vec<String> = Vec::new();
        for (ref_id, has_can_see) in hidden_candidates {
            if !has_can_see {
                hidden_refs.push(ref_id);
                continue;
            }
            match self.fire_hook(&ref_id, "can_see", actor_ref, Some(&room_ref), None) {
                Ok(run) if !run.denied => {}
                _ => hidden_refs.push(ref_id),
            }
        }

        let room = match self.world.get(&room_ref) {
            Some(r) => r,
            None => return,
        };

        let exits: Vec<ExitData> = self.world.exits_from(&room_ref).iter().map(|e| {
            let dest_name = e.target_ref.as_ref()
                .and_then(|t| self.world.get(t))
                .map(|r| r.display_name().to_string())
                .unwrap_or_default();
            ExitData { dir: e.key.clone(), name: dest_name }
        }).collect();

        let contents: Vec<EntityData> = self.world.objects_in(&room_ref).into_iter()
            .filter(|o| {
                o.ref_id != actor_ref
                    && !o.tags.contains(&offline_tag)
                    && !hidden_refs.contains(&o.ref_id)
            })
            .map(|o| EntityData {
                name: o.display_name().to_string(),
                kind: format!("{}", o.kind),
                ref_id: o.ref_id.clone(),
                owned: o.owner_ref.as_deref() == Some(actor_ref),
            })
            .collect();

        if let Some(session) = self.sessions.get(session_id) {
            let _ = session.tx.send(ClientMessage::Room {
                name: room.display_name().to_string(),
                description: room.description.clone(),
                exits,
                contents,
            });
        }
    }

    fn cmd_whisper(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        // whisper <player> <message>
        let (target_name, message) = match args.split_once(' ') {
            Some((t, m)) => (t.trim(), m.trim()),
            None => return "Usage: whisper <player> <message>\r\n".to_string(),
        };
        if message.is_empty() {
            return "Usage: whisper <player> <message>\r\n".to_string();
        }

        let actor = match self.world.get(actor_ref) {
            Some(a) => a,
            None => return "You don't exist.\r\n".to_string(),
        };
        let room_ref = match &actor.location_ref {
            Some(r) => r.clone(),
            None => return "You're nowhere.\r\n".to_string(),
        };
        let sender_name = actor.display_name().to_string();

        let lower_target = target_name.to_lowercase();
        let target_session = self.sessions.iter().find(|(sid, s)| {
            *sid != session_id
                && if let SessionState::Playing { actor_ref: ar, .. } = &s.state {
                    self.world
                        .get(ar)
                        .map(|o| {
                            o.location_ref.as_deref() == Some(&room_ref)
                                && (o.key.to_lowercase() == lower_target
                                    || o.display_name().to_lowercase() == lower_target)
                        })
                        .unwrap_or(false)
                } else {
                    false
                }
        });

        match target_session {
            Some((_, session)) => {
                let _ = session
                    .tx
                    .send(ClientMessage::Text { text: format!("{} whispers, \"{}\"\r\n", sender_name, message) });

                // Fire on_whisper on the room
                let _ = self.fire_hook(&room_ref, "on_whisper", actor_ref, Some(&room_ref), None);

                format!("You whisper to {}, \"{}\"\r\n", target_name, message)
            }
            None => format!("You don't see '{}' here.\r\n", target_name),
        }
    }

    fn do_emote(&mut self, speaker_session: &str, actor_ref: &str, message: &str) -> String {
        if message.is_empty() {
            return "Emote what?\r\n".to_string();
        }

        let actor = match self.world.get(actor_ref) {
            Some(a) => a,
            None => return "You don't exist.\r\n".to_string(),
        };

        let room_ref = match &actor.location_ref {
            Some(r) => r.clone(),
            None => return "You're nowhere.\r\n".to_string(),
        };

        let name = actor.display_name().to_string();

        let others_msg = ClientMessage::Text { text: format!("{} {}\r\n", name, message) };
        for (sid, session) in &self.sessions {
            if sid == speaker_session {
                continue;
            }
            if let SessionState::Playing { actor_ref: ar, .. } = &session.state {
                if let Some(other_actor) = self.world.get(ar) {
                    if other_actor.location_ref.as_deref() == Some(&room_ref) {
                        let _ = session.tx.send(others_msg.clone());
                    }
                }
            }
        }

        let _ = self.fire_hook(&room_ref, "on_emote", actor_ref, Some(&room_ref), None);

        format!("{} {}\r\n", name, message)
    }

    // -- Global script commands --

    fn cmd_script(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @script <name> = <luau source>
        let (name, source) = match args.split_once('=') {
            Some((n, s)) => (n.trim(), s.trim()),
            None => return "Usage: @script <name> = <luau source>\r\n".to_string(),
        };
        if name.is_empty() || source.is_empty() {
            return "Usage: @script <name> = <luau source>\r\n".to_string();
        }
        if let Err(e) = self.softcode.check_syntax(source) {
            return format!("Syntax error: {}\r\n", e);
        }
        let is_new = !self.world.scripts.contains_key(name);
        let script = Script::new(name, source);
        self.world.scripts.insert(name.to_string(), script);
        if is_new {
            format!("Script '{}' created (ticks every 1s).\r\n", name)
        } else {
            format!("Script '{}' updated.\r\n", name)
        }
    }

    fn cmd_scripts(&self, session_id: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        if self.world.scripts.is_empty() {
            return "No global scripts.\r\n".to_string();
        }
        let mut out = "\r\nGlobal scripts:\r\n".to_string();
        let mut names: Vec<&String> = self.world.scripts.keys().collect();
        names.sort();
        for name in names {
            let script = &self.world.scripts[name];
            let status = if script.enabled { "on" } else { "off" };
            let state_keys = script.state.len();
            out.push_str(&format!(
                "  {} [{}] interval={}  state_keys={}\r\n",
                name, status, script.interval, state_keys
            ));
        }
        out
    }

    fn cmd_rmscript(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let name = args.trim();
        if name.is_empty() {
            return "Usage: @rmscript <name>\r\n".to_string();
        }
        if self.world.scripts.remove(name).is_some() {
            format!("Script '{}' removed.\r\n", name)
        } else {
            format!("No script named '{}'.\r\n", name)
        }
    }

    fn cmd_script_interval(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @script-interval <name> = <ticks>
        let (name, interval_str) = match args.split_once('=') {
            Some((n, i)) => (n.trim(), i.trim()),
            None => return "Usage: @script-interval <name> = <ticks>\r\n".to_string(),
        };
        let interval: u64 = match interval_str.parse() {
            Ok(n) => n,
            Err(_) => return "Interval must be a positive integer.\r\n".to_string(),
        };
        if interval == 0 {
            return "Interval must be at least 1.\r\n".to_string();
        }
        match self.world.scripts.get_mut(name) {
            Some(script) => {
                script.interval = interval;
                format!("Script '{}' interval set to {} tick(s).\r\n", name, interval)
            }
            None => format!("No script named '{}'.\r\n", name),
        }
    }

    fn cmd_tag(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @tag <ref> = <tag_spec>
        let (target_ref, tag_spec) = match args.split_once('=') {
            Some((r, t)) => (r.trim(), t.trim()),
            None => return "Usage: @tag <ref> = <tag_spec>\r\n".to_string(),
        };
        if target_ref.is_empty() || tag_spec.is_empty() {
            return "Usage: @tag <ref> = <tag_spec>\r\n".to_string();
        }
        let resolved = if target_ref == "here" {
            match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
                Some(r) => r,
                None => return "You're nowhere.\r\n".to_string(),
            }
        } else {
            target_ref.to_string()
        };
        let tag = match crate::world::Tag::parse(tag_spec) {
            Ok(t) => t,
            Err(e) => return format!("{}\r\n", e),
        };
        if let Some(obj) = self.world.get_mut(&resolved) {
            obj.tags.insert(tag.clone());
            format!("Tag '{}' added to {}.\r\n", tag.as_spec(), resolved)
        } else {
            format!("No object with ref '{}'.\r\n", resolved)
        }
    }

    fn cmd_untag(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let (target_ref, tag_spec) = match args.split_once('=') {
            Some((r, t)) => (r.trim(), t.trim()),
            None => return "Usage: @untag <ref> = <tag_spec>\r\n".to_string(),
        };
        if target_ref.is_empty() || tag_spec.is_empty() {
            return "Usage: @untag <ref> = <tag_spec>\r\n".to_string();
        }
        let resolved = if target_ref == "here" {
            match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
                Some(r) => r,
                None => return "You're nowhere.\r\n".to_string(),
            }
        } else {
            target_ref.to_string()
        };
        let tag = match crate::world::Tag::parse(tag_spec) {
            Ok(t) => t,
            Err(e) => return format!("{}\r\n", e),
        };
        if let Some(obj) = self.world.get_mut(&resolved) {
            if obj.tags.remove(&tag) {
                format!("Tag '{}' removed from {}.\r\n", tag.as_spec(), resolved)
            } else {
                format!("Object {} doesn't have tag '{}'.\r\n", resolved, tag.as_spec())
            }
        } else {
            format!("No object with ref '{}'.\r\n", resolved)
        }
    }

    fn cmd_shutdown(&mut self, session_id: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        self.broadcast_to_all("\r\n[ADMIN] Server is shutting down...\r\n", "");
        self.do_save();
        self.rx.close();
        "Shutting down.\r\n".to_string()
    }

    fn cmd_reload_world(&mut self, session_id: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        let game_dir = match &self.game_dir {
            Some(g) => g.clone(),
            None => return "No game_dir configured.\r\n".to_string(),
        };
        let game_path = std::path::Path::new(&game_dir);
        self.softcode.invalidate_cache();
        self.softcode
            .load_modules(crate::loader::load_modules(game_path));
        match crate::loader::load_game_dir(game_path, &mut self.world, &self.file_hashes) {
            Ok(result) => {
                if let Some(ref_id) = result.key_map.get(&self.spawn_room) {
                    self.spawn_room_ref = ref_id.clone();
                }
                self.file_hashes = result.file_hashes;
                self.fire_lifecycle_hook("on_reload");

                let mut msg = String::new();
                if result.changed_files.is_empty() && result.created == 0 {
                    msg.push_str("No changes detected.\r\n");
                } else {
                    use std::fmt::Write;
                    let _ = write!(
                        msg,
                        "Reload: {} created, {} updated, {} unchanged\r\n",
                        result.created, result.updated, result.skipped
                    );
                    for file in &result.changed_files {
                        let _ = write!(msg, "  modified: {}\r\n", file);
                    }
                }
                msg.push_str("Script cache cleared.\r\n");
                msg
            }
            Err(e) => format!("Reload error: {}\r\n", e),
        }
    }

    fn cmd_reload(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @reload <ref>/<hook>  — re-validate and re-enable a program
        let (target_ref, hook) = match self.resolve_ref_hook_path(actor_ref, args.trim()) {
            Ok(v) => v,
            Err(e) => return format!("{}\r\n", e),
        };
        let obj = match self.world.get_mut(&target_ref) {
            Some(o) => o,
            None => return format!("No object with ref '{}'.\r\n", target_ref),
        };
        let program = match obj.programs.get_mut(&hook) {
            Some(p) => p,
            None => return format!("{} has no '{}' program.\r\n", target_ref, hook),
        };
        if let Err(e) = self.softcode.check_syntax(&program.source) {
            return format!("Syntax error: {}\r\n", e);
        }
        program.enabled = true;
        format!("Program {}/{} reloaded and enabled.\r\n", target_ref, hook)
    }

    fn cmd_display(&mut self, actor_ref: &str, args: &str) -> String {
        let arg = args.trim().to_lowercase();
        let obj = match self.world.get_mut(actor_ref) {
            Some(o) => o,
            None => return "No player object.\r\n".to_string(),
        };
        match arg.as_str() {
            "accessible" | "a11y" | "screenreader" => {
                obj.attrs.insert(
                    "_display_mode".into(),
                    serde_json::json!("accessible"),
                );
                "Display mode set to accessible. Formatting simplified for screen readers.\r\n"
                    .to_string()
            }
            "visual" | "default" | "" => {
                obj.attrs.insert(
                    "_display_mode".into(),
                    serde_json::json!("visual"),
                );
                "Display mode set to visual.\r\n".to_string()
            }
            _ => {
                let current = obj
                    .attrs
                    .get("_display_mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("visual");
                format!(
                    "Current mode: {}\r\nUsage: @display visual | @display accessible\r\n",
                    current
                )
            }
        }
    }

    // -- Hook-aware gameplay commands --

    fn account_scopes_for_actor(&self, actor_ref: &str) -> Vec<String> {
        for session in self.sessions.values() {
            if let SessionState::Playing {
                actor_ref: ar,
                account_id,
                ..
            } = &session.state
            {
                if ar == actor_ref {
                    if let Some(acct) = self.accounts.get(account_id) {
                        return acct.scope_labels().iter().map(|s| s.to_string()).collect();
                    }
                }
            }
        }
        vec![]
    }

    fn check_lock(
        &self,
        lock_type: &str,
        locks: &HashMap<String, String>,
        actor_ref: &str,
        target_ref: Option<&str>,
    ) -> Option<bool> {
        let expr_str = locks.get(lock_type)?;
        let actor = self.world.get(actor_ref)?;
        let scopes = self.account_scopes_for_actor(actor_ref);
        let ctx = AccessContext {
            actor,
            world: &self.world,
            account_scopes: &scopes,
            target: target_ref.and_then(|r| self.world.get(r)),
        };
        match locks::evaluate_lock_string(expr_str, &ctx) {
            Ok(result) => Some(result),
            Err(e) => {
                tracing::warn!(lock_type, error = %e, "Lock evaluation error");
                Some(false)
            }
        }
    }

    fn look_with_visibility(&mut self, actor_ref: &str) -> String {
        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return "You're floating in the void.\r\n".to_string(),
        };

        // If the room has an on_look hook, let it handle everything
        let room_has_on_look = self
            .world
            .get(&room_ref)
            .is_some_and(|o| o.programs.contains_key("on_look"));
        if room_has_on_look {
            let _ = self.fire_hook(&room_ref, "on_look", actor_ref, Some(&room_ref), None);
            return String::new();
        }

        let hidden_tag = crate::world::Tag {
            category: "system".into(),
            key: "hidden".into(),
        };

        let mut hidden_refs = Vec::new();
        let candidates: Vec<(String, bool)> = self
            .world
            .objects_in(&room_ref)
            .iter()
            .filter(|o| o.tags.contains(&hidden_tag) && o.ref_id != actor_ref)
            .map(|o| (o.ref_id.clone(), o.programs.contains_key("can_see")))
            .collect();

        for (ref_id, has_can_see) in candidates {
            if !has_can_see {
                hidden_refs.push(ref_id);
                continue;
            }
            match self.fire_hook(&ref_id, "can_see", actor_ref, Some(&room_ref), None) {
                Ok(run) if !run.denied => {}
                _ => hidden_refs.push(ref_id),
            }
        }

        commands::format_look(&self.world, &room_ref, actor_ref, &hidden_refs)
    }

    fn cmd_look(&mut self, actor_ref: &str, args: &str) -> String {
        if !args.is_empty() {
            let room_ref = self
                .world
                .get(actor_ref)
                .and_then(|a| a.location_ref.clone())
                .unwrap_or_default();
            let target_name = args.to_lowercase();
            let target_ref = self
                .world
                .objects_in(&room_ref)
                .into_iter()
                .chain(self.world.exits_from(&room_ref))
                .chain(self.world.objects_in(actor_ref))
                .find(|o| {
                    o.key.to_lowercase().contains(&target_name)
                        || o.display_name().to_lowercase().contains(&target_name)
                })
                .map(|o| o.ref_id.clone());

            if let Some(ref target_ref) = target_ref {
                let locks = self
                    .world
                    .get(target_ref)
                    .map(|o| o.locks.clone())
                    .unwrap_or_default();
                if let Some(false) = self.check_lock("look", &locks, actor_ref, Some(target_ref)) {
                    return "You can't see that.\r\n".to_string();
                }
                if let Ok(run) = self.fire_hook(target_ref, "can_look", actor_ref, Some(&room_ref), None) {
                    if run.denied {
                        return if run.emitted_to_actor {
                            String::new()
                        } else {
                            "You can't see that.\r\n".to_string()
                        };
                    }
                }
                let _ = self.fire_hook(target_ref, "on_look", actor_ref, Some(&room_ref), None);
            }
            return commands::do_examine(&self.world, actor_ref, args);
        }

        self.look_with_visibility(actor_ref)
    }

    fn cmd_go(&mut self, actor_ref: &str, args: &str) -> String {
        if args.is_empty() {
            return "Go where?\r\n".to_string();
        }
        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return "You're nowhere.\r\n".to_string(),
        };
        let (exit_ref, target_ref) = match self.world.find_exit(&room_ref, args) {
            Some(e) => (
                e.ref_id.clone(),
                e.target_ref.clone().unwrap_or_default(),
            ),
            None => return format!("You can't go '{}'.\r\n", args),
        };
        self.do_move(actor_ref, &exit_ref, &target_ref)
    }

    fn do_move(&mut self, actor_ref: &str, exit_ref: &str, target_ref: &str) -> String {
        if let Some(msg) = self.world.get(actor_ref)
            .and_then(|a| a.attrs.get("movement_blocked"))
        {
            let reason = msg.as_str().unwrap_or("You can't move right now.");
            return format!("{}\r\n", reason);
        }

        let old_room = self
            .world
            .get(actor_ref)
            .and_then(|a| a.location_ref.clone())
            .unwrap_or_default();

        // Check traverse lock on the exit (DSL)
        let exit_locks = self
            .world
            .get(exit_ref)
            .map(|o| o.locks.clone())
            .unwrap_or_default();
        if let Some(false) = self.check_lock("traverse", &exit_locks, actor_ref, Some(exit_ref)) {
            return "You can't go that way.\r\n".to_string();
        }

        // Check can_traverse hook on the exit's target room (Luau)
        match self.fire_hook(target_ref, "can_traverse", actor_ref, Some(&old_room), None) {
            Ok(run) if run.denied => {
                return if run.emitted_to_actor {
                    String::new()
                } else {
                    "You can't go that way.\r\n".to_string()
                };
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(hook = "can_traverse", error = %e, "softcode error");
            }
        }

        // Check enter lock on the destination room (DSL)
        let room_locks = self
            .world
            .get(target_ref)
            .map(|o| o.locks.clone())
            .unwrap_or_default();
        if let Some(false) = self.check_lock("enter", &room_locks, actor_ref, Some(target_ref)) {
            return "You can't go there.\r\n".to_string();
        }

        // Check can_enter hook on the destination room (Luau)
        match self.fire_hook(target_ref, "can_enter", actor_ref, Some(&old_room), None) {
            Ok(run) if run.denied => {
                return if run.emitted_to_actor {
                    String::new()
                } else {
                    "You can't go there.\r\n".to_string()
                };
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(hook = "can_enter", error = %e, "softcode error");
            }
        }

        // Fire on_leave on old room
        let _ = self.fire_hook(&old_room, "on_leave", actor_ref, Some(&old_room), None);
        self.fire_global_hooks("on_leave", actor_ref, Some(&old_room), None);

        // Move the player
        if self.world.get(target_ref).is_none() {
            return "That destination doesn't exist.\r\n".to_string();
        }
        if let Some(actor) = self.world.get_mut(actor_ref) {
            actor.location_ref = Some(target_ref.to_string());
        }

        // Move followers (troupe members tagged troupe:<actor_ref>)
        let troupe_tag = crate::world::Tag {
            category: "troupe".to_string(),
            key: actor_ref.to_string(),
        };
        let followers: Vec<String> = self.world.objects.values()
            .filter(|o| o.tags.contains(&troupe_tag))
            .map(|o| o.ref_id.clone())
            .collect();
        for ref_id in followers {
            if let Some(obj) = self.world.get_mut(&ref_id) {
                obj.location_ref = Some(target_ref.to_string());
            }
        }

        // Fire on_move on the actor
        let _ = self.fire_hook(actor_ref, "on_move", actor_ref, Some(target_ref), None);

        // Fire on_enter on new room
        let _ = self.fire_hook(target_ref, "on_enter", actor_ref, Some(target_ref), None);
        self.fire_global_hooks("on_enter", actor_ref, Some(target_ref), None);

        self.look_with_visibility(actor_ref)
    }

    fn cmd_drop(&mut self, actor_ref: &str, args: &str) -> String {
        if args.is_empty() {
            return "Drop what?\r\n".to_string();
        }
        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return "You're nowhere.\r\n".to_string(),
        };
        let target_name = args.to_lowercase();
        let item_ref = self
            .world
            .objects_in(actor_ref)
            .into_iter()
            .find(|o| {
                o.kind == Kind::Item
                    && (o.key.to_lowercase().contains(&target_name)
                        || o.display_name().to_lowercase().contains(&target_name))
            })
            .map(|o| o.ref_id.clone());

        let item_ref = match item_ref {
            Some(r) => r,
            None => return format!("You aren't carrying '{}'.\r\n", args),
        };

        // Check drop lock (DSL)
        let item_locks = self
            .world
            .get(&item_ref)
            .map(|o| o.locks.clone())
            .unwrap_or_default();
        if let Some(false) = self.check_lock("drop", &item_locks, actor_ref, Some(&item_ref)) {
            return "You can't drop that.\r\n".to_string();
        }

        // Check can_drop hook (Luau)
        match self.fire_hook(&item_ref, "can_drop", actor_ref, Some(&room_ref), None) {
            Ok(run) if run.denied => {
                return if run.emitted_to_actor {
                    String::new()
                } else {
                    "You can't drop that.\r\n".to_string()
                };
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(hook = "can_drop", target = %item_ref, error = %e, "softcode error");
            }
        }

        let name = self.world.get(&item_ref).unwrap().display_name().to_string();
        if let Some(obj) = self.world.get_mut(&item_ref) {
            obj.location_ref = Some(room_ref.clone());
        }

        let _ = self.fire_hook(&item_ref, "on_move", actor_ref, Some(&room_ref), None);

        // Fire on_drop
        let _ = self.fire_hook(&item_ref, "on_drop", actor_ref, Some(&room_ref), None);

        format!("You drop {}.\r\n", name)
    }

    fn cmd_use(&mut self, actor_ref: &str, args: &str) -> String {
        if args.is_empty() {
            return "Use what?\r\n".to_string();
        }
        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return "You're nowhere.\r\n".to_string(),
        };
        let target_name = args.to_lowercase();
        // Search room contents then inventory
        let target_ref = self
            .world
            .objects_in(&room_ref)
            .into_iter()
            .chain(self.world.objects_in(actor_ref))
            .find(|o| {
                o.key.to_lowercase().contains(&target_name)
                    || o.display_name().to_lowercase().contains(&target_name)
            })
            .map(|o| o.ref_id.clone());

        let target_ref = match target_ref {
            Some(r) => r,
            None => return format!("You don't see '{}' here.\r\n", args),
        };

        // Check use lock (DSL)
        let target_locks = self
            .world
            .get(&target_ref)
            .map(|o| o.locks.clone())
            .unwrap_or_default();
        if let Some(false) = self.check_lock("use", &target_locks, actor_ref, Some(&target_ref)) {
            return "You can't use that.\r\n".to_string();
        }

        // Check can_use hook (Luau)
        match self.fire_hook(&target_ref, "can_use", actor_ref, Some(&room_ref), None) {
            Ok(run) if run.denied => {
                return if run.emitted_to_actor {
                    String::new()
                } else {
                    "You can't use that.\r\n".to_string()
                };
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(hook = "can_use", target = %target_ref, error = %e, "softcode error");
            }
        }

        // Fire on_use hook — if no hook exists, there's nothing to "use"
        match self.fire_hook(&target_ref, "on_use", actor_ref, Some(&room_ref), None) {
            Ok(run) if run.emitted_to_actor => String::new(),
            Ok(_) => {
                let name = self
                    .world
                    .get(&target_ref)
                    .map(|o| o.display_name().to_string())
                    .unwrap_or_default();
                format!("You use {}. Nothing happens.\r\n", name)
            }
            Err(e) => {
                tracing::warn!(hook = "on_use", target = %target_ref, error = %e, "softcode error");
                "Something goes wrong.\r\n".to_string()
            }
        }
    }

    fn cmd_say(&mut self, speaker_session: &str, actor_ref: &str, message: &str) -> String {
        if message.is_empty() {
            return "Say what?\r\n".to_string();
        }

        let actor = match self.world.get(actor_ref) {
            Some(a) => a,
            None => return "You don't exist.\r\n".to_string(),
        };
        let room_ref = match &actor.location_ref {
            Some(r) => r.clone(),
            None => return "You're nowhere.\r\n".to_string(),
        };

        // Check can_say on the room
        match self.fire_hook(&room_ref, "can_say", actor_ref, Some(&room_ref), None) {
            Ok(run) if run.denied => {
                return if run.emitted_to_actor {
                    String::new()
                } else {
                    "You can't speak here.\r\n".to_string()
                };
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(hook = "can_say", target = %room_ref, error = %e, "softcode error");
            }
        }

        // If the room has on_say, let the hook handle all distribution
        let room_has_on_say = self
            .world
            .get(&room_ref)
            .is_some_and(|o| o.programs.contains_key("on_say"));
        if room_has_on_say {
            // Store the message as an attr so the hook can read it
            if let Some(room_obj) = self.world.get_mut(&room_ref) {
                room_obj.attrs.insert("_say_message".into(), serde_json::json!(message));
            }
            let _ = self.fire_hook(&room_ref, "on_say", actor_ref, Some(&room_ref), None);
            if let Some(room_obj) = self.world.get_mut(&room_ref) {
                room_obj.attrs.remove("_say_message");
            }
            return String::new();
        }

        let speaker_name = self
            .world
            .get(actor_ref)
            .map(|a| a.display_name().to_string())
            .unwrap_or_default();

        let others_msg = ClientMessage::Text { text: format!("{} says, \"{}\"\r\n", speaker_name, message) };
        for (sid, session) in &self.sessions {
            if sid == speaker_session {
                continue;
            }
            if let SessionState::Playing { actor_ref: ar, .. } = &session.state {
                if let Some(other_actor) = self.world.get(ar) {
                    if other_actor.location_ref.as_deref() == Some(&room_ref) {
                        let _ = session.tx.send(others_msg.clone());
                    }
                }
            }
        }

        format!("You say, \"{}\"\r\n", message)
    }

    // -- Lock builder commands --

    fn cmd_lock(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @lock <ref>/<type> = <expression>
        let (path, expr_str) = match args.split_once('=') {
            Some((p, e)) => (p.trim(), e.trim()),
            None => return "Usage: @lock <ref>/<type> = <expression>\r\nTypes: traverse, get, drop, enter, use, look, teleport, put\r\nPredicates: perm(), has_tag(), has_attr(), in_inventory(), is_kind(), is_owner(), time_between()\r\n".to_string(),
        };
        let (target_ref, lock_type) = match path.rsplit_once('/') {
            Some((r, t)) => (r.trim(), t.trim()),
            None => return "Usage: @lock <ref>/<type> = <expression>\r\n".to_string(),
        };
        let resolved = if target_ref == "here" {
            match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
                Some(r) => r,
                None => return "You're nowhere.\r\n".to_string(),
            }
        } else {
            target_ref.to_string()
        };

        if !self.can_modify_object(session_id, actor_ref, &resolved) {
            return "Permission denied (not owner).\r\n".to_string();
        }

        // Validate the expression parses
        if let Err(e) = locks::parse(expr_str) {
            return format!("Invalid lock expression: {}\r\n", e);
        }

        if let Some(obj) = self.world.get_mut(&resolved) {
            obj.locks.insert(lock_type.to_string(), expr_str.to_string());
            format!("Lock '{}' set on {}.\r\n", lock_type, resolved)
        } else {
            format!("No object with ref '{}'.\r\n", resolved)
        }
    }

    fn cmd_unlock(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @unlock <ref>/<type>
        let (target_ref, lock_type) = match args.trim().rsplit_once('/') {
            Some((r, t)) => (r.trim(), t.trim()),
            None => return "Usage: @unlock <ref>/<type>\r\n".to_string(),
        };
        let resolved = if target_ref == "here" {
            match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
                Some(r) => r,
                None => return "You're nowhere.\r\n".to_string(),
            }
        } else {
            target_ref.to_string()
        };

        if let Some(obj) = self.world.get_mut(&resolved) {
            if obj.locks.remove(lock_type).is_some() {
                format!("Lock '{}' removed from {}.\r\n", lock_type, resolved)
            } else {
                format!("{} has no '{}' lock.\r\n", resolved, lock_type)
            }
        } else {
            format!("No object or exit with ref '{}'.\r\n", resolved)
        }
    }

    fn cmd_locks(&self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let target_ref = if args.trim().is_empty() {
            self.world
                .get(actor_ref)
                .and_then(|a| a.location_ref.clone())
                .unwrap_or_default()
        } else {
            args.trim().to_string()
        };

        match self.world.get(&target_ref) {
            Some(obj) => {
                if obj.locks.is_empty() {
                    format!("{} has no locks.\r\n", target_ref)
                } else {
                    let mut out = format!("Locks on {}:\r\n", target_ref);
                    for (lock_type, expr) in &obj.locks {
                        out.push_str(&format!("  {}: {}\r\n", lock_type, expr));
                    }
                    out
                }
            }
            None => format!("No object with ref '{}'.\r\n", target_ref),
        }
    }

    fn cmd_test(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }

        let game_dir = match &self.game_dir {
            Some(g) => g.clone(),
            None => return "No game_dir configured.\r\n".to_string(),
        };

        let game_path = std::path::Path::new(&game_dir);
        let test_files = if args.trim().is_empty() {
            crate::loader::discover_test_files(game_path)
        } else {
            let path = game_path.join(args.trim());
            match std::fs::read_to_string(&path) {
                Ok(source) => {
                    let is_lib = path.starts_with(game_path.join("lib"));
                    vec![crate::loader::TestFile {
                        path: path.clone(),
                        relative: args.trim().to_string(),
                        source,
                        is_lib,
                    }]
                }
                Err(e) => return format!("Cannot read '{}': {}\r\n", args.trim(), e),
            }
        };

        if test_files.is_empty() {
            return "No .test.luau files found.\r\n".to_string();
        }

        let mut out = String::new();
        let mut total_passed = 0usize;
        let mut total_failed = 0usize;

        for tf in &test_files {
            let world = if tf.is_lib {
                None
            } else {
                Some(self.world.clone())
            };

            let result = self.softcode.run_tests(
                &tf.source,
                &tf.relative,
                world.as_ref(),
                softcode::Budget::default(),
            );

            match result {
                Ok(file_result) => {
                    out.push_str(&format!("\r\n{}:\r\n", tf.relative));
                    for tr in &file_result.tests {
                        if tr.passed {
                            total_passed += 1;
                            out.push_str(&format!("  PASS {}\r\n", tr.name));
                        } else {
                            total_failed += 1;
                            out.push_str(&format!(
                                "  FAIL {} -- {}\r\n",
                                tr.name,
                                tr.error.as_deref().unwrap_or("?")
                            ));
                        }
                    }
                }
                Err(e) => {
                    total_failed += 1;
                    out.push_str(&format!("\r\n{}: ERROR -- {}\r\n", tf.relative, e));
                }
            }
        }

        out.push_str(&format!(
            "\r\n{} passed, {} failed\r\n",
            total_passed, total_failed
        ));
        out
    }

    fn can_modify_object(&self, session_id: &str, actor_ref: &str, target_ref: &str) -> bool {
        if self.session_has_scope(session_id, Scope::Admin) {
            return true;
        }
        self.world
            .get(target_ref)
            .and_then(|o| o.owner_ref.as_ref())
            .is_some_and(|owner| owner == actor_ref)
    }

    fn cmd_chown(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied (admin only).\r\n".to_string();
        }
        // @chown <ref> = <player_ref>
        let (target_ref, new_owner) = match args.split_once('=') {
            Some((r, o)) => (r.trim(), o.trim()),
            None => return "Usage: @chown <ref> = <player_ref>\r\n".to_string(),
        };
        if target_ref.is_empty() || new_owner.is_empty() {
            return "Usage: @chown <ref> = <player_ref>\r\n".to_string();
        }
        if self.world.get(new_owner).is_none() {
            return format!("No object with ref '{}'.\r\n", new_owner);
        }
        if let Some(obj) = self.world.get_mut(target_ref) {
            obj.owner_ref = Some(new_owner.to_string());
            format!("Owner of {} set to {}.\r\n", target_ref, new_owner)
        } else {
            format!("No object with ref '{}'.\r\n", target_ref)
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const TEST_TOKEN: &str = "test-api-token";

    async fn test_engine() -> (mpsc::UnboundedSender<EngineMessage>, tokio::task::JoinHandle<()>) {
        let db = crate::db::Database::open(Path::new(":memory:")).unwrap();
        let config = Config::default();
        let (tx, rx) = mpsc::unbounded_channel();
        let mut engine = Engine::new(rx, db, &config);
        let account = engine.accounts.create("test_builder", "password123").unwrap();
        let account_id = account.id.clone();
        engine.accounts.grant_scope(&account_id, Scope::Builder);
        engine.accounts.grant_scope(&account_id, Scope::Admin);
        let token_hash = Engine::hash_token(TEST_TOKEN);
        engine.api_tokens.insert(token_hash, TokenInfo {
            account_id,
            label: "test".to_string(),
            persistent: false,
            expires_at: None,
        });
        let handle = tokio::spawn(engine.run());
        (tx, handle)
    }

    async fn api_call(tx: &mpsc::UnboundedSender<EngineMessage>, req: ApiRequest) -> ApiResponse {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        tx.send(EngineMessage::ApiRequest {
            request: req,
            token: Some(TEST_TOKEN.to_string()),
            reply: reply_tx,
        }).unwrap();
        reply_rx.await.unwrap()
    }

    #[tokio::test]
    async fn api_list_rooms() {
        let (tx, handle) = test_engine().await;
        let resp = api_call(&tx, ApiRequest::ListRooms).await;
        assert!(resp.ok);
        let rooms = resp.data.unwrap();
        let rooms = rooms.as_array().unwrap();
        assert_eq!(rooms.len(), 1); // starter world has just the spawn room
        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_create_and_examine_room() {
        let (tx, handle) = test_engine().await;

        let resp = api_call(&tx, ApiRequest::CreateRoom {
            area: "test".into(),
            key: "cave".into(),
            title: "A Cave".into(),
            description: Some("Dark and damp.".into()),
        }).await;
        assert!(resp.ok);
        let ref_id = resp.data.unwrap()["ref_id"].as_str().unwrap().to_string();

        let resp = api_call(&tx, ApiRequest::Examine { ref_id: ref_id.clone() }).await;
        assert!(resp.ok);
        let data = resp.data.unwrap();
        assert_eq!(data["title"], "A Cave");
        assert_eq!(data["description"], "Dark and damp.");
        assert_eq!(data["kind"], "room");

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_create_object_with_attrs_and_tags() {
        let (tx, handle) = test_engine().await;

        let resp = api_call(&tx, ApiRequest::CreateObject {
            area: "test".into(),
            key: "gem".into(),
            kind: "item".into(),
            title: Some("a ruby".into()),
            description: None,
            location: Some("#1".into()),
        }).await;
        assert!(resp.ok);
        let ref_id = resp.data.unwrap()["ref_id"].as_str().unwrap().to_string();

        let resp = api_call(&tx, ApiRequest::SetAttribute {
            ref_id: ref_id.clone(),
            key: "value".into(),
            value: serde_json::json!(500),
        }).await;
        assert!(resp.ok);

        let resp = api_call(&tx, ApiRequest::AddTag {
            ref_id: ref_id.clone(),
            tag: "loot:treasure".into(),
        }).await;
        assert!(resp.ok);

        let resp = api_call(&tx, ApiRequest::Examine { ref_id: ref_id.clone() }).await;
        let data = resp.data.unwrap();
        assert_eq!(data["attrs"]["value"], 500);
        assert!(data["tags"].as_array().unwrap().contains(&serde_json::json!("loot:treasure")));

        drop(tx);
        let _ = handle.await;
    }

    async fn create_test_item(tx: &mpsc::UnboundedSender<EngineMessage>) -> String {
        let resp = api_call(tx, ApiRequest::CreateObject {
            area: "test".into(),
            key: "sword".into(),
            kind: "item".into(),
            title: Some("a test sword".into()),
            description: None,
            location: Some("#1".into()),
        }).await;
        assert!(resp.ok);
        resp.data.unwrap()["ref_id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn api_create_exit() {
        let (tx, handle) = test_engine().await;

        // Create a second room for the exit to target
        let resp = api_call(&tx, ApiRequest::CreateRoom {
            area: "test".into(),
            key: "cellar".into(),
            title: "The Cellar".into(),
            description: None,
        }).await;
        assert!(resp.ok);
        let cellar_ref = resp.data.unwrap()["ref_id"].as_str().unwrap().to_string();

        let resp = api_call(&tx, ApiRequest::CreateExit {
            source: "#1".into(),
            direction: "down".into(),
            target: cellar_ref,
            aliases: Some(vec!["d".into()]),
        }).await;
        assert!(resp.ok);

        let resp = api_call(&tx, ApiRequest::ListExits {
            room_ref: "#1".into(),
        }).await;
        assert!(resp.ok);
        let exits = resp.data.unwrap();
        let exits = exits.as_array().unwrap();
        let down = exits.iter().find(|e| e["direction"] == "down");
        assert!(down.is_some());

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_set_and_remove_program() {
        let (tx, handle) = test_engine().await;
        let item_ref = create_test_item(&tx).await;

        let resp = api_call(&tx, ApiRequest::SetProgram {
            ref_id: item_ref.clone(),
            hook: "on_get".into(),
            source: "function on_get(this, actor, room) emit(actor, \"Hum!\") end".into(),
        }).await;
        assert!(resp.ok);

        let resp = api_call(&tx, ApiRequest::ListPrograms {
            ref_id: item_ref.clone(),
        }).await;
        assert!(resp.ok);
        let programs = resp.data.unwrap().as_array().unwrap().clone();
        assert_eq!(programs.len(), 1);
        assert_eq!(programs[0]["hook"], "on_get");

        let resp = api_call(&tx, ApiRequest::RemoveProgram {
            ref_id: item_ref.clone(),
            hook: "on_get".into(),
        }).await;
        assert!(resp.ok);

        let resp = api_call(&tx, ApiRequest::ListPrograms {
            ref_id: item_ref.clone(),
        }).await;
        let programs = resp.data.unwrap().as_array().unwrap().clone();
        assert_eq!(programs.len(), 0);

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_delete_object() {
        let (tx, handle) = test_engine().await;
        let item_ref = create_test_item(&tx).await;

        let resp = api_call(&tx, ApiRequest::DeleteObject {
            ref_id: item_ref.clone(),
        }).await;
        assert!(resp.ok);

        let resp = api_call(&tx, ApiRequest::Examine {
            ref_id: item_ref.clone(),
        }).await;
        assert!(!resp.ok);

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_syntax_error_rejected() {
        let (tx, handle) = test_engine().await;
        let item_ref = create_test_item(&tx).await;

        let resp = api_call(&tx, ApiRequest::SetProgram {
            ref_id: item_ref,
            hook: "on_get".into(),
            source: "function on_get(this actor room) end".into(),
        }).await;
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("Syntax error"));

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_save_world() {
        let (tx, handle) = test_engine().await;
        let resp = api_call(&tx, ApiRequest::SaveWorld).await;
        assert!(resp.ok);
        drop(tx);
        let _ = handle.await;
    }
}
