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
use crate::world::{GameObject, Kind, Tag, World};

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
    Commands { commands: Vec<String> },
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
    /// Move an object into a new location (e.g. an NPC/item into a room) — the
    /// builder's place/relocate action. Builder-gated.
    SetLocation { ref_id: String, location: String },
    /// Edit an existing exit in place: change its direction (`key`) and/or
    /// retarget it. Validates the target exists and the object is an exit.
    /// Builder-gated.
    UpdateExit {
        ref_id: String,
        #[serde(default)]
        direction: Option<String>,
        #[serde(default)]
        target: Option<String>,
    },
    /// Replace an object's aliases (name-match aliases for items/npcs, or an
    /// exit's alt directions). Builder-gated.
    SetAliases { ref_id: String, aliases: Vec<String> },
    AddTag { ref_id: String, tag: String },
    RemoveTag { ref_id: String, tag: String },
    DeleteObject { ref_id: String },
    SetProgram { ref_id: String, hook: String, source: String },
    RemoveProgram { ref_id: String, hook: String },
    ListPrograms { ref_id: String },
    /// Compile-check Luau without running or saving it — the linter backend
    /// for the code editor. Wraps `Softcode::check_syntax`; returns
    /// `{valid, error?}`. Builder-gated (authoring surface), like the other
    /// program actions.
    CheckProgram { source: String },
    /// Every object that carries programs, with its hook names — the code
    /// editor's explorer feed, in one call instead of per-object. Builder-gated.
    ListProgramsAll,
    /// A whole-world health check for the builder's Problems panel: dangling
    /// exits, unreachable / exitless / description-less rooms, and every hook
    /// program that fails to compile. One server-side pass (the engine holds
    /// the whole world) instead of many client round-trips. Builder-gated.
    WorldCheck,
    /// A flat, richer object listing for the builder's table view: every
    /// non-exit object (rooms, npcs, items, players) with its area, location
    /// and tags — the columns the table renders. `ListObjects` deliberately
    /// stays lean and public (see `is_read`), so this Builder-gated variant
    /// carries the tag/area detail the same way `ListWorldSlice` does.
    /// Optional `kind` ("room"/"npc"/"item"/"player") and `area` narrow it.
    ListObjectsFull {
        kind: Option<String>,
        area: Option<String>,
        limit: Option<u32>,
    },
    /// The engine's hook vocabulary, for the builder's hook autocomplete: the
    /// fixed `KNOWN_HOOKS` the engine fires on its own (each with a one-line
    /// description) plus the open-ended prefixes (`on_`, `cmd_`, `lib_`) that
    /// `is_valid_hook_name` also accepts. Pure static schema — no world data —
    /// so it rides in the public `is_read` set, and the client never has to
    /// hard-code (and drift from) the list.
    ListHooks,
    /// Version history for one `(ref_id, hook)` — the REST counterpart of
    /// `@program/history`. Requires auth like any other write-tier action
    /// (see `is_read` below): it serves full historical Program source,
    /// same as `ListPrograms`.
    ProgramHistory { ref_id: String, hook: String },
    /// Restore version `version` (1-based, oldest first) of `(ref_id,
    /// hook)` as a *new* version — the REST counterpart of
    /// `@program/restore`. Non-destructive, same as the telnet command.
    ProgramRestore { ref_id: String, hook: String, version: usize },
    ListExits { room_ref: String },
    /// Distinct room areas (derived from each room's `FILE_KEY_ATTR` prefix)
    /// with counts — the picker feed for the room builder's scope bar.
    /// Builder-gated (NOT in `is_read`): like `Examine`, it surfaces
    /// authored structure the play surface doesn't.
    ListAreas,
    /// A bounded slice of the world graph for the room builder, so it never
    /// loads every room at once. Filter by `area` (FILE_KEY prefix), `tag`
    /// (`category:key`, or `category:*`), and/or a `near` room with a BFS
    /// `depth` (default 2). `limit` caps the room set (default 400) and sets
    /// `truncated`. Returns rooms (with area+tags), the exits leaving those
    /// rooms (targets may fall outside the slice), and those outside targets
    /// as `boundary` stubs. Builder-gated, same rationale as `ListAreas`.
    ListWorldSlice {
        #[serde(default)]
        area: Option<String>,
        #[serde(default)]
        tag: Option<String>,
        #[serde(default)]
        near: Option<String>,
        #[serde(default)]
        depth: Option<u32>,
        #[serde(default)]
        limit: Option<u32>,
    },
    SaveWorld,
    /// Run a one-shot Luau script against the live world — the REST
    /// counterpart of `@eval`. Gated on `Scope::Admin` explicitly below,
    /// not just the generic `Builder` gate every other write action gets:
    /// this is arbitrary code with the full write API, so a token that can
    /// eval owns the server.
    Eval { source: String },
    /// Install a TOML+`.luau` bundle into the DB — the REST counterpart of
    /// `@import` — see docs/plans/program-authoring.md Stage 4. `path` is
    /// resolved on the server's own filesystem (relative to the server
    /// process's working directory, or absolute), the same way `game_dir`
    /// itself is — this is not a file-upload endpoint. Gated on
    /// `Scope::Admin` explicitly below, same tier as `Eval`: an import can
    /// install arbitrary Luau just as freely as `Eval` can run it.
    Import { path: String, #[serde(default)] dry_run: bool },
    /// Emit DB-owned (`FILE_KEY_ATTR`-carrying) content back to files under
    /// `path` on the server's filesystem — the REST counterpart of
    /// `@export`. Same admin gate as `Import`.
    Export { path: String },
    InkCompile { source: String },
    InkSave { ref_id: String, source: String },
    InkLoad { ref_id: String },
    /// List the DB-owned map names plus the shared `terrain.toml` source —
    /// what the map builder loads to populate its picker and palette.
    /// Builder-gated (below), NOT in the unauthenticated `is_read` set.
    ListMaps,
    /// The TOML source of one map, by name.
    GetMap { name: String },
    /// The shared terrain-palette TOML source.
    GetTerrain,
    /// Write a map's TOML into the DB and rebuild the live templates — the
    /// builder's save. Admin-gated below: like `Import`/`Eval`, it installs
    /// content that becomes rooms. `name` is validated to a bare filename.
    PutMap { name: String, toml: String },
    /// Write the shared terrain-palette TOML into the DB and rebuild. Admin.
    PutTerrain { toml: String },
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
    SelectCharacter { account_id: String },
    CreateCharacterName { account_id: String },
    Playing { actor_ref: String, account_id: String, puppet_ref: Option<String> },
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

/// Format a Unix timestamp (seconds) as `YYYY-MM-DD HH:MM:SS UTC` for
/// `@program/history` output, without pulling in a date/time dependency for
/// one display line. `civil_from_days` is Howard Hinnant's `civil_from_days`
/// algorithm (public domain; http://howardhinnant.github.io/date_algorithms.html),
/// good over the entire proleptic Gregorian calendar.
pub(crate) fn format_epoch_secs(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC", y, m, d, hour, min, sec)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
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
    /// DB-owned map + terrain TOML sources, keyed by game-dir-relative path
    /// (`"terrain.toml"`, `"maps/<name>.toml"`). The builder reads and writes
    /// these; `map_templates` above is the parsed runtime form rebuilt from
    /// them (see `rebuild_map_templates`).
    file_sources: HashMap<String, String>,
    /// Hooks scheduled to fire in the future via `after(ticks, target, hook)`.
    scheduled_hooks: Vec<ScheduledHook>,
    /// API tokens: hash → info. Both session (ephemeral) and persistent tokens.
    api_tokens: HashMap<String, TokenInfo>,
    /// Content hashes from the last load/reload, used to skip unchanged files.
    file_hashes: HashMap<std::path::PathBuf, String>,
    max_characters: u8,
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
        //
        // Seeded from the world we just loaded rather than starting empty:
        // with `load_world_files = false` the loader never runs, and an empty
        // map fails every `<area>/<key>` lookup — starting with `spawn_room`
        // below, which then builds a duplicate empty room and drops players
        // into it. The file load overwrites these entries with its own when
        // it runs, so this is a floor rather than a competing source.
        let mut key_map: HashMap<String, String> = crate::loader::key_map_from_world(&world);
        // Restored from the database rather than starting empty. `load_game_dir`
        // skips files whose content hash is unchanged — which is why
        // `@reload-world` is cheap — but boot had nowhere to get a previous set
        // from, so every restart re-read and reinstalled the whole game
        // directory. See docs/plans/program-authoring.md, "how this went
        // wrong", symptom 2.
        let mut file_hashes: HashMap<std::path::PathBuf, String> =
            db.load_file_hashes().unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Could not read file hashes; treating every file as changed");
                HashMap::new()
            });
        let softcode = SoftcodeRuntime::new();
        if let Some(game_dir) = &config.game_dir {
            let game_path = std::path::Path::new(game_dir);
            // Boot-time world-content loading, gated by `load_world_files`
            // (default `true` — unchanged behaviour). See
            // docs/plans/program-authoring.md Stage 4: this is the switch a
            // maintainer flips, deliberately not flipped here, once
            // `@import`/`hearth import` cover installing content instead.
            // Lib modules and ink files below are unaffected — they are
            // never persisted to the DB regardless of this setting.
            if config.load_world_files {
                match crate::loader::load_game_dir(game_path, &mut world, &file_hashes) {
                    Ok(result) => {
                        key_map = result.key_map;
                        file_hashes = result.file_hashes;
                        if let Err(e) = db.save_file_hashes(&file_hashes) {
                            tracing::warn!(error = %e, "Failed to persist file hashes");
                        }
                        // File installs are an authoring path too (see
                        // docs/plans/program-authoring.md Stage 3) — record a
                        // version for each. Content-addressed dedupe means an
                        // unchanged file across restarts costs a lookup, not a
                        // new row. No `actor_ref` exists this early (there is
                        // no session), so the author is `None`, same as ticks
                        // and timers.
                        for (obj_ref, hook, source) in &result.installed_programs {
                            if let Err(e) = db.record_program_version(obj_ref, hook, source, None) {
                                tracing::warn!(error = %e, obj_ref, hook, "Failed to record program version for file install");
                            }
                        }
                    }
                    Err(e) => tracing::error!(error = %e, "Failed to load game content"),
                }
            } else {
                tracing::info!("load_world_files = false — skipping boot-time world content load");
            }
            softcode.load_modules(crate::loader::load_modules(game_path));
            softcode.ink_runtime().borrow_mut().set_ink_dir(game_path.to_path_buf());
            let ink_files = crate::loader::load_ink_files(game_path);
            for source in ink_files.values() {
                if let Err(e) = softcode.ink_runtime().borrow_mut().compile(source) {
                    tracing::warn!(error = %e, "Failed to pre-compile ink file");
                }
            }
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

        // Map + terrain sources are DB-owned (see docs/plans/map-builder.md).
        // On-disk `maps/*.toml` + `terrain.toml` SEED a fresh database, but
        // never overwrite what the DB already holds — so builder edits and
        // imports persist across restart and redeploy the way world content
        // does. The runtime `map_templates` (grid + palette folded in) are
        // then built from the DB sources, not the filesystem.
        if let Some(dir) = config.game_dir.as_deref() {
            for (path, toml) in crate::map_template::read_map_source_files(std::path::Path::new(dir)) {
                if let Err(e) = db.seed_file_source(&path, &toml) {
                    tracing::warn!(error = %e, path = %path, "failed to seed map source");
                }
                // Record the seed hash as the import baseline the first time a
                // path is seen, so a later `@import` of a changed file can tell
                // "nobody edited the DB copy" (silent update) from "the builder
                // edited it" (conflict). Never overwrite an existing baseline.
                if db
                    .get_import_hash(crate::import_export::FILE_SOURCE_REF, &path)
                    .ok()
                    .flatten()
                    .is_none()
                {
                    let hash = crate::import_export::blake3_hex(&toml);
                    let _ = db.set_import_hash(crate::import_export::FILE_SOURCE_REF, &path, &hash);
                }
            }
        }
        let file_sources = db.load_file_sources().unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to load map sources from DB");
            std::collections::HashMap::new()
        });
        let map_templates = crate::map_template::build_templates_from_sources(&file_sources);

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
            file_sources,
            scheduled_hooks,
            api_tokens,
            file_hashes,
            max_characters: config.max_characters,
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

        // -- on_tick hooks --
        // Every object with an on_tick Program ticks here, including
        // Kind::Code objects — a "global script" is just a Code object with
        // no location, so it needs no separate scheduler (see
        // docs/plans/program-authoring.md Stage 2).
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
            if *interval == 0 || !tick.is_multiple_of(*interval) {
                continue;
            }
            match self.fire_tick_hook(ref_id) {
                Ok(_) => ran += 1,
                Err(e) => {
                    tracing::warn!(hook = "on_tick", target = %ref_id, error = %e, "Tick script error");
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
        if tick.is_multiple_of(60) {
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

    /// If `batch` wrote a `lib_<name>` Program (via softcode's
    /// `set_program`/`apply_template`), invalidate the module result cache
    /// so the next `require("<name>")` anywhere re-evaluates from the new
    /// source instead of returning a stale cached module — see
    /// docs/plans/program-authoring.md Stage 2.
    fn invalidate_libs_touched_by(&self, batch: &softcode::IntentBatch) {
        let touched_lib = batch.intents.iter().any(|intent| {
            matches!(intent, softcode::Intent::SetProgram { hook, .. } if hook.starts_with("lib_"))
        });
        if touched_lib {
            self.softcode.invalidate_module_cache();
        }
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
                self.tick_count,
            )
            .map_err(|e| e.to_string())?;

        let effects = softcode::apply_batch(&mut self.world, &result.batch)?;
        self.world.next_id = dbref_counter.get();
        self.invalidate_libs_touched_by(&result.batch);
        self.deliver_effects(&effects, this_ref);

        if !result.state.is_empty()
            && let Some(obj) = self.world.get_mut(this_ref)
                && let Some(prog) = obj.programs.get_mut(hook_name) {
                    prog.state = result.state;
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
                self.tick_count,
            )
            .map_err(|e| e.to_string())?;

        let effects = softcode::apply_batch(&mut self.world, &result.batch)?;
        self.world.next_id = dbref_counter.get();
        self.invalidate_libs_touched_by(&result.batch);
        self.deliver_effects(&effects, this_ref);

        if !result.state.is_empty()
            && let Some(obj) = self.world.get_mut(this_ref)
                && let Some(prog) = obj.programs.get_mut(hook_name) {
                    prog.state = result.state;
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
        info.expires_at.is_some_and(|exp| Self::now_secs() > exp)
    }

    /// Reparse the DB-owned map sources into the live `map_templates` set —
    /// called after any builder write so `get_map_template`/`instantiate_map`
    /// see the change immediately, no restart.
    fn rebuild_map_templates(&mut self) {
        self.map_templates = crate::map_template::build_templates_from_sources(&self.file_sources);
    }

    /// Reload DB-owned map/terrain sources into memory and rebuild the live
    /// templates — after an `@import`/`Import` that may have written
    /// `file_sources` rows, so the running game reflects them without restart.
    fn reload_map_sources_from_db(&mut self) {
        self.file_sources = self.db.load_file_sources().unwrap_or_default();
        self.rebuild_map_templates();
    }

    /// A map name safe to use as `maps/<name>.toml` — bare filename only, no
    /// separators, `..`, or empties. This is the guard that keeps `PutMap`
    /// from becoming a write-anywhere primitive.
    fn valid_map_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 64
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    /// The area an object belongs to, from the `<area>/<key>` prefix of its
    /// `FILE_KEY_ATTR`. Rooms built in-game (or otherwise unfiled) carry no
    /// file identity and report the empty string. Drives `ListAreas` and the
    /// `area` filter/field of `ListWorldSlice`.
    fn room_area(o: &GameObject) -> String {
        o.attrs
            .get(crate::loader::FILE_KEY_ATTR)
            .and_then(|v| v.as_str())
            .and_then(|fk| fk.split_once('/'))
            .map(|(area, _)| area.to_string())
            .unwrap_or_default()
    }

    fn handle_api_request(&mut self, req: ApiRequest, token: Option<String>) -> ApiResponse {
        // `ListPrograms` deliberately does NOT sit in this set — it serves
        // full Program source, which is not something to hand out
        // unauthenticated (see docs/plans/program-authoring.md's Risks
        // section). It falls through to the generic Builder-gated write
        // path below, same as `SetProgram`.
        //
        // `Examine` doesn't either, for the same reason: it returns full
        // `attrs`/`tags`/`locks` for any object regardless of `system:hidden`
        // or a `can_see` hook, bypassing the in-game visibility rules a
        // player would otherwise be subject to (see
        // docs/plans/program-authoring.md Stage 4). The Svelte web client
        // only calls it from the Editor/Admin panels, both reachable only
        // after login — by the time either can open, `setToken` has already
        // stored a real token, so requiring one here doesn't break it.
        let is_read = matches!(
            &req,
            ApiRequest::ListRooms
                | ApiRequest::ListObjects { .. }
                | ApiRequest::ListExits { .. }
                | ApiRequest::ListHooks
        );

        // Populated for authenticated writes — the account performing this
        // request, threaded down to `SetProgram`/`RemoveProgram` below as
        // the program-version `author` (see
        // docs/plans/program-authoring.md Stage 3's "Author": "API
        // SetProgram — account_id, resolved in the auth block").
        let mut acting_account: Option<String> = None;

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

            // `Eval` is arbitrary admin code with the full write API — the
            // same gate `cmd_eval` applies for telnet (`Scope::Admin`, not
            // just `Builder`). A token that can eval owns the server.
            if matches!(
                &req,
                ApiRequest::SaveWorld
                    | ApiRequest::Eval { .. }
                    | ApiRequest::Import { .. }
                    | ApiRequest::Export { .. }
                    | ApiRequest::PutMap { .. }
                    | ApiRequest::PutTerrain { .. }
            ) {
                let has_admin = self
                    .accounts
                    .get(&account_id)
                    .map(|a| a.has_scope(Scope::Admin))
                    .unwrap_or(false);
                if !has_admin {
                    return ApiResponse::error("Admin scope required");
                }
            }

            acting_account = Some(account_id);
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
            ApiRequest::ListObjectsFull { kind, area, limit } => {
                let limit = limit.unwrap_or(600).min(3000) as usize;
                let kind_want = kind.as_deref().filter(|k| !k.is_empty());
                let area_want = area.as_deref().filter(|a| !a.is_empty());

                let mut objs: Vec<&GameObject> = self
                    .world
                    .objects
                    .values()
                    .filter(|o| o.kind != Kind::Exit)
                    .filter(|o| kind_want.is_none_or(|k| o.kind.to_string() == k))
                    .filter(|o| area_want.is_none_or(|a| Self::room_area(o) == a))
                    .collect();
                // Stable order (title, then ref) so the table's initial sort is
                // deterministic and truncation drops a consistent tail.
                objs.sort_by(|a, b| {
                    let at = a.title.as_deref().unwrap_or(&a.key);
                    let bt = b.title.as_deref().unwrap_or(&b.key);
                    at.to_lowercase().cmp(&bt.to_lowercase()).then_with(|| a.ref_id.cmp(&b.ref_id))
                });
                let truncated = objs.len() > limit;
                objs.truncate(limit);

                let rows: Vec<serde_json::Value> = objs
                    .iter()
                    .map(|o| {
                        let tags: Vec<String> = o.tags.iter().map(|t| t.as_spec()).collect();
                        serde_json::json!({
                            "ref_id": o.ref_id,
                            "key": o.key,
                            "kind": o.kind.to_string(),
                            "title": o.title,
                            "area": Self::room_area(o),
                            "location_ref": o.location_ref,
                            "tags": tags,
                        })
                    })
                    .collect();
                ApiResponse::success(serde_json::json!({
                    "objects": rows,
                    "truncated": truncated,
                }))
            }
            ApiRequest::ListHooks => {
                let known: Vec<serde_json::Value> = hooks::KNOWN_HOOKS
                    .iter()
                    .map(|h| serde_json::json!({
                        "name": h,
                        "describes": hooks::describe_hook(h),
                    }))
                    .collect();
                // Mirrors `is_valid_hook_name`'s open-ended arms: a name that
                // starts with one of these (and is longer than the prefix) is
                // accepted even when it isn't in `known`.
                ApiResponse::success(serde_json::json!({
                    "known": known,
                    "open_prefixes": ["on_", "cmd_", "lib_"],
                }))
            }
            ApiRequest::CreateRoom { area, key, title, description } => {
                // `key` and `area` feed the room's `_file_key`, which drives
                // `@export`'s output path — keep them to safe identifiers so a
                // separator or `..` can never reach the filesystem (the export
                // sink guards too, but reject early with a clear message).
                if !Self::valid_map_name(&key) {
                    return ApiResponse::error(
                        "invalid room key: use letters, digits, '_' or '-' (max 64)",
                    );
                }
                if !area.trim().is_empty() && !Self::valid_map_name(area.trim()) {
                    return ApiResponse::error(
                        "invalid area name: use letters, digits, '_' or '-' (max 64)",
                    );
                }
                let ref_id = self.world.next_dbref();
                let mut room = GameObject::new(&ref_id, &key, Kind::Room).with_title(&title);
                if let Some(desc) = description {
                    room.description = desc;
                }
                // Stamp the file identity so a room created while scoped to an
                // area belongs to that slice and exports into the area's file.
                // Same `<area>/<key>` mechanism the loader uses for reload
                // identity (see loader::FILE_KEY_ATTR); empty area = unfiled.
                if !area.trim().is_empty() {
                    room.attrs.insert(
                        crate::loader::FILE_KEY_ATTR.to_string(),
                        serde_json::Value::String(format!("{}/{}", area.trim(), key)),
                    );
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
            ApiRequest::SetLocation { ref_id, location } => {
                // An object inside itself is a containment cycle — reject it.
                if location == ref_id {
                    return ApiResponse::error("Cannot locate an object inside itself");
                }
                if self.world.get(&location).is_none() {
                    return ApiResponse::error(format!("Location '{}' not found", location));
                }
                match self.world.get_mut(&ref_id) {
                    // Relocating a live player by ref is a teleport — not a
                    // builder-tier edit. Admins have the teleport command for it.
                    Some(obj) if obj.kind == Kind::Player => {
                        ApiResponse::error("Refusing to relocate a player via set_location")
                    }
                    Some(obj) => {
                        obj.location_ref = Some(location);
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::UpdateExit { ref_id, direction, target } => {
                // A blank direction leaves the exit un-typeable — reject it, and
                // trim so "  up  " doesn't become the stored key.
                let direction = match direction {
                    Some(d) => {
                        let trimmed = d.trim();
                        if trimmed.is_empty() {
                            return ApiResponse::error("Exit direction cannot be blank");
                        }
                        Some(trimmed.to_string())
                    }
                    None => None,
                };
                if let Some(t) = &target
                    && self.world.get(t).is_none()
                {
                    return ApiResponse::error(format!("Target '{}' not found", t));
                }
                match self.world.get_mut(&ref_id) {
                    Some(obj) if obj.kind == Kind::Exit => {
                        if let Some(d) = direction {
                            obj.key = d;
                        }
                        if let Some(t) = target {
                            obj.target_ref = Some(t);
                        }
                        ApiResponse::ok()
                    }
                    Some(_) => ApiResponse::error("Object is not an exit"),
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::SetAliases { ref_id, aliases } => {
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        // Trim each entry (not just filter blank ones): a padded
                        // "  climb  " would otherwise never match player input.
                        obj.aliases = aliases
                            .into_iter()
                            .map(|a| a.trim().to_string())
                            .filter(|a| !a.is_empty())
                            .collect();
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
                if let Some(lib_name) = hook.strip_prefix("lib_")
                    && self.softcode.is_shipped_module(lib_name) {
                        return ApiResponse::error(format!(
                            "'{}' is a shipped module — choose a different name for your library",
                            lib_name
                        ));
                    }
                if let Err(e) = self.softcode.check_syntax(&source) {
                    return ApiResponse::error(format!("Syntax error: {}", e));
                }
                match self.world.get_mut(&ref_id) {
                    Some(obj) => match hooks::set_program(obj, &hook, source.clone()) {
                        Ok(()) => {
                            if hook.starts_with("lib_") {
                                self.softcode.invalidate_module_cache();
                            }
                            self.record_program_version(&ref_id, &hook, &source, acting_account.as_deref());
                            ApiResponse::ok()
                        }
                        Err(e) => ApiResponse::error(e),
                    },
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::RemoveProgram { ref_id, hook } => {
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        if hooks::remove_program(obj, &hook) {
                            if hook.starts_with("lib_") {
                                self.softcode.invalidate_module_cache();
                            }
                            self.record_program_tombstone(&ref_id, &hook, acting_account.as_deref());
                        }
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
            ApiRequest::ProgramHistory { ref_id, hook } => {
                let versions = match self.db.list_program_versions(&ref_id, &hook) {
                    Ok(v) => v,
                    Err(e) => return ApiResponse::error(format!("Failed to read history: {}", e)),
                };
                let items: Vec<serde_json::Value> = versions
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        serde_json::json!({
                            "version": i + 1,
                            "created_at": v.created_at,
                            "author": self.display_author(v.author.as_deref()),
                            "deleted": v.deleted,
                            "source": v.source,
                        })
                    })
                    .collect();
                ApiResponse::success(serde_json::json!(items))
            }
            ApiRequest::ProgramRestore { ref_id, hook, version } => {
                let ver = match self.db.get_program_version(&ref_id, &hook, version) {
                    Ok(Some(v)) => v,
                    Ok(None) => {
                        return ApiResponse::error(format!(
                            "{}/{} has no version {}",
                            ref_id, hook, version
                        ));
                    }
                    Err(e) => return ApiResponse::error(format!("Failed to read history: {}", e)),
                };
                // Non-destructive, same as `@program/restore`: always
                // appends a new version (or a new tombstone) rather than
                // rewinding — see docs/plans/program-authoring.md Stage 3's
                // "Restore is non-destructive".
                if ver.deleted {
                    if let Some(obj) = self.world.get_mut(&ref_id) {
                        hooks::remove_program(obj, &hook);
                    }
                    if hook.starts_with("lib_") {
                        self.softcode.invalidate_module_cache();
                    }
                    self.record_program_tombstone(&ref_id, &hook, acting_account.as_deref());
                    return ApiResponse::success(serde_json::json!({ "restored_deleted": true }));
                }
                if let Err(e) = self.softcode.check_syntax(&ver.source) {
                    return ApiResponse::error(format!("Syntax error: {}", e));
                }
                match self.world.get_mut(&ref_id) {
                    Some(obj) => match hooks::set_program(obj, &hook, ver.source.clone()) {
                        Ok(()) => {
                            if hook.starts_with("lib_") {
                                self.softcode.invalidate_module_cache();
                            }
                            self.record_program_version(&ref_id, &hook, &ver.source, acting_account.as_deref());
                            ApiResponse::ok()
                        }
                        Err(e) => ApiResponse::error(e),
                    },
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
            ApiRequest::ListAreas => {
                use std::collections::BTreeMap;
                let mut counts: BTreeMap<String, usize> = BTreeMap::new();
                for o in self.world.objects.values().filter(|o| o.kind == Kind::Room) {
                    *counts.entry(Self::room_area(o)).or_insert(0) += 1;
                }
                let areas: Vec<serde_json::Value> = counts
                    .into_iter()
                    .map(|(area, count)| serde_json::json!({ "area": area, "count": count }))
                    .collect();
                ApiResponse::success(serde_json::json!(areas))
            }
            ApiRequest::ListWorldSlice { area, tag, near, depth, limit } => {
                let limit = limit.unwrap_or(400).min(2000) as usize;
                let depth = (depth.unwrap_or(2) as usize).min(50); // clamp, mirroring `limit`
                let tag_match = tag.as_deref().and_then(|t| Tag::parse(t).ok());

                // area + tag predicate applied to every candidate room
                let keep = |o: &GameObject| -> bool {
                    if let Some(a) = &area
                        && &Self::room_area(o) != a
                    {
                        return false;
                    }
                    if let Some(t) = &tag_match {
                        let hit = if t.key == "*" {
                            o.tags.iter().any(|x| x.category == t.category)
                        } else {
                            o.tags.contains(t)
                        };
                        if !hit {
                            return false;
                        }
                    }
                    true
                };

                // Base room set: a BFS neighbourhood around `near`, else every
                // room. Filters narrow whichever base we pick.
                let mut room_refs: Vec<String> = Vec::new();
                if let Some(start) = &near {
                    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
                    for e in self.world.objects.values().filter(|o| o.kind == Kind::Exit) {
                        if let (Some(f), Some(t)) =
                            (e.location_ref.as_deref(), e.target_ref.as_deref())
                        {
                            adj.entry(f.to_string()).or_default().push(t.to_string());
                            adj.entry(t.to_string()).or_default().push(f.to_string());
                        }
                    }
                    let mut seen: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    let mut queue: std::collections::VecDeque<(String, usize)> =
                        std::collections::VecDeque::new();
                    if self.world.get(start).is_some() {
                        seen.insert(start.clone());
                        queue.push_back((start.clone(), 0));
                    }
                    while let Some((r, d)) = queue.pop_front() {
                        room_refs.push(r.clone());
                        if d < depth
                            && let Some(neighbours) = adj.get(&r)
                        {
                            for n in neighbours {
                                if seen.insert(n.clone()) {
                                    queue.push_back((n.clone(), d + 1));
                                }
                            }
                        }
                    }
                    room_refs.retain(|r| {
                        self.world
                            .get(r)
                            .map(|o| o.kind == Kind::Room && keep(o))
                            .unwrap_or(false)
                    });
                } else {
                    for o in self
                        .world
                        .objects
                        .values()
                        .filter(|o| o.kind == Kind::Room && keep(o))
                    {
                        room_refs.push(o.ref_id.clone());
                    }
                }

                room_refs.sort();
                let truncated = room_refs.len() > limit;
                room_refs.truncate(limit);
                let in_set: std::collections::HashSet<String> =
                    room_refs.iter().cloned().collect();

                let rooms: Vec<serde_json::Value> = room_refs
                    .iter()
                    .filter_map(|r| self.world.get(r))
                    .map(|o| {
                        let tags: Vec<String> = o.tags.iter().map(|t| t.as_spec()).collect();
                        // Saved builder layout position, if the room has one
                        // (a number, or a stringified number from older saves).
                        let coord = |k: &str| {
                            o.attrs.get(k).and_then(|v| {
                                v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                            })
                        };
                        serde_json::json!({
                            "ref_id": o.ref_id,
                            "key": o.key,
                            "title": o.title,
                            "description": o.description,
                            "area": Self::room_area(o),
                            "tags": tags,
                            "rx": coord("_rx"),
                            "ry": coord("_ry"),
                        })
                    })
                    .collect();

                // Exits leaving the slice's rooms; targets outside it become
                // boundary stubs so the client can offer to expand outward.
                let mut exits: Vec<serde_json::Value> = Vec::new();
                let mut boundary: std::collections::BTreeSet<String> =
                    std::collections::BTreeSet::new();
                for e in self.world.objects.values().filter(|o| o.kind == Kind::Exit) {
                    if let (Some(f), Some(t)) =
                        (e.location_ref.as_deref(), e.target_ref.as_deref())
                        && in_set.contains(f)
                    {
                        exits.push(serde_json::json!({
                            "ref_id": e.ref_id,
                            "from": f,
                            "dir": e.key,
                            "to": t,
                        }));
                        if !in_set.contains(t) {
                            boundary.insert(t.to_string());
                        }
                    }
                }
                let boundary_nodes: Vec<serde_json::Value> = boundary
                    .iter()
                    .filter_map(|r| self.world.get(r))
                    .map(|o| serde_json::json!({
                        "ref_id": o.ref_id,
                        "key": o.key,
                        "title": o.title,
                    }))
                    .collect();

                ApiResponse::success(serde_json::json!({
                    "rooms": rooms,
                    "exits": exits,
                    "boundary": boundary_nodes,
                    "truncated": truncated,
                }))
            }
            ApiRequest::CheckProgram { source } => match self.softcode.check_syntax(&source) {
                Ok(()) => ApiResponse::success(serde_json::json!({ "valid": true })),
                Err(e) => ApiResponse::success(serde_json::json!({ "valid": false, "error": e })),
            },
            ApiRequest::ListProgramsAll => {
                let mut out: Vec<serde_json::Value> = self
                    .world
                    .objects
                    .values()
                    .filter(|o| !o.programs.is_empty())
                    .map(|o| {
                        let mut hooks: Vec<String> = o.programs.keys().cloned().collect();
                        hooks.sort();
                        serde_json::json!({
                            "ref_id": o.ref_id,
                            "key": o.key,
                            "title": o.title,
                            "kind": o.kind.to_string(),
                            "area": Self::room_area(o),
                            "hooks": hooks,
                        })
                    })
                    .collect();
                out.sort_by(|a, b| a["ref_id"].as_str().cmp(&b["ref_id"].as_str()));
                ApiResponse::success(serde_json::json!(out))
            }
            ApiRequest::WorldCheck => {
                use std::collections::{HashMap, HashSet};
                let mut problems: Vec<serde_json::Value> = Vec::new();

                let exists: HashSet<&str> = self.world.objects.keys().map(|s| s.as_str()).collect();
                let is_room: HashSet<&str> = self
                    .world
                    .objects
                    .values()
                    .filter(|o| o.kind == Kind::Room)
                    .map(|o| o.ref_id.as_str())
                    .collect();

                // Walk exits once: flag dangling targets, tally in/out degree.
                let mut incoming: HashMap<&str, u32> = HashMap::new();
                let mut outgoing: HashMap<&str, u32> = HashMap::new();
                for e in self.world.objects.values().filter(|o| o.kind == Kind::Exit) {
                    if let Some(f) = e.location_ref.as_deref() {
                        *outgoing.entry(f).or_insert(0) += 1;
                    }
                    match e.target_ref.as_deref() {
                        Some(t) if exists.contains(t) => {
                            if is_room.contains(t) {
                                *incoming.entry(t).or_insert(0) += 1;
                            }
                        }
                        target => problems.push(serde_json::json!({
                            "kind": "broken_exit", "severity": "high",
                            "ref": e.ref_id, "from": e.location_ref, "dir": e.key, "target": target,
                            "message": format!("exit '{}' points at a target that no longer exists", e.key),
                        })),
                    }
                }

                // Per-room structural checks.
                for o in self.world.objects.values().filter(|o| o.kind == Kind::Room) {
                    let r = o.ref_id.as_str();
                    if o.description.trim().is_empty() {
                        problems.push(serde_json::json!({
                            "kind": "no_description", "severity": "low",
                            "ref": o.ref_id, "key": o.key, "message": "room has no description",
                        }));
                    }
                    if outgoing.get(r).copied().unwrap_or(0) == 0 {
                        problems.push(serde_json::json!({
                            "kind": "no_exits", "severity": "low",
                            "ref": o.ref_id, "key": o.key, "message": "room has no exits out (dead end)",
                        }));
                    }
                    if incoming.get(r).copied().unwrap_or(0) == 0 {
                        problems.push(serde_json::json!({
                            "kind": "unreachable", "severity": "medium",
                            "ref": o.ref_id, "key": o.key,
                            "message": "no exits lead into this room (unreachable unless it's an entry point)",
                        }));
                    }
                }

                // Compile every hook program; report the ones that don't parse.
                for o in self.world.objects.values().filter(|o| !o.programs.is_empty()) {
                    for p in hooks::list_programs(o) {
                        if let Err(err) = self.softcode.check_syntax(&p.source) {
                            problems.push(serde_json::json!({
                                "kind": "syntax_error", "severity": "high",
                                "ref": o.ref_id, "key": o.key, "hook": p.hook, "message": err,
                            }));
                        }
                    }
                }

                let rank = |s: &str| match s { "high" => 0, "medium" => 1, _ => 2 };
                problems.sort_by(|a, b| {
                    rank(a["severity"].as_str().unwrap_or("low"))
                        .cmp(&rank(b["severity"].as_str().unwrap_or("low")))
                        .then_with(|| a["ref"].as_str().cmp(&b["ref"].as_str()))
                });
                ApiResponse::success(serde_json::json!({ "problems": problems, "count": problems.len() }))
            }
            ApiRequest::SaveWorld => {
                self.do_save();
                ApiResponse::ok()
            }
            ApiRequest::Eval { source } => {
                // No session/telnet actor for a REST caller, so fall back to
                // the account's active character (same ref `cmd_charlist`
                // resolves) — `this` reads sensibly if the script inspects
                // `actor`, and any effects it emits reach the account's live
                // sessions, if it has any, the same way a hook's would.
                // With no active character the actor is simply absent from
                // the world; `object_to_value` degrades to `nil`, which is a
                // reasonable "no actor" for a headless eval.
                let actor_ref = acting_account
                    .as_deref()
                    .and_then(|id| self.accounts.get(id))
                    .and_then(|a| a.active_character.clone())
                    .unwrap_or_default();
                let output = self.eval_and_report(&actor_ref, &source);
                if output.starts_with("Eval error") {
                    ApiResponse::error(output.trim_end().to_string())
                } else {
                    ApiResponse::success(serde_json::json!({ "output": output }))
                }
            }
            ApiRequest::Import { path, dry_run } => {
                let bundle_path = std::path::Path::new(&path);
                match crate::import_export::import_bundle(
                    bundle_path,
                    &mut self.world,
                    &self.db,
                    dry_run,
                    acting_account.as_deref(),
                ) {
                    Ok(report) => {
                        if !dry_run {
                            self.reload_map_sources_from_db();
                        }
                        let output = crate::import_export::render_import_report(&report, dry_run, &path);
                        ApiResponse::success(serde_json::json!({ "output": output }))
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::Export { path } => {
                let export_path = std::path::Path::new(&path);
                match crate::import_export::export_bundle(export_path, &mut self.world) {
                    Ok(mut report) => {
                        match crate::import_export::export_file_sources(export_path, &self.file_sources) {
                            Ok(written) => report.maps_written = written,
                            Err(e) => return ApiResponse::error(e),
                        }
                        let output = crate::import_export::render_export_report(&report, &path);
                        ApiResponse::success(serde_json::json!({ "output": output }))
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::ListMaps => {
                let maps: Vec<String> = self
                    .file_sources
                    .keys()
                    .filter_map(|p| {
                        p.strip_prefix("maps/").and_then(|f| f.strip_suffix(".toml")).map(str::to_string)
                    })
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                let terrain = self.file_sources.get("terrain.toml").cloned().unwrap_or_default();
                ApiResponse::success(serde_json::json!({ "maps": maps, "terrain": terrain }))
            }
            ApiRequest::GetMap { name } => {
                if !Self::valid_map_name(&name) {
                    return ApiResponse::error("invalid map name");
                }
                match self.file_sources.get(&format!("maps/{}.toml", name)) {
                    Some(toml) => ApiResponse::success(serde_json::json!({ "name": name, "toml": toml })),
                    None => ApiResponse::error("no such map"),
                }
            }
            ApiRequest::GetTerrain => ApiResponse::success(serde_json::json!({
                "toml": self.file_sources.get("terrain.toml").cloned().unwrap_or_default()
            })),
            ApiRequest::PutMap { name, toml } => {
                if !Self::valid_map_name(&name) {
                    return ApiResponse::error("invalid map name (letters, digits, '_' or '-' only)");
                }
                if let Err(e) = crate::map_template::parse_map_template(&toml) {
                    return ApiResponse::error(format!("invalid map TOML: {}", e));
                }
                let path = format!("maps/{}.toml", name);
                if let Err(e) = self.db.save_file_source(&path, &toml) {
                    return ApiResponse::error(format!("database error: {}", e));
                }
                self.file_sources.insert(path, toml);
                self.rebuild_map_templates();
                ApiResponse::success(serde_json::json!({ "name": name }))
            }
            ApiRequest::PutTerrain { toml } => {
                if let Err(e) = crate::map_template::validate_terrain_toml(&toml) {
                    return ApiResponse::error(format!("invalid terrain TOML: {}", e));
                }
                if let Err(e) = self.db.save_file_source("terrain.toml", &toml) {
                    return ApiResponse::error(format!("database error: {}", e));
                }
                self.file_sources.insert("terrain.toml".to_string(), toml);
                self.rebuild_map_templates();
                ApiResponse::ok()
            }
            ApiRequest::InkCompile { source } => {
                match self.softcode.ink_runtime().borrow_mut().compile(&source) {
                    Ok(_) => ApiResponse::success(serde_json::json!({ "valid": true })),
                    Err(e) => ApiResponse::success(serde_json::json!({
                        "valid": false,
                        "errors": e,
                    })),
                }
            }
            ApiRequest::InkSave { ref_id, source } => {
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        let errors = self.softcode.ink_runtime().borrow_mut().compile(&source).err();
                        obj.attrs.insert("_ink_source".into(), serde_json::json!(source));
                        if let Some(ref e) = errors {
                            obj.attrs.insert("_ink_errors".into(), serde_json::json!(e));
                        } else {
                            obj.attrs.remove("_ink_errors");
                        }
                        ApiResponse::success(serde_json::json!({
                            "saved": true,
                            "valid": errors.is_none(),
                            "errors": errors,
                        }))
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::InkLoad { ref_id } => {
                match self.world.get(&ref_id) {
                    Some(obj) => {
                        let source = obj.attrs.get("_ink_source").and_then(|v| v.as_str());
                        let errors = obj.attrs.get("_ink_errors").and_then(|v| v.as_str());
                        ApiResponse::success(serde_json::json!({
                            "source": source,
                            "errors": errors,
                        }))
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
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

                self.softcode.ink_runtime().borrow_mut().cleanup_player(actor_ref);

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
                SessionState::SelectCharacter { .. } => "select_character",
                SessionState::CreateCharacterName { .. } => "create_character_name",
            },
            None => return,
        };

        match state {
            "prompt_username" => self.handle_login_username(session_id, input),
            "prompt_password" => self.handle_login_password(session_id, input),
            "create_username" => self.handle_create_username(session_id, input),
            "create_password" => self.handle_create_password(session_id, input),
            "confirm_password" => self.handle_confirm_password(session_id, input),
            "select_character" => self.handle_select_character(session_id, input),
            "create_character_name" => self.handle_create_character_name(session_id, input),
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
        let characters = account.characters.clone();
        let active_character = account.active_character.clone().unwrap_or_default();

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

        match characters.len() {
            0 => self.enter_world(session_id, &username, "", &account_id),
            1 => self.enter_world(session_id, &username, &characters[0], &account_id),
            _ => {
                if !active_character.is_empty() && characters.contains(&active_character) {
                    self.enter_world(session_id, &username, &active_character, &account_id);
                } else {
                    self.show_character_select(session_id, &account_id);
                }
            }
        }
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
                let characters = account.characters.clone();
                let active_character = account.active_character.clone().unwrap_or_default();

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

                match characters.len() {
                    0 => self.enter_world(session_id, &username, "", &account_id),
                    1 => self.enter_world(session_id, &username, &characters[0], &account_id),
                    _ => {
                        if !active_character.is_empty() && characters.contains(&active_character) {
                            self.enter_world(session_id, &username, &active_character, &account_id);
                        } else {
                            self.show_character_select(session_id, &account_id);
                        }
                    }
                }
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
                self.send(session_id, &format!("\r\nAccount created! Welcome, {}.\r\n", username));
                self.enter_world(session_id, &username, "", &account_id);
            }
            Err(msg) => {
                self.send(session_id, &format!("{}\r\nChoose a username: ", msg));
                if let Some(session) = self.sessions.get_mut(session_id) {
                    session.state = SessionState::CreateUsername;
                }
            }
        }
    }

    fn show_character_select(&mut self, session_id: &str, account_id: &str) {
        let characters = self
            .accounts
            .get(account_id)
            .map(|a| a.characters.clone())
            .unwrap_or_default();

        let mut msg = "\r\nSelect a character:\r\n".to_string();
        for (i, ref_id) in characters.iter().enumerate() {
            let name = self
                .world
                .get(ref_id)
                .map(|o| o.display_name().to_string())
                .unwrap_or_else(|| ref_id.clone());
            let location = self
                .world
                .get(ref_id)
                .and_then(|o| o.location_ref.as_ref())
                .and_then(|loc| self.world.get(loc))
                .map(|r| r.display_name().to_string())
                .unwrap_or_else(|| "unknown".into());
            msg.push_str(&format!("  {}) {} ({})\r\n", i + 1, name, location));
        }
        msg.push_str("  create) Create a new character\r\n");
        msg.push_str("\r\nChoice: ");
        self.send(session_id, &msg);

        if let Some(session) = self.sessions.get_mut(session_id) {
            session.state = SessionState::SelectCharacter {
                account_id: account_id.to_string(),
            };
        }
    }

    fn handle_select_character(&mut self, session_id: &str, input: &str) {
        let account_id = match self.sessions.get(session_id) {
            Some(Session {
                state: SessionState::SelectCharacter { account_id },
                ..
            }) => account_id.clone(),
            _ => return,
        };

        let input = input.trim().to_lowercase();

        if input == "create" {
            self.send(session_id, "Character name: ");
            if let Some(session) = self.sessions.get_mut(session_id) {
                session.state = SessionState::CreateCharacterName { account_id };
            }
            return;
        }

        let index: usize = match input.parse::<usize>() {
            Ok(n) if n >= 1 => n - 1,
            _ => {
                self.send(session_id, "Invalid choice. Enter a number or 'create': ");
                return;
            }
        };

        let (username, character_ref) = match self.accounts.get(&account_id) {
            Some(account) => {
                if index >= account.characters.len() {
                    self.send(session_id, "Invalid choice. Enter a number or 'create': ");
                    return;
                }
                (account.username.clone(), account.characters[index].clone())
            }
            None => return,
        };

        self.enter_world(session_id, &username, &character_ref, &account_id);
    }

    fn handle_create_character_name(&mut self, session_id: &str, input: &str) {
        let account_id = match self.sessions.get(session_id) {
            Some(Session {
                state: SessionState::CreateCharacterName { account_id },
                ..
            }) => account_id.clone(),
            _ => return,
        };

        let name = input.trim();
        if name.len() < 3 {
            self.send(session_id, "Name must be at least 3 characters. Try again: ");
            return;
        }
        if name.len() > 20 {
            self.send(session_id, "Name must be 20 characters or fewer. Try again: ");
            return;
        }
        if !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ' ') {
            self.send(session_id, "Name may only contain letters, numbers, underscores, and spaces. Try again: ");
            return;
        }

        let max = self
            .accounts
            .get(&account_id)
            .and_then(|a| a.max_characters)
            .unwrap_or(self.max_characters);
        let current = self
            .accounts
            .get(&account_id)
            .map(|a| a.characters.len())
            .unwrap_or(0);
        if current as u8 >= max {
            self.send(
                session_id,
                &format!("You already have the maximum of {} characters.\r\n", max),
            );
            self.show_character_select(session_id, &account_id);
            return;
        }

        self.enter_world(session_id, name, "", &account_id);
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
                if !account.characters.contains(&ref_id) {
                    account.characters.push(ref_id.clone());
                }
                account.active_character = Some(ref_id.clone());
            }
            self.db.save_accounts(&self.accounts).ok();
            ref_id
        };

        if let Some(account) = self.accounts.get_mut(account_id) {
            account.active_character = Some(player_ref.clone());
        }

        if let Some(session) = self.sessions.get_mut(session_id) {
            session.state = SessionState::Playing {
                actor_ref: player_ref.clone(),
                account_id: account_id.to_string(),
                puppet_ref: None,
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
        self.send_commands(session_id, &player_ref);

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

        let (actor_ref, puppet_ref) = match self.sessions.get(session_id) {
            Some(Session {
                state: SessionState::Playing { actor_ref, puppet_ref, .. },
                ..
            }) => (actor_ref.clone(), puppet_ref.clone()),
            _ => return,
        };
        let _effective_ref = puppet_ref.as_deref().unwrap_or(&actor_ref).to_string();

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

        // Check for multi-line ink editor mode
        if let Some(actor) = self.world.get(&actor_ref)
            && actor.attrs.contains_key("_ink_editing") {
                self.handle_ink_editor_input(session_id, &actor_ref, input);
                return;
            }

        // Check for multi-line @eval editor mode
        if let Some(actor) = self.world.get(&actor_ref)
            && actor.attrs.contains_key("_eval_editing")
        {
            self.handle_eval_editor_input(session_id, &actor_ref, input);
            return;
        }

        // Check for multi-line @program editor mode
        if let Some(actor) = self.world.get(&actor_ref)
            && actor.attrs.contains_key("_program_editing")
        {
            self.handle_program_editor_input(session_id, &actor_ref, input);
            return;
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
            "@program/history" => self.cmd_program_history(session_id, &actor_ref, &args),
            "@program/restore" => self.cmd_program_restore(session_id, &actor_ref, &args),
            "@program/diff" => self.cmd_program_diff(session_id, &actor_ref, &args),
            "@tag" => self.cmd_tag(session_id, &actor_ref, &args),
            "@untag" => self.cmd_untag(session_id, &actor_ref, &args),
            "@script" => self.cmd_script(session_id, &actor_ref, &args),
            "@scripts" => self.cmd_scripts(session_id),
            "@rmscript" => self.cmd_rmscript(session_id, &actor_ref, &args),
            "@script-interval" => self.cmd_script_interval(session_id, &args),
            "@lib" => self.cmd_lib(session_id, &actor_ref, &args),
            "@libs" => self.cmd_libs(session_id),
            "@rmlib" => self.cmd_rmlib(session_id, &actor_ref, &args),
            "@lock" => self.cmd_lock(session_id, &actor_ref, &args),
            "@unlock" => self.cmd_unlock(session_id, &actor_ref, &args),
            "@locks" => self.cmd_locks(session_id, &actor_ref, &args),

            "@charlist" => self.cmd_charlist(session_id),
            "@charcreate" => self.cmd_charcreate(session_id, &args),
            "@charswitch" => self.cmd_charswitch(session_id, &args),
            "@chardelete" => self.cmd_chardelete(session_id, &args),
            "@puppet" => self.cmd_puppet(session_id, &actor_ref, &args),
            "@unpuppet" => self.cmd_unpuppet(session_id),

            "@chown" => self.cmd_chown(session_id, &args),
            "@dialogue" | "@dialog" => self.cmd_dialogue(session_id, &actor_ref, &args),

            // Admin commands
            "@grant" => self.cmd_grant(session_id, &args),
            "@revoke" => self.cmd_revoke(session_id, &args),
            "@scopes" => self.cmd_scopes(session_id, &args),
            "@wall" => self.cmd_wall(session_id, &args),
            "@boot" => self.cmd_boot(session_id, &args),
            "@save" => self.cmd_save(session_id),
            "@shutdown" => self.cmd_shutdown(session_id),
            "@reload-world" => self.cmd_reload_world(session_id),
            "@eval" => self.cmd_eval(session_id, &actor_ref, &args),
            "@import" => self.cmd_import(session_id, &args),
            "@export" => self.cmd_export(session_id, &args),
            "@maxchars" => self.cmd_maxchars(session_id, &args),
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
            self.send_commands(session_id, &actor_ref);
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

    /// Record a version of `(obj_ref, hook)`'s source — see
    /// docs/plans/program-authoring.md Stage 3. Called from the *authoring*
    /// write paths only (`@program`, `@lib`, REST `SetProgram`, and the
    /// loader's file installs). Never called for softcode's
    /// `Intent::SetProgram` — that path is instantiation (attaching
    /// behaviour to a procedurally generated object from a source that's
    /// already a string constant under git), not authoring; see the plan's
    /// "Instantiation is not authoring".
    ///
    /// A DB write on every call site, but small, rare (author-driven, not
    /// per-tick), and off the hot request-handling path — the engine loop
    /// is otherwise blocking here anyway for `save_world`/`save_accounts`,
    /// so this is consistent with the rest of `Engine`'s persistence, not a
    /// new category of risk.
    fn record_program_version(&self, obj_ref: &str, hook: &str, source: &str, author: Option<&str>) {
        if let Err(e) = self.db.record_program_version(obj_ref, hook, source, author) {
            tracing::warn!(error = %e, obj_ref, hook, "Failed to record program version");
        }
    }

    /// Record a tombstone version marking `(obj_ref, hook)` as deleted — see
    /// the plan's "record deletions as tombstone versions" note.
    fn record_program_tombstone(&self, obj_ref: &str, hook: &str, author: Option<&str>) {
        if let Err(e) = self.db.record_program_tombstone(obj_ref, hook, author) {
            tracing::warn!(error = %e, obj_ref, hook, "Failed to record program deletion tombstone");
        }
    }

    /// Validate everything about a program write except the source itself:
    /// the target exists, the actor may modify it, the hook name is known,
    /// and (for `lib_*`) the name doesn't collide with a shipped module.
    /// Shared between the single-line and multi-line `@program` paths so
    /// entering the multi-line editor fails fast on a bad target instead of
    /// only discovering it after the user has typed a whole program.
    fn check_program_write(
        &self,
        session_id: &str,
        actor_ref: &str,
        target_ref: &str,
        hook: &str,
    ) -> Option<String> {
        if self.world.get(target_ref).is_none() {
            return Some(format!("No object with ref '{}'.\r\n", target_ref));
        }
        if !self.can_modify_object(session_id, actor_ref, target_ref) {
            return Some("Permission denied (not owner).\r\n".to_string());
        }
        if !hooks::is_valid_hook_name(hook) {
            return Some(format!(
                "Unknown hook '{}'. Known hooks: {}, or cmd_<name>.\r\n",
                hook,
                hooks::KNOWN_HOOKS.join(", ")
            ));
        }
        if let Some(lib_name) = hook.strip_prefix("lib_")
            && self.softcode.is_shipped_module(lib_name)
        {
            return Some(format!(
                "'{}' is a shipped module — choose a different name for your library.\r\n",
                lib_name
            ));
        }
        None
    }

    /// Check syntax, write the Program, and record a version — the shared
    /// tail of both the single-line and multi-line `@program` paths.
    fn install_program(&mut self, actor_ref: &str, target_ref: &str, hook: &str, source: &str) -> String {
        if let Err(e) = self.softcode.check_syntax(source) {
            return format!("Syntax error in program: {}\r\n", e);
        }
        let obj = match self.world.get_mut(target_ref) {
            Some(o) => o,
            None => return format!("No object with ref '{}'.\r\n", target_ref),
        };
        if let Err(e) = hooks::set_program(obj, hook, source.to_string()) {
            return format!("{}\r\n", e);
        }
        if hook.starts_with("lib_") {
            self.softcode.invalidate_module_cache();
        }
        self.record_program_version(target_ref, hook, source, Some(actor_ref));
        format!(
            "Program installed: {}/{} ({})\r\n",
            target_ref,
            hook,
            hooks::describe_hook(hook)
        )
    }

    fn cmd_program(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @program <ref>/<hook> = <luau source>
        //
        // With nothing after the `=`, enters the multi-line editor —
        // `@program` reading only a single line meant no multi-line Luau
        // could be authored from telnet at all (real authoring only worked
        // through the web Editor). Reuses the `@eval` multi-line machinery
        // (bare `.` commits, `@abort` cancels) rather than a second copy —
        // see docs/plans/program-authoring.md Stage 3's "Prerequisite:
        // multi-line authoring".
        let (path, source) = match args.split_once('=') {
            Some((p, s)) => (p.trim(), s.trim()),
            None => return "Usage: @program <ref>/<hook> = <luau source>\r\n".to_string(),
        };
        let (target_ref, hook) = match self.resolve_ref_hook_path(actor_ref, path) {
            Ok(v) => v,
            Err(e) => return format!("{}\r\n", e),
        };
        if let Some(err) = self.check_program_write(session_id, actor_ref, &target_ref, &hook) {
            return err;
        }
        if source.is_empty() {
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.insert("_program_editing".into(), serde_json::json!(true));
                actor.attrs.insert("_program_buffer".into(), serde_json::json!(""));
                actor.attrs.insert("_program_target".into(), serde_json::json!(target_ref));
                actor.attrs.insert("_program_hook".into(), serde_json::json!(hook));
            }
            return "Enter Luau source. Type '.' on a line by itself to install it, '@abort' to cancel:\r\n"
                .to_string();
        }
        self.install_program(actor_ref, &target_ref, &hook, source)
    }

    fn handle_program_editor_input(&mut self, session_id: &str, actor_ref: &str, input: &str) {
        if input == "." {
            let (buffer, target_ref, hook) = {
                let actor = match self.world.get(actor_ref) {
                    Some(a) => a,
                    None => return,
                };
                let buffer = actor
                    .attrs
                    .get("_program_buffer")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let target_ref = actor
                    .attrs
                    .get("_program_target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let hook = actor
                    .attrs
                    .get("_program_hook")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                (buffer, target_ref, hook)
            };
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.remove("_program_editing");
                actor.attrs.remove("_program_buffer");
                actor.attrs.remove("_program_target");
                actor.attrs.remove("_program_hook");
            }
            if buffer.is_empty() {
                self.send(session_id, "Empty source, nothing installed.\r\n");
                return;
            }
            let output = self.install_program(actor_ref, &target_ref, &hook, &buffer);
            self.send(session_id, &output);
        } else if input == "@abort" {
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.remove("_program_editing");
                actor.attrs.remove("_program_buffer");
                actor.attrs.remove("_program_target");
                actor.attrs.remove("_program_hook");
            }
            self.send(session_id, "Program edit cancelled.\r\n");
        } else if let Some(actor) = self.world.get_mut(actor_ref) {
            let current = actor
                .attrs
                .get("_program_buffer")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let new_buffer = if current.is_empty() {
                input.to_string()
            } else {
                format!("{}\n{}", current, input)
            };
            actor
                .attrs
                .insert("_program_buffer".into(), serde_json::json!(new_buffer));
        }
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
                    if hook.starts_with("lib_") {
                        self.softcode.invalidate_module_cache();
                    }
                    self.record_program_tombstone(&target_ref, &hook, Some(actor_ref));
                    format!("Removed program {}/{}.\r\n", target_ref, hook)
                } else {
                    format!("{} has no '{}' program.\r\n", target_ref, hook)
                }
            }
            None => format!("No object with ref '{}'.\r\n", target_ref),
        }
    }

    /// Resolve an `author` column value (an object ref or, for the REST
    /// path, an account id — see `record_program_version`'s doc comment)
    /// to a display name at read time. Per the plan's "Author" note: store
    /// the ref, not a display name, because names change and a stale name
    /// in a history listing is worse than none — so history rows carry the
    /// ref and this only runs when someone actually looks at the list.
    fn display_author(&self, author: Option<&str>) -> String {
        match author {
            None => "(system)".to_string(),
            Some(a) => {
                if let Some(obj) = self.world.get(a) {
                    return obj.title.clone().unwrap_or_else(|| obj.key.clone());
                }
                if let Some(acct) = self.accounts.get(a) {
                    return acct.username.clone();
                }
                a.to_string()
            }
        }
    }

    fn cmd_program_history(&self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let path = args.trim();
        if path.is_empty() {
            return "Usage: @program/history <ref>/<hook>\r\n".to_string();
        }
        let (target_ref, hook) = match self.resolve_ref_hook_path(actor_ref, path) {
            Ok(v) => v,
            Err(e) => return format!("{}\r\n", e),
        };
        let versions = match self.db.list_program_versions(&target_ref, &hook) {
            Ok(v) => v,
            Err(e) => return format!("Failed to read history: {}\r\n", e),
        };
        if versions.is_empty() {
            return format!("No version history for {}/{}.\r\n", target_ref, hook);
        }
        let mut out = format!("History for {}/{}:\r\n", target_ref, hook);
        for (i, v) in versions.iter().enumerate() {
            let n = i + 1;
            let when = format_epoch_secs(v.created_at);
            let author = self.display_author(v.author.as_deref());
            let marker = if v.deleted { "  (deleted)" } else { "" };
            out.push_str(&format!("  {:>3}  {}  {}{}\r\n", n, when, author, marker));
        }
        out.push_str("Use @program/diff or @program/restore with one of the numbers above.\r\n");
        out
    }

    fn cmd_program_restore(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @program/restore <ref>/<hook> <n>
        let (path, n_str) = match args.trim().rsplit_once(' ') {
            Some((p, n)) => (p.trim(), n.trim()),
            None => return "Usage: @program/restore <ref>/<hook> <n>\r\n".to_string(),
        };
        let n: usize = match n_str.parse() {
            Ok(v) if v > 0 => v,
            _ => return "Version number must be a positive integer.\r\n".to_string(),
        };
        let (target_ref, hook) = match self.resolve_ref_hook_path(actor_ref, path) {
            Ok(v) => v,
            Err(e) => return format!("{}\r\n", e),
        };
        if let Some(err) = self.check_program_write(session_id, actor_ref, &target_ref, &hook) {
            return err;
        }
        let version = match self.db.get_program_version(&target_ref, &hook, n) {
            Ok(Some(v)) => v,
            Ok(None) => return format!("{}/{} has no version {}.\r\n", target_ref, hook, n),
            Err(e) => return format!("Failed to read history: {}\r\n", e),
        };
        // Non-destructive: restoring always *appends* a new version rather
        // than rewinding, so history can never be made worse by using it —
        // see docs/plans/program-authoring.md Stage 3's "Restore is
        // non-destructive".
        if version.deleted {
            if let Some(obj) = self.world.get_mut(&target_ref) {
                hooks::remove_program(obj, &hook);
            }
            if hook.starts_with("lib_") {
                self.softcode.invalidate_module_cache();
            }
            self.record_program_tombstone(&target_ref, &hook, Some(actor_ref));
            return format!(
                "Version {} of {}/{} was a deletion — restored as a new deletion.\r\n",
                n, target_ref, hook
            );
        }
        let result = self.install_program(actor_ref, &target_ref, &hook, &version.source);
        format!("Restored version {} as a new version. {}", n, result)
    }

    fn cmd_program_diff(&self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @program/diff <ref>/<hook> <n> [<m>] — diffs version <n> against
        // version <m>, or against the object's current live source if <m>
        // is omitted.
        let mut parts = args.split_whitespace();
        let (path, n_str) = match (parts.next(), parts.next()) {
            (Some(p), Some(n)) => (p, n),
            _ => return "Usage: @program/diff <ref>/<hook> <n> [<m>]\r\n".to_string(),
        };
        let m_str = parts.next();
        let n: usize = match n_str.parse() {
            Ok(v) if v > 0 => v,
            _ => return "Version number must be a positive integer.\r\n".to_string(),
        };
        let (target_ref, hook) = match self.resolve_ref_hook_path(actor_ref, path) {
            Ok(v) => v,
            Err(e) => return format!("{}\r\n", e),
        };
        let from = match self.db.get_program_version(&target_ref, &hook, n) {
            Ok(Some(v)) => v,
            Ok(None) => return format!("{}/{} has no version {}.\r\n", target_ref, hook, n),
            Err(e) => return format!("Failed to read history: {}\r\n", e),
        };
        let (to_source, to_label) = match m_str {
            Some(m_str) => {
                let m: usize = match m_str.parse() {
                    Ok(v) if v > 0 => v,
                    _ => return "Version number must be a positive integer.\r\n".to_string(),
                };
                match self.db.get_program_version(&target_ref, &hook, m) {
                    Ok(Some(v)) => (v.source, format!("v{}", m)),
                    Ok(None) => return format!("{}/{} has no version {}.\r\n", target_ref, hook, m),
                    Err(e) => return format!("Failed to read history: {}\r\n", e),
                }
            }
            None => match self.world.get(&target_ref).and_then(|o| o.programs.get(&hook)) {
                Some(p) => (p.source.clone(), "current".to_string()),
                None => (String::new(), "current (no program)".to_string()),
            },
        };
        let diff = similar::TextDiff::from_lines(&from.source, &to_source);
        let mut out = format!("Diff {}/{} v{} -> {}:\r\n", target_ref, hook, n, to_label);
        let mut any = false;
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => '-',
                similar::ChangeTag::Insert => '+',
                similar::ChangeTag::Equal => ' ',
            };
            if change.tag() != similar::ChangeTag::Equal {
                any = true;
            }
            let line = change.value().trim_end_matches('\n');
            out.push_str(&format!("{}{}\r\n", sign, line));
        }
        if !any {
            out.push_str("(no differences)\r\n");
        }
        out
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
                self.tick_count,
            )
            .map_err(|e| e.to_string())?;

        let denied = result.denied;
        let effects = softcode::apply_batch(&mut self.world, &result.batch)?;
        self.world.next_id = dbref_counter.get();
        self.invalidate_libs_touched_by(&result.batch);
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

    /// Whether the owner of `target` already has the most pending timers they
    /// are allowed. An unowned target belongs to the system layer and is
    /// exempt. See [`crate::softcode::OWNER_TIMER_QUOTA`].
    fn timer_quota_reached(&self, target: &str) -> bool {
        let Some(owner) = self.world.get(target).and_then(|o| o.owner_ref.clone()) else {
            return false;
        };
        self.scheduled_hooks
            .iter()
            .filter(|s| {
                self.world
                    .get(&s.target)
                    .and_then(|o| o.owner_ref.as_deref())
                    == Some(owner.as_str())
            })
            .count()
            >= crate::softcode::OWNER_TIMER_QUOTA
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
                    // Fork-bomb guard. Counted against the *target's* owner
                    // rather than whoever scheduled it, so the cap holds
                    // however the timer was created. Unowned targets are the
                    // system layer and exempt, same as the object quota.
                    if self.timer_quota_reached(target) {
                        tracing::warn!(
                            target = %target,
                            hook = %hook,
                            "timer quota reached; refusing to schedule"
                        );
                        continue;
                    }
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
                    for session in self.sessions.values() {
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
                Effect::EmitRadius { room, radius, messages, exclude } => {
                    // BFS walk from source room through exits
                    let mut visited: HashMap<String, u32> = HashMap::new();
                    let mut queue: std::collections::VecDeque<(String, u32)> = std::collections::VecDeque::new();
                    visited.insert(room.clone(), 0);
                    queue.push_back((room.clone(), 0));

                    while let Some((current_room, dist)) = queue.pop_front() {
                        if let Some(msg) = messages.get(&dist) {
                            for session in self.sessions.values() {
                                if let SessionState::Playing { actor_ref: ar, .. } = &session.state {
                                    if exclude.contains(ar) {
                                        continue;
                                    }
                                    if let Some(actor) = self.world.get(ar)
                                        && actor.location_ref.as_deref() == Some(&current_room) {
                                            let _ = session.tx.send(ClientMessage::Text {
                                                text: msg.clone(),
                                            });
                                        }
                                }
                            }
                        }

                        if dist < *radius {
                            for exit in self.world.exits_from(&current_room) {
                                if let Some(target_ref) = &exit.target_ref {
                                    let muffle = exit
                                        .attrs
                                        .get("muffle")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0) as u32;
                                    let blocked = exit
                                        .attrs
                                        .get("blocked_sound")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false);
                                    if blocked {
                                        continue;
                                    }
                                    let next_dist = dist + 1 + muffle;
                                    if next_dist <= *radius && !visited.contains_key(target_ref) {
                                        visited.insert(target_ref.clone(), next_dist);
                                        queue.push_back((target_ref.clone(), next_dist));
                                    }
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
            if data.is_some()
                && let Some(obj) = self.world.get_mut(&target) {
                    obj.attrs.insert("_trigger_data".into(), data.clone().unwrap());
                }
            if let Err(e) = self.fire_hook(&target, &hook, actor_ref, room_ref.as_deref(), None) {
                tracing::warn!(hook = %hook, target = %target, error = %e, "Triggered hook error");
            }
            if data.is_some()
                && let Some(obj) = self.world.get_mut(&target) {
                    obj.attrs.remove("_trigger_data");
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
            if let Some(account_id) = self.session_account_id(session_id)
                && let Some(acct) = self.accounts.get(&account_id) {
                    return format!("Your scopes: {}\r\n", acct.scope_labels().join(", "));
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
        for session in self.sessions.values() {
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
            if let SessionState::Playing { actor_ref, .. } = &session.state
                && let Some(actor) = self.world.get(actor_ref) {
                    out.push_str(&format!("  {}\r\n", actor.display_name()));
                    count += 1;
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

    fn send_commands(&self, session_id: &str, actor_ref: &str) {
        let is_builder = self.session_has_scope(session_id, Scope::Builder);
        let is_admin = self.session_has_scope(session_id, Scope::Admin);

        let mut cmds: Vec<String> = vec![
            "look", "say", "go", "quit", "inventory", "get", "put", "drop",
            "use", "examine", "who", "whisper", "emote", "help",
            "@password", "@email", "@token", "@display",
            "@charlist", "@charcreate", "@charswitch", "@chardelete",
        ].into_iter().map(String::from).collect();

        if is_builder {
            cmds.extend([
                "@dig", "@open", "@describe", "@create", "@destroy", "@set",
                "@teleport", "@name", "@program", "@programs", "@rmprogram",
                "@program/history", "@program/restore", "@program/diff",
                "@tag", "@untag", "@script", "@scripts", "@rmscript",
                "@script-interval", "@lib", "@libs", "@rmlib",
                "@lock", "@unlock", "@locks",
                "@dialogue", "@test", "@reload", "@puppet", "@unpuppet", "@chown",
            ].iter().map(|s| String::from(*s)));
        }

        if is_admin {
            cmds.extend([
                "@grant", "@revoke", "@scopes", "@wall", "@boot",
                "@save", "@shutdown", "@reload-world", "@maxchars",
                "@eval", "@import", "@export",
            ].iter().map(|s| String::from(*s)));
        }

        let room_ref = self.world.get(actor_ref).and_then(|a| a.location_ref.clone());
        if let Some(room_ref) = &room_ref {
            for exit in self.world.exits_from(room_ref) {
                cmds.push(exit.key.clone());
                for alias in &exit.aliases {
                    cmds.push(alias.clone());
                }
            }

            let global_tag = Tag { category: "system".into(), key: "global".into() };
            let room_itself = self.world.get(room_ref).into_iter();
            let room_objs = self.world.objects_in(room_ref).into_iter()
                .filter(|o| o.ref_id != actor_ref);
            let inv_objs = self.world.objects_in(actor_ref).into_iter();
            let global_objs = self.world.objects.values()
                .filter(|o| o.tags.contains(&global_tag));

            for obj in room_itself.chain(room_objs).chain(inv_objs).chain(global_objs) {
                for (hook, prog) in &obj.programs {
                    if hook.starts_with("cmd_") && prog.enabled {
                        cmds.push(hook[4..].to_string());
                    }
                }
            }
        }

        cmds.sort();
        cmds.dedup();

        if let Some(session) = self.sessions.get(session_id) {
            let _ = session.tx.send(ClientMessage::Commands { commands: cmds });
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
            if let SessionState::Playing { actor_ref: ar, .. } = &session.state
                && let Some(other_actor) = self.world.get(ar)
                    && other_actor.location_ref.as_deref() == Some(&room_ref) {
                        let _ = session.tx.send(others_msg.clone());
                    }
        }

        let _ = self.fire_hook(&room_ref, "on_emote", actor_ref, Some(&room_ref), None);

        format!("{} {}\r\n", name, message)
    }

    // -- Global script commands --
    //
    // A global script is a `Kind::Code` object carrying an `on_tick`
    // Program and a `tick_interval` attr — see docs/plans/program-authoring.md
    // Stage 2. It's found by `key`, since a Code object has no other stable
    // human-facing name. `key` isn't unique across the world in general, but
    // these commands only ever look among `Kind::Code` objects, so a builder
    // reusing a script's name for something else can't collide with it.

    /// Find the ref of the `Kind::Code` object named `name` that carries an
    /// enabled-or-not Program at `hook`, if any.
    fn find_code_object_ref(&self, name: &str, hook: &str) -> Option<String> {
        self.world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == name && o.programs.contains_key(hook))
            .map(|o| o.ref_id.clone())
    }

    fn cmd_script(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
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
        let existing_ref = self.find_code_object_ref(name, "on_tick");
        let is_new = existing_ref.is_none();
        let ref_id = existing_ref.unwrap_or_else(|| {
            let ref_id = self.world.next_dbref();
            self.world.add_object(GameObject::new(&ref_id, name, Kind::Code));
            ref_id
        });
        let obj = self.world.get_mut(&ref_id).unwrap();
        if is_new {
            obj.attrs.insert("tick_interval".into(), serde_json::json!(1));
        }
        if let Err(e) = hooks::set_program(obj, "on_tick", source.to_string()) {
            return format!("{}\r\n", e);
        }
        // A global script is exactly `@program <ref>/on_tick = ...` with
        // friendlier ergonomics (see Stage 2) — a human writing one is
        // authoring, same as `@program`/`@lib`, so it gets a version too.
        self.record_program_version(&ref_id, "on_tick", source, Some(actor_ref));
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
        let mut scripts: Vec<&GameObject> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == Kind::Code && o.programs.contains_key("on_tick"))
            .collect();
        if scripts.is_empty() {
            return "No global scripts.\r\n".to_string();
        }
        scripts.sort_by(|a, b| a.key.cmp(&b.key));
        let mut out = "\r\nGlobal scripts:\r\n".to_string();
        for obj in scripts {
            let program = &obj.programs["on_tick"];
            let status = if program.enabled { "on" } else { "off" };
            let interval = obj
                .attrs
                .get("tick_interval")
                .and_then(|v| v.as_u64())
                .unwrap_or(1);
            let state_keys = program.state.len();
            out.push_str(&format!(
                "  {} [{}] interval={}  state_keys={}\r\n",
                obj.key, status, interval, state_keys
            ));
        }
        out
    }

    fn cmd_rmscript(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let name = args.trim();
        if name.is_empty() {
            return "Usage: @rmscript <name>\r\n".to_string();
        }
        match self.find_code_object_ref(name, "on_tick") {
            Some(ref_id) => {
                let obj = self.world.get_mut(&ref_id).unwrap();
                hooks::remove_program(obj, "on_tick");
                if obj.programs.is_empty() {
                    self.world.objects.remove(&ref_id);
                }
                self.record_program_tombstone(&ref_id, "on_tick", Some(actor_ref));
                format!("Script '{}' removed.\r\n", name)
            }
            None => format!("No script named '{}'.\r\n", name),
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
        match self.find_code_object_ref(name, "on_tick") {
            Some(ref_id) => {
                let obj = self.world.get_mut(&ref_id).unwrap();
                obj.attrs.insert("tick_interval".into(), serde_json::json!(interval));
                format!("Script '{}' interval set to {} tick(s).\r\n", name, interval)
            }
            None => format!("No script named '{}'.\r\n", name),
        }
    }

    // -- Library commands --
    //
    // A library is a `Kind::Code` object carrying a `lib_<name>` Program,
    // loadable from any Program as `require("<name>")` — see
    // `crate::softcode::mod::install_require` and
    // docs/plans/program-authoring.md Stage 2.

    fn cmd_lib(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @lib <name> = <luau source>
        let (name, source) = match args.split_once('=') {
            Some((n, s)) => (n.trim(), s.trim()),
            None => return "Usage: @lib <name> = <luau source>\r\n".to_string(),
        };
        if name.is_empty() || source.is_empty() {
            return "Usage: @lib <name> = <luau source>\r\n".to_string();
        }
        if self.softcode.is_shipped_module(name) {
            return format!(
                "'{}' is a shipped module — choose a different name.\r\n",
                name
            );
        }
        if let Err(e) = self.softcode.check_syntax(source) {
            return format!("Syntax error: {}\r\n", e);
        }
        let hook = format!("lib_{}", name);
        let existing_ref = self.find_code_object_ref(name, &hook);
        let is_new = existing_ref.is_none();
        let ref_id = existing_ref.unwrap_or_else(|| {
            let ref_id = self.world.next_dbref();
            self.world.add_object(GameObject::new(&ref_id, name, Kind::Code));
            ref_id
        });
        let obj = self.world.get_mut(&ref_id).unwrap();
        if let Err(e) = hooks::set_program(obj, &hook, source.to_string()) {
            return format!("{}\r\n", e);
        }
        self.softcode.invalidate_module_cache();
        self.record_program_version(&ref_id, &hook, source, Some(actor_ref));
        if is_new {
            format!("Library '{}' created — require(\"{}\").\r\n", name, name)
        } else {
            format!("Library '{}' updated.\r\n", name)
        }
    }

    fn cmd_libs(&self, session_id: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let mut libs: Vec<(&str, &ProgramRecord)> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == Kind::Code)
            .flat_map(|o| o.programs.values().map(move |p| (o.key.as_str(), p)))
            .filter(|(_, p)| p.hook.starts_with("lib_"))
            .collect();
        if libs.is_empty() {
            return "No libraries.\r\n".to_string();
        }
        libs.sort_by_key(|(key, _)| *key);
        let mut out = "\r\nLibraries:\r\n".to_string();
        for (key, program) in libs {
            let name = program.hook.strip_prefix("lib_").unwrap_or(&program.hook);
            let status = if program.enabled { "on" } else { "off" };
            out.push_str(&format!("  {} [{}] (object key: {})\r\n", name, status, key));
        }
        out
    }

    fn cmd_rmlib(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let name = args.trim();
        if name.is_empty() {
            return "Usage: @rmlib <name>\r\n".to_string();
        }
        let hook = format!("lib_{}", name);
        match self.find_code_object_ref(name, &hook) {
            Some(ref_id) => {
                let obj = self.world.get_mut(&ref_id).unwrap();
                hooks::remove_program(obj, &hook);
                if obj.programs.is_empty() {
                    self.world.objects.remove(&ref_id);
                }
                self.softcode.invalidate_module_cache();
                self.record_program_tombstone(&ref_id, &hook, Some(actor_ref));
                format!("Library '{}' removed.\r\n", name)
            }
            None => format!("No library named '{}'.\r\n", name),
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
        let ink_files = crate::loader::load_ink_files(game_path);
        for source in ink_files.values() {
            if let Err(e) = self.softcode.ink_runtime().borrow_mut().compile(source) {
                tracing::warn!(error = %e, "Failed to pre-compile ink file on reload");
            }
        }
        match crate::loader::load_game_dir(game_path, &mut self.world, &self.file_hashes) {
            Ok(result) => {
                if let Some(ref_id) = result.key_map.get(&self.spawn_room) {
                    self.spawn_room_ref = ref_id.clone();
                }
                self.file_hashes = result.file_hashes;
                // Persist so the next boot can skip what this reload already
                // saw, rather than re-reading the whole directory.
                if let Err(e) = self.db.save_file_hashes(&self.file_hashes) {
                    tracing::warn!(error = %e, "Failed to persist file hashes");
                }
                for (obj_ref, hook, source) in &result.installed_programs {
                    self.record_program_version(obj_ref, hook, source, None);
                }
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

                let playing: Vec<(String, String)> = self.sessions.iter()
                    .filter_map(|(sid, s)| match &s.state {
                        SessionState::Playing { actor_ref, .. } => {
                            Some((sid.clone(), actor_ref.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                for (sid, actor) in playing {
                    self.send_commands(&sid, &actor);
                }

                msg
            }
            Err(e) => format!("Reload error: {}\r\n", e),
        }
    }

    /// `@import <path> [--dry-run]` — install a TOML+`.luau` bundle into the
    /// DB. See docs/plans/program-authoring.md Stage 4 and
    /// `crate::import_export`. `<path>` is resolved on the server's own
    /// filesystem (relative to the server process's working directory, or
    /// absolute) — same convention `@test`'s path argument uses, and the
    /// same reasoning `game_dir` itself follows.
    fn cmd_import(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        let (path, dry_run) = match args.trim().strip_suffix("--dry-run") {
            Some(rest) => (rest.trim(), true),
            None => (args.trim(), false),
        };
        if path.is_empty() {
            return "Usage: @import <path> [--dry-run]\r\n".to_string();
        }
        let actor_ref = self.session_account_id(session_id);
        let bundle_path = std::path::Path::new(path);
        match crate::import_export::import_bundle(
            bundle_path,
            &mut self.world,
            &self.db,
            dry_run,
            actor_ref.as_deref(),
        ) {
            Ok(report) => {
                if !dry_run {
                    self.reload_map_sources_from_db();
                }
                crate::import_export::render_import_report(&report, dry_run, path)
            }
            Err(e) => format!("Import error: {}\r\n", e),
        }
    }

    /// `@export <path>` — emit DB-owned (`FILE_KEY_ATTR`-carrying) content
    /// back to files under `<path>` on the server's filesystem. See
    /// docs/plans/program-authoring.md Stage 4.
    fn cmd_export(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        let path = args.trim();
        if path.is_empty() {
            return "Usage: @export <path>\r\n".to_string();
        }
        let export_path = std::path::Path::new(path);
        match crate::import_export::export_bundle(export_path, &mut self.world) {
            Ok(mut report) => {
                match crate::import_export::export_file_sources(export_path, &self.file_sources) {
                    Ok(written) => report.maps_written = written,
                    Err(e) => return format!("Export error: {}\r\n", e),
                }
                crate::import_export::render_export_report(&report, path)
            }
            Err(e) => format!("Export error: {}\r\n", e),
        }
    }

    fn cmd_dialogue(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let args = args.trim();
        if args.is_empty() {
            return concat!(
                "Usage: @dialogue <ref> [subcommand]\r\n",
                "  @dialogue <ref>           - Show current ink source\r\n",
                "  @dialogue <ref> edit       - Enter multi-line editor\r\n",
                "  @dialogue <ref> test       - Compile and report errors\r\n",
                "  @dialogue <ref> clear      - Remove dialogue source\r\n",
                "  @dialogue <ref> export     - Show raw .ink source\r\n",
            )
            .to_string();
        }

        let (target_input, subcommand) = match args.split_once(' ') {
            Some((t, s)) => (t.trim(), s.trim()),
            None => (args, "show"),
        };

        let target_ref = if target_input == "here" {
            match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
                Some(r) => r,
                None => return "You're nowhere.\r\n".to_string(),
            }
        } else if self.world.get(target_input).is_some() {
            target_input.to_string()
        } else {
            let target_lower = target_input.to_lowercase();
            let room_ref = self
                .world
                .get(actor_ref)
                .and_then(|a| a.location_ref.clone());
            let candidates: Vec<&crate::world::GameObject> = room_ref
                .as_deref()
                .map(|r| self.world.objects_in(r))
                .unwrap_or_default()
                .into_iter()
                .chain(self.world.objects_in(actor_ref))
                .collect();
            match candidates.iter().find(|o| {
                o.key.to_lowercase().contains(&target_lower)
                    || o.display_name().to_lowercase().contains(&target_lower)
            }) {
                Some(o) => o.ref_id.clone(),
                None => return format!("Cannot find '{}'.\r\n", target_input),
            }
        };

        match subcommand {
            "show" => {
                let source = self
                    .world
                    .get(&target_ref)
                    .and_then(|o| o.attrs.get("_ink_source"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                match source {
                    Some(s) => {
                        let lines: Vec<&str> = s.lines().collect();
                        let preview = if lines.len() > 10 {
                            format!(
                                "{}\r\n  ... ({} more lines)\r\n",
                                lines[..10].join("\r\n"),
                                lines.len() - 10
                            )
                        } else {
                            s.replace('\n', "\r\n")
                        };
                        format!("Ink source for {}:\r\n{}\r\n", target_ref, preview)
                    }
                    None => format!("{} has no dialogue.\r\n", target_ref),
                }
            }
            "edit" => {
                if let Some(actor) = self.world.get_mut(actor_ref) {
                    actor.attrs.insert(
                        "_ink_editing".into(),
                        serde_json::json!(target_ref),
                    );
                    actor
                        .attrs
                        .insert("_ink_buffer".into(), serde_json::json!(""));
                }
                "Enter .ink source. Type '.' on a line by itself to finish, '@abort' to cancel:\r\n"
                    .to_string()
            }
            "test" => {
                let source = self
                    .world
                    .get(&target_ref)
                    .and_then(|o| o.attrs.get("_ink_source"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                match source {
                    Some(s) => {
                        match self.softcode.ink_runtime().borrow_mut().compile(&s) {
                            Ok(_) => format!("{}: compiles OK.\r\n", target_ref),
                            Err(e) => format!("{}: compile error:\r\n{}\r\n", target_ref, e),
                        }
                    }
                    None => format!("{} has no dialogue to test.\r\n", target_ref),
                }
            }
            "clear" => {
                if let Some(obj) = self.world.get_mut(&target_ref) {
                    obj.attrs.remove("_ink_source");
                    obj.attrs.remove("_ink_errors");
                    obj.attrs
                        .retain(|k, _| !k.starts_with("_ink_state_"));
                }
                format!("Dialogue cleared from {}.\r\n", target_ref)
            }
            "export" => {
                let source = self
                    .world
                    .get(&target_ref)
                    .and_then(|o| o.attrs.get("_ink_source"))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                match source {
                    Some(s) => format!("{}\r\n", s.replace('\n', "\r\n")),
                    None => format!("{} has no dialogue.\r\n", target_ref),
                }
            }
            _ => format!("Unknown subcommand '{}'. See @dialogue for usage.\r\n", subcommand),
        }
    }

    fn handle_ink_editor_input(&mut self, session_id: &str, actor_ref: &str, input: &str) {
        let editing_ref = self
            .world
            .get(actor_ref)
            .and_then(|a| a.attrs.get("_ink_editing"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let editing_ref = match editing_ref {
            Some(r) => r,
            None => return,
        };

        if input == "." {
            let buffer = self
                .world
                .get(actor_ref)
                .and_then(|a| a.attrs.get("_ink_buffer"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.remove("_ink_editing");
                actor.attrs.remove("_ink_buffer");
            }
            if buffer.is_empty() {
                self.send(session_id, "Empty source, dialogue not saved.\r\n");
                return;
            }
            match self.softcode.ink_runtime().borrow_mut().compile(&buffer) {
                Ok(_) => {
                    if let Some(obj) = self.world.get_mut(&editing_ref) {
                        obj.attrs
                            .insert("_ink_source".into(), serde_json::json!(buffer));
                        obj.attrs.remove("_ink_errors");
                    }
                    self.send(session_id, "Dialogue saved and validated.\r\n");
                }
                Err(e) => {
                    if let Some(obj) = self.world.get_mut(&editing_ref) {
                        obj.attrs
                            .insert("_ink_source".into(), serde_json::json!(buffer));
                        obj.attrs
                            .insert("_ink_errors".into(), serde_json::json!(e));
                    }
                    self.send(
                        session_id,
                        &format!("Dialogue saved with compile errors:\r\n{}\r\n", e),
                    );
                }
            }
        } else if input == "@abort" {
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.remove("_ink_editing");
                actor.attrs.remove("_ink_buffer");
            }
            self.send(session_id, "Editing cancelled.\r\n");
        } else {
            if let Some(actor) = self.world.get_mut(actor_ref) {
                let current = actor
                    .attrs
                    .get("_ink_buffer")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let new_buffer = if current.is_empty() {
                    input.to_string()
                } else {
                    format!("{}\n{}", current, input)
                };
                actor
                    .attrs
                    .insert("_ink_buffer".into(), serde_json::json!(new_buffer));
            }
        }
    }

    /// `@eval` — run a one-shot Luau script against the live world. This is
    /// Evennia's `@batchcode` / MUSH's paste-a-command-script: the mechanism
    /// for fixing up existing world data when code changes, not a hook on
    /// any object. Admin-only, because it is arbitrary code with the full
    /// write API — the most dangerous command in the system.
    fn cmd_eval(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        let source = args.trim();
        if source.is_empty() {
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor
                    .attrs
                    .insert("_eval_editing".into(), serde_json::json!(true));
                actor
                    .attrs
                    .insert("_eval_buffer".into(), serde_json::json!(""));
            }
            return "Enter Luau source. Type '.' on a line by itself to run it, '@abort' to cancel:\r\n"
                .to_string();
        }
        self.eval_and_report(actor_ref, source)
    }

    fn handle_eval_editor_input(&mut self, session_id: &str, actor_ref: &str, input: &str) {
        if input == "." {
            let buffer = self
                .world
                .get(actor_ref)
                .and_then(|a| a.attrs.get("_eval_buffer"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.remove("_eval_editing");
                actor.attrs.remove("_eval_buffer");
            }
            if buffer.is_empty() {
                self.send(session_id, "Empty source, nothing run.\r\n");
                return;
            }
            let output = self.eval_and_report(actor_ref, &buffer);
            self.send(session_id, &output);
        } else if input == "@abort" {
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.remove("_eval_editing");
                actor.attrs.remove("_eval_buffer");
            }
            self.send(session_id, "Eval cancelled.\r\n");
        } else if let Some(actor) = self.world.get_mut(actor_ref) {
            let current = actor
                .attrs
                .get("_eval_buffer")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let new_buffer = if current.is_empty() {
                input.to_string()
            } else {
                format!("{}\n{}", current, input)
            };
            actor
                .attrs
                .insert("_eval_buffer".into(), serde_json::json!(new_buffer));
        }
    }

    /// Run `source` under a [`Budget`] via [`SoftcodeRuntime::run_eval`],
    /// apply whatever [`softcode::Intent`]s it queued through the normal
    /// batch path (see ADR 0001 — `@eval` gets no shortcut around it), and
    /// render the outcome for the caller. Errors are reported as text, never
    /// propagated — a bad `@eval` must not be able to take the engine down.
    fn eval_and_report(&mut self, actor_ref: &str, source: &str) -> String {
        tracing::info!(actor = %actor_ref, source = %source, "@eval");

        let room_ref = self.world.get(actor_ref).and_then(|a| a.location_ref.clone());
        let dbref_counter = Rc::new(Cell::new(self.world.next_id));
        let result = self.softcode.run_eval(
            &self.world,
            source,
            actor_ref,
            room_ref.as_deref(),
            Budget::for_eval(),
            Rc::clone(&dbref_counter),
            &self.themes,
            &self.map_templates,
            &self.scheduled_hooks,
            self.tick_count,
        );

        let result = match result {
            Ok(r) => r,
            Err(e) => return format!("Eval error: {}\r\n", e),
        };

        let write_count = result.batch.len();
        let effects = match softcode::apply_batch(&mut self.world, &result.batch) {
            Ok(effects) => effects,
            Err(e) => return format!("Eval error applying world changes: {}\r\n", e),
        };
        self.world.next_id = dbref_counter.get();
        self.invalidate_libs_touched_by(&result.batch);
        self.deliver_effects(&effects, actor_ref);

        let mut msg = String::new();
        if let Some(returned) = result.returned {
            msg.push_str(&format!("=> {}\r\n", returned));
        }
        msg.push_str(&format!(
            "OK. {} write{} applied.\r\n",
            write_count,
            if write_count == 1 { "" } else { "s" }
        ));
        msg
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
                && ar == actor_ref
                    && let Some(acct) = self.accounts.get(account_id) {
                        return acct.scope_labels().iter().map(|s| s.to_string()).collect();
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
                if let Ok(run) = self.fire_hook(target_ref, "can_look", actor_ref, Some(&room_ref), None)
                    && run.denied {
                        return if run.emitted_to_actor {
                            String::new()
                        } else {
                            "You can't see that.\r\n".to_string()
                        };
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
            if let SessionState::Playing { actor_ref: ar, .. } = &session.state
                && let Some(other_actor) = self.world.get(ar)
                    && other_actor.location_ref.as_deref() == Some(&room_ref) {
                        let _ = session.tx.send(others_msg.clone());
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

    fn cmd_charlist(&mut self, session_id: &str) -> String {
        let account_id = match self.session_account_id(session_id) {
            Some(id) => id,
            None => return "Not logged in.\r\n".to_string(),
        };
        let characters = match self.accounts.get(&account_id) {
            Some(a) => a.characters.clone(),
            None => return "Account not found.\r\n".to_string(),
        };
        if characters.is_empty() {
            return "You have no characters.\r\n".to_string();
        }
        let active = self
            .accounts
            .get(&account_id)
            .and_then(|a| a.active_character.clone())
            .unwrap_or_default();
        let mut out = "Your characters:\r\n".to_string();
        for ref_id in &characters {
            let name = self
                .world
                .get(ref_id)
                .map(|o| o.display_name().to_string())
                .unwrap_or_else(|| ref_id.clone());
            let location = self
                .world
                .get(ref_id)
                .and_then(|o| o.location_ref.as_ref())
                .and_then(|loc| self.world.get(loc))
                .map(|r| r.display_name().to_string())
                .unwrap_or_else(|| "unknown".into());
            let marker = if *ref_id == active { " [active]" } else { "" };
            out.push_str(&format!("  {} ({}){}\r\n", name, location, marker));
        }
        out
    }

    fn cmd_charcreate(&mut self, session_id: &str, args: &str) -> String {
        let account_id = match self.session_account_id(session_id) {
            Some(id) => id,
            None => return "Not logged in.\r\n".to_string(),
        };
        let name = args.trim();
        if name.is_empty() {
            return "Usage: @charcreate <name>\r\n".to_string();
        }
        if name.len() < 3 {
            return "Name must be at least 3 characters.\r\n".to_string();
        }
        if name.len() > 20 {
            return "Name must be 20 characters or fewer.\r\n".to_string();
        }

        let max = self
            .accounts
            .get(&account_id)
            .and_then(|a| a.max_characters)
            .unwrap_or(self.max_characters);
        let current = self
            .accounts
            .get(&account_id)
            .map(|a| a.characters.len())
            .unwrap_or(0);
        if current as u8 >= max {
            return format!("You already have the maximum of {} characters.\r\n", max);
        }

        let spawn_room_ref = self.spawn_room_ref.clone();
        let ref_id = self.world.next_dbref();
        let player = GameObject::new(&ref_id, name, Kind::Player)
            .with_title(name)
            .with_description("A traveler.")
            .with_location(&spawn_room_ref);
        self.world.add_object(player);
        self.fire_on_create(&ref_id);

        if let Some(account) = self.accounts.get_mut(&account_id) {
            account.characters.push(ref_id.clone());
        }
        self.db.save_accounts(&self.accounts).ok();

        format!("Character '{}' created. Use @charswitch {} to play as them.\r\n", name, name)
    }

    fn cmd_charswitch(&mut self, session_id: &str, args: &str) -> String {
        let account_id = match self.session_account_id(session_id) {
            Some(id) => id,
            None => return "Not logged in.\r\n".to_string(),
        };
        let name = args.trim().to_lowercase();
        if name.is_empty() {
            return "Usage: @charswitch <name>\r\n".to_string();
        }

        let (current_ref, target_ref) = {
            let account = match self.accounts.get(&account_id) {
                Some(a) => a,
                None => return "Account not found.\r\n".to_string(),
            };
            let current = account.active_character.clone().unwrap_or_default();
            let target = account
                .characters
                .iter()
                .find(|r| {
                    self.world
                        .get(r.as_str())
                        .map(|o| o.display_name().to_lowercase().contains(&name) || o.key.to_lowercase().contains(&name))
                        .unwrap_or(false)
                })
                .cloned();
            (current, target)
        };

        let target_ref = match target_ref {
            Some(r) => r,
            None => return format!("No character matching '{}'.\r\n", args.trim()),
        };

        if target_ref == current_ref {
            return "You're already playing that character.\r\n".to_string();
        }

        // Disconnect current character
        let room = self.world.get(&current_ref).and_then(|o| o.location_ref.clone());
        let _ = self.fire_hook(&current_ref, "on_disconnect", &current_ref, room.as_deref(), None);
        if let Some(room_ref) = &room {
            let _ = self.fire_hook(room_ref, "on_disconnect", &current_ref, Some(room_ref), None);
        }
        self.fire_global_hooks("on_disconnect", &current_ref, room.as_deref(), None);
        if let Some(obj) = self.world.get_mut(&current_ref) {
            obj.tags.insert(crate::world::Tag {
                category: "system".to_string(),
                key: "offline".to_string(),
            });
        }

        let username = self
            .accounts
            .get(&account_id)
            .map(|a| a.username.clone())
            .unwrap_or_default();
        self.enter_world(session_id, &username, &target_ref, &account_id);
        String::new()
    }

    fn cmd_chardelete(&mut self, session_id: &str, args: &str) -> String {
        let account_id = match self.session_account_id(session_id) {
            Some(id) => id,
            None => return "Not logged in.\r\n".to_string(),
        };
        let name = args.trim().to_lowercase();
        if name.is_empty() {
            return "Usage: @chardelete <name>\r\n".to_string();
        }

        let (active, target_ref) = {
            let account = match self.accounts.get(&account_id) {
                Some(a) => a,
                None => return "Account not found.\r\n".to_string(),
            };
            let active = account.active_character.clone().unwrap_or_default();
            let target = account
                .characters
                .iter()
                .find(|r| {
                    self.world
                        .get(r.as_str())
                        .map(|o| o.display_name().to_lowercase() == name || o.key.to_lowercase() == name)
                        .unwrap_or(false)
                })
                .cloned();
            (active, target)
        };

        let target_ref = match target_ref {
            Some(r) => r,
            None => return format!("No character matching '{}'.\r\n", args.trim()),
        };

        if target_ref == active {
            return "You can't delete your currently active character. Switch first.\r\n".to_string();
        }

        let char_name = self
            .world
            .get(&target_ref)
            .map(|o| o.display_name().to_string())
            .unwrap_or_default();

        if let Some(account) = self.accounts.get_mut(&account_id) {
            account.characters.retain(|r| r != &target_ref);
        }
        self.world.objects.remove(&target_ref);
        self.db.save_accounts(&self.accounts).ok();

        format!("Character '{}' deleted.\r\n", char_name)
    }

    fn cmd_puppet(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        let target_ref = args.trim();
        if target_ref.is_empty() {
            return "Usage: @puppet <ref>\r\n".to_string();
        }

        let target = match self.world.get(target_ref) {
            Some(o) => o,
            None => return format!("No object with ref '{}'.\r\n", target_ref),
        };

        if target.kind == Kind::Player {
            return "You can't puppet a player.\r\n".to_string();
        }

        let is_admin = self.session_has_scope(session_id, Scope::Admin);
        let can_puppet_own = self.session_has_scope(session_id, Scope::Builder)
            || self.session_has_scope(session_id, Scope::Puppeteer);
        let is_owner = target.owner_ref.as_deref() == Some(actor_ref);

        if !is_admin && !(can_puppet_own && is_owner) {
            return "Permission denied. You need builder/puppeteer scope and ownership, or admin.\r\n".to_string();
        }

        let target_name = target.display_name().to_string();

        if let Some(session) = self.sessions.get_mut(session_id)
            && let SessionState::Playing { puppet_ref, .. } = &mut session.state {
                *puppet_ref = Some(target_ref.to_string());
            }

        format!("You are now puppeting {}. Use @unpuppet to return.\r\n", target_name)
    }

    fn cmd_unpuppet(&mut self, session_id: &str) -> String {
        let was_puppeting = if let Some(session) = self.sessions.get_mut(session_id) {
            if let SessionState::Playing { puppet_ref, .. } = &mut session.state {
                let was = puppet_ref.take();
                was.is_some()
            } else {
                false
            }
        } else {
            false
        };

        if was_puppeting {
            "You return to your own body.\r\n".to_string()
        } else {
            "You aren't puppeting anything.\r\n".to_string()
        }
    }

    fn cmd_maxchars(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied (admin only).\r\n".to_string();
        }
        let (username, count_str) = match args.split_once(' ') {
            Some((u, c)) => (u.trim(), c.trim()),
            None => return "Usage: @maxchars <username> <count>\r\n".to_string(),
        };
        let count: u8 = match count_str.parse() {
            Ok(n) if n >= 1 => n,
            _ => return "Count must be a positive number.\r\n".to_string(),
        };
        let account_id = match self.accounts.get_id_by_username(username) {
            Some(id) => id,
            None => return format!("No account '{}'.\r\n", username),
        };
        if let Some(account) = self.accounts.get_mut(&account_id) {
            account.max_characters = Some(count);
        }
        self.db.save_accounts(&self.accounts).ok();
        format!("Max characters for '{}' set to {}.\r\n", username, count)
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

    // -- @eval --
    //
    // Unlike the tests above, these drive the Engine directly and
    // synchronously rather than through the `EngineMessage` channel: nothing
    // `cmd_eval`/`handle_input` touch requires the tokio runtime (the Luau
    // VM runs in-process), so a plain `Engine::new` plus a hand-built
    // `Session` is enough, without spawning `engine.run()`.

    /// Build an `Engine` with one logged-in, playing session for a fresh
    /// player character, granted `Scope::Builder` and, if `admin` is true,
    /// also `Scope::Admin`. Returns the engine, the session id, and the
    /// player's ref.
    fn test_engine_with_session(admin: bool) -> (Engine, String, String) {
        let db = crate::db::Database::open(Path::new(":memory:")).unwrap();
        let config = Config::default();
        let (_tx, rx) = mpsc::unbounded_channel();
        let mut engine = Engine::new(rx, db, &config);

        // The first account ever created on a fresh store is auto-granted
        // every scope (see CLAUDE.md), so explicitly strip Admin back off
        // for the non-admin case rather than relying on a second account.
        let account = engine.accounts.create("eval_tester", "password123").unwrap();
        let account_id = account.id.clone();
        engine.accounts.grant_scope(&account_id, Scope::Builder);
        if admin {
            engine.accounts.grant_scope(&account_id, Scope::Admin);
        } else {
            engine.accounts.revoke_scope(&account_id, Scope::Admin);
        }

        let spawn_room_ref = engine.spawn_room_ref.clone();
        let actor_ref = engine.world.next_dbref();
        let actor = GameObject::new(&actor_ref, "tester", Kind::Player)
            .with_title("Tester")
            .with_location(&spawn_room_ref);
        engine.world.add_object(actor);

        let (client_tx, _client_rx) = mpsc::unbounded_channel();
        let session_id = "eval-test-session".to_string();
        engine.sessions.insert(
            session_id.clone(),
            Session {
                tx: client_tx,
                state: SessionState::Playing {
                    actor_ref: actor_ref.clone(),
                    account_id,
                    puppet_ref: None,
                },
            },
        );

        (engine, session_id, actor_ref)
    }

    #[test]
    fn eval_denies_non_admin() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(false);
        let out = engine.cmd_eval(&session_id, &actor_ref, "return 1");
        assert_eq!(out, "Permission denied.\r\n");
    }

    #[test]
    fn eval_reports_syntax_error_without_panicking() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let out = engine.cmd_eval(&session_id, &actor_ref, "this is not valid luau (((");
        assert!(out.contains("Eval error"), "unexpected output: {}", out);
    }

    #[test]
    fn eval_reports_runtime_error_without_panicking() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let out = engine.cmd_eval(&session_id, &actor_ref, r#"error("kaboom")"#);
        assert!(out.contains("Eval error"), "unexpected output: {}", out);
        assert!(out.contains("kaboom"), "unexpected output: {}", out);
    }

    #[test]
    fn eval_world_writes_land() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let target = engine
            .world
            .get(&actor_ref)
            .and_then(|a| a.location_ref.clone())
            .expect("actor should be in a room");

        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(r#"set_attr("{}", "eval_touched", true)"#, target),
        );
        assert!(out.starts_with("OK."), "unexpected output: {}", out);
        assert_eq!(
            engine.world.get(&target).unwrap().attrs["eval_touched"],
            true
        );
    }

    #[test]
    fn eval_reports_returned_value() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let out = engine.cmd_eval(&session_id, &actor_ref, "return 1 + 1");
        assert!(out.contains("=> 2"), "unexpected output: {}", out);
    }

    #[test]
    fn eval_all_objects_matches_world_object_count() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let expected = engine.world.objects.len();
        let out = engine.cmd_eval(&session_id, &actor_ref, "return #all_objects()");
        assert!(
            out.contains(&format!("=> {}", expected)),
            "unexpected output: {}",
            out
        );
    }

    #[test]
    fn eval_multiline_editor_buffers_and_runs_on_dot() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let target = engine
            .world
            .get(&actor_ref)
            .and_then(|a| a.location_ref.clone())
            .expect("actor should be in a room");

        // Bare `@eval` enters the multi-line editor.
        engine.handle_input(&session_id, "@eval");
        assert!(
            engine
                .world
                .get(&actor_ref)
                .unwrap()
                .attrs
                .contains_key("_eval_editing")
        );

        engine.handle_input(&session_id, &format!(r#"set_attr("{}", "line_one", 1)"#, target));
        engine.handle_input(&session_id, &format!(r#"set_attr("{}", "line_two", 2)"#, target));
        engine.handle_input(&session_id, ".");

        // Editor state is cleared once the buffer runs.
        assert!(
            !engine
                .world
                .get(&actor_ref)
                .unwrap()
                .attrs
                .contains_key("_eval_editing")
        );
        let attrs = &engine.world.get(&target).unwrap().attrs;
        assert_eq!(attrs["line_one"], 1);
        assert_eq!(attrs["line_two"], 2);
    }

    #[test]
    fn eval_multiline_editor_abort_discards_buffer() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        engine.handle_input(&session_id, "@eval");
        engine.handle_input(&session_id, "should_never_run()");
        engine.handle_input(&session_id, "@abort");

        assert!(
            !engine
                .world
                .get(&actor_ref)
                .unwrap()
                .attrs
                .contains_key("_eval_editing")
        );
    }

    // -- Stage 2: Kind::Code (docs/plans/program-authoring.md) --

    /// A `Kind::Code` object must never be observable through room
    /// contents, `look`, `examine`, `inventory`, or `get` — the exclusion
    /// property the Kind exists to buy. Placed with a `location_ref`
    /// matching both the room and the actor, to prove it's excluded no
    /// matter where it ends up, not just because nothing ever gives it a
    /// location in practice.
    #[test]
    fn code_object_excluded_from_room_inventory_examine_and_get() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.world.get(&actor_ref).unwrap().location_ref.clone().unwrap();

        let code_ref = engine.world.next_dbref();
        let code_obj = GameObject::new(&code_ref, "secret_script", Kind::Code)
            .with_title("Secret Script")
            .with_location(&room_ref);
        engine.world.add_object(code_obj);

        let look = engine.cmd_look(&actor_ref, "");
        assert!(!look.contains("Secret Script"), "look should not reveal a Code object: {}", look);

        let examine = commands::do_examine(&engine.world, &actor_ref, "secret_script");
        assert!(examine.contains("don't see"), "examine should not find a Code object: {}", examine);

        let get = engine.cmd_get(&actor_ref, "secret_script");
        assert!(get.contains("don't see"), "get should not find a Code object: {}", get);

        // Even sitting "in" the actor's inventory, it's excluded.
        engine.world.get_mut(&code_ref).unwrap().location_ref = Some(actor_ref.clone());
        let inv = commands::do_inventory(&engine.world, &actor_ref);
        assert!(!inv.contains("Secret Script"), "inventory should not reveal a Code object: {}", inv);
    }

    /// A `Kind::Code` object's `on_tick` Program runs through the same
    /// per-object tick loop as any other object — no separate scheduler —
    /// and its state persists between ticks. This is the shape
    /// `db::Database::load_world`'s legacy-`scripts`-table migration
    /// produces, so this doubles as proof a migrated script keeps ticking.
    #[test]
    fn code_object_on_tick_ticks_and_persists_state() {
        let (mut engine, _session_id, _actor_ref) = test_engine_with_session(true);
        let ref_id = engine.world.next_dbref();
        let mut obj = GameObject::new(&ref_id, "weather", Kind::Code);
        obj.attrs.insert("tick_interval".into(), serde_json::json!(1));
        hooks::set_program(
            &mut obj,
            "on_tick",
            "function on_tick(this, state, room) state.ticks = (state.ticks or 0) + 1 end".into(),
        )
        .unwrap();
        engine.world.add_object(obj);

        engine.do_tick();
        engine.do_tick();

        let ticks = engine.world.get(&ref_id).unwrap().programs["on_tick"]
            .state
            .get("ticks")
            .cloned();
        assert_eq!(ticks, Some(serde_json::json!(2)));
    }

    #[test]
    fn script_command_creates_ticking_code_object() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let out = engine.cmd_script(
            &session_id,
            &actor_ref,
            r#"weather = function on_tick(this, state, room) state.ticks = (state.ticks or 0) + 1 end"#,
        );
        assert!(out.contains("created"), "unexpected output: {}", out);

        let ref_id = engine
            .world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "weather")
            .unwrap()
            .ref_id
            .clone();

        engine.do_tick();
        assert_eq!(
            engine.world.get(&ref_id).unwrap().programs["on_tick"].state.get("ticks"),
            Some(&serde_json::json!(1))
        );
    }

    /// `@lib` creates a `Kind::Code` object whose `lib_<name>` Program is
    /// resolvable via `require("<name>")` from any other Program — proven
    /// here through `@eval`.
    #[test]
    fn lib_command_creates_requireable_library() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let out = engine.cmd_lib(
            &session_id,
            &actor_ref,
            r#"greet = local M = {} function M.hello() return "hi" end return M"#,
        );
        assert!(out.contains("created"), "unexpected output: {}", out);

        let eval_out = engine.cmd_eval(&session_id, &actor_ref, r#"return require("greet").hello()"#);
        assert!(eval_out.contains("hi"), "unexpected output: {}", eval_out);
    }

    /// Authoring a library whose name collides with a shipped module is
    /// refused at write time, from the `@lib` command.
    #[test]
    fn lib_command_refuses_shipped_module_name() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        engine.softcode.load_modules(HashMap::from([("str".to_string(), "return {}".to_string())]));

        let out = engine.cmd_lib(&session_id, &actor_ref, r#"str = return {}"#);
        assert!(out.contains("shipped module"), "unexpected output: {}", out);
        assert!(
            !engine.world.objects.values().any(|o| o.kind == Kind::Code && o.key == "str"),
            "no object should have been created"
        );
    }

    // -- Stage 3: Program versions (docs/plans/program-authoring.md) --

    #[test]
    fn program_command_records_a_version_with_author() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.world.get(&actor_ref).and_then(|a| a.location_ref.clone()).unwrap();

        let out = engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 1", room_ref));
        assert!(out.contains("installed"), "unexpected output: {}", out);

        let versions = engine.db.list_program_versions(&room_ref, "on_look").unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].author.as_deref(), Some(actor_ref.as_str()));
        assert!(versions[0].source.contains("return 1"));
        assert!(!versions[0].deleted);
    }

    #[test]
    fn program_command_resaving_identical_source_dedupes() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.world.get(&actor_ref).and_then(|a| a.location_ref.clone()).unwrap();

        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 1", room_ref));
        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 1", room_ref));

        let versions = engine.db.list_program_versions(&room_ref, "on_look").unwrap();
        assert_eq!(versions.len(), 1, "identical resave should not create a second version");
    }

    #[test]
    fn program_command_changed_source_creates_a_new_version() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.world.get(&actor_ref).and_then(|a| a.location_ref.clone()).unwrap();

        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 1", room_ref));
        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 2", room_ref));

        let versions = engine.db.list_program_versions(&room_ref, "on_look").unwrap();
        assert_eq!(versions.len(), 2);
        assert!(versions[0].source.contains("return 1"));
        assert!(versions[1].source.contains("return 2"));
    }

    #[test]
    fn rmprogram_records_a_tombstone() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.world.get(&actor_ref).and_then(|a| a.location_ref.clone()).unwrap();

        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 1", room_ref));
        let out = engine.cmd_rmprogram(&session_id, &actor_ref, &format!("{}/on_look", room_ref));
        assert!(out.contains("Removed"), "unexpected output: {}", out);

        let versions = engine.db.list_program_versions(&room_ref, "on_look").unwrap();
        assert_eq!(versions.len(), 2, "deletion should append a tombstone, not erase history");
        assert!(versions[1].deleted);
        assert_eq!(versions[1].author.as_deref(), Some(actor_ref.as_str()));
    }

    #[test]
    fn program_restore_writes_a_new_version_not_a_rewind() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.world.get(&actor_ref).and_then(|a| a.location_ref.clone()).unwrap();

        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 1", room_ref));
        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 2", room_ref));

        let out = engine.cmd_program_restore(&session_id, &actor_ref, &format!("{}/on_look 1", room_ref));
        assert!(out.contains("Restored"), "unexpected output: {}", out);

        let versions = engine.db.list_program_versions(&room_ref, "on_look").unwrap();
        assert_eq!(versions.len(), 3, "restore must append a new version, never rewind history");
        assert!(versions[0].source.contains("return 1"));
        assert!(versions[1].source.contains("return 2"));
        assert!(versions[2].source.contains("return 1"));
        assert_eq!(versions[2].author.as_deref(), Some(actor_ref.as_str()));

        // The live program now runs the restored source.
        assert!(engine.world.get(&room_ref).unwrap().programs["on_look"].source.contains("return 1"));
    }

    #[test]
    fn program_history_lists_versions_with_numbers() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.world.get(&actor_ref).and_then(|a| a.location_ref.clone()).unwrap();
        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 1", room_ref));
        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 2", room_ref));

        let out = engine.cmd_program_history(&session_id, &actor_ref, &format!("{}/on_look", room_ref));
        assert!(out.contains("History for"), "unexpected output: {}", out);
        assert!(out.contains("1  "), "expected version 1 listed: {}", out);
        assert!(out.contains("2  "), "expected version 2 listed: {}", out);
    }

    #[test]
    fn program_diff_reports_line_differences() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.world.get(&actor_ref).and_then(|a| a.location_ref.clone()).unwrap();
        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 1", room_ref));
        engine.cmd_program(&session_id, &actor_ref, &format!("{}/on_look = return 2", room_ref));

        let out = engine.cmd_program_diff(&session_id, &actor_ref, &format!("{}/on_look 1 2", room_ref));
        assert!(out.contains("-return 1"), "unexpected output: {}", out);
        assert!(out.contains("+return 2"), "unexpected output: {}", out);
    }

    /// `@script` is `@program <ref>/on_tick = ...` with friendlier
    /// ergonomics (Stage 2 collapsed global scripts into `Kind::Code`
    /// objects) — a human writing one is authoring, same as `@program`/
    /// `@lib`, so it must record a version too.
    #[test]
    fn script_command_records_a_version_with_author() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        engine.cmd_script(&session_id, &actor_ref, "weather = function on_tick(this, state, room) end");

        let ref_id = engine
            .world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "weather")
            .unwrap()
            .ref_id
            .clone();
        let versions = engine.db.list_program_versions(&ref_id, "on_tick").unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].author.as_deref(), Some(actor_ref.as_str()));
        assert!(!versions[0].deleted);
    }

    #[test]
    fn script_command_editing_creates_a_second_version() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        engine.cmd_script(&session_id, &actor_ref, "weather = function on_tick(this, state, room) end");
        engine.cmd_script(
            &session_id,
            &actor_ref,
            "weather = function on_tick(this, state, room) state.x = 1 end",
        );

        let ref_id = engine
            .world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "weather")
            .unwrap()
            .ref_id
            .clone();
        let versions = engine.db.list_program_versions(&ref_id, "on_tick").unwrap();
        assert_eq!(versions.len(), 2);
        assert!(versions[1].source.contains("state.x"));
    }

    #[test]
    fn rmscript_records_a_tombstone() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        engine.cmd_script(&session_id, &actor_ref, "weather = function on_tick(this, state, room) end");
        let ref_id = engine
            .world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "weather")
            .unwrap()
            .ref_id
            .clone();

        let out = engine.cmd_rmscript(&session_id, &actor_ref, "weather");
        assert!(out.contains("removed"), "unexpected output: {}", out);

        let versions = engine.db.list_program_versions(&ref_id, "on_tick").unwrap();
        assert_eq!(versions.len(), 2, "deletion should append a tombstone, not erase history");
        assert!(versions[1].deleted);
        assert_eq!(versions[1].author.as_deref(), Some(actor_ref.as_str()));
    }

    /// `@script-interval` changes an attr (`tick_interval`), not the
    /// program's source — it must not create a version.
    #[test]
    fn script_interval_does_not_record_a_version() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        engine.cmd_script(&session_id, &actor_ref, "weather = function on_tick(this, state, room) end");
        let ref_id = engine
            .world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "weather")
            .unwrap()
            .ref_id
            .clone();

        engine.cmd_script_interval(&session_id, "weather = 5");

        let versions = engine.db.list_program_versions(&ref_id, "on_tick").unwrap();
        assert_eq!(versions.len(), 1, "@script-interval must not create a new version");
    }

    /// `@program/history` and `@program/restore` must work against a
    /// `Kind::Code` object created through `@script`, not just an object
    /// built directly in a test — this exercises the same ref resolution
    /// (`resolve_ref_hook_path`) and dbref a real user would hit, not just
    /// the storage layer underneath.
    #[test]
    fn program_history_and_restore_work_on_a_script_created_object() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        engine.cmd_script(&session_id, &actor_ref, "weather = function on_tick(this, state, room) end");
        engine.cmd_script(
            &session_id,
            &actor_ref,
            "weather = function on_tick(this, state, room) state.x = 1 end",
        );

        let ref_id = engine
            .world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "weather")
            .unwrap()
            .ref_id
            .clone();

        let history = engine.cmd_program_history(&session_id, &actor_ref, &format!("{}/on_tick", ref_id));
        assert!(history.contains("History for"), "unexpected output: {}", history);
        // `display_author` resolves the stored ref to the actor's display
        // name ("Tester" — see `test_engine_with_session`), not the raw ref.
        assert!(history.contains("Tester"), "expected author display name in history: {}", history);

        let restore = engine.cmd_program_restore(&session_id, &actor_ref, &format!("{}/on_tick 1", ref_id));
        assert!(restore.contains("Restored"), "unexpected output: {}", restore);

        let versions = engine.db.list_program_versions(&ref_id, "on_tick").unwrap();
        assert_eq!(versions.len(), 3, "restore must append, not rewind");
        assert!(!versions[2].source.contains("state.x"), "restored version should be the first source, not the second");

        assert!(
            !engine.world.get(&ref_id).unwrap().programs["on_tick"].source.contains("state.x"),
            "the live program should now run the restored (first) source"
        );
    }

    #[test]
    fn lib_command_records_a_version_with_author() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        engine.cmd_lib(&session_id, &actor_ref, "greet = return {}");

        let ref_id = engine
            .world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "greet")
            .unwrap()
            .ref_id
            .clone();
        let versions = engine.db.list_program_versions(&ref_id, "lib_greet").unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].author.as_deref(), Some(actor_ref.as_str()));
    }

    #[test]
    fn rmlib_records_a_tombstone() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        engine.cmd_lib(&session_id, &actor_ref, "greet = return {}");
        let ref_id = engine
            .world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == "greet")
            .unwrap()
            .ref_id
            .clone();

        engine.cmd_rmlib(&session_id, &actor_ref, "greet");
        let versions = engine.db.list_program_versions(&ref_id, "lib_greet").unwrap();
        assert_eq!(versions.len(), 2);
        assert!(versions[1].deleted);
        assert_eq!(versions[1].author.as_deref(), Some(actor_ref.as_str()));
    }

    #[test]
    fn program_multiline_editor_installs_and_records_version() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.world.get(&actor_ref).and_then(|a| a.location_ref.clone()).unwrap();

        // Leaving the source blank after `=` enters the multi-line editor —
        // see docs/plans/program-authoring.md Stage 3's "Prerequisite:
        // multi-line authoring".
        engine.handle_input(&session_id, &format!("@program {}/on_look =", room_ref));
        assert!(
            engine.world.get(&actor_ref).unwrap().attrs.contains_key("_program_editing"),
            "blank source after '=' should enter the multi-line editor"
        );

        engine.handle_input(&session_id, "local x = 1");
        engine.handle_input(&session_id, "return x");
        engine.handle_input(&session_id, ".");

        assert!(!engine.world.get(&actor_ref).unwrap().attrs.contains_key("_program_editing"));
        let program = &engine.world.get(&room_ref).unwrap().programs["on_look"];
        assert!(program.source.contains("return x"), "unexpected source: {}", program.source);

        let versions = engine.db.list_program_versions(&room_ref, "on_look").unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].author.as_deref(), Some(actor_ref.as_str()));
    }

    #[test]
    fn program_multiline_editor_abort_discards_buffer_and_records_nothing() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.world.get(&actor_ref).and_then(|a| a.location_ref.clone()).unwrap();

        engine.handle_input(&session_id, &format!("@program {}/on_look =", room_ref));
        engine.handle_input(&session_id, "return 1");
        engine.handle_input(&session_id, "@abort");

        assert!(!engine.world.get(&actor_ref).unwrap().attrs.contains_key("_program_editing"));
        assert!(!engine.world.get(&room_ref).unwrap().programs.contains_key("on_look"));
        let versions = engine.db.list_program_versions(&room_ref, "on_look").unwrap();
        assert!(versions.is_empty());
    }

    /// The one negative case the plan is explicit about: softcode's
    /// `set_program()` (the `Intent::SetProgram` path, used to attach
    /// behaviour to procedurally generated objects) is instantiation, not
    /// authoring, and must never create a program version — see
    /// docs/plans/program-authoring.md Stage 3's "Instantiation is not
    /// authoring". Driven through `@eval` here since it exercises the exact
    /// same `Intent::SetProgram` → `apply_to` path a hook would.
    #[test]
    fn softcode_set_program_does_not_create_a_version() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let target = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&target, "spawned", Kind::Item));

        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(r#"set_program("{}", "on_use", "function on_use() end")"#, target),
        );
        assert!(out.starts_with("OK."), "unexpected output: {}", out);
        assert!(
            engine.world.get(&target).unwrap().programs.contains_key("on_use"),
            "set_program should still have written the Program itself"
        );

        let versions = engine.db.list_program_versions(&target, "on_use").unwrap();
        assert!(
            versions.is_empty(),
            "softcode set_program is instantiation, not authoring, and must not be versioned"
        );
    }

    #[test]
    fn format_epoch_secs_matches_known_values() {
        assert_eq!(format_epoch_secs(0), "1970-01-01 00:00:00 UTC");
        // 2023-11-14T22:13:20Z
        assert_eq!(format_epoch_secs(1_700_000_000), "2023-11-14 22:13:20 UTC");
        // 2000-02-29 exercises the leap-day branch of civil_from_days.
        assert_eq!(format_epoch_secs(951_782_400), "2000-02-29 00:00:00 UTC");
    }

    /// REST `SetProgram`/`RemoveProgram` record the acting account as
    /// author — see docs/plans/program-authoring.md Stage 3's "Author":
    /// "API SetProgram — account_id, resolved in the auth block".
    #[test]
    fn api_set_and_remove_program_record_account_as_author() {
        let db = crate::db::Database::open(Path::new(":memory:")).unwrap();
        let config = Config::default();
        let (_tx, rx) = mpsc::unbounded_channel();
        let mut engine = Engine::new(rx, db, &config);
        let account = engine.accounts.create("api_tester", "password123").unwrap();
        let account_id = account.id.clone();
        engine.accounts.grant_scope(&account_id, Scope::Builder);
        let token = "api-author-test-token".to_string();
        let token_hash = Engine::hash_token(&token);
        engine.api_tokens.insert(token_hash, TokenInfo {
            account_id: account_id.clone(),
            label: "test".to_string(),
            persistent: false,
            expires_at: None,
        });

        let ref_id = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&ref_id, "gem", Kind::Item));

        let resp = engine.handle_api_request(
            ApiRequest::SetProgram {
                ref_id: ref_id.clone(),
                hook: "on_get".into(),
                source: "function on_get(this, actor, room) end".into(),
            },
            Some(token.clone()),
        );
        assert!(resp.ok, "{:?}", resp.error);

        let versions = engine.db.list_program_versions(&ref_id, "on_get").unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].author.as_deref(), Some(account_id.as_str()));

        let resp = engine.handle_api_request(
            ApiRequest::RemoveProgram { ref_id: ref_id.clone(), hook: "on_get".into() },
            Some(token),
        );
        assert!(resp.ok, "{:?}", resp.error);

        let versions = engine.db.list_program_versions(&ref_id, "on_get").unwrap();
        assert_eq!(versions.len(), 2);
        assert!(versions[1].deleted);
        assert_eq!(versions[1].author.as_deref(), Some(account_id.as_str()));
    }

    /// Builds an in-memory `Engine` with an account holding `scopes` and a
    /// live API token for it — the setup every REST-auth test below needs.
    fn engine_with_api_token(scopes: &[Scope]) -> (Engine, String, String) {
        let db = crate::db::Database::open(Path::new(":memory:")).unwrap();
        let config = Config::default();
        let (_tx, rx) = mpsc::unbounded_channel();
        let mut engine = Engine::new(rx, db, &config);
        let account = engine.accounts.create("api_tester", "password123").unwrap();
        let account_id = account.id.clone();
        // The first account ever created on a fresh store is auto-granted
        // every scope (see CLAUDE.md and `test_engine_with_session`'s note
        // above), so strip back to exactly `scopes` rather than trusting
        // what `create` left in place.
        engine.accounts.revoke_scope(&account_id, Scope::Admin);
        engine.accounts.revoke_scope(&account_id, Scope::Builder);
        for scope in scopes {
            engine.accounts.grant_scope(&account_id, *scope);
        }
        let token = format!("test-token-{}", account_id);
        let token_hash = Engine::hash_token(&token);
        engine.api_tokens.insert(
            token_hash,
            TokenInfo { account_id: account_id.clone(), label: "test".to_string(), persistent: false, expires_at: None },
        );
        (engine, token, account_id)
    }

    // ---- map builder REST surface: the three invariants a future refactor
    // is most likely to loosen silently (see docs/plans/map-builder.md). ----

    #[test]
    fn valid_map_name_rejects_traversal_and_separators() {
        assert!(Engine::valid_map_name("iron_hills"));
        assert!(Engine::valid_map_name("a-b_1"));
        assert!(Engine::valid_map_name("Map2"));
        // Rejections make a traversal path unrepresentable, not blocklisted —
        // PutMap builds maps/<name>.toml, so any of these leaking through is a
        // write-anywhere hole.
        assert!(!Engine::valid_map_name(""));
        assert!(!Engine::valid_map_name(".."));
        assert!(!Engine::valid_map_name("a/b"));
        assert!(!Engine::valid_map_name("../evil"));
        assert!(!Engine::valid_map_name("a\\b"));
        assert!(!Engine::valid_map_name("a.b")); // '.' disallowed: no name.toml tricks
        assert!(!Engine::valid_map_name("/etc/passwd"));
        assert!(!Engine::valid_map_name(&"x".repeat(65)));
    }

    #[test]
    fn map_writes_require_admin_not_just_builder() {
        let toml = "[map]\nname = \"t\"\ngrid = \"\"\"\n.\n\"\"\"\n";
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let resp = engine.handle_api_request(
            ApiRequest::PutMap { name: "t".into(), toml: toml.into() },
            Some(token.clone()),
        );
        assert_eq!(resp.error.as_deref(), Some("Admin scope required"));
        let resp = engine
            .handle_api_request(ApiRequest::PutTerrain { toml: String::new() }, Some(token));
        assert_eq!(resp.error.as_deref(), Some("Admin scope required"));

        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder, Scope::Admin]);
        let resp = engine.handle_api_request(
            ApiRequest::PutMap { name: "t".into(), toml: toml.into() },
            Some(token),
        );
        assert!(resp.ok, "{:?}", resp.error);
    }

    #[test]
    fn map_reads_require_auth_and_stay_out_of_is_read() {
        let (mut engine, _t, _) = engine_with_api_token(&[Scope::Builder]);
        for req in [
            ApiRequest::ListMaps,
            ApiRequest::GetMap { name: "x".into() },
            ApiRequest::GetTerrain,
        ] {
            let resp = engine.handle_api_request(req, None);
            assert_eq!(
                resp.error.as_deref(),
                Some("Authentication required"),
                "map reads must not be public"
            );
        }
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let resp = engine.handle_api_request(ApiRequest::ListMaps, Some(token));
        assert!(resp.ok, "{:?}", resp.error);
    }

    #[test]
    fn put_map_rejects_bad_name_and_bad_toml_before_writing() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder, Scope::Admin]);
        let resp = engine.handle_api_request(
            ApiRequest::PutMap { name: "../evil".into(), toml: "[map]".into() },
            Some(token.clone()),
        );
        assert!(resp.error.as_deref().unwrap().contains("invalid map name"));
        let resp = engine.handle_api_request(
            ApiRequest::PutMap { name: "ok".into(), toml: "not toml [".into() },
            Some(token),
        );
        assert!(resp.error.as_deref().unwrap().contains("invalid map TOML"));
        // neither bad request half-landed
        assert!(!engine.file_sources.contains_key("maps/ok.toml"));
    }

    #[test]
    fn put_map_persists_and_rebuilds_live_templates() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder, Scope::Admin]);
        let toml = "[map]\nname = \"loop\"\ngrid = \"\"\"\n.f\nf.\n\"\"\"\n";
        let resp = engine.handle_api_request(
            ApiRequest::PutMap { name: "loop".into(), toml: toml.into() },
            Some(token.clone()),
        );
        assert!(resp.ok, "{:?}", resp.error);
        assert!(engine.file_sources.contains_key("maps/loop.toml"));
        assert!(engine.map_templates.contains_key("loop"), "live templates rebuilt");
        let resp = engine.handle_api_request(ApiRequest::GetMap { name: "loop".into() }, Some(token));
        assert_eq!(resp.data.unwrap()["name"].as_str(), Some("loop"));
    }

    // ---- room builder slice surface: scoping keeps the whole world from
    // loading at once (see docs/plans/room-builder.md). ----

    /// Create a room via the API and return its dbref.
    fn mk_room(engine: &mut Engine, token: &str, area: &str, key: &str) -> String {
        let resp = engine.handle_api_request(
            ApiRequest::CreateRoom {
                area: area.into(),
                key: key.into(),
                title: key.into(),
                description: None,
            },
            Some(token.to_string()),
        );
        resp.data.unwrap()["ref_id"].as_str().unwrap().to_string()
    }

    fn mk_exit(engine: &mut Engine, token: &str, from: &str, to: &str) {
        let resp = engine.handle_api_request(
            ApiRequest::CreateExit {
                source: from.to_string(),
                direction: "e".into(),
                target: to.to_string(),
                aliases: None,
            },
            Some(token.to_string()),
        );
        assert!(resp.ok, "{:?}", resp.error);
    }

    #[test]
    fn world_slice_and_areas_require_builder_not_public() {
        // They surface authored area/tags like `Examine`, so they must NOT be
        // in `is_read`: unauthenticated calls are refused.
        let (mut engine, _t, _) = engine_with_api_token(&[Scope::Builder]);
        for req in [
            ApiRequest::ListAreas,
            ApiRequest::ListWorldSlice {
                area: None,
                tag: None,
                near: None,
                depth: None,
                limit: None,
            },
        ] {
            let resp = engine.handle_api_request(req, None);
            assert_eq!(
                resp.error.as_deref(),
                Some("Authentication required"),
                "slice reads must not be public"
            );
        }
        // A token without Builder is refused.
        let (mut engine, token, _) = engine_with_api_token(&[]);
        let resp = engine.handle_api_request(ApiRequest::ListAreas, Some(token));
        assert_eq!(resp.error.as_deref(), Some("Builder scope required"));
        // Builder succeeds.
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        assert!(engine.handle_api_request(ApiRequest::ListAreas, Some(token)).ok);
    }

    #[test]
    fn create_room_rejects_path_unsafe_key_and_area() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        // A traversal key/area must never reach _file_key -> @export path.
        let bad_key = engine.handle_api_request(
            ApiRequest::CreateRoom {
                area: "town".into(),
                key: "../evil".into(),
                title: "x".into(),
                description: None,
            },
            Some(token.clone()),
        );
        assert!(bad_key.error.as_deref().unwrap().contains("invalid room key"));
        let bad_area = engine.handle_api_request(
            ApiRequest::CreateRoom {
                area: "..".into(),
                key: "ok".into(),
                title: "x".into(),
                description: None,
            },
            Some(token.clone()),
        );
        assert!(bad_area.error.as_deref().unwrap().contains("invalid area"));
        // A normal room still works.
        let good = engine.handle_api_request(
            ApiRequest::CreateRoom {
                area: "town".into(),
                key: "market".into(),
                title: "Market".into(),
                description: None,
            },
            Some(token),
        );
        assert!(good.ok, "{:?}", good.error);
    }

    #[test]
    fn create_room_stamps_current_area_and_slice_filters_by_it() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let green = mk_room(&mut engine, &token, "village", "green");
        let stag = mk_room(&mut engine, &token, "village", "stag");
        let smithy = mk_room(&mut engine, &token, "village", "smithy");
        let gate = mk_room(&mut engine, &token, "hills", "gate");
        mk_exit(&mut engine, &token, &green, &stag);
        mk_exit(&mut engine, &token, &green, &smithy);
        mk_exit(&mut engine, &token, &green, &gate); // crosses out of village

        // The stamp shows up as a distinct area with the right count.
        let areas = engine
            .handle_api_request(ApiRequest::ListAreas, Some(token.clone()))
            .data
            .unwrap();
        let count = |name: &str| {
            areas
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["area"] == name)
                .map(|a| a["count"].as_u64().unwrap())
        };
        assert_eq!(count("village"), Some(3));
        assert_eq!(count("hills"), Some(1));

        // Scoped to village: exactly the 3 village rooms, and the exit leaving
        // to `gate` reports it as a single boundary stub (not a full node).
        let data = engine
            .handle_api_request(
                ApiRequest::ListWorldSlice {
                    area: Some("village".into()),
                    tag: None,
                    near: None,
                    depth: None,
                    limit: None,
                },
                Some(token.clone()),
            )
            .data
            .unwrap();
        let rooms = data["rooms"].as_array().unwrap();
        assert_eq!(rooms.len(), 3);
        assert!(rooms.iter().all(|r| r["area"] == "village"));
        let boundary = data["boundary"].as_array().unwrap();
        assert_eq!(boundary.len(), 1);
        assert_eq!(boundary[0]["ref_id"].as_str(), Some(gate.as_str()));
        let exits = data["exits"].as_array().unwrap();
        assert!(exits.iter().any(|e| e["to"].as_str() == Some(gate.as_str())));
    }

    #[test]
    fn list_objects_full_filters_by_kind_and_area_and_is_builder_gated() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let square = mk_room(&mut engine, &token, "village", "square");
        let cave = mk_room(&mut engine, &token, "hills", "cave");
        // An NPC in the square and an item in the cave — both non-room objects
        // that the room graph/table would never surface on their own.
        let mk = |engine: &mut Engine, kind: &str, key: &str, loc: &str| {
            engine
                .handle_api_request(
                    ApiRequest::CreateObject {
                        area: "village".into(),
                        key: key.into(),
                        kind: kind.into(),
                        title: Some(key.into()),
                        description: None,
                        location: Some(loc.to_string()),
                    },
                    Some(token.clone()),
                )
                .data
                .unwrap()["ref_id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let kael = mk(&mut engine, "npc", "kael", &square);
        let chest = mk(&mut engine, "item", "chest", &cave);

        // Public (no token) is refused — it carries tags like the slice reads.
        let pub_resp = engine.handle_api_request(
            ApiRequest::ListObjectsFull { kind: None, area: None, limit: None },
            None,
        );
        assert_eq!(pub_resp.error.as_deref(), Some("Authentication required"));

        // Unfiltered: both rooms + npc + item, never an exit.
        let all = engine
            .handle_api_request(
                ApiRequest::ListObjectsFull { kind: None, area: None, limit: None },
                Some(token.clone()),
            )
            .data
            .unwrap();
        let objs = all["objects"].as_array().unwrap();
        assert!(objs.iter().all(|o| o["kind"] != "exit"), "exits must be excluded");
        let has = |r: &str| objs.iter().any(|o| o["ref_id"].as_str() == Some(r));
        assert!(has(&square) && has(&cave) && has(&kael) && has(&chest));

        // kind filter narrows to just the npc, with location + tags present.
        let npcs = engine
            .handle_api_request(
                ApiRequest::ListObjectsFull {
                    kind: Some("npc".into()),
                    area: None,
                    limit: None,
                },
                Some(token.clone()),
            )
            .data
            .unwrap();
        let npcs = npcs["objects"].as_array().unwrap();
        assert_eq!(npcs.len(), 1);
        assert_eq!(npcs[0]["ref_id"].as_str(), Some(kael.as_str()));
        assert_eq!(npcs[0]["location_ref"].as_str(), Some(square.as_str()));
        assert!(npcs[0]["tags"].is_array());

        // area filter scopes by `_file_key`, which only rooms carry (loose
        // objects are "unfiled"): "hills" yields the cave room alone.
        let hills = engine
            .handle_api_request(
                ApiRequest::ListObjectsFull {
                    kind: None,
                    area: Some("hills".into()),
                    limit: None,
                },
                Some(token.clone()),
            )
            .data
            .unwrap();
        let hills = hills["objects"].as_array().unwrap();
        assert_eq!(hills.len(), 1);
        assert_eq!(hills[0]["key"].as_str(), Some("cave"));
        // The loose npc/item report an empty area (they're located, not filed).
        assert_eq!(npcs[0]["area"].as_str(), Some(""));
    }

    #[test]
    fn list_hooks_is_public_and_matches_the_engine_vocabulary() {
        // Public (no token): it's static schema, not world data.
        let (mut engine, _t, _) = engine_with_api_token(&[Scope::Builder]);
        let resp = engine.handle_api_request(ApiRequest::ListHooks, None);
        assert!(resp.ok, "list_hooks must be a public read");
        let data = resp.data.unwrap();

        let known = data["known"].as_array().unwrap();
        assert_eq!(known.len(), hooks::KNOWN_HOOKS.len());
        // Every KNOWN_HOOKS entry is present with a non-empty description.
        for h in hooks::KNOWN_HOOKS {
            let row = known.iter().find(|r| r["name"] == *h).unwrap();
            assert!(!row["describes"].as_str().unwrap().is_empty());
        }
        let prefixes: Vec<&str> =
            data["open_prefixes"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
        assert_eq!(prefixes, ["on_", "cmd_", "lib_"]);
        // The advertised vocabulary agrees with the real validator: a listed
        // hook and each open prefix pass; a bare word and a bogus can_ fail.
        assert!(hooks::is_valid_hook_name(known[0]["name"].as_str().unwrap()));
        assert!(prefixes.iter().all(|p| hooks::is_valid_hook_name(&format!("{p}x"))));
        assert!(!hooks::is_valid_hook_name("frobnicate"));
        assert!(!hooks::is_valid_hook_name("can_frobnicate"));
    }

    #[test]
    fn world_slice_near_depth_bounds_the_graph() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        // a — b — c — d, one-directional exits (BFS is undirected).
        let a = mk_room(&mut engine, &token, "line", "a");
        let b = mk_room(&mut engine, &token, "line", "b");
        let c = mk_room(&mut engine, &token, "line", "c");
        let d = mk_room(&mut engine, &token, "line", "d");
        mk_exit(&mut engine, &token, &a, &b);
        mk_exit(&mut engine, &token, &b, &c);
        mk_exit(&mut engine, &token, &c, &d);

        let slice_len = |engine: &mut Engine, depth: u32| {
            engine
                .handle_api_request(
                    ApiRequest::ListWorldSlice {
                        area: None,
                        tag: None,
                        near: Some(a.clone()),
                        depth: Some(depth),
                        limit: None,
                    },
                    Some(token.clone()),
                )
                .data
                .unwrap()["rooms"]
                .as_array()
                .unwrap()
                .len()
        };
        assert_eq!(slice_len(&mut engine, 1), 2, "depth 1 → a, b");
        assert_eq!(slice_len(&mut engine, 2), 3, "depth 2 → a, b, c");
    }

    #[test]
    fn world_slice_limit_truncates_and_flags() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        for i in 0..5 {
            mk_room(&mut engine, &token, "big", &format!("r{i}"));
        }
        let data = engine
            .handle_api_request(
                ApiRequest::ListWorldSlice {
                    area: Some("big".into()),
                    tag: None,
                    near: None,
                    depth: None,
                    limit: Some(3),
                },
                Some(token.clone()),
            )
            .data
            .unwrap();
        assert_eq!(data["rooms"].as_array().unwrap().len(), 3);
        assert_eq!(data["truncated"].as_bool(), Some(true));
    }

    #[test]
    fn check_program_reports_syntax_without_saving() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let valid = engine
            .handle_api_request(
                ApiRequest::CheckProgram { source: "local x = 1\nreturn x".into() },
                Some(token.clone()),
            )
            .data
            .unwrap();
        assert_eq!(valid["valid"].as_bool(), Some(true));

        let bad = engine
            .handle_api_request(
                ApiRequest::CheckProgram { source: "local x = ".into() },
                Some(token.clone()),
            )
            .data
            .unwrap();
        assert_eq!(bad["valid"].as_bool(), Some(false));
        assert!(bad["error"].as_str().is_some(), "reports the compile error");

        // Builder-gated, not public.
        let resp = engine.handle_api_request(ApiRequest::CheckProgram { source: "x".into() }, None);
        assert_eq!(resp.error.as_deref(), Some("Authentication required"));
    }

    #[test]
    fn list_programs_all_returns_objects_with_hooks() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let r = mk_room(&mut engine, &token, "town", "hall");
        let set = engine.handle_api_request(
            ApiRequest::SetProgram { ref_id: r.clone(), hook: "on_enter".into(), source: "return true".into() },
            Some(token.clone()),
        );
        assert!(set.ok, "{:?}", set.error);
        let data = engine
            .handle_api_request(ApiRequest::ListProgramsAll, Some(token))
            .data
            .unwrap();
        let entry = data.as_array().unwrap().iter().find(|e| e["ref_id"] == r).unwrap();
        assert_eq!(entry["hooks"].as_array().unwrap()[0].as_str(), Some("on_enter"));
        assert_eq!(entry["area"].as_str(), Some("town"));
    }

    #[test]
    fn world_check_flags_broken_exits_unreachable_and_syntax_errors() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let a = mk_room(&mut engine, &token, "town", "a");
        let b = mk_room(&mut engine, &token, "town", "b");
        mk_exit(&mut engine, &token, &a, &b); // a -> b
        // Delete b so the a->b exit dangles.
        engine.handle_api_request(ApiRequest::DeleteObject { ref_id: b.clone() }, Some(token.clone()));
        // Inject a program that doesn't compile (bypassing SetProgram's own check).
        if let Some(obj) = engine.world.get_mut(&a) {
            let _ = hooks::set_program(obj, "on_enter", "local x = (".to_string());
        }

        let data = engine
            .handle_api_request(ApiRequest::WorldCheck, Some(token))
            .data
            .unwrap();
        let kinds: Vec<&str> = data["problems"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["kind"].as_str())
            .collect();
        assert!(kinds.contains(&"broken_exit"), "dangling exit: {:?}", kinds);
        assert!(kinds.contains(&"syntax_error"), "bad program: {:?}", kinds);
        assert!(kinds.contains(&"unreachable"), "'a' has no incoming exits: {:?}", kinds);

        // Requires Builder (not public).
        let resp = engine.handle_api_request(ApiRequest::WorldCheck, None);
        assert_eq!(resp.error.as_deref(), Some("Authentication required"));
    }

    #[test]
    fn set_location_moves_object_into_room_and_rejects_missing_target() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let room = mk_room(&mut engine, &token, "town", "hall");
        let item = engine
            .handle_api_request(
                ApiRequest::CreateObject {
                    area: "town".into(),
                    key: "gem".into(),
                    kind: "item".into(),
                    title: Some("a gem".into()),
                    description: None,
                    location: None,
                },
                Some(token.clone()),
            )
            .data
            .unwrap()["ref_id"]
            .as_str()
            .unwrap()
            .to_string();

        let moved = engine.handle_api_request(
            ApiRequest::SetLocation { ref_id: item.clone(), location: room.clone() },
            Some(token.clone()),
        );
        assert!(moved.ok, "{:?}", moved.error);

        let contents = engine
            .handle_api_request(ApiRequest::ListObjects { location: Some(room) }, Some(token.clone()))
            .data
            .unwrap();
        assert!(contents.as_array().unwrap().iter().any(|o| o["ref_id"] == item));

        let bad = engine.handle_api_request(
            ApiRequest::SetLocation { ref_id: item.clone(), location: "#999999".into() },
            Some(token.clone()),
        );
        assert!(bad.error.as_deref().unwrap().contains("not found"));

        // An object can't be its own location (containment cycle).
        let cycle = engine.handle_api_request(
            ApiRequest::SetLocation { ref_id: item.clone(), location: item.clone() },
            Some(token.clone()),
        );
        assert!(cycle.error.as_deref().unwrap().contains("inside itself"));

        // A player can't be relocated by ref through the builder.
        let player = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&player, "hero", Kind::Player).with_location(&item));
        let ptele = engine.handle_api_request(
            ApiRequest::SetLocation { ref_id: player, location: item },
            Some(token),
        );
        assert!(ptele.error.as_deref().unwrap().contains("player"));
    }

    #[test]
    fn update_exit_redirects_retargets_validates_and_set_aliases() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let a = mk_room(&mut engine, &token, "town", "a");
        let b = mk_room(&mut engine, &token, "town", "b");
        let c = mk_room(&mut engine, &token, "town", "c");
        let exit = engine
            .handle_api_request(
                ApiRequest::CreateExit { source: a.clone(), direction: "north".into(), target: b, aliases: None },
                Some(token.clone()),
            )
            .data
            .unwrap()["ref_id"]
            .as_str()
            .unwrap()
            .to_string();

        // rename direction + retarget to c
        let up = engine.handle_api_request(
            ApiRequest::UpdateExit { ref_id: exit.clone(), direction: Some("up".into()), target: Some(c.clone()) },
            Some(token.clone()),
        );
        assert!(up.ok, "{:?}", up.error);
        let listed = engine
            .handle_api_request(ApiRequest::ListExits { room_ref: a.clone() }, Some(token.clone()))
            .data
            .unwrap();
        let e = listed.as_array().unwrap().iter().find(|e| e["ref_id"] == exit).unwrap();
        assert_eq!(e["direction"].as_str(), Some("up"));
        assert_eq!(e["target_ref"].as_str(), Some(c.as_str()));

        // bad target rejected; non-exit rejected
        let bad = engine.handle_api_request(
            ApiRequest::UpdateExit { ref_id: exit.clone(), direction: None, target: Some("#999999".into()) },
            Some(token.clone()),
        );
        assert!(bad.error.as_deref().unwrap().contains("not found"));
        let notexit = engine.handle_api_request(
            ApiRequest::UpdateExit { ref_id: a, direction: Some("x".into()), target: None },
            Some(token.clone()),
        );
        assert!(notexit.error.as_deref().unwrap().contains("not an exit"));

        // A blank (or whitespace-only) direction is rejected, and the key isn't
        // touched — the exit stays typeable as "up".
        let blank = engine.handle_api_request(
            ApiRequest::UpdateExit { ref_id: exit.clone(), direction: Some("   ".into()), target: None },
            Some(token.clone()),
        );
        assert!(blank.error.as_deref().unwrap().contains("blank"));
        // A padded direction is trimmed before it's stored.
        let padded = engine.handle_api_request(
            ApiRequest::UpdateExit { ref_id: exit.clone(), direction: Some("  down  ".into()), target: None },
            Some(token.clone()),
        );
        assert!(padded.ok, "{:?}", padded.error);
        let ex2 = engine.handle_api_request(ApiRequest::Examine { ref_id: exit.clone() }, Some(token.clone())).data.unwrap();
        assert_eq!(ex2["key"].as_str(), Some("down"));

        // aliases: blank entries dropped, and padded entries trimmed so a
        // "  climb  " matches player input rather than being stored verbatim.
        let al = engine.handle_api_request(
            ApiRequest::SetAliases { ref_id: exit.clone(), aliases: vec!["u".into(), "  climb  ".into(), " ".into()] },
            Some(token.clone()),
        );
        assert!(al.ok, "{:?}", al.error);
        let ex = engine.handle_api_request(ApiRequest::Examine { ref_id: exit }, Some(token)).data.unwrap();
        let aliases: Vec<&str> = ex["aliases"].as_array().unwrap().iter().filter_map(|a| a.as_str()).collect();
        assert!(aliases.contains(&"u") && aliases.contains(&"climb") && aliases.len() == 2);
    }

    /// `Eval` over the REST API is arbitrary code with the full write API —
    /// same stakes as `@eval` on telnet, which is `Scope::Admin`-gated. The
    /// generic write-tier check in `handle_api_request` only requires
    /// `Builder`, so `Eval` needs (and gets) its own explicit gate. This
    /// pins that a builder-only token — which *can* use `SetProgram` — is
    /// still refused here.
    #[test]
    fn api_eval_rejects_builder_without_admin() {
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder]);

        let resp = engine.handle_api_request(
            ApiRequest::Eval { source: "return 1".into() },
            Some(token),
        );
        assert!(!resp.ok, "builder-only token should not be able to eval");
        assert_eq!(resp.error.as_deref(), Some("Admin scope required"));
    }

    #[test]
    fn api_eval_accepts_admin_and_applies_writes() {
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder, Scope::Admin]);
        let ref_id = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&ref_id, "gem", Kind::Item));

        let resp = engine.handle_api_request(
            ApiRequest::Eval { source: format!(r#"set_attr("{}", "eval_touched", true)"#, ref_id) },
            Some(token),
        );
        assert!(resp.ok, "{:?}", resp.error);
        assert_eq!(engine.world.get(&ref_id).unwrap().attrs["eval_touched"], true);
    }

    #[test]
    fn api_eval_requires_a_token_at_all() {
        let (mut engine, _token, _account_id) = engine_with_api_token(&[Scope::Builder, Scope::Admin]);
        let resp = engine.handle_api_request(ApiRequest::Eval { source: "return 1".into() }, None);
        assert!(!resp.ok);
        assert_eq!(resp.error.as_deref(), Some("Authentication required"));
    }

    /// `ListPrograms` used to sit in the unauthenticated `is_read` set,
    /// serving full Program source to anyone who could reach `/api` — see
    /// docs/plans/program-authoring.md's Risks section. It must now require
    /// a token like any other write-tier action.
    #[test]
    fn api_list_programs_requires_auth() {
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder]);
        let ref_id = engine.world.next_dbref();
        let mut obj = GameObject::new(&ref_id, "trap", Kind::Item);
        hooks::set_program(&mut obj, "on_use", "-- secret source\nreturn 1".to_string()).unwrap();
        engine.world.add_object(obj);

        let unauthenticated = engine.handle_api_request(ApiRequest::ListPrograms { ref_id: ref_id.clone() }, None);
        assert!(!unauthenticated.ok, "ListPrograms must not serve Program source with no token");
        assert_eq!(unauthenticated.error.as_deref(), Some("Authentication required"));

        let authenticated = engine.handle_api_request(ApiRequest::ListPrograms { ref_id }, Some(token));
        assert!(authenticated.ok, "{:?}", authenticated.error);
    }

    #[test]
    fn api_program_history_and_restore_round_trip() {
        let (mut engine, token, account_id) = engine_with_api_token(&[Scope::Builder]);
        let ref_id = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&ref_id, "trap", Kind::Item));

        let resp = engine.handle_api_request(
            ApiRequest::SetProgram { ref_id: ref_id.clone(), hook: "on_use".into(), source: "return 1".into() },
            Some(token.clone()),
        );
        assert!(resp.ok, "{:?}", resp.error);
        let resp = engine.handle_api_request(
            ApiRequest::SetProgram { ref_id: ref_id.clone(), hook: "on_use".into(), source: "return 2".into() },
            Some(token.clone()),
        );
        assert!(resp.ok, "{:?}", resp.error);

        let history = engine.handle_api_request(
            ApiRequest::ProgramHistory { ref_id: ref_id.clone(), hook: "on_use".into() },
            Some(token.clone()),
        );
        assert!(history.ok, "{:?}", history.error);
        let versions = history.data.unwrap();
        let versions = versions.as_array().unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0]["source"], "return 1");
        assert_eq!(versions[1]["source"], "return 2");
        // `ProgramHistory` resolves the stored account id to a display name
        // at read time (`display_author`), same as `@program/history`.
        let expected_author = engine.accounts.get(&account_id).unwrap().username.clone();
        assert_eq!(versions[0]["author"], expected_author);

        // No token — same hardening as ListPrograms, this also serves
        // Program source.
        let unauthenticated = engine.handle_api_request(
            ApiRequest::ProgramHistory { ref_id: ref_id.clone(), hook: "on_use".into() },
            None,
        );
        assert!(!unauthenticated.ok);

        let restore = engine.handle_api_request(
            ApiRequest::ProgramRestore { ref_id: ref_id.clone(), hook: "on_use".into(), version: 1 },
            Some(token),
        );
        assert!(restore.ok, "{:?}", restore.error);
        assert_eq!(engine.world.get(&ref_id).unwrap().programs["on_use"].source, "return 1");
        // Restore is non-destructive — it appends a third version, never
        // rewinds history.
        let versions = engine.db.list_program_versions(&ref_id, "on_use").unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[2].source, "return 1");
    }

    // -- Examine hardening (Stage 4) --

    /// `Examine` used to sit in the unauthenticated `is_read` set, serving
    /// full `attrs`/`tags`/`locks` for any object to anyone who could reach
    /// `/api` — bypassing `system:hidden`/`can_see` visibility rules a
    /// player would otherwise be subject to. It must now require a token
    /// like `ListPrograms`/`ProgramHistory`.
    #[test]
    fn api_examine_requires_auth() {
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder]);
        let ref_id = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&ref_id, "secret", Kind::Item));

        let unauthenticated = engine.handle_api_request(ApiRequest::Examine { ref_id: ref_id.clone() }, None);
        assert!(!unauthenticated.ok, "Examine must not serve object data with no token");
        assert_eq!(unauthenticated.error.as_deref(), Some("Authentication required"));

        let authenticated = engine.handle_api_request(ApiRequest::Examine { ref_id }, Some(token));
        assert!(authenticated.ok, "{:?}", authenticated.error);
    }

    // -- @import / @export (Stage 4) --

    struct TempBundleDir {
        path: std::path::PathBuf,
    }

    impl TempBundleDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "hearth-engine-import-test-{}-{}-{}",
                std::process::id(),
                tag,
                Engine::now_secs()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn write(&self, subdir: &str, filename: &str, contents: &str) {
            let dir = self.path.join(subdir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(filename), contents).unwrap();
        }
    }

    impl Drop for TempBundleDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    const IMPORT_TEST_TOML: &str = r#"
        area = "gate"
        [[rooms]]
        key = "hall"
        title = "A Hall"
        description = "An echoing hall."
    "#;

    #[test]
    fn api_import_requires_admin_not_just_builder() {
        let dir = TempBundleDir::new("admin-gate");
        dir.write("gate", "gate.toml", IMPORT_TEST_TOML);
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder]);

        let count_before = engine.world.objects.len();
        let resp = engine.handle_api_request(
            ApiRequest::Import { path: dir.path.to_string_lossy().to_string(), dry_run: false },
            Some(token),
        );
        assert!(!resp.ok, "builder-only token should not be able to import");
        assert_eq!(resp.error.as_deref(), Some("Admin scope required"));
        assert_eq!(engine.world.objects.len(), count_before, "a rejected import must write nothing");
    }

    #[test]
    fn api_export_requires_admin_not_just_builder() {
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder]);
        let resp = engine.handle_api_request(
            ApiRequest::Export { path: "/tmp/should-not-be-created".to_string() },
            Some(token),
        );
        assert!(!resp.ok, "builder-only token should not be able to export");
        assert_eq!(resp.error.as_deref(), Some("Admin scope required"));
    }

    /// The whole point of closing the export coverage gap: content built
    /// entirely in-game (`@create`, `@dig`, `@tag`, `put ... in ...` — the
    /// same commands a builder actually types, not hand-constructed
    /// `World`/`GameObject`s with a key already attached) must survive
    /// `@export` and re-import as a no-op. Exercises `cmd_create` (an item
    /// in a room that itself has no file identity — `test_engine_with_session`'s
    /// spawn room is a from-scratch fallback room, see `Engine::new`, so
    /// this also covers "an object whose room has no area") and nested
    /// containment via `put`.
    #[test]
    fn export_round_trips_objects_created_entirely_in_game() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);

        assert!(
            !engine
                .world
                .get(&engine.spawn_room_ref)
                .unwrap()
                .attrs
                .contains_key(crate::loader::FILE_KEY_ATTR),
            "the from-scratch spawn room must start with no file identity"
        );

        engine.cmd_create(&session_id, &actor_ref, "Rusty Sword");
        engine.cmd_create(&session_id, &actor_ref, "Wooden Box");
        let box_ref = engine
            .world
            .objects
            .values()
            .find(|o| o.key == "wooden_box")
            .unwrap()
            .ref_id
            .clone();
        engine.cmd_tag(&session_id, &actor_ref, &format!("{} = item:container", box_ref));
        let put_result = engine.cmd_put(&actor_ref, "rusty sword in wooden box");
        assert!(!put_result.to_lowercase().contains("don't"), "put should have succeeded: {}", put_result);

        let sword_ref = engine
            .world
            .objects
            .values()
            .find(|o| o.key == "rusty_sword")
            .unwrap()
            .ref_id
            .clone();
        assert_eq!(
            engine.world.get(&sword_ref).unwrap().location_ref.as_deref(),
            Some(box_ref.as_str()),
            "the sword must actually be nested inside the box before export"
        );

        let export_dir = TempBundleDir::new("adhoc-round-trip");
        let export_output = engine.cmd_export(&session_id, &export_dir.path.to_string_lossy());
        assert!(!export_output.to_lowercase().contains("error"), "{}", export_output);

        let object_count_before = engine.world.objects.len();
        let report = crate::import_export::import_bundle(
            &export_dir.path,
            &mut engine.world,
            &engine.db,
            false,
            Some(&actor_ref),
        )
        .unwrap();

        assert!(report.created.is_empty(), "export->import of ad-hoc content must be a no-op: {:?}", report);
        assert!(report.updated.is_empty(), "{:?}", report);
        assert_eq!(engine.world.objects.len(), object_count_before);
        assert_eq!(
            engine.world.get(&sword_ref).unwrap().location_ref.as_deref(),
            Some(box_ref.as_str()),
            "nested containment must survive the round trip"
        );
    }

    #[test]
    fn api_import_creates_objects_and_reports_via_output() {
        let dir = TempBundleDir::new("api-create");
        dir.write("gate", "gate.toml", IMPORT_TEST_TOML);
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder, Scope::Admin]);

        let resp = engine.handle_api_request(
            ApiRequest::Import { path: dir.path.to_string_lossy().to_string(), dry_run: false },
            Some(token),
        );
        assert!(resp.ok, "{:?}", resp.error);
        let output = resp.data.unwrap()["output"].as_str().unwrap().to_string();
        assert!(output.contains("1 created"), "unexpected output: {}", output);
        assert!(
            engine.world.objects.values().any(|o| o.key == "hall" && o.kind == Kind::Room),
            "the room from the bundle should now exist"
        );
    }

    #[test]
    fn api_import_dry_run_writes_nothing() {
        let dir = TempBundleDir::new("api-dry-run");
        dir.write("gate", "gate.toml", IMPORT_TEST_TOML);
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder, Scope::Admin]);

        let count_before = engine.world.objects.len();
        let resp = engine.handle_api_request(
            ApiRequest::Import { path: dir.path.to_string_lossy().to_string(), dry_run: true },
            Some(token),
        );
        assert!(resp.ok, "{:?}", resp.error);
        let output = resp.data.unwrap()["output"].as_str().unwrap().to_string();
        assert!(output.contains("dry run"), "unexpected output: {}", output);
        assert_eq!(engine.world.objects.len(), count_before, "dry run must write nothing");
    }

    #[test]
    fn api_import_collision_refuses_and_reports_error() {
        let dir = TempBundleDir::new("api-collision");
        dir.write(
            "gate",
            "a.toml",
            r#"
                area = "gate"
                [[rooms]]
                key = "hall"
                title = "A"
            "#,
        );
        dir.write(
            "gate",
            "b.toml",
            r#"
                area = "gate"
                [[rooms]]
                key = "hall"
                title = "B"
            "#,
        );
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder, Scope::Admin]);
        let count_before = engine.world.objects.len();

        let resp = engine.handle_api_request(
            ApiRequest::Import { path: dir.path.to_string_lossy().to_string(), dry_run: false },
            Some(token),
        );
        assert!(!resp.ok);
        assert_eq!(engine.world.objects.len(), count_before, "nothing should be written on collision");
    }

    #[test]
    fn telnet_import_then_export_round_trips() {
        let import_dir = TempBundleDir::new("telnet-import");
        import_dir.write("gate", "gate.toml", IMPORT_TEST_TOML);
        let (mut engine, session_id, _actor_ref) = test_engine_with_session(true);

        let out = engine.cmd_import(&session_id, &import_dir.path.to_string_lossy());
        assert!(out.contains("1 created"), "unexpected output: {}", out);

        let export_dir = TempBundleDir::new("telnet-export");
        let out = engine.cmd_export(&session_id, &export_dir.path.to_string_lossy());
        // Two areas: "gate" from the import above, plus "unfiled" — the
        // test session's spawn room (`test_engine_with_session`'s
        // from-scratch fallback room, no `game_dir` configured) has no
        // file identity of its own and now gets swept up too, per the
        // export coverage fix (see docs/plans/program-authoring.md Stage
        // 4 and `stamp_missing_identities`).
        assert!(out.contains("2 area(s) written"), "unexpected output: {}", out);
        assert!(export_dir.path.join("gate").join("gate.toml").exists());

        // Re-importing the export must be a no-op.
        let out = engine.cmd_import(&session_id, &export_dir.path.to_string_lossy());
        assert!(out.contains("0 created, 0 updated"), "export->import round trip should be a no-op: {}", out);
    }

    #[test]
    fn telnet_import_requires_admin() {
        let dir = TempBundleDir::new("telnet-admin-gate");
        dir.write("gate", "gate.toml", IMPORT_TEST_TOML);
        let (mut engine, session_id, _actor_ref) = test_engine_with_session(false);
        let out = engine.cmd_import(&session_id, &dir.path.to_string_lossy());
        assert!(out.contains("Permission denied"));
        assert!(engine.world.objects.values().all(|o| o.key != "hall"));
    }

    /// Booting with `load_world_files = false` used to leave the file-key map
    /// empty, so `spawn_room` never resolved and the engine built a duplicate
    /// empty room — players landed in "An empty room" instead of the real
    /// spawn. Verified against The Last Stag before the fix: 34 objects became
    /// 35, with two crossroads rooms.
    #[test]
    fn spawn_room_resolves_from_the_database_when_file_loading_is_off() {
        let db = crate::db::Database::open(Path::new(":memory:")).unwrap();

        let mut seeded = World::new();
        let crossroads = seeded.next_dbref();
        let mut room = GameObject::new(&crossroads, "crossroads", Kind::Room)
            .with_title("The Crossroads");
        room.attrs.insert(
            "_file_key".into(),
            serde_json::json!("town/crossroads"),
        );
        seeded.add_object(room);
        db.save_world(&seeded).unwrap();

        let config = Config {
            spawn_room: "town/crossroads".into(),
            game_dir: None,
            load_world_files: false,
            ..Config::default()
        };
        let (_tx, rx) = mpsc::unbounded_channel();
        let engine = Engine::new(rx, db, &config);

        assert_eq!(
            engine.spawn_room_ref, crossroads,
            "spawn should resolve to the room already in the database"
        );
        assert_eq!(
            engine.world.objects.len(),
            1,
            "no duplicate spawn room should have been created"
        );
    }

    /// A hook that schedules two timers is a fork bomb, and because timers
    /// persist to `scheduled_hooks` it survives a restart — bouncing the
    /// server does not clear it. The instruction budget is no defence, since
    /// every individual run is well inside it; only the count grows.
    #[test]
    fn timers_are_capped_per_owner() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);

        let owned = engine.world.next_dbref();
        engine.world.add_object(
            GameObject::new(&owned, "bomb", Kind::Item).with_owner(&actor_ref),
        );

        for i in 0..crate::softcode::OWNER_TIMER_QUOTA {
            engine.scheduled_hooks.push(ScheduledHook {
                id: format!("pre-{}", i),
                fire_at_tick: 999,
                target: owned.clone(),
                hook: "on_expire".into(),
                data: None,
            });
        }
        let before = engine.scheduled_hooks.len();

        engine.deliver_effects(
            &[Effect::ScheduleHook {
                target: owned.clone(),
                hook: "on_expire".into(),
                ticks: 1,
                data: None,
            }],
            &actor_ref,
        );

        assert_eq!(
            engine.scheduled_hooks.len(),
            before,
            "scheduling past the quota must not add a timer"
        );
    }

    #[test]
    fn timers_on_unowned_objects_are_not_capped() {
        // Every file-authored object is unowned, and system content schedules
        // timers freely.
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);

        let system_obj = engine.world.next_dbref();
        engine
            .world
            .add_object(GameObject::new(&system_obj, "beacon", Kind::Item));

        for i in 0..crate::softcode::OWNER_TIMER_QUOTA {
            engine.scheduled_hooks.push(ScheduledHook {
                id: format!("pre-{}", i),
                fire_at_tick: 999,
                target: system_obj.clone(),
                hook: "on_expire".into(),
                data: None,
            });
        }
        let before = engine.scheduled_hooks.len();

        engine.deliver_effects(
            &[Effect::ScheduleHook {
                target: system_obj.clone(),
                hook: "on_expire".into(),
                ticks: 1,
                data: None,
            }],
            &actor_ref,
        );

        assert_eq!(engine.scheduled_hooks.len(), before + 1);
    }
}
