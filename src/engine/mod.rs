mod authoring;
mod commands;

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use tokio::sync::mpsc;

use crate::accounts::{AccountStore, Scope};
use crate::config::Config;
use crate::db::{Database, ScriptLock};
use crate::locks::{self, AccessContext};
use crate::softcode::hooks::{self};
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
        /// The room's ref (dbref) — a stable id for map-aware clients (GMCP
        /// `Room.Info.num`). Defaulted so older clients ignore it.
        #[serde(default)]
        num: String,
        #[serde(default)]
        area: String,
        /// Map name / terrain char / grid coords, present only for rooms
        /// instantiated from a map template (stamped `map_name`/`terrain`/
        /// `map_x`/`map_y`). Drive a Mudlet-style mapper's area + environment
        /// + coordinates.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        map: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        environment: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y: Option<i64>,
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
    /// Destination room ref (dbref), for map-aware clients (GMCP `Room.Info`
    /// exits link `dir → to`). Empty when the exit has no resolved target.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub to: String,
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
    /// Advance the heartbeat `count` times, right now, on the message loop.
    ///
    /// The engine's own tick is a wall-clock `tokio::time::interval`, which is
    /// what production wants and what a test cannot use: it can't be waited on
    /// without sleeping, and it fires on its own schedule regardless of where
    /// the test is. Driving ticks through the *same FIFO channel* as player
    /// input makes them orderable — a fence sent afterwards can't reply until
    /// the ticks are done and their output is queued. See `session_test`.
    Tick {
        count: u64,
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
    /// Attach a Lock DSL expression to one of an object's hooks. Builder-gated;
    /// refuses locked objects (see `locked_target`).
    SetLock { ref_id: String, hook: String, expr: String },
    /// Remove the lock from one of an object's hooks (inverse of `SetLock`).
    ClearLock { ref_id: String, hook: String },
    /// Deep-copy an existing object into a fresh dbref (the REST counterpart of
    /// softcode `clone_object`). Builder-gated; strips `system:*` status and
    /// refuses a locked source. Returns `{ ref_id }` of the new object.
    CloneObject {
        source: String,
        #[serde(default)]
        location: Option<String>,
        #[serde(default)]
        owner: Option<String>,
    },
    /// Force a player to run a command as if typed (the REST counterpart of
    /// softcode `run_command_as`). Admin-gated; `@`-commands and `quit` are
    /// refused on the forced path.
    RunCommandAs { ref_id: String, command: String },
    AddTag { ref_id: String, tag: String },
    RemoveTag { ref_id: String, tag: String },
    /// `cascade`: if `ref_id` is an archetype with live instances, flatten
    /// (detach) each of them first instead of refusing — see the same guard
    /// in apply_to's `Intent::Destroy`.
    DeleteObject {
        ref_id: String,
        #[serde(default)]
        cascade: bool,
    },
    /// Set an object's whole behavior script (hooks as functions in one shared
    /// scope). Replaces the object's existing script. `base_version` opts into
    /// versioned, merge-aware writes (see docs/plans/softcode-versioning.md):
    /// with it, a stale base triggers a server 3-way merge or a conflict
    /// response; without it, a plain overwrite (legacy behavior).
    SetScript {
        ref_id: String,
        source: String,
        #[serde(default)]
        base_version: Option<i64>,
    },
    /// Remove an object's script entirely.
    ClearScript { ref_id: String },
    /// Point an object at an existing archetype (or clear it with null).
    /// Reparents to an existing object; does not create a new archetype.
    SetArchetype { ref_id: String, archetype_ref: Option<String> },
    /// Flatten an instance off its archetype (copy resolved fields/script down,
    /// clear the delegation) — the REST counterpart of softcode `clone`.
    DetachObject { ref_id: String },
    /// Candidate entities for a `ref`-typed attribute's dropdown (see
    /// `crate::attr_schema`). `ref_source` vocabulary:
    /// `kind:<npc|item|room|exit|code>`, `tag:<cat>:<key>` (matched against
    /// resolved tags), or `archetype` (objects with live instances). Returns
    /// `{ candidates: [{ ref_id, label }] }`, sorted by label. Read-only.
    ListRefCandidates { ref_source: String },
    /// An object's script: `{ source, hooks: [..], enabled }` (or null).
    GetScript { ref_id: String },
    /// Set a `require`able lib module on a `Kind::Code` object, keyed by bare
    /// `<name>` (loaded as `require("<name>")`). `base_version` opts into the
    /// same versioned merge path as `SetScript`.
    SetLib {
        ref_id: String,
        name: String,
        source: String,
        #[serde(default)]
        base_version: Option<i64>,
    },
    /// Remove a lib module by bare `<name>`.
    RemoveLib { ref_id: String, name: String },
    /// Who am I — the account behind the token. Authenticated but not
    /// builder-gated; the client needs it to render lock ownership.
    Me,
    /// Softcode version history for a target (object script when `name` is
    /// omitted, a lib module otherwise), newest first, bodies excluded.
    ListScriptVersions { ref_id: String, #[serde(default)] name: Option<String> },
    /// The source of one historical version.
    GetScriptVersion { ref_id: String, #[serde(default)] name: Option<String>, version: i64 },
    /// Re-apply a historical version's source as a new version (rollback).
    RevertScript { ref_id: String, #[serde(default)] name: Option<String>, version: i64 },
    /// Claim the person-held edit lock on a script/lib.
    LockScript { ref_id: String, #[serde(default)] name: Option<String> },
    /// Release an edit lock (holder or admin).
    UnlockScript { ref_id: String, #[serde(default)] name: Option<String> },
    /// Create a new standalone library: find-or-create a `Kind::Code` host for
    /// `<name>` and seed a starter module — the builder-facing, file-free
    /// counterpart of `@lib`. Refuses an existing name (edit it via `set_lib`).
    /// Returns `{ ref_id, name }`.
    CreateLibrary { name: String },
    /// An object's lib modules: `[{ name, source }]`.
    ListLibs { ref_id: String },
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
    /// Run softcode tests for the builder's test panel — the REST counterpart
    /// of `@test`. With `source`, runs that one ad-hoc test file (the editor /
    /// REPL "run as test"); with `file`, one discovered `.test.luau` by its
    /// game-dir-relative path; with neither, discovers and runs them all.
    /// Every run is on a *clone* of the world, so a test's writes never leak —
    /// strictly less privileged than the `set_program` a Builder already has,
    /// hence Builder-gated like `@test`. Returns
    /// `{files: [{file, tests: [{name, passed, error}], error?}], passed, failed}`.
    RunTests {
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        file: Option<String>,
        /// Run the `test_*` functions embedded in this object's own script
        /// (tests co-located with hooks). Takes precedence over `source`/`file`.
        #[serde(default)]
        ref_id: Option<String>,
    },
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
    /// Preview a one-shot Luau script against the live world WITHOUT committing
    /// — the builder REPL's kernel. Runs the script through the same immutable
    /// `run_eval` path `Eval` uses, then formats the intent batch it produced
    /// and **discards it** instead of applying. Reads hit real live state
    /// (real refs, real attrs), but nothing is ever mutated, so — unlike
    /// `Eval` — it needs only `Scope::Builder`, the same tier as `RunTests`.
    /// Returns `{returned, writes: [human-readable intent, …], write_count}`.
    EvalPreview { source: String },
    /// Preview-FIRE a hook: run `hook` on object `ref_id` as if the event
    /// happened, with a chosen `actor` and `room`, and report the writes it
    /// WOULD make (emits included) without committing — the editor's hook
    /// test button. `source` overrides the saved program so an unsaved buffer
    /// can be fired; `actor_ref` defaults to the caller's character and
    /// `room_ref` to the actor's (or `this`'s) location. Discards the batch,
    /// same safety and Builder gate as `EvalPreview`. Returns
    /// `{writes: […], write_count, denied}` (`denied` = a `can_*` guard vetoed).
    PreviewHook {
        ref_id: String,
        hook: String,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        actor_ref: Option<String>,
        #[serde(default)]
        room_ref: Option<String>,
    },
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
    /// Playtest a dialogue in the builder without touching a real player's
    /// conversation. Runs against a per-builder preview key so two builders
    /// (or a builder and their own live game session) never share ink state.
    /// `source` lets the editor play the unsaved buffer; when omitted it falls
    /// back to the object's saved `_ink_source`.
    InkPlayStart {
        ref_id: String,
        #[serde(default)]
        source: Option<String>,
    },
    InkPlayContinue { ref_id: String },
    InkPlayChoose { ref_id: String, index: usize },
    InkPlayEnd { ref_id: String },
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
    /// `actor_ref` is the account's active **Character** (see `Session::character`).
    /// It is *not* the effective actor while a Puppet is driven — read the
    /// object commands act as through `Session::effective_actor`, never this
    /// field directly on the dispatch path.
    Playing { actor_ref: String, account_id: String, puppet_ref: Option<String> },
}

/// A multi-line, engine-owned input editor a Session can be inside. Set by
/// `@program`/`@eval`/`@dialogue`, consumed line-by-line by the matching
/// handler, cleared when the editor finishes. This is session state the engine
/// owns — distinct from softcode's `prompt()` callback, which lives on the
/// Character object because only Intents can reach it (see `handle_game_input`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditorMode {
    Program,
    Eval,
    Ink,
}

/// Maximum nesting of `run_command_as`: a forced command may itself fire hooks
/// that force further commands, so this bounds the chain (charm/puppet loop
/// guard). Small on purpose — legitimate cascades are shallow.
const MAX_FORCE_DEPTH: u32 = 5;

struct Session {
    tx: mpsc::UnboundedSender<ClientMessage>,
    state: SessionState,
    /// The multi-line editor this session is inside, if any (see [`EditorMode`]).
    /// Replaces the former `_program_editing`/`_eval_editing`/`_ink_editing`
    /// object attrs — engine-owned session state has no business on the object.
    editor: Option<EditorMode>,
}

impl Session {
    /// The **Character** this session plays — the account's active PC. Stable
    /// while puppeting (the puppet does not become the character). `None` until
    /// login reaches `Playing`. Authoring (`@`-verbs) and ownership checks act
    /// as the character; only gameplay follows [`Self::effective_actor`].
    fn character(&self) -> Option<&str> {
        match &self.state {
            SessionState::Playing { actor_ref, .. } => Some(actor_ref),
            _ => None,
        }
    }

    /// The object commands act **as**: the Puppet when one is active, otherwise
    /// the Character. Gameplay dispatch and room/output routing use this. Equal
    /// to [`Self::character`] when not puppeting, so non-puppet behavior is
    /// unchanged. See ADR-0008 and the CONTEXT.md Puppet term.
    fn effective_actor(&self) -> Option<&str> {
        match &self.state {
            SessionState::Playing { actor_ref, puppet_ref, .. } => {
                Some(puppet_ref.as_deref().unwrap_or(actor_ref))
            }
            _ => None,
        }
    }

    /// The Puppet being driven, if any.
    fn puppet(&self) -> Option<&str> {
        match &self.state {
            SessionState::Playing { puppet_ref, .. } => puppet_ref.as_deref(),
            _ => None,
        }
    }
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

/// Serialize an `InkOutput` for the REST playtest actions. Mirrors the shape
/// `ink_output_to_table` builds for the Luau runtime, so a web playtest pane
/// and an in-game conversation render from the same fields.
fn ink_output_json(out: &crate::softcode::ink::InkOutput) -> serde_json::Value {
    serde_json::json!({
        "text": out.text,
        "can_continue": out.can_continue,
        "ended": out.ended,
        "tags": out.tags,
        "choices": out
            .choices
            .iter()
            .map(|c| serde_json::json!({ "index": c.index, "text": c.text, "tags": c.tags }))
            .collect::<Vec<_>>(),
    })
}

/// Prefix a hook error with which archetype it actually ran on, when that
/// differs from the instance the hook was fired on. Plain `err` (unchanged)
/// when the instance's own script was what ran. See
/// docs/plans/archetypes.md's "error attribution" integration note — without
/// this, an error from inherited behavior names only the instance, which is
/// baffling to debug once chains get deep.
fn annotate_archetype_error(err: String, this_ref: &str, resolving_ref: &str) -> String {
    if resolving_ref == this_ref {
        err
    } else {
        format!("{} (via archetype {}): {}", this_ref, resolving_ref, err)
    }
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
    /// Person-held softcode edit locks, keyed by (ref_id, name) — `name` is
    /// `None` for object scripts, `Some(module)` for libs. Seeded from the DB
    /// at boot (expired entries dropped), persisted on claim/release. See
    /// docs/plans/softcode-versioning.md.
    script_locks: HashMap<(String, Option<String>), ScriptLock>,
    softcode: SoftcodeRuntime,
    rx: mpsc::UnboundedReceiver<EngineMessage>,
    tick_count: u64,
    tick_secs: u64,
    autosave_secs: u64,
    /// In-world game clock config (`None` = no clock). See `crate::clock`.
    clock: Option<crate::clock::ClockConfig>,
    /// Monotonic in-world minute counter (minutes since the epoch `start`),
    /// advanced on each tick by `clock.minutes_per_tick`. Persisted in the DB
    /// `meta` table so game time survives a restart.
    game_minute: u64,
    /// Fractional-minute remainder, so a `minutes_per_tick < 1` still advances.
    game_minute_accum: f64,
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
    /// The map name whose GMCP `Terrain.Legend` we last sent each session, so
    /// the legend rides only map *entry*, not every move. Cleared on disconnect.
    legend_sent_map: HashMap<String, String>,
    /// Hooks scheduled to fire in the future via `after(ticks, target, hook)`.
    scheduled_hooks: Vec<ScheduledHook>,
    /// API tokens: hash → info. Both session (ephemeral) and persistent tokens.
    api_tokens: HashMap<String, TokenInfo>,
    /// Re-entrancy depth of `run_command_as` (forced commands). A forced
    /// command can fire hooks that force further commands; this bounds the
    /// chain so a charm/puppet loop can't recurse without limit. See
    /// `MAX_FORCE_DEPTH`.
    force_depth: u32,
    /// Recent failed-login tracking per lowercased username:
    /// `(consecutive_failures, first_failure_at)`. Used to throttle repeated
    /// bad passwords so Argon2 verification can't be used as a CPU-DoS
    /// amplifier and to slow online guessing (RBAC audit M1). Cleared on a
    /// successful login or once the lockout window elapses.
    login_failures: HashMap<String, (u32, std::time::Instant)>,
    /// Content hashes from the last load/reload, used to skip unchanged files.
    file_hashes: HashMap<std::path::PathBuf, String>,
    max_characters: u8,
    /// File-key/area prefixes whose managed objects are stamped
    /// `system:locked` (definition read-only to authoring). From
    /// `Config::locked`; re-applied on every `@reload-world`. See
    /// `crate::loader::stamp_locked`.
    locked_prefixes: Vec<String>,
    /// Cached command list from `send_commands`, keyed by (location,
    /// builder scope, admin scope, world version) so repeated looks/updates
    /// in the same room skip the rebuild-and-sort.
    commands_cache: Option<(Option<String>, bool, bool, u64, Vec<String>)>,
    /// Lazily rebuilt derived indexes (tickables, global hooks by name,
    /// troupe followers). Rebuilt in one O(N) pass whenever
    /// `world.version` changes — see `Engine::indexes`.
    derived: Option<DerivedIndexes>,
}

/// World-derived lookup structures, invalidated wholesale on any world
/// mutation via `World::version`. Conservative: `get_mut` bumps the version
/// even for read-modify writes that change nothing, but a spurious rebuild
/// is only a perf cost. One O(N) pass fills every structure so a burst of
/// queries after a mutation shares a single scan.
#[derive(Default, Clone)]
struct DerivedIndexes {
    epoch: u64,
    /// `(ref_id, tick_interval)` for every object with an enabled `on_tick`
    /// program.
    tickables: Vec<(String, u64)>,
    /// `system:global`-tagged objects that have an enabled program for a
    /// given hook, e.g. `"on_enter" -> ["#12", "#40"]`.
    globals_by_hook: HashMap<String, Vec<String>>,
    /// Followers per troupe leader: `troupe:<leader_ref>` tag → member refs.
    troupes: HashMap<String, Vec<String>>,
}

impl DerivedIndexes {
    fn build(world: &World) -> Self {
        let mut idx = DerivedIndexes {
            epoch: world.struct_version(),
            tickables: Vec::new(),
            globals_by_hook: HashMap::new(),
            troupes: HashMap::new(),
        };
        for obj in world.objects.values() {
            // `object_responds`/`resolve_hook_names` walk the archetype chain
            // (see docs/plans/archetypes.md) so an instance that only
            // delegates its `on_tick`/`cmd_*`/etc. still ticks and dispatches
            // — without this an archetype-based NPC would look inert.
            if hooks::object_responds(world, obj, "on_tick") {
                let interval = world
                    .resolved_attr(obj, "tick_interval")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1);
                idx.tickables.push((obj.ref_id.clone(), interval));
            }
            // `system:global` may itself be inherited from an archetype (a
            // shared "rules" object) — resolved_tags covers that.
            if world.resolved_tags(obj).iter().any(|t| t.category == "system" && t.key == "global") {
                for hook in hooks::resolve_hook_names(world, obj) {
                    idx.globals_by_hook.entry(hook).or_default().push(obj.ref_id.clone());
                }
            }
            for tag in &obj.tags {
                if tag.category == "troupe" {
                    idx.troupes.entry(tag.key.clone()).or_default().push(obj.ref_id.clone());
                }
            }
        }
        idx
    }
}

use crate::softcode::ScheduledHook;

impl Engine {
    pub fn new(rx: mpsc::UnboundedReceiver<EngineMessage>, db: Database, config: &Config) -> Self {
        let (mut world, mut accounts) = if db.has_world_data() {
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

        // Bootstrap admin: on a truly fresh store (no accounts yet), seed the
        // first account from `HEARTH_ADMIN_USER` / `HEARTH_ADMIN_PASSWORD` if
        // both are set. Because the store is empty, `create` grants it the
        // admin/builder/player scopes automatically — no separate scope path.
        // This makes an unattended deploy (container/Fly) come up with a usable
        // admin without anyone having to race to the login screen. It never
        // touches an existing deployment's accounts: the moment any account
        // exists, this is skipped. See CLAUDE.md "First account created gets
        // admin/builder/player scopes."
        if accounts.is_empty() {
            match (std::env::var("HEARTH_ADMIN_USER"), std::env::var("HEARTH_ADMIN_PASSWORD")) {
                (Ok(user), Ok(pass)) if !user.is_empty() && !pass.is_empty() => {
                    match accounts.create(&user, &pass) {
                        Ok(account) => {
                            tracing::info!(username = %account.username, "Seeded bootstrap admin account from environment");
                            if let Err(e) = db.save_accounts(&accounts) {
                                tracing::error!(error = %e, "Failed to persist seeded admin account");
                            }
                        }
                        Err(e) => tracing::error!(error = %e, "HEARTH_ADMIN_USER/PASSWORD set but admin seed failed"),
                    }
                }
                (Ok(_), Err(_)) | (Err(_), Ok(_)) => {
                    tracing::warn!("Only one of HEARTH_ADMIN_USER / HEARTH_ADMIN_PASSWORD is set; skipping admin seed (both are required)");
                }
                _ => {}
            }
        }

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
                    }
                    Err(e) => tracing::error!(error = %e, "Failed to load game content"),
                }
            } else {
                tracing::info!("load_world_files = false — skipping boot-time world content load");
            }
            softcode.load_modules(crate::loader::load_modules(game_path));
            softcode
                .wasm_host()
                .borrow_mut()
                .load_dir(&game_path.join("wasm"));
            softcode.ink_runtime().borrow_mut().set_ink_dir(game_path.to_path_buf());
            let ink_files = crate::loader::load_ink_files(game_path);
            for source in ink_files.values() {
                if let Err(e) = softcode.ink_runtime().borrow_mut().compile(source) {
                    tracing::warn!(error = %e, "Failed to pre-compile ink file");
                }
            }
        }

        // Stamp `system:locked` on file-authoritative objects (config-driven
        // `locked` prefixes). Runs regardless of `load_world_files`: the DB is
        // authoritative for content, but the config is authoritative for which
        // keys are locked, so a newly-added prefix takes effect on boot. See
        // `crate::loader::stamp_locked` and `docs/plans/archetypes.md`.
        crate::loader::stamp_locked(&mut world, &config.locked);
        // Warn (non-fatally) about declared attrs whose values don't match
        // their type — a builder mistake worth surfacing, never a boot blocker.
        crate::loader::validate_attr_schemas(&world);

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

        // Game clock: restore the counter and prime the softcode `get_time()`
        // snapshot. No `[clock]` config → the counter is inert and get_time()
        // stays nil.
        let clock = config.clock.clone();
        let game_minute = db.load_game_minute().unwrap_or(0);
        if let Some(cfg) = &clock {
            softcode.set_game_time(Some(cfg.to_json(game_minute)));
        }

        // Seed edit locks, dropping any already expired.
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut script_locks = HashMap::new();
        if let Ok(locks) = db.load_script_locks() {
            for lock in locks {
                if lock.expires_at > now_secs {
                    script_locks.insert((lock.ref_id.clone(), lock.name.clone()), lock);
                }
            }
        }

        let engine = Self {
            world,
            accounts,
            sessions: HashMap::new(),
            db,
            script_locks,
            softcode,
            rx,
            tick_count: 0,
            tick_secs: config.tick_secs,
            autosave_secs: config.autosave_secs,
            clock,
            game_minute,
            game_minute_accum: 0.0,
            spawn_room: config.spawn_room.clone(),
            spawn_room_ref,
            game_dir: config.game_dir.clone(),
            themes,
            map_templates,
            file_sources,
            legend_sent_map: HashMap::new(),
            scheduled_hooks,
            api_tokens,
            force_depth: 0,
            login_failures: HashMap::new(),
            file_hashes,
            max_characters: config.max_characters,
            locked_prefixes: config.locked.clone(),
            derived: None,
            commands_cache: None,
        };
        // Snapshot file-authored programs into history (no-op after the first
        // boot unless a file changed) — see docs/plans/softcode-versioning.md.
        engine.record_file_program_versions();
        engine
    }

    /// Derived world indexes, rebuilt lazily in one pass when the world has
    /// changed since the last build. Returns owned data (cloned ref lists)
    /// so the borrow is released before callers touch the world again.
    fn indexes(&mut self) -> DerivedIndexes {
        match &self.derived {
            Some(d) if d.epoch == self.world.struct_version() => d.clone(),
            _ => {
                let idx = DerivedIndexes::build(&self.world);
                self.derived = Some(idx.clone());
                idx
            }
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
                        Some(EngineMessage::Tick { count }) => {
                            for _ in 0..count {
                                self.do_tick();
                            }
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

    /// The current in-world clock hour, when a clock is configured — backs the
    /// `game_time_between()` lock predicate.
    fn current_game_hour(&self) -> Option<u32> {
        self.clock.as_ref().map(|c| c.at(self.game_minute).hour)
    }

    /// Advance the game clock by one tick's worth of minutes, refresh the
    /// softcode `get_time()` snapshot, and fire rollover hooks. No-op without a
    /// `[clock]` config.
    fn advance_clock(&mut self) {
        let Some(cfg) = self.clock.clone() else {
            return;
        };
        self.game_minute_accum += cfg.minutes_per_tick;
        let whole = self.game_minute_accum.floor();
        if whole < 1.0 {
            return;
        }
        self.game_minute_accum -= whole;
        let whole = whole as u64;

        let before = cfg.at(self.game_minute);
        self.game_minute += whole;
        let after = cfg.at(self.game_minute);

        // Refresh the get_time() snapshot for this tick's hooks and reads.
        self.softcode.set_game_time(Some(cfg.to_json(self.game_minute)));

        // Rollover hooks fire on `system:global` objects that DEFINE them, with
        // no actor — the hook reads the time itself via `get_time()` (already
        // refreshed above). Collapsed to at most one fire per type per tick: a
        // tick advancing several hours still fires on_hour once. At the default
        // 1 min/tick a tick never spans an hour, so this is exact in practice;
        // dawn/dusk detection lands on the exact hour, so a tick that jumps
        // *past* dawn/dusk (very fast clocks) can skip them — acceptable for a
        // coarse day/night signal.
        if after.hour != before.hour {
            self.fire_global_hooks("on_hour", "", None, None);
        }
        if after.abs_day != before.abs_day {
            self.fire_global_hooks("on_day", "", None, None);
        }
        if after.hour == cfg.dawn_hour && before.hour != cfg.dawn_hour {
            self.fire_global_hooks("on_dawn", "", None, None);
        }
        if after.hour == cfg.dusk_hour && before.hour != cfg.dusk_hour {
            self.fire_global_hooks("on_dusk", "", None, None);
        }
    }

    fn do_tick(&mut self) {
        self.tick_count += 1;
        let tick = self.tick_count;

        // Advance the in-world clock first, so on_tick hooks reading get_time()
        // see this tick's time, then fire any rollover hooks.
        self.advance_clock();
        let tick_budget = std::time::Duration::from_millis(500);
        let mut ran = 0u32;

        // -- on_tick hooks --
        // Every object with an on_tick Program ticks here, including
        // Kind::Code objects — a "global script" is just a Code object with
        // no location, so it needs no separate scheduler (see
        // docs/plans/program-authoring.md Stage 2). The tickable list comes
        // from the derived index (rebuilt only when the world changed).
        let tickable = self.indexes().tickables;
        let start = std::time::Instant::now();

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
        let touched_lib = batch
            .intents
            .iter()
            .any(|intent| matches!(intent, softcode::Intent::SetLib { .. }));
        if touched_lib {
            self.softcode.invalidate_module_cache();
        }
    }

    fn fire_lifecycle_hook(&mut self, hook_name: &str) {
        let refs: Vec<String> = self
            .world
            .objects
            .values()
            .filter(|obj| hooks::object_responds(&self.world, obj, hook_name))
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
        let responds = match self.world.get(ref_id) {
            Some(o) => hooks::object_responds(&self.world, o, "on_create"),
            None => false,
        };
        if !responds {
            return;
        }
        if let Err(e) = self.fire_tick_hook_named(ref_id, "on_create") {
            tracing::warn!(hook = "on_create", target = %ref_id, error = %e, "on_create hook error");
        }
    }

    /// Resolve the script that should run for `hook_name` on `this_ref` — its
    /// own if it defines the hook, else the first archetype ancestor's (see
    /// `hooks::resolve_script`). Returns the script (cloned) plus the
    /// resolving object's ref, for error attribution.
    ///
    /// Crucially, when the resolving ref differs from `this_ref` (the hook
    /// came from an archetype), `state` is swapped for `this_ref`'s own —
    /// `state` is never delegated (docs/plans/archetypes.md), so the ancestor
    /// script's *code* runs but `state` seen and written back is always the
    /// instance's.
    fn resolve_hook_script(
        &self,
        this_ref: &str,
        hook_name: &str,
    ) -> Option<(hooks::ObjectScript, String)> {
        let obj = self.world.get(this_ref)?;
        let (script, resolving_ref) = hooks::resolve_script(&self.world, obj, hook_name)?;
        let resolving_ref = resolving_ref.to_string();
        let mut script = script.clone();
        if resolving_ref != this_ref {
            script.state = obj.script.as_ref().map(|s| s.state.clone()).unwrap_or_default();
        }
        Some((script, resolving_ref))
    }

    fn fire_tick_hook_with_args(
        &mut self,
        this_ref: &str,
        hook_name: &str,
        args: Option<&str>,
    ) -> Result<(), String> {
        let (script, resolving_ref) = match self.resolve_hook_script(this_ref, hook_name) {
            Some(x) => x,
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
                &script,
                hook_name,
                &resolving_ref,
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
                None,
            )
            .map_err(|e| annotate_archetype_error(e.to_string(), this_ref, &resolving_ref))?;

        let effects = softcode::apply_batch(&mut self.world, &result.batch)?;
        self.world.next_id = dbref_counter.get();
        self.invalidate_libs_touched_by(&result.batch);
        self.deliver_effects(&effects, this_ref);

        self.write_back_script_state(this_ref, hook_name, result.state);

        Ok(())
    }

    fn fire_tick_hook_named(&mut self, this_ref: &str, hook_name: &str) -> Result<(), String> {
        self.fire_tick_hook_with_args(this_ref, hook_name, None)
    }

    /// Persist the object's `state` table after a hook run. Only `on_tick`
    /// hooks read/write `state` (see [`SoftcodeRuntime::run_hook`]), so other
    /// hooks return an empty map — don't clobber the stored state with it.
    ///
    /// Uses [`hooks::ensure_own_state_slot`] rather than requiring `obj.script`
    /// to already exist: an instance that only *delegates* its `on_tick` (no
    /// script of its own) still needs somewhere of its own to persist `state`
    /// between runs, since `state` is never resolved from the archetype.
    fn write_back_script_state(
        &mut self,
        this_ref: &str,
        hook_name: &str,
        state: HashMap<String, serde_json::Value>,
    ) {
        if hook_name == "on_tick"
            && let Some(obj) = self.world.get_mut(this_ref)
        {
            hooks::ensure_own_state_slot(obj).state = state;
        }
    }

    /// Set an object's script (deriving its hooks) and bump the world's
    /// structural epoch so the derived indexes pick up any new/removed hook.
    /// The single funnel for native (non-softcode) script writes — `set_script`
    /// itself can't reach `World::bump_struct` across the layering boundary.
    /// Returns false if the ref doesn't resolve.
    fn set_object_script(&mut self, ref_id: &str, source: String) -> bool {
        let ok = match self.world.get_mut(ref_id) {
            Some(obj) => {
                hooks::set_script(obj, source);
                true
            }
            None => false,
        };
        if ok {
            self.world.bump_struct();
        }
        ok
    }

    fn do_save(&mut self) {
        self.fire_lifecycle_hook("on_save");
        // Incremental: only objects touched since the last drain are
        // serialized. On a fresh DB the change set holds every object (the
        // loader's add_object calls mark them), so behavior matches a full
        // save there.
        let changes = self.world.drain_dirty();
        let saved = if changes.is_empty() {
            Ok(())
        } else {
            self.db.save_world_delta(&self.world, &changes)
        };
        match saved {
            Ok(()) => tracing::info!(
                objects = self.world.objects.len(),
                changed = changes.len(),
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
        if self.clock.is_some()
            && let Err(e) = self.db.save_game_minute(self.game_minute)
        {
            tracing::error!(error = %e, "Failed to save game clock");
        }
    }

    /// Hash of an API token as stored in the DB and looked up on each request.
    /// SHA-256, not `DefaultHasher` — this is a stored-credential digest, so it
    /// must be a cryptographic one-way function (RBAC audit H2). Changing the
    /// algorithm invalidates any tokens hashed by the old one, so those
    /// sessions must re-authenticate once.
    fn hash_token(token: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        format!("{:x}", hasher.finalize())
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

    // --- Softcode versioning + edit locks (docs/plans/softcode-versioning.md) ---

    fn wall_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Edit-lock lifetime: 30 minutes, renewed on each publish.
    const LOCK_TTL_SECS: i64 = 30 * 60;

    /// Resolve an author id to a display name: `system:*` pass through, an
    /// account id resolves to its username (falling back to the id).
    fn author_display(&self, author: &str) -> String {
        if author.starts_with("system:") {
            return author.to_string();
        }
        self.accounts
            .get(author)
            .map(|a| a.username.clone())
            .unwrap_or_else(|| author.to_string())
    }

    /// Append a version for a softcode write, suppressing a no-op (identical
    /// source to the current version). Returns the new version number, or the
    /// existing one when suppressed. `name` is `None` for object scripts.
    fn record_script_version(
        &self,
        ref_id: &str,
        name: Option<&str>,
        source: &str,
        author: &str,
        origin: &str,
        merged_from: Option<i64>,
    ) -> i64 {
        let kind = if name.is_some() { "lib" } else { "script" };
        let hash = crate::import_export::blake3_hex(source);
        match self.db.latest_script_version(ref_id, kind, name) {
            Ok(Some(latest)) if latest.hash == hash => latest.version,
            _ => self
                .db
                .append_script_version(ref_id, kind, name, source, &hash, author, origin, merged_from)
                .unwrap_or(0),
        }
    }

    /// Snapshot every File-origin script + lib into version history. Called
    /// after boot load, `@reload-world`, and `@import`, so a file/deploy change
    /// lands in history authored as `system:file`. Hash-suppressed, so an
    /// unchanged file is a no-op — only actual changes append a version.
    fn record_file_program_versions(&self) {
        let mut writes: Vec<(String, Option<String>, String)> = Vec::new();
        for obj in self.world.objects.values() {
            if let Some(s) = obj.script.as_ref()
                && s.origin == hooks::ProgramOrigin::File
                && (!s.source.is_empty() || !s.hooks.is_empty())
            {
                writes.push((obj.ref_id.clone(), None, s.source.clone()));
            }
            for (name, m) in &obj.libs {
                if m.origin == hooks::ProgramOrigin::File {
                    writes.push((obj.ref_id.clone(), Some(name.clone()), m.source.clone()));
                }
            }
        }
        for (ref_id, name, source) in writes {
            self.record_script_version(&ref_id, name.as_deref(), &source, "system:file", "file", None);
        }
    }

    /// The active (unexpired) edit lock on a target, if any.
    fn active_edit_lock(&self, ref_id: &str, name: Option<&str>) -> Option<&ScriptLock> {
        let key = (ref_id.to_string(), name.map(str::to_string));
        self.script_locks
            .get(&key)
            .filter(|l| l.expires_at > Self::wall_secs())
    }

    /// JSON for a lock, resolving the holder's display name.
    fn edit_lock_json(&self, lock: &ScriptLock) -> serde_json::Value {
        serde_json::json!({
            "held_by": lock.held_by,
            "held_by_name": self.author_display(&lock.held_by),
            "held_at": lock.held_at,
            "expires_at": lock.expires_at,
        })
    }

    /// Refuse a write when someone else holds an active edit lock. The holder
    /// (or a target with no active lock) passes.
    fn check_edit_lock(&self, ref_id: &str, name: Option<&str>, account: &str) -> Result<(), String> {
        if let Some(lock) = self.active_edit_lock(ref_id, name)
            && lock.held_by != account
        {
            return Err(format!(
                "locked by {} — claim released or expires at {}",
                self.author_display(&lock.held_by),
                lock.expires_at
            ));
        }
        Ok(())
    }

    /// Renew the holder's lock (push expiry out) after a successful publish.
    fn renew_edit_lock(&mut self, ref_id: &str, name: Option<&str>, account: &str) {
        let key = (ref_id.to_string(), name.map(str::to_string));
        if let Some(lock) = self.script_locks.get_mut(&key)
            && lock.held_by == account
        {
            lock.expires_at = Self::wall_secs() + Self::LOCK_TTL_SECS;
            let snapshot = lock.clone();
            let _ = self.db.save_script_lock(&snapshot);
        }
    }

    /// Resolve a versioned write against its base. Returns the source to store
    /// and the version it merged across (if any). `Err` carries a ready-made
    /// conflict `ApiResponse`. `current_live` is the object's current source.
    fn resolve_versioned_write(
        &self,
        ref_id: &str,
        name: Option<&str>,
        incoming: &str,
        base_version: Option<i64>,
        current_live: &str,
    ) -> Result<(String, Option<i64>), ApiResponse> {
        let base = match base_version {
            None => return Ok((incoming.to_string(), None)),
            Some(b) => b,
        };
        let kind = if name.is_some() { "lib" } else { "script" };
        let current = self.db.latest_script_version(ref_id, kind, name).ok().flatten();
        match current {
            Some(cur) if cur.version != base => {
                let base_src = self
                    .db
                    .get_script_version(ref_id, kind, name, base)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                match diffy::merge(&base_src, incoming, current_live) {
                    Ok(merged) => Ok((merged, Some(cur.version))),
                    Err(_) => Err(ApiResponse {
                        ok: false,
                        error: Some("conflict".to_string()),
                        data: Some(serde_json::json!({
                            "conflict": true,
                            "base": base_src,
                            "theirs": current_live,
                            "ours": incoming,
                            "current_version": cur.version,
                        })),
                    }),
                }
            }
            // base == current, or no history yet → clean apply.
            _ => Ok((incoming.to_string(), None)),
        }
    }

    /// Version + lock JSON for a read response (object script when `name` is
    /// `None`). Returns `(version, lock?)`.
    fn version_and_lock(
        &self,
        ref_id: &str,
        name: Option<&str>,
    ) -> (Option<i64>, Option<serde_json::Value>) {
        let kind = if name.is_some() { "lib" } else { "script" };
        let version = self
            .db
            .latest_script_version(ref_id, kind, name)
            .ok()
            .flatten()
            .map(|m| m.version);
        let lock = self.active_edit_lock(ref_id, name).map(|l| self.edit_lock_json(l));
        (version, lock)
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
        // `ListHooks` is static engine vocabulary (the known-hooks schema), not
        // world data, so it stays public — it leaks nothing (see
        // `list_hooks_is_public_and_matches_the_engine_vocabulary`).
        let is_public_read = matches!(&req, ApiRequest::ListHooks);

        // World reads enumerate live rooms/objects/exits — they leak area
        // layout and, bypassing `system:hidden`/`can_see`, hidden content, so
        // they require a valid token (any authenticated account, not just
        // Builder). RBAC audit M2.
        let is_world_read = matches!(
            &req,
            ApiRequest::ListRooms
                | ApiRequest::ListObjects { .. }
                | ApiRequest::ListExits { .. }
                | ApiRequest::Me
        );

        // The specific object a request mutates, if any — used by both the
        // `system:global` guard and the locked-definition guard below.
        let locked_target: Option<&str> = match &req {
            ApiRequest::SetAttribute { ref_id, .. }
            | ApiRequest::SetDescription { ref_id, .. }
            | ApiRequest::SetTitle { ref_id, .. }
            | ApiRequest::SetLocation { ref_id, .. }
            | ApiRequest::SetAliases { ref_id, .. }
            | ApiRequest::SetLock { ref_id, .. }
            | ApiRequest::ClearLock { ref_id, .. }
            | ApiRequest::UpdateExit { ref_id, .. }
            | ApiRequest::AddTag { ref_id, .. }
            | ApiRequest::RemoveTag { ref_id, .. }
            | ApiRequest::DeleteObject { ref_id, .. }
            | ApiRequest::SetScript { ref_id, .. }
            | ApiRequest::ClearScript { ref_id }
            | ApiRequest::SetArchetype { ref_id, .. }
            | ApiRequest::DetachObject { ref_id }
            | ApiRequest::SetLib { ref_id, .. }
            | ApiRequest::RemoveLib { ref_id, .. }
            | ApiRequest::RevertScript { ref_id, .. }
            | ApiRequest::LockScript { ref_id, .. }
            | ApiRequest::InkSave { ref_id, .. } => Some(ref_id.as_str()),
            _ => None,
        };

        // Populated for authenticated writes — the account performing this
        // request, threaded down to `SetProgram`/`RemoveProgram` below as
        // the program-version `author` (see
        // docs/plans/program-authoring.md Stage 3's "Author": "API
        // SetProgram — account_id, resolved in the auth block").
        let mut acting_account: Option<String> = None;

        if !is_public_read {
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

            if !is_world_read {
                let has_builder = self
                    .accounts
                    .get(&account_id)
                    .map(|a| a.has_scope(Scope::Builder))
                    .unwrap_or(false);

                if !has_builder {
                    return ApiResponse::error("Builder scope required");
                }

                let has_admin = self
                    .accounts
                    .get(&account_id)
                    .map(|a| a.has_scope(Scope::Admin))
                    .unwrap_or(false);

                // `Eval` is arbitrary admin code with the full write API — the
                // same gate `cmd_eval` applies for telnet (`Scope::Admin`, not
                // just `Builder`). A token that can eval owns the server.
                // `RunCommandAs` drives another player's session, so it's a
                // wizard-only tool too (admin-gated like telnet `@force`).
                if matches!(
                    &req,
                    ApiRequest::SaveWorld
                        | ApiRequest::Eval { .. }
                        | ApiRequest::Import { .. }
                        | ApiRequest::Export { .. }
                        | ApiRequest::PutMap { .. }
                        | ApiRequest::PutTerrain { .. }
                        | ApiRequest::RunCommandAs { .. }
                ) && !has_admin
                {
                    return ApiResponse::error("Admin scope required");
                }

                // RBAC audit H1: the `system:global` command surface is
                // admin-only to *author*. A `system:global` object's `cmd_*`
                // hooks run for every player regardless of location, so letting
                // a non-admin Builder rewrite one — or promote any object into
                // one via the tag — is privilege escalation into every player's
                // command path. Runtime state (softcode intents via
                // `apply_batch`) is deliberately not gated; only authoring is.
                // `system:managed` content stays Builder-editable (that is the
                // in-game building model) — the code tier is protected by
                // `system:locked` instead.
                if !has_admin {
                    if let Some(ref_id) = locked_target
                        && self.is_ref_global(ref_id)
                    {
                        return ApiResponse::error("system:global objects are admin-only to edit");
                    }
                    if let ApiRequest::AddTag { tag, .. } | ApiRequest::RemoveTag { tag, .. } = &req
                        && tag.as_str() == "system:global"
                    {
                        return ApiResponse::error(
                            "Only admins can set or remove the system:global tag",
                        );
                    }
                }
            }

            acting_account = Some(account_id);
        }

        // Locked-definition guard: an object stamped `system:locked` is
        // file-authoritative and refuses authoring edits at every REST entry
        // point that mutates a specific object's definition. Runtime state
        // (softcode intents via `apply_batch`) is deliberately NOT gated here
        // — only the authoring surface. See `is_object_locked`.
        if let Some(ref_id) = locked_target
            && self.is_ref_locked(ref_id)
        {
            return ApiResponse::error(Self::locked_error(ref_id));
        }

        // Authoring writes share softcode's mutation mechanism. The preamble
        // above is the *authorization* (Builder/Admin scope, `system:locked`,
        // `system:global`); `apply_batch` then supplies *integrity* (atomicity,
        // rollback, validation) with `authority = None` — system-trusted,
        // since the caller is already authorized. Most write actions have an
        // exact Intent twin and collapse into `authoring::write_batch`; the
        // exceptions (creation returning a ref, `SetLocation`'s player guard,
        // `DeleteObject`'s locked-cascade policy, program/asset authoring) fall
        // through to the match below. See `engine/authoring.rs` and ADR-0007.
        if let Some(result) = authoring::write_batch(&req) {
            return match result {
                Ok(batch) => match softcode::apply_batch(&mut self.world, &batch) {
                    Ok(_) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e),
                },
                Err(msg) => ApiResponse::error(msg),
            };
        }

        match req {
            // These are translated to an Intent batch and applied by the
            // `authoring::write_batch` dispatch above, so they never reach the
            // match at runtime. Listed explicitly (not a wildcard) to keep the
            // match exhaustive while still forcing a compile error if a newly
            // added ApiRequest variant goes unhandled.
            ApiRequest::SetAttribute { .. }
            | ApiRequest::SetDescription { .. }
            | ApiRequest::SetTitle { .. }
            | ApiRequest::SetAliases { .. }
            | ApiRequest::AddTag { .. }
            | ApiRequest::RemoveTag { .. }
            | ApiRequest::SetLock { .. }
            | ApiRequest::ClearLock { .. }
            | ApiRequest::UpdateExit { .. }
            | ApiRequest::SetArchetype { .. }
            | ApiRequest::DetachObject { .. } => {
                unreachable!("authoring write handled before match")
            }
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
                    "open_prefixes": ["on_", "cmd_"],
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
                        let hook_names: Vec<String> = obj
                            .script
                            .as_ref()
                            .map(|s| s.hooks.clone())
                            .unwrap_or_default();
                        let lib_names: Vec<String> = obj.libs.keys().cloned().collect();
                        let locks: &HashMap<String, String> = &obj.locks;

                        // Archetype summary (the direct parent), if any.
                        let archetype = obj
                            .archetype_ref
                            .as_deref()
                            .and_then(|a| self.world.get(a))
                            .map(|anc| serde_json::json!({
                                "ref_id": anc.ref_id,
                                "title": self.world.resolved_title(anc),
                            }));

                        // Per-attr provenance: own value wins, else the nearest
                        // ancestor that supplies the key (nearest-first walk).
                        // Own attrs are marked `overrides` when an ancestor also
                        // defines the key (so the builder can offer "revert to
                        // inherited").
                        let ancestors = self.world.archetype_ancestors(obj);
                        let ancestor_keys: std::collections::HashSet<&String> =
                            ancestors.iter().flat_map(|a| a.attrs.keys()).collect();
                        let mut resolved_attrs = serde_json::Map::new();
                        for (k, v) in &obj.attrs {
                            resolved_attrs.insert(
                                k.clone(),
                                serde_json::json!({
                                    "value": v,
                                    "source": "own",
                                    "overrides": ancestor_keys.contains(k),
                                }),
                            );
                        }
                        for anc in &ancestors {
                            for (k, v) in &anc.attrs {
                                resolved_attrs.entry(k.clone()).or_insert_with(|| {
                                    serde_json::json!({ "value": v, "source": anc.ref_id })
                                });
                            }
                        }

                        // Per-hook origin: every hook the object responds to
                        // (own + inherited), with the ref it resolves from.
                        let mut resolved_hooks: Vec<serde_json::Value> =
                            hooks::resolve_hook_names(&self.world, obj)
                                .into_iter()
                                .filter_map(|hook| {
                                    hooks::resolve_script(&self.world, obj, &hook).map(|(_, src)| {
                                        let source = if src == obj.ref_id { "own".to_string() } else { src.to_string() };
                                        serde_json::json!({ "hook": hook, "source": source })
                                    })
                                })
                                .collect();
                        resolved_hooks.sort_by(|a, b| a["hook"].as_str().cmp(&b["hook"].as_str()));

                        // How many objects delegate directly to this one.
                        let instance_count = self
                            .world
                            .objects
                            .values()
                            .filter(|o| o.archetype_ref.as_deref() == Some(obj.ref_id.as_str()))
                            .count();

                        // Effective (chain-resolved) title/description, so a pure
                        // delegate that sets neither still shows the inherited
                        // values in the builder rather than a blank field.
                        let resolved_title = self.world.resolved_title(obj);
                        let resolved_description = self.world.resolved_description(obj);

                        // Per-tag provenance: own tags first ("own"), then each
                        // ancestor (nearest first) contributes any tag not
                        // already seen. Tags are a union up the chain.
                        let mut resolved_tags: Vec<serde_json::Value> = Vec::new();
                        let mut seen_tags: std::collections::HashSet<String> =
                            std::collections::HashSet::new();
                        for t in &obj.tags {
                            let spec = t.as_spec();
                            if seen_tags.insert(spec.clone()) {
                                resolved_tags
                                    .push(serde_json::json!({ "tag": spec, "source": "own" }));
                            }
                        }
                        for anc in &ancestors {
                            for t in &anc.tags {
                                let spec = t.as_spec();
                                if seen_tags.insert(spec.clone()) {
                                    resolved_tags.push(serde_json::json!({
                                        "tag": spec,
                                        "source": anc.ref_id,
                                    }));
                                }
                            }
                        }

                        // Declared attribute schema, resolved up the archetype
                        // chain. Each descriptor is its serialized form plus a
                        // `source` ("own" or the ancestor ref it's inherited
                        // from), so the builder renders typed fields and knows
                        // which are declared here vs inherited.
                        let attr_schema: Vec<serde_json::Value> = self
                            .world
                            .resolved_attr_schema(obj)
                            .into_iter()
                            .map(|(d, source)| {
                                let mut v = serde_json::to_value(&d)
                                    .unwrap_or_else(|_| serde_json::json!({}));
                                if let Some(map) = v.as_object_mut() {
                                    map.insert("source".into(), serde_json::json!(source));
                                }
                                v
                            })
                            .collect();

                        ApiResponse::success(serde_json::json!({
                            "ref_id": obj.ref_id,
                            "key": obj.key,
                            "kind": obj.kind.to_string(),
                            "title": obj.title,
                            "resolved_title": resolved_title,
                            "description": obj.description,
                            "resolved_description": resolved_description,
                            "location_ref": obj.location_ref,
                            "target_ref": obj.target_ref,
                            "archetype_ref": obj.archetype_ref,
                            "archetype": archetype,
                            "instance_count": instance_count,
                            "locked": Self::is_object_locked(obj),
                            "attrs": obj.attrs,
                            "resolved_attrs": resolved_attrs,
                            "attr_schema": attr_schema,
                            "tags": tags,
                            "resolved_tags": resolved_tags,
                            "has_script": hooks::has_authored_script(obj),
                            "hooks": hook_names,
                            "resolved_hooks": resolved_hooks,
                            "libs": lib_names,
                            "locks": locks,
                            "aliases": obj.aliases.iter().collect::<Vec<_>>(),
                        }))
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
                // Relocating a live player by ref is a teleport — not a
                // builder-tier edit. Admins have the teleport command for it.
                // This is authoring-only policy: `Intent::Move` deliberately
                // allows moving players (builder teleporters need it), so the
                // guard lives here rather than in the shared mechanism.
                if self.world.get(&ref_id).map(|o| o.kind == Kind::Player).unwrap_or(false) {
                    return ApiResponse::error("Refusing to relocate a player via set_location");
                }
                // Route the mutation through the Intent seam (no announce, no
                // hooks — a builder placement is silent, exactly as before).
                let batch = softcode::IntentBatch::from_intents(vec![softcode::Intent::Move {
                    target: ref_id,
                    destination: location,
                    announce: false,
                    fire_hooks: false,
                }]);
                match softcode::apply_batch(&mut self.world, &batch) {
                    Ok(_) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::CloneObject { source, location, owner } => {
                // Reuse the softcode clone path (Intent::CloneObject) so the
                // system:*/file-key stripping and locked-source guard are shared
                // — permission is already gated (Builder) above. Pre-mint the
                // ref so we can return it.
                let ref_id = self.world.next_dbref();
                let batch = softcode::IntentBatch::from_intents(vec![softcode::Intent::CloneObject {
                    ref_id: ref_id.clone(),
                    source,
                    location,
                    owner,
                }]);
                match softcode::apply_batch(&mut self.world, &batch) {
                    Ok(_) => ApiResponse::success(serde_json::json!({ "ref_id": ref_id })),
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::RunCommandAs { ref_id, command } => {
                // Admin-gated above. Same forced-command gate as telnet `@force`.
                if !Self::forced_command_allowed(&command) {
                    return ApiResponse::error(
                        "That command cannot be forced (@-commands and quit are refused).",
                    );
                }
                let kind = match self.world.get(&ref_id) {
                    Some(o) => o.kind.clone(),
                    None => return ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                };
                if !matches!(kind, Kind::Player | Kind::Npc) {
                    return ApiResponse::error("Target is not a player or NPC");
                }
                if kind == Kind::Player && self.session_for_actor(&ref_id).is_none() {
                    return ApiResponse::error("That player has no active session");
                }
                if self.force_depth >= MAX_FORCE_DEPTH {
                    return ApiResponse::error("Forced-command depth limit reached");
                }
                self.dispatch_as_actor(&ref_id, &command);
                ApiResponse::ok()
            }
            ApiRequest::DeleteObject { ref_id, cascade } => {
                if self.world.get(&ref_id).map(|o| o.kind == Kind::Player).unwrap_or(false) {
                    return ApiResponse::error("Cannot delete player objects");
                }
                let instances: Vec<String> = self
                    .world
                    .objects
                    .values()
                    .filter(|o| o.archetype_ref.as_deref() == Some(ref_id.as_str()))
                    .map(|o| o.ref_id.clone())
                    .collect();
                if !instances.is_empty() {
                    if !cascade {
                        return ApiResponse::error(format!(
                            "{} is an archetype with live instances",
                            ref_id
                        ));
                    }
                    // A cascade flattens every instance (detach_object rewrites
                    // its title/description/attrs/tags/script), which would
                    // mutate a locked instance's definition indirectly — behind
                    // the target-only guard above. Refuse rather than edit a
                    // locked definition through the back door.
                    if let Some(locked_inst) = instances.iter().find(|r| self.is_ref_locked(r)) {
                        return ApiResponse::error(Self::locked_error(locked_inst));
                    }
                    // Flatten every instance before removing the archetype
                    // they depend on — cascade never orphans.
                    for instance_ref in &instances {
                        if let Err(e) = softcode::detach_object(&mut self.world, instance_ref) {
                            return ApiResponse::error(e);
                        }
                    }
                }
                if self.world.remove_object(&ref_id).is_some() {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("No object with ref '{}'", ref_id))
                }
            }
            ApiRequest::SetScript { ref_id, source, base_version } => {
                let account = acting_account.clone().unwrap_or_default();
                if let Err(e) = self.softcode.check_syntax(&source) {
                    return ApiResponse::error(format!("Syntax error: {}", e));
                }
                if let Err(e) = self.check_edit_lock(&ref_id, None, &account) {
                    return ApiResponse::error(e);
                }
                let current_live = match self.world.get(&ref_id) {
                    Some(obj) => obj.script.as_ref().map(|s| s.source.clone()).unwrap_or_default(),
                    None => return ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                };
                let (final_source, merged_from) =
                    match self.resolve_versioned_write(&ref_id, None, &source, base_version, &current_live) {
                        Ok(pair) => pair,
                        Err(conflict) => return conflict,
                    };
                self.set_object_script(&ref_id, final_source.clone());
                let version =
                    self.record_script_version(&ref_id, None, &final_source, &account, "in_game", merged_from);
                self.renew_edit_lock(&ref_id, None, &account);
                ApiResponse::success(serde_json::json!({
                    "version": version,
                    "merged_from": merged_from,
                    "source": final_source,
                }))
            }
            ApiRequest::ClearScript { ref_id } => {
                let ok = match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        hooks::clear_script(obj);
                        true
                    }
                    None => false,
                };
                if ok {
                    self.world.bump_struct();
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("No object with ref '{}'", ref_id))
                }
            }
            ApiRequest::ListRefCandidates { ref_source } => {
                ApiResponse::success(serde_json::json!({
                    "candidates": self.ref_candidates(&ref_source),
                }))
            }
            ApiRequest::GetScript { ref_id } => {
                if self.world.get(&ref_id).is_none() {
                    return ApiResponse::error(format!("No object with ref '{}'", ref_id));
                }
                let (version, lock) = self.version_and_lock(&ref_id, None);
                let obj = self.world.get(&ref_id).unwrap();
                // When the object exists but carries no script yet, still return
                // an (empty) script object so `version`/`lock` can ride — a lock
                // can be claimed before the first script is written.
                let script = match obj.script.as_ref() {
                    Some(s) => serde_json::json!({
                        "source": s.source,
                        "hooks": s.hooks,
                        "enabled": s.enabled,
                        "version": version,
                        "lock": lock,
                    }),
                    None => serde_json::json!({
                        "source": "",
                        "hooks": [],
                        "enabled": true,
                        "version": version,
                        "lock": lock,
                    }),
                };
                ApiResponse::success(script)
            }
            ApiRequest::SetLib { ref_id, name, source, base_version } => {
                let account = acting_account.clone().unwrap_or_default();
                if self.softcode.is_shipped_module(&name) {
                    return ApiResponse::error(format!(
                        "'{}' is a shipped module — choose a different name for your library",
                        name
                    ));
                }
                if let Err(e) = self.softcode.check_syntax(&source) {
                    return ApiResponse::error(format!("Syntax error: {}", e));
                }
                if let Err(e) = self.check_edit_lock(&ref_id, Some(&name), &account) {
                    return ApiResponse::error(e);
                }
                let current_live = match self.world.get(&ref_id) {
                    Some(obj) => obj.libs.get(&name).map(|l| l.source.clone()).unwrap_or_default(),
                    None => return ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                };
                let (final_source, merged_from) = match self.resolve_versioned_write(
                    &ref_id,
                    Some(&name),
                    &source,
                    base_version,
                    &current_live,
                ) {
                    Ok(pair) => pair,
                    Err(conflict) => return conflict,
                };
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        hooks::set_lib(obj, &name, final_source.clone(), hooks::ProgramOrigin::InGame);
                    }
                    None => return ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
                self.softcode.invalidate_module_cache();
                let version = self.record_script_version(
                    &ref_id,
                    Some(&name),
                    &final_source,
                    &account,
                    "in_game",
                    merged_from,
                );
                self.renew_edit_lock(&ref_id, Some(&name), &account);
                ApiResponse::success(serde_json::json!({
                    "version": version,
                    "merged_from": merged_from,
                    "source": final_source,
                }))
            }
            ApiRequest::Me => {
                let account_id = acting_account.clone().unwrap_or_default();
                match self.accounts.get(&account_id) {
                    Some(acc) => ApiResponse::success(serde_json::json!({
                        "account_id": acc.id,
                        "username": acc.username,
                        "scopes": acc.scope_labels(),
                        "email": acc.email,
                        "active_character": acc.active_character,
                    })),
                    None => ApiResponse::error("Unknown account"),
                }
            }
            ApiRequest::ListScriptVersions { ref_id, name } => {
                let kind = if name.is_some() { "lib" } else { "script" };
                match self.db.list_script_versions(&ref_id, kind, name.as_deref()) {
                    Ok(versions) => {
                        let out: Vec<_> = versions
                            .iter()
                            .map(|m| {
                                serde_json::json!({
                                    "version": m.version,
                                    "author": m.author,
                                    "author_name": self.author_display(&m.author),
                                    "origin": m.origin,
                                    "created_at": m.created_at,
                                    "hash": m.hash,
                                    "merged_from": m.merged_from,
                                })
                            })
                            .collect();
                        ApiResponse::success(serde_json::json!({ "versions": out }))
                    }
                    Err(e) => ApiResponse::error(format!("{e}")),
                }
            }
            ApiRequest::GetScriptVersion { ref_id, name, version } => {
                let kind = if name.is_some() { "lib" } else { "script" };
                match self.db.get_script_version(&ref_id, kind, name.as_deref(), version) {
                    Ok(Some(source)) => ApiResponse::success(serde_json::json!({ "source": source })),
                    Ok(None) => {
                        ApiResponse::error(format!("No version {} for '{}'", version, ref_id))
                    }
                    Err(e) => ApiResponse::error(format!("{e}")),
                }
            }
            ApiRequest::RevertScript { ref_id, name, version } => {
                let account = acting_account.clone().unwrap_or_default();
                if let Err(e) = self.check_edit_lock(&ref_id, name.as_deref(), &account) {
                    return ApiResponse::error(e);
                }
                let kind = if name.is_some() { "lib" } else { "script" };
                let source = match self.db.get_script_version(&ref_id, kind, name.as_deref(), version) {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        return ApiResponse::error(format!("No version {} for '{}'", version, ref_id))
                    }
                    Err(e) => return ApiResponse::error(format!("{e}")),
                };
                if let Err(e) = self.softcode.check_syntax(&source) {
                    return ApiResponse::error(format!("Syntax error in stored version: {}", e));
                }
                match &name {
                    None => {
                        if !self.set_object_script(&ref_id, source.clone()) {
                            return ApiResponse::error(format!("No object with ref '{}'", ref_id));
                        }
                    }
                    Some(n) => {
                        match self.world.get_mut(&ref_id) {
                            Some(obj) => {
                                hooks::set_lib(obj, n, source.clone(), hooks::ProgramOrigin::InGame)
                            }
                            None => {
                                return ApiResponse::error(format!("No object with ref '{}'", ref_id))
                            }
                        }
                        self.softcode.invalidate_module_cache();
                    }
                }
                let new_version =
                    self.record_script_version(&ref_id, name.as_deref(), &source, &account, "in_game", None);
                self.renew_edit_lock(&ref_id, name.as_deref(), &account);
                ApiResponse::success(serde_json::json!({ "version": new_version, "source": source }))
            }
            ApiRequest::LockScript { ref_id, name } => {
                let account = acting_account.clone().unwrap_or_default();
                if let Some(existing) = self.active_edit_lock(&ref_id, name.as_deref())
                    && existing.held_by != account
                {
                    let data = self.edit_lock_json(existing);
                    return ApiResponse {
                        ok: false,
                        error: Some(format!(
                            "locked by {}",
                            self.author_display(&existing.held_by)
                        )),
                        data: Some(data),
                    };
                }
                let now = Self::wall_secs();
                let lock = ScriptLock {
                    ref_id: ref_id.clone(),
                    name: name.clone(),
                    held_by: account.clone(),
                    held_at: now,
                    expires_at: now + Self::LOCK_TTL_SECS,
                };
                let _ = self.db.save_script_lock(&lock);
                let json = self.edit_lock_json(&lock);
                self.script_locks.insert((ref_id.clone(), name.clone()), lock);
                ApiResponse::success(json)
            }
            ApiRequest::UnlockScript { ref_id, name } => {
                let account = acting_account.clone().unwrap_or_default();
                let is_admin = self
                    .accounts
                    .get(&account)
                    .map(|a| a.is_admin())
                    .unwrap_or(false);
                let key = (ref_id.clone(), name.clone());
                match self.script_locks.get(&key) {
                    Some(lock) if lock.held_by == account || is_admin => {
                        self.script_locks.remove(&key);
                        let _ = self.db.delete_script_lock(&ref_id, name.as_deref());
                        ApiResponse::ok()
                    }
                    Some(_) => ApiResponse::error(
                        "Lock held by another builder — only the holder or an admin can release it",
                    ),
                    None => {
                        let _ = self.db.delete_script_lock(&ref_id, name.as_deref());
                        ApiResponse::ok()
                    }
                }
            }
            ApiRequest::RemoveLib { ref_id, name } => {
                match self.world.get_mut(&ref_id) {
                    Some(obj) => {
                        if hooks::remove_lib(obj, &name) {
                            self.softcode.invalidate_module_cache();
                        }
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("No object with ref '{}'", ref_id)),
                }
            }
            ApiRequest::CreateLibrary { name } => {
                if self.find_lib_object_ref(&name).is_some() {
                    return ApiResponse::error(format!(
                        "A library named '{}' already exists — open it to edit.",
                        name
                    ));
                }
                let starter =
                    format!("-- {name} library — loaded with require(\"{name}\")\nlocal M = {{}}\n\nreturn M\n");
                match self.upsert_library(&name, &starter) {
                    Ok((ref_id, _)) => {
                        ApiResponse::success(serde_json::json!({ "ref_id": ref_id, "name": name }))
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::ListLibs { ref_id } => {
                let entries: Option<Vec<(String, String)>> = self.world.get(&ref_id).map(|obj| {
                    obj.libs
                        .iter()
                        .map(|(name, m)| (name.clone(), m.source.clone()))
                        .collect()
                });
                match entries {
                    Some(mut entries) => {
                        entries.sort_by(|a, b| a.0.cmp(&b.0));
                        let libs: Vec<serde_json::Value> = entries
                            .into_iter()
                            .map(|(name, source)| {
                                let (version, lock) = self.version_and_lock(&ref_id, Some(&name));
                                serde_json::json!({
                                    "name": name,
                                    "source": source,
                                    "version": version,
                                    "lock": lock,
                                })
                            })
                            .collect();
                        ApiResponse::success(serde_json::json!(libs))
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
                // A script that's only the empty state-only stub
                // (`hooks::ensure_own_state_slot` — an instance that ticks
                // purely on an inherited `on_tick`) doesn't count as "has a
                // script" here: nobody authored it, so it shouldn't appear
                // as a program to edit.
                let mut out: Vec<serde_json::Value> = self
                    .world
                    .objects
                    .values()
                    .filter(|o| hooks::has_authored_script(o) || !o.libs.is_empty())
                    .map(|o| {
                        let mut hooks: Vec<String> = o
                            .script
                            .as_ref()
                            .map(|s| s.hooks.clone())
                            .unwrap_or_default();
                        hooks.sort();
                        let mut libs: Vec<String> = o.libs.keys().cloned().collect();
                        libs.sort();
                        serde_json::json!({
                            "ref_id": o.ref_id,
                            "key": o.key,
                            "title": o.title,
                            "kind": o.kind.to_string(),
                            "area": Self::room_area(o),
                            "has_script": hooks::has_authored_script(o),
                            "hooks": hooks,
                            "libs": libs,
                            "locked": Self::is_object_locked(o),
                        })
                    })
                    .collect();
                out.sort_by(|a, b| a["ref_id"].as_str().cmp(&b["ref_id"].as_str()));
                // Second pass (world borrow released): attach the object
                // script's version + any active edit lock, so the tree renders
                // both without a per-node call.
                for v in out.iter_mut() {
                    if let Some(ref_id) = v["ref_id"].as_str().map(str::to_string) {
                        let (version, lock) = self.version_and_lock(&ref_id, None);
                        v["version"] = serde_json::json!(version);
                        v["lock"] = lock.unwrap_or(serde_json::Value::Null);
                    }
                }
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

                // Compile every object script and lib module; report the ones
                // that don't parse.
                for o in self.world.objects.values() {
                    if let Some(script) = o.script.as_ref()
                        && let Err(err) = self.softcode.check_syntax(&script.source)
                    {
                        problems.push(serde_json::json!({
                            "kind": "syntax_error", "severity": "high",
                            "ref": o.ref_id, "key": o.key, "hook": "script", "message": err,
                        }));
                    }
                    for (name, lib) in &o.libs {
                        if let Err(err) = self.softcode.check_syntax(&lib.source) {
                            problems.push(serde_json::json!({
                                "kind": "syntax_error", "severity": "high",
                                "ref": o.ref_id, "key": o.key, "hook": format!("lib_{}", name), "message": err,
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
            ApiRequest::RunTests { source, file, ref_id } => {
                // Object mode: run the test_* functions embedded in one object's
                // own script (ctx.this bound to it). Takes precedence.
                if let Some(ref_id) = ref_id {
                    let label = format!("{} (embedded)", ref_id);
                    return match self.run_object_tests(&ref_id) {
                        None => ApiResponse::error(format!("{} has no script", ref_id)),
                        Some(result) => {
                            let (mut passed, mut failed) = (0usize, 0usize);
                            let file = match result {
                                Ok(fr) => {
                                    let tests: Vec<serde_json::Value> = fr
                                        .tests
                                        .iter()
                                        .map(|tr| {
                                            if tr.passed { passed += 1; } else { failed += 1; }
                                            serde_json::json!({ "name": tr.name, "passed": tr.passed, "error": tr.error })
                                        })
                                        .collect();
                                    serde_json::json!({ "file": label, "tests": tests, "error": serde_json::Value::Null })
                                }
                                Err(e) => {
                                    failed += 1;
                                    serde_json::json!({ "file": label, "tests": [], "error": e.to_string() })
                                }
                            };
                            ApiResponse::success(serde_json::json!({
                                "files": [file], "passed": passed, "failed": failed,
                            }))
                        }
                    };
                }
                // Mirror `cmd_test`: ad-hoc `source`, a single named file, or
                // discover them all. Each non-lib file runs against a clone so
                // its writes never touch the live world (see the variant doc).
                let test_files: Vec<crate::loader::TestFile> = if let Some(src) = source {
                    vec![crate::loader::TestFile {
                        path: std::path::PathBuf::from("<scratch>"),
                        relative: "<scratch>".to_string(),
                        source: src,
                        is_lib: false,
                    }]
                } else {
                    let game_dir = match &self.game_dir {
                        Some(g) => g.clone(),
                        None => return ApiResponse::error("No game_dir configured"),
                    };
                    let game_path = std::path::Path::new(&game_dir);
                    match file {
                        Some(rel) => {
                            let path = game_path.join(&rel);
                            match std::fs::read_to_string(&path) {
                                Ok(source) => {
                                    let is_lib = path.starts_with(game_path.join("lib"));
                                    vec![crate::loader::TestFile { path, relative: rel, source, is_lib }]
                                }
                                Err(e) => {
                                    return ApiResponse::error(format!("Cannot read '{}': {}", rel, e));
                                }
                            }
                        }
                        None => crate::loader::discover_test_files(game_path),
                    }
                };

                let mut files: Vec<serde_json::Value> = Vec::new();
                let mut passed = 0usize;
                let mut failed = 0usize;
                for tf in &test_files {
                    let world = if tf.is_lib { None } else { Some(self.world.clone()) };
                    match self.softcode.run_tests(
                        &tf.source,
                        &tf.relative,
                        world.as_ref(),
                        None,
                        &self.map_templates,
                        softcode::Budget::default(),
                    ) {
                        Ok(fr) => {
                            let tests: Vec<serde_json::Value> = fr
                                .tests
                                .iter()
                                .map(|tr| {
                                    if tr.passed {
                                        passed += 1;
                                    } else {
                                        failed += 1;
                                    }
                                    serde_json::json!({
                                        "name": tr.name,
                                        "passed": tr.passed,
                                        "error": tr.error,
                                    })
                                })
                                .collect();
                            files.push(serde_json::json!({
                                "file": tf.relative, "tests": tests, "error": serde_json::Value::Null,
                            }));
                        }
                        // A file that won't even compile is one failure, not a
                        // silent gap — surface it as the file's `error`.
                        Err(e) => {
                            failed += 1;
                            files.push(serde_json::json!({
                                "file": tf.relative, "tests": [], "error": e.to_string(),
                            }));
                        }
                    }
                }
                ApiResponse::success(serde_json::json!({
                    "files": files, "passed": passed, "failed": failed,
                }))
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
            ApiRequest::EvalPreview { source } => {
                // Same actor resolution as `Eval` — the account's active
                // character, so `actor`/`room`/`this` read sensibly.
                let actor_ref = acting_account
                    .as_deref()
                    .and_then(|id| self.accounts.get(id))
                    .and_then(|a| a.active_character.clone())
                    .unwrap_or_default();
                let room_ref = self.world.get(&actor_ref).and_then(|a| a.location_ref.clone());
                // run_eval borrows the world immutably and returns the intent
                // batch without touching anything — we simply never apply it.
                let dbref_counter = Rc::new(Cell::new(self.world.next_id));
                let result = self.softcode.run_eval(
                    &self.world,
                    &source,
                    &actor_ref,
                    room_ref.as_deref(),
                    Budget::for_eval(),
                    dbref_counter,
                    &self.themes,
                    &self.map_templates,
                    &self.scheduled_hooks,
                    self.tick_count,
                );
                match result {
                    Ok(r) => {
                        let writes: Vec<String> = r
                            .batch
                            .intents
                            .iter()
                            .map(|i| describe_intent(i, &self.world))
                            .collect();
                        ApiResponse::success(serde_json::json!({
                            "returned": r.returned,
                            "writes": writes,
                            "write_count": r.batch.len(),
                        }))
                    }
                    Err(e) => ApiResponse::error(format!("Eval error: {}", e)),
                }
            }
            ApiRequest::PreviewHook { ref_id, hook, source, actor_ref, room_ref } => {
                // The source to fire: the unsaved buffer if given, else the
                // object's saved script (the hook function lives inside it).
                let src = match source {
                    Some(s) => s,
                    None => match self.world.get(&ref_id).and_then(|o| o.script.as_ref()) {
                        Some(s) => s.source.clone(),
                        None => {
                            return ApiResponse::error(format!("{} has no script", ref_id));
                        }
                    },
                };
                if let Err(e) = self.softcode.check_syntax(&src) {
                    return ApiResponse::error(format!("Syntax error: {}", e));
                }
                // actor → chosen, else the caller's character; room → chosen,
                // else the actor's location, else `this` when it's a room.
                let actor = actor_ref.unwrap_or_else(|| {
                    acting_account
                        .as_deref()
                        .and_then(|id| self.accounts.get(id))
                        .and_then(|a| a.active_character.clone())
                        .unwrap_or_default()
                });
                let room = room_ref
                    .or_else(|| self.world.get(&actor).and_then(|a| a.location_ref.clone()))
                    .or_else(|| {
                        self.world
                            .get(&ref_id)
                            .filter(|o| o.kind == Kind::Room)
                            .map(|_| ref_id.clone())
                    });
                let script = hooks::ObjectScript {
                    source: src,
                    enabled: true,
                    state: std::collections::HashMap::new(),
                    origin: Default::default(),
                    hooks: vec![hook.clone()],
                };
                let dbref_counter = Rc::new(Cell::new(self.world.next_id));
                let result = self.softcode.run_hook(
                    &self.world,
                    &script,
                    &hook,            // hook to fire
                    &ref_id,          // resolving_ref — this synthetic script plays ref_id's own
                    &ref_id,          // this
                    &actor,           // actor
                    room.as_deref(),  // room
                    None,             // args
                    Budget::for_eval(),
                    dbref_counter,
                    &self.themes,
                    &self.map_templates,
                    &self.scheduled_hooks,
                    self.tick_count,
                    None,             // data (structured trigger payload)
                );
                match result {
                    Ok(r) => {
                        let writes: Vec<String> = r
                            .batch
                            .intents
                            .iter()
                            .map(|i| describe_intent(i, &self.world))
                            .collect();
                        ApiResponse::success(serde_json::json!({
                            "writes": writes,
                            "write_count": r.batch.len(),
                            "denied": r.denied,
                        }))
                    }
                    Err(e) => ApiResponse::error(format!("Eval error: {}", e)),
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
                            self.record_file_program_versions();
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
            ApiRequest::InkPlayStart { ref_id, source } => {
                let src = match source {
                    Some(s) => s,
                    None => match self
                        .world
                        .get(&ref_id)
                        .and_then(|o| o.attrs.get("_ink_source"))
                        .and_then(|v| v.as_str())
                    {
                        Some(s) => s.to_string(),
                        None => return ApiResponse::error("No dialogue to play"),
                    },
                };
                let key = Self::ink_preview_key(acting_account.as_deref());
                match self
                    .softcode
                    .ink_runtime()
                    .borrow_mut()
                    .start_conversation(&key, &ref_id, &src, None)
                {
                    Ok(out) => ApiResponse::success(ink_output_json(&out)),
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::InkPlayContinue { ref_id } => {
                let key = Self::ink_preview_key(acting_account.as_deref());
                match self
                    .softcode
                    .ink_runtime()
                    .borrow_mut()
                    .continue_story(&key, &ref_id)
                {
                    Ok(out) => ApiResponse::success(ink_output_json(&out)),
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::InkPlayChoose { ref_id, index } => {
                let key = Self::ink_preview_key(acting_account.as_deref());
                match self
                    .softcode
                    .ink_runtime()
                    .borrow_mut()
                    .choose(&key, &ref_id, index)
                {
                    Ok(out) => ApiResponse::success(ink_output_json(&out)),
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::InkPlayEnd { ref_id } => {
                let key = Self::ink_preview_key(acting_account.as_deref());
                // Preview state is disposable — never persisted back to the NPC.
                let _ = self
                    .softcode
                    .ink_runtime()
                    .borrow_mut()
                    .end_conversation(&key, &ref_id, false);
                ApiResponse::ok()
            }
        }
    }

    /// Conversation key for builder playtests. Namespaced per acting account so
    /// a preview can never collide with a real player's live conversation with
    /// the same NPC (whose key is the player's own ref).
    fn ink_preview_key(account: Option<&str>) -> String {
        format!("@ink-preview:{}", account.unwrap_or("anon"))
    }

    fn handle_connect(&mut self, session_id: String, tx: mpsc::UnboundedSender<ClientMessage>) {
        let session = Session {
            tx,
            state: SessionState::PromptUsername,
            editor: None,
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
                    .map(|o| self.world.display_name(o))
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

                self.world.add_tag(actor_ref, crate::world::Tag {
                    category: "system".to_string(),
                    key: "offline".to_string(),
                });
                if !name.is_empty() {
                    self.broadcast_to_all(&format!("{} has disconnected.\r\n", name), session_id);
                }
            }
            tracing::info!(session_id, "Player disconnected");
        }

        // Forget the last-sent terrain legend so a reconnect gets it fresh.
        self.legend_sent_map.remove(session_id);

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

        // RBAC audit M1: throttle repeated failed logins per username before
        // spending Argon2 CPU, so login can't be used as a DoS amplifier and
        // online guessing is slowed. The window resets after LOCKOUT elapses or
        // on any successful login.
        const MAX_FAILURES: u32 = 5;
        const LOCKOUT: std::time::Duration = std::time::Duration::from_secs(30);
        let fail_key = username.to_lowercase();
        if let Some((count, first)) = self.login_failures.get(&fail_key)
            && *count >= MAX_FAILURES
        {
            if first.elapsed() < LOCKOUT {
                self.send(
                    session_id,
                    "Too many failed attempts. Try again shortly.\r\nUsername: ",
                );
                if let Some(session) = self.sessions.get_mut(session_id) {
                    session.state = SessionState::PromptUsername;
                }
                return;
            }
            // Window elapsed — forget the failures and let this attempt run.
            self.login_failures.remove(&fail_key);
        }

        match self.accounts.authenticate(&username, input) {
            Ok(account) => {
                let account_id = account.id.clone();
                let characters = account.characters.clone();
                let active_character = account.active_character.clone().unwrap_or_default();
                self.login_failures.remove(&fail_key);

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
                let entry = self
                    .login_failures
                    .entry(fail_key)
                    .or_insert((0, std::time::Instant::now()));
                entry.0 += 1;
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
                .map(|o| self.world.display_name(o))
                .unwrap_or_else(|| ref_id.clone());
            let location = self
                .world
                .get(ref_id)
                .and_then(|o| o.location_ref.as_ref())
                .and_then(|loc| self.world.get(loc))
                .map(|r| self.world.display_name(r))
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
            self.world.bump_struct();
            if needs_fix {
                self.world.relocate(character_ref, Some(spawn_room_ref.clone()));
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

    /// Put a session into (or out of) a multi-line editor. Engine-owned session
    /// state — the input router keys on it in `handle_game_input`.
    fn set_editor(&mut self, session_id: &str, editor: Option<EditorMode>) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.editor = editor;
        }
    }

    fn handle_game_input(&mut self, session_id: &str, input: &str) {
        if input.is_empty() {
            return;
        }

        // Three identities, resolved once through the Session interface. Input
        // interception (softcode prompts, multi-line editors) belongs to the
        // *human's* stream, so it keys on the Character. Gameplay dispatch and
        // room/output routing act as the effective actor (the Puppet when one is
        // driven). Equal when not puppeting, so ordinary play is unchanged.
        let (character_ref, effective_ref, editor) = match self.sessions.get(session_id) {
            Some(session) => match (session.character(), session.effective_actor()) {
                (Some(c), Some(e)) => (c.to_string(), e.to_string(), session.editor),
                _ => return,
            },
            None => return,
        };

        // Check for pending prompt — intercept input before command dispatch.
        // The prompt callback is registered by softcode's `prompt()` on the
        // Character object (only Intents can reach it), so it reads the Character.
        if let Some(actor) = self.world.get(&character_ref) {
            let prompt_obj = actor.attrs.get("_prompt_object").and_then(|v| v.as_str()).map(String::from);
            let prompt_hook = actor.attrs.get("_prompt_hook").and_then(|v| v.as_str()).map(String::from);
            if let (Some(obj_ref), Some(hook)) = (prompt_obj, prompt_hook) {
                // Clear the prompt attrs before firing the hook
                if let Some(actor) = self.world.get_mut(&character_ref) {
                    actor.attrs.remove("_prompt_object");
                    actor.attrs.remove("_prompt_hook");
                }
                let room_ref = self.world.get(&character_ref).and_then(|o| o.location_ref.clone());
                let output = match self.fire_hook(&obj_ref, &hook, &character_ref, room_ref.as_deref(), Some(input)) {
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

        // Multi-line editor modes — engine-owned session state, keyed on the
        // Character's session (not an object attr).
        match editor {
            Some(EditorMode::Ink) => {
                self.handle_ink_editor_input(session_id, &character_ref, input);
                return;
            }
            Some(EditorMode::Eval) => {
                self.handle_eval_editor_input(session_id, &character_ref, input);
                return;
            }
            Some(EditorMode::Program) => {
                self.handle_program_editor_input(session_id, &character_ref, input);
                return;
            }
            None => {}
        }

        self.run_command(&character_ref, &effective_ref, Some(session_id), input);
    }

    /// Dispatch one command. `character_ref` is the account's **Character** —
    /// the authoring/ownership identity used by the `@`-verbs. `effective_ref`
    /// is the object **gameplay acts as**: the Puppet when one is driven, else
    /// the Character (see [`Session::effective_actor`]). The two are equal for a
    /// session-less NPC and whenever no Puppet is active, so ordinary play is
    /// unchanged. `session_id` is `Some` for a player — output and room data
    /// flow to their client and the session-only verbs (quit, who, the
    /// `@`-builder/admin verbs, character management, help) are available. It is
    /// `None` for a `run_command_as`-driven NPC, which runs only the gameplay
    /// subset with no client to echo to.
    fn run_command(
        &mut self,
        character_ref: &str,
        effective_ref: &str,
        session_id: Option<&str>,
        input: &str,
    ) {
        if input.is_empty() {
            return;
        }
        // Sentinel used by say/emote/whisper only to skip echoing to the
        // speaker's own session. `""` matches no real session, which is exactly
        // right for a session-less NPC (nothing to skip, nothing to echo).
        let sid = session_id.unwrap_or("");

        let (cmd, args) = match input.split_once(' ') {
            Some((c, a)) => (c.to_lowercase(), a.trim().to_string()),
            None => (input.to_lowercase(), String::new()),
        };

        // Gameplay presence follows the effective actor (the Puppet, when one is
        // driven), so a move/look reports the puppeted object's room.
        let room_before = self.world.get(effective_ref).and_then(|a| a.location_ref.clone());

        let output = match cmd.as_str() {
            // Gameplay commands — run as the effective actor. Available with or
            // without a session.
            "look" | "l" => self.cmd_look(effective_ref, &args),
            "say" | "\"" => {
                let msg = if cmd == "\"" {
                    input[1..].trim().to_string()
                } else {
                    args.clone()
                };
                self.cmd_say(sid, effective_ref, &msg)
            }
            "go" => self.cmd_go(effective_ref, &args),
            "inventory" | "inv" | "i" => commands::do_inventory(&self.world, effective_ref),
            "get" | "take" => self.cmd_get(effective_ref, &args),
            "put" | "place" => self.cmd_put(effective_ref, &args),
            "drop" => self.cmd_drop(effective_ref, &args),
            "use" => self.cmd_use(effective_ref, &args),
            "examine" | "ex" => commands::do_examine(&self.world, effective_ref, &args),
            "whisper" => self.cmd_whisper(sid, effective_ref, &args),
            "emote" | "pose" | ":" => {
                let msg = if cmd == ":" {
                    input[1..].trim().to_string()
                } else {
                    args.clone()
                };
                self.do_emote(sid, effective_ref, &msg)
            }

            // Session-only commands (quit, who, the `@`-builder/admin verbs,
            // character management, help). Unreachable for a session-less actor
            // — a forced NPC never gets here (the forced gate bans `@`/quit), and
            // any other verb falls through to the game `cmd_*` dispatch below.
            _ if session_id.is_some() => {
                let session_id = sid;
                match cmd.as_str() {
                    "quit" | "q" => {
                        self.send(session_id, "Farewell.\r\n");
                        self.handle_disconnect(session_id);
                        return;
                    }
                    "who" => self.do_who(session_id),
                    "@password" => self.cmd_password(session_id, &args),
                    "@email" => self.cmd_email(session_id, &args),
                    "@dig" => self.cmd_dig(session_id, character_ref, &args),
                    "@open" => self.cmd_open(session_id, character_ref, &args),
                    "@describe" | "@desc" => self.cmd_describe(session_id, character_ref, &args),
                    "@create" => self.cmd_create(session_id, character_ref, &args),
                    "@destroy" => self.cmd_destroy(session_id, character_ref, &args),
                    "@set" => self.cmd_set(session_id, character_ref, &args),
                    "@teleport" | "@tel" => self.cmd_teleport(session_id, character_ref, &args),
                    "@name" => self.cmd_name(session_id, character_ref, &args),
                    "@program" => self.cmd_program(session_id, character_ref, &args),
                    "@programs" => self.cmd_programs(session_id, character_ref, &args),
                    "@rmprogram" => self.cmd_rmprogram(session_id, character_ref, &args),
                    "@tag" => self.cmd_tag(session_id, character_ref, &args),
                    "@untag" => self.cmd_untag(session_id, character_ref, &args),
                    "@script" => self.cmd_script(session_id, character_ref, &args),
                    "@scripts" => self.cmd_scripts(session_id),
                    "@rmscript" => self.cmd_rmscript(session_id, character_ref, &args),
                    "@script-interval" => self.cmd_script_interval(session_id, &args),
                    "@lib" => self.cmd_lib(session_id, character_ref, &args),
                    "@libs" => self.cmd_libs(session_id),
                    "@rmlib" => self.cmd_rmlib(session_id, character_ref, &args),
                    "@lock" => self.cmd_lock(session_id, character_ref, &args),
                    "@alias" => self.cmd_alias(session_id, character_ref, &args),
                    "@clone" => self.cmd_clone(session_id, character_ref, &args),
                    "@force" => self.cmd_force(session_id, character_ref, &args),
                    "@unlock" => self.cmd_unlock(session_id, character_ref, &args),
                    "@locks" => self.cmd_locks(session_id, character_ref, &args),

                    "@charlist" => self.cmd_charlist(session_id),
                    "@charcreate" => self.cmd_charcreate(session_id, &args),
                    "@charswitch" => self.cmd_charswitch(session_id, &args),
                    "@chardelete" => self.cmd_chardelete(session_id, &args),
                    // Ownership is the Character's, never the Puppet's — a
                    // puppeteer must not gain authority through the object.
                    "@puppet" => self.cmd_puppet(session_id, character_ref, &args),
                    "@unpuppet" => self.cmd_unpuppet(session_id),

                    "@chown" => self.cmd_chown(session_id, &args),
                    "@archetype" | "@chparent" => self.cmd_archetype(session_id, character_ref, &args),
                    "@dialogue" | "@dialog" => self.cmd_dialogue(session_id, character_ref, &args),

                    "@grant" => self.cmd_grant(session_id, &args),
                    "@revoke" => self.cmd_revoke(session_id, &args),
                    "@scopes" => self.cmd_scopes(session_id, &args),
                    "@wall" => self.cmd_wall(session_id, &args),
                    "@boot" => self.cmd_boot(session_id, &args),
                    "@save" => self.cmd_save(session_id),
                    "@shutdown" => self.cmd_shutdown(session_id),
                    "@reload-world" => self.cmd_reload_world(session_id),
                    "@eval" => self.cmd_eval(session_id, character_ref, &args),
                    "@import" => self.cmd_import(session_id, &args),
                    "@export" => self.cmd_export(session_id, &args),
                    "@maxchars" => self.cmd_maxchars(session_id, &args),
                    "@test" => self.cmd_test(session_id, &args),
                    "@reload" => self.cmd_reload(session_id, character_ref, &args),

                    "@token" | "@tokens" => self.cmd_token(session_id, &args),
                    "@display" => self.cmd_display(character_ref, &args),

                    "help" | "?" => {
                        let is_builder = self.session_has_scope(session_id, Scope::Builder);
                        let is_admin = self.session_has_scope(session_id, Scope::Admin);
                        commands::do_help_with_roles(is_builder, is_admin)
                    }
                    // An unknown verb is a custom game `cmd_*` hook — gameplay,
                    // so it runs as the effective actor.
                    _ => self.dispatch_fallback(effective_ref, &cmd, &args),
                }
            }
            // Session-less actor (an NPC): an unknown verb resolves to a game
            // `cmd_*` hook, same as for players.
            _ => self.dispatch_fallback(effective_ref, &cmd, &args),
        };

        if let Some(session_id) = session_id {
            self.send(session_id, &output);

            // Room panels follow the effective actor's location.
            let room_after = self.world.get(effective_ref).and_then(|a| a.location_ref.clone());
            if matches!(cmd.as_str(), "look" | "l") || room_before != room_after {
                self.send_room_data(session_id, effective_ref);
                self.send_commands(session_id, effective_ref);
            }
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

        if self.is_ref_locked(&target_ref) {
            return format!("{}\r\n", Self::locked_error(&target_ref));
        }
        if let Some(obj) = self.world.get_mut(&target_ref) {
            obj.description = desc;
            format!("Description set on {}.\r\n", target_ref)
        } else {
            "Target not found.\r\n".to_string()
        }
    }

    /// `@archetype <ref> <archetype-ref>` sets `<ref>`'s archetype (delegation);
    /// `@archetype <ref> none` detaches it (flatten-then-stop). The telnet
    /// counterpart of the builder's Set/Detach — same guards as the REST
    /// `SetArchetype`/`DetachObject` handlers.
    fn cmd_archetype(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.len() != 2 {
            return "Usage: @archetype <ref> <archetype-ref>  (or: @archetype <ref> none)\r\n"
                .to_string();
        }
        let target = self.resolve_object_ref(actor_ref, parts[0]);
        if self.world.get(&target).is_none() {
            return format!("No object with ref '{}'.\r\n", parts[0]);
        }
        if !self.can_modify_object(session_id, actor_ref, &target) {
            return "Permission denied (not owner).\r\n".to_string();
        }
        // Setting/clearing delegation is an authoring edit — a locked,
        // file-authoritative object refuses it, like @name/@describe.
        if self.is_ref_locked(&target) {
            return format!("{}\r\n", Self::locked_error(&target));
        }
        if parts[1].eq_ignore_ascii_case("none") {
            return match softcode::detach_object(&mut self.world, &target) {
                Ok(()) => {
                    self.world.bump_struct();
                    format!("{} detached — no longer delegates.\r\n", target)
                }
                Err(e) => format!("{}\r\n", e),
            };
        }
        let archetype = self.resolve_object_ref(actor_ref, parts[1]);
        if self.world.get(&archetype).is_none() {
            return format!("No archetype with ref '{}'.\r\n", parts[1]);
        }
        if self.world.would_cycle_archetype(&target, &archetype) {
            return format!("'{}' would create an archetype cycle.\r\n", archetype);
        }
        self.world.set_object_archetype(&target, Some(archetype.clone()));
        format!("{} now delegates to {}.\r\n", target, archetype)
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
            return "Usage: @destroy <ref> [--cascade]\r\n".to_string();
        }
        // `--cascade` opts into deleting an archetype that still has live
        // instances — see the same guard in apply_to's Intent::Destroy.
        let (target_ref, cascade) = match args.trim().strip_suffix("--cascade") {
            Some(rest) => (rest.trim(), true),
            None => (args.trim(), false),
        };
        if !self.can_modify_object(session_id, actor_ref, target_ref) {
            return "Permission denied (not owner).\r\n".to_string();
        }
        if self.is_ref_locked(target_ref) {
            return format!("{}\r\n", Self::locked_error(target_ref));
        }
        if self.world.get(target_ref).map(|o| o.kind == Kind::Player).unwrap_or(false) {
            return "Cannot destroy player objects.\r\n".to_string();
        }
        let instances: Vec<String> = self
            .world
            .objects
            .values()
            .filter(|o| o.archetype_ref.as_deref() == Some(target_ref))
            .map(|o| o.ref_id.clone())
            .collect();
        if !instances.is_empty() {
            if !cascade {
                return format!(
                    "{} is an archetype with live instances — pass --cascade to delete anyway.\r\n",
                    target_ref
                );
            }
            // A cascade flattens each instance (rewriting its definition), which
            // would mutate a locked instance indirectly — refuse rather than
            // edit a locked definition through the back door.
            if let Some(locked_inst) = instances.iter().find(|r| self.is_ref_locked(r)) {
                return format!("{}\r\n", Self::locked_error(locked_inst));
            }
            // Flatten every instance before the archetype they depend on is
            // removed — cascade means "detach then delete", never "delete
            // and orphan" (matches apply_to's Intent::Destroy handling).
            for instance_ref in &instances {
                if let Err(e) = softcode::detach_object(&mut self.world, instance_ref) {
                    tracing::warn!(target = %instance_ref, error = %e, "cascade detach failed");
                }
            }
        }
        if let Some(obj) = self.world.get(target_ref) {
            if obj.kind == Kind::Room {
                let occupants = self.world.objects_in(target_ref);
                if !occupants.is_empty() {
                    return "Cannot destroy a room with objects in it.\r\n".to_string();
                }
            }
            let name = self.world.display_name(obj);
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
                    self.world.remove_object(&r);
                }
            }
            let _ = self.fire_hook(target_ref, "on_destroy", actor_ref, None, None);
            self.world.remove_object(target_ref);
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
        if self.is_ref_locked(&resolved_ref) {
            return format!("{}\r\n", Self::locked_error(&resolved_ref));
        }

        let json_val: serde_json::Value = match serde_json::from_str(value) {
            Ok(v) => v,
            Err(_) => serde_json::Value::String(value.to_string()),
        };

        if let Some(obj) = self.world.get_mut(&resolved_ref) {
            obj.attrs.insert(attr_key.to_string(), json_val);
            let name = self.world.display_name(self.world.get(&resolved_ref).unwrap());
            format!("Set {}/{} on {}.\r\n", resolved_ref, attr_key, name)
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
        if self.is_ref_locked(&target_ref) {
            return format!("{}\r\n", Self::locked_error(&target_ref));
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

    /// Validate everything about a script write except the source itself:
    /// the target exists and the actor may modify it.
    /// Shared between the single-line and multi-line `@program` paths so
    /// entering the multi-line editor fails fast on a bad target instead of
    /// only discovering it after the user has typed a whole program.
    fn check_program_write(
        &self,
        session_id: &str,
        actor_ref: &str,
        target_ref: &str,
    ) -> Option<String> {
        if self.world.get(target_ref).is_none() {
            return Some(format!("No object with ref '{}'.\r\n", target_ref));
        }
        if !self.can_modify_object(session_id, actor_ref, target_ref) {
            return Some("Permission denied (not owner).\r\n".to_string());
        }
        if self.is_ref_locked(target_ref) {
            return Some(format!("{}\r\n", Self::locked_error(target_ref)));
        }
        None
    }

    /// Check syntax and write the object's whole script — the shared tail of
    /// both the single-line and multi-line `@program` paths.
    fn install_program(&mut self, session_id: &str, _actor_ref: &str, target_ref: &str, source: &str) -> String {
        if let Err(e) = self.softcode.check_syntax(source) {
            return format!("Syntax error in program: {}\r\n", e);
        }
        if !self.set_object_script(target_ref, source.to_string()) {
            return format!("No object with ref '{}'.\r\n", target_ref);
        }
        // Record the version, authored by the account behind this session.
        let author = self
            .session_account_id(session_id)
            .unwrap_or_else(|| "system:script".to_string());
        self.record_script_version(target_ref, None, source, &author, "in_game", None);
        let hooks_list = self
            .world
            .get(target_ref)
            .and_then(|o| o.script.as_ref())
            .map(|s| s.hooks.join(", "))
            .unwrap_or_default();
        format!(
            "Script installed on {} (hooks: {})\r\n",
            target_ref,
            if hooks_list.is_empty() { "none detected".into() } else { hooks_list }
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
            None => (args.trim(), ""),
        };
        if path.is_empty() {
            return "Usage: @program <ref> [= <luau source>]  (defines the object's whole script; hooks are functions in it)\r\n".to_string();
        }
        let target_ref = self.resolve_object_ref(actor_ref, path);
        if let Some(err) = self.check_program_write(session_id, actor_ref, &target_ref) {
            return err;
        }
        if source.is_empty() {
            // Seed the multi-line editor with the current script so an edit is
            // a real edit, not a blank-slate rewrite.
            let current = self
                .world
                .get(&target_ref)
                .and_then(|o| o.script.as_ref())
                .map(|s| s.source.clone())
                .unwrap_or_default();
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.insert("_program_buffer".into(), serde_json::json!(current));
                actor.attrs.insert("_program_target".into(), serde_json::json!(target_ref));
            }
            self.set_editor(session_id, Some(EditorMode::Program));
            return "Enter Luau source (define hooks as functions). Type '.' on a line by itself to install it, '@abort' to cancel:\r\n"
                .to_string();
        }
        self.install_program(session_id, actor_ref, &target_ref, source)
    }

    /// Resolve a bare object-ref argument (a `#N` ref, or `here` for the
    /// actor's room) for the script commands.
    fn resolve_object_ref(&self, actor_ref: &str, arg: &str) -> String {
        if arg == "here" {
            self.world
                .get(actor_ref)
                .and_then(|a| a.location_ref.clone())
                .unwrap_or_else(|| arg.to_string())
        } else {
            arg.to_string()
        }
    }

    fn handle_program_editor_input(&mut self, session_id: &str, actor_ref: &str, input: &str) {
        if input == "." {
            let (buffer, target_ref) = {
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
                (buffer, target_ref)
            };
            self.set_editor(session_id, None);
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.remove("_program_buffer");
                actor.attrs.remove("_program_target");
            }
            if buffer.is_empty() {
                self.send(session_id, "Empty source, nothing installed.\r\n");
                return;
            }
            let output = self.install_program(session_id, actor_ref, &target_ref, &buffer);
            self.send(session_id, &output);
        } else if input == "@abort" {
            self.set_editor(session_id, None);
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.remove("_program_buffer");
                actor.attrs.remove("_program_target");
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
        let Some(script) = obj.script.as_ref() else {
            return format!("{} has no script.\r\n", target_ref);
        };
        let mut out = format!(
            "Script on {}{}:\r\n",
            target_ref,
            if script.enabled { "" } else { " (disabled)" }
        );
        if script.hooks.is_empty() {
            out.push_str("  (no hook functions detected)\r\n");
        } else {
            out.push_str("  hooks: ");
            out.push_str(&script.hooks.join(", "));
            out.push_str("\r\n");
        }
        let lines = script.source.lines().count();
        out.push_str(&format!("  {} line(s). Use @program {} to edit.\r\n", lines, target_ref));
        out
    }

    fn cmd_rmprogram(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        // @rmprogram <ref> — removes the object's whole script
        if args.trim().is_empty() {
            return "Usage: @rmprogram <ref>\r\n".to_string();
        }
        let target_ref = self.resolve_object_ref(actor_ref, args.trim());
        if let Some(err) = self.check_program_write(session_id, actor_ref, &target_ref) {
            return err;
        }
        let cleared = self.world.get_mut(&target_ref).map(hooks::clear_script);
        match cleared {
            Some(true) => {
                self.world.bump_struct();
                format!("Removed script on {}.\r\n", target_ref)
            }
            Some(false) => format!("{} has no script.\r\n", target_ref),
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
        self.fire_hook_inner(this_ref, hook_name, actor_ref, room_ref, args, None)
    }

    /// Fire a hook with a structured `data` payload (a triggered hook) — the
    /// data reaches the hook as its 4th arg as a real Lua table. Used by the
    /// `Intent::Trigger` path; `args` (the command string) is unused here.
    fn fire_hook_data(
        &mut self,
        this_ref: &str,
        hook_name: &str,
        actor_ref: &str,
        room_ref: Option<&str>,
        data: Option<serde_json::Value>,
    ) -> Result<HookRun, String> {
        self.fire_hook_inner(this_ref, hook_name, actor_ref, room_ref, None, data)
    }

    fn fire_hook_inner(
        &mut self,
        this_ref: &str,
        hook_name: &str,
        actor_ref: &str,
        room_ref: Option<&str>,
        args: Option<&str>,
        data: Option<serde_json::Value>,
    ) -> Result<HookRun, String> {
        let (script, resolving_ref) = match self.resolve_hook_script(this_ref, hook_name) {
            Some(x) => x,
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
                &script,
                hook_name,
                &resolving_ref,
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
                data,
            )
            .map_err(|e| annotate_archetype_error(e.to_string(), this_ref, &resolving_ref))?;

        let denied = result.denied;
        let effects = softcode::apply_batch(&mut self.world, &result.batch)?;
        self.world.next_id = dbref_counter.get();
        self.invalidate_libs_touched_by(&result.batch);
        self.write_back_script_state(this_ref, hook_name, result.state);
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
        let refs = self.indexes().globals_by_hook.get(hook_name).cloned().unwrap_or_default();
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
        // Scripted moves whose `fire_hooks` flag is set — deferred to after the
        // effects loop (like triggers) so hook re-entrancy stays out of the
        // loop. Each is (mover, old_room, new_room).
        let mut moves: Vec<(String, Option<String>, String)> = Vec::new();
        // Forced commands (`run_command_as`) — deferred for the same
        // re-entrancy reason. Each is (actor, command).
        let mut forced: Vec<(String, String)> = Vec::new();
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
                Effect::TriggerHook { target, hook, data, actor } => {
                    triggers.push((target.clone(), hook.clone(), data.clone(), actor.clone()));
                }
                Effect::MovedObject { mover, old_room, new_room, announce, fire_hooks } => {
                    if *announce {
                        let name = self
                            .world
                            .get(mover)
                            .map(|o| self.world.display_name(o))
                            .unwrap_or_else(|| mover.clone());
                        let exclude = vec![mover.clone()];
                        if let Some(old) = old_room {
                            self.send_to_room(old, &format!("{} leaves.", name), &exclude);
                        }
                        self.send_to_room(new_room, &format!("{} arrives.", name), &exclude);
                    }
                    if *fire_hooks {
                        moves.push((mover.clone(), old_room.clone(), new_room.clone()));
                    }
                }
                Effect::RunCommand { actor, command } => {
                    forced.push((actor.clone(), command.clone()));
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
        for (target, hook, data, actor) in triggers {
            let room_ref = self
                .world
                .get(&target)
                .and_then(|o| o.location_ref.clone());
            // The hook fires as the trigger's chosen actor, or the ambient
            // actor when none was given. `data` reaches the hook as its 4th
            // argument (a real Lua table) — no more `_trigger_data` attr.
            let who = actor.as_deref().unwrap_or(actor_ref);
            if let Err(e) = self.fire_hook_data(&target, &hook, who, room_ref.as_deref(), data) {
                tracing::warn!(hook = %hook, target = %target, error = %e, "Triggered hook error");
            }
        }
        // Event-aware move choreography, with the moved object as actor — the
        // same on_leave/on_move/on_enter sequence (plus global hooks) that
        // `cmd_go` runs, minus the pre-move lock/deny checks (scripted moves
        // are unrestricted). Fired after the relocation has committed.
        for (mover, old_room, new_room) in moves {
            if let Some(old) = &old_room {
                let _ = self.fire_hook(old, "on_leave", &mover, Some(old), None);
                self.fire_global_hooks("on_leave", &mover, Some(old), None);
            }
            let _ = self.fire_hook(&mover, "on_move", &mover, Some(&new_room), None);
            let _ = self.fire_hook(&new_room, "on_enter", &mover, Some(&new_room), None);
            self.fire_global_hooks("on_enter", &mover, Some(&new_room), None);
        }
        // Forced commands (`run_command_as`). Run through the normal command
        // dispatch as the target's own session, so the command executes under
        // *their* scopes and every existing gate applies. The `@`-command and
        // `quit` bans (`forced_command_allowed`) and the depth guard are the
        // security line — a charmed player can never be forced into authoring/
        // admin commands or a disconnect, and forced-command chains can't
        // recurse without bound.
        for (target, command) in forced {
            if self.force_depth >= MAX_FORCE_DEPTH {
                tracing::warn!(
                    target = %target,
                    command = %command,
                    "run_command_as: force depth limit reached; refusing"
                );
                continue;
            }
            if !Self::forced_command_allowed(&command) {
                tracing::warn!(
                    target = %target,
                    command = %command,
                    "run_command_as: refused a privileged/quit command on the forced path"
                );
                self.send_to_actor_ref(&target, "You resist the compulsion.\r\n");
                continue;
            }
            // Player → their session; NPC → session-less dispatch. An offline
            // player is a no-op (see `dispatch_as_actor`).
            self.dispatch_as_actor(&target, &command);
        }
    }

    /// Whether a forced command (`run_command_as`) is permitted. The hard
    /// security line: never let a charm/puppet force an `@`-command (all
    /// authoring/admin verbs are `@`-prefixed) or `quit`/`q` (a forced
    /// disconnect). Everything else — movement, `say`, `get`/`drop`, game
    /// `cmd_*` verbs — runs under the target's own scopes, same as if typed.
    fn forced_command_allowed(command: &str) -> bool {
        let first = command.split_whitespace().next().unwrap_or("");
        if first.starts_with('@') {
            return false;
        }
        !matches!(first.to_lowercase().as_str(), "quit" | "q")
    }

    /// The session id currently playing as `actor_ref`, if any.
    fn session_for_actor(&self, actor_ref: &str) -> Option<String> {
        self.sessions.iter().find_map(|(sid, s)| match &s.state {
            SessionState::Playing { actor_ref: ar, .. } if ar == actor_ref => Some(sid.clone()),
            _ => None,
        })
    }

    /// Run a forced (`run_command_as`) command as `target`, bumping the
    /// recursion-depth guard around it: a player with a live session runs
    /// through their session; a session-less NPC runs through the session-less
    /// dispatch. An offline player (no session, not an NPC) is a no-op — there
    /// is no body to drive. Callers apply `forced_command_allowed` and the
    /// depth ceiling first.
    fn dispatch_as_actor(&mut self, target: &str, command: &str) {
        self.force_depth += 1;
        if let Some(session_id) = self.session_for_actor(target) {
            // Players go through the full session path (prompt/editor prelude
            // included), exactly as before.
            self.handle_game_input(&session_id, command);
        } else if self.world.get(target).map(|o| o.kind == Kind::Npc).unwrap_or(false) {
            // A forced NPC has no Puppet: character and effective actor are one.
            self.run_command(target, target, None, command);
        }
        self.force_depth = self.force_depth.saturating_sub(1);
    }

    fn send_to_actor_ref(&self, actor_ref: &str, message: &str) {
        for session in self.sessions.values() {
            if let SessionState::Playing { actor_ref: ar, .. } = &session.state
                && ar == actor_ref
            {
                let _ = session.tx.send(ClientMessage::Text { text: wire_text(message) });
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
                    let _ = session.tx.send(ClientMessage::Text { text: wire_text(message) });
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

        let name = self.world.display_name(self.world.get(&item_ref).unwrap());
        self.world.relocate(&item_ref, Some(actor_ref.to_string()));

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

        let item_display = self.world.display_name(self.world.get(&item_ref).unwrap());
        let container_display = self.world.display_name(self.world.get(&container_ref).unwrap());
        self.world.relocate(&item_ref, Some(actor_ref.to_string()));

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
            // `item:container` may be inherited from an archetype — see
            // docs/plans/archetypes.md.
            Some(c) if !self.world.resolved_tags(c).contains(&container_tag) => {
                return format!("{} is not a container.\r\n", self.world.display_name(c));
            }
            None => return "Container not found.\r\n".to_string(),
            _ => {}
        }

        if item_ref == container_ref {
            return "You can't put something inside itself.\r\n".to_string();
        }

        // Check capacity — also archetype-resolved (a shared "all bags hold
        // 10 items" default on the archetype).
        if let Some(capacity) = self
            .world
            .get(&container_ref)
            .and_then(|c| self.world.resolved_attr(c, "container_capacity"))
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

        let item_display = self.world.display_name(self.world.get(&item_ref).unwrap());
        let container_display = self.world.display_name(self.world.get(&container_ref).unwrap());
        self.world.relocate(&item_ref, Some(container_ref.clone()));

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
            // Global objects defining this exact `cmd_*` hook come from the
            // derived index (`system:global` resolved, own or inherited),
            // rather than scanning every object in the world — see
            // `DerivedIndexes`. They stay LAST in the resolution chain so a
            // room/inventory object's command still shadows a global one.
            let global_refs = self
                .indexes()
                .globals_by_hook
                .get(&hook_name)
                .cloned()
                .unwrap_or_default();
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
                let global_objs = global_refs.iter().filter_map(|r| self.world.get(r));
                hooks::find_cmd_hook(
                    &self.world,
                    room_itself.chain(room_objs).chain(inv_objs).chain(global_objs),
                    cmd,
                )
                .map(|obj| obj.ref_id.clone())
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
        // Persist immediately — a scope change lost to a crash before the next
        // autosave is a silent, surprising regression (see the same save after
        // character creation and @chown).
        self.db.save_accounts(&self.accounts).ok();
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
            self.db.save_accounts(&self.accounts).ok();
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
                    out.push_str(&format!("  {}\r\n", self.world.display_name(actor)));
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

    /// Build the GMCP `Terrain.Legend` payload for a map: each terrain char →
    /// a stable `env_id` (1000 + its index in sorted char order, offset past
    /// Mudlet's built-in environment ids) plus its color and presentation
    /// fields. A terrain with no declared color defaults to neutral gray so
    /// every entry carries one. Pure/deterministic — same map → same payload.
    fn terrain_legend(
        map_name: &str,
        tmpl: &crate::map_template::MapTemplateFile,
    ) -> serde_json::Value {
        let mut chars: Vec<&String> = tmpl.terrain.keys().collect();
        chars.sort();
        let mut terrains = serde_json::Map::new();
        for (i, ch) in chars.iter().enumerate() {
            let def = &tmpl.terrain[*ch];
            let mut entry = serde_json::Map::new();
            entry.insert("env_id".into(), serde_json::json!(1000 + i as i64));
            entry.insert(
                "color".into(),
                serde_json::json!(def.color.clone().unwrap_or_else(|| "#808080".into())),
            );
            entry.insert("passable".into(), serde_json::json!(def.passable));
            if let Some(tp) = &def.title_prefix {
                entry.insert("title_prefix".into(), serde_json::json!(tp));
            }
            if let Some(ti) = &def.tile_image {
                entry.insert("tile_image".into(), serde_json::json!(ti));
                entry.insert("tile_rotation".into(), serde_json::json!(def.tile_rotation.as_str()));
            }
            terrains.insert((*ch).clone(), serde_json::Value::Object(entry));
        }
        serde_json::json!({ "map": map_name, "terrains": terrains })
    }

    fn send_room_data(&mut self, session_id: &str, actor_ref: &str) {
        let room_ref = match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
            Some(r) => r,
            None => return,
        };

        let hidden_tag = Tag { category: "system".into(), key: "hidden".into() };
        let offline_tag = Tag { category: "system".into(), key: "offline".into() };

        // Resolve can_see for hidden objects (same logic as look_with_visibility).
        // `system:hidden` may be inherited from an archetype.
        let hidden_candidates: Vec<(String, bool)> = self
            .world
            .objects_in(&room_ref)
            .iter()
            .filter(|o| self.world.resolved_tags(o).contains(&hidden_tag) && o.ref_id != actor_ref)
            .map(|o| (o.ref_id.clone(), hooks::object_responds(&self.world, o, "can_see")))
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
                .map(|r| self.world.display_name(r))
                .unwrap_or_default();
            ExitData {
                dir: e.key.clone(),
                name: dest_name,
                to: e.target_ref.clone().unwrap_or_default(),
            }
        }).collect();

        // Resolved names — an archetype-based instance with no title of its
        // own shows the archetype's (docs/plans/archetypes.md).
        let contents: Vec<EntityData> = self.world.objects_in(&room_ref).into_iter()
            .filter(|o| {
                o.ref_id != actor_ref
                    && !o.tags.contains(&offline_tag)
                    && !hidden_refs.contains(&o.ref_id)
            })
            .map(|o| EntityData {
                name: self.world.display_name(o),
                kind: format!("{}", o.kind),
                ref_id: o.ref_id.clone(),
                owned: o.owner_ref.as_deref() == Some(actor_ref),
            })
            .collect();

        let attr_str = |k: &str| room.attrs.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let attr_int = |k: &str| room.attrs.get(k).and_then(|v| v.as_i64());
        let map_name = attr_str("map_name");
        if let Some(session) = self.sessions.get(session_id) {
            let _ = session.tx.send(ClientMessage::Room {
                name: self.world.display_name(room),
                description: self.world.resolved_description(room),
                exits,
                contents,
                num: room_ref.clone(),
                area: Self::room_area(room),
                map: map_name.clone(),
                environment: attr_str("terrain"),
                x: attr_int("map_x"),
                y: attr_int("map_y"),
            });
        }

        // On entering a mapped area, send its terrain legend once — colors +
        // stable env ids — so a GMCP mapper (Mudlet) can paint rooms by
        // terrain. Rides map *entry*, not every move: deduped per session by
        // map name (see `legend_sent_map`). Non-GMCP telnet clients drop the
        // `Game` message; web clients receive it as a game frame.
        if let Some(map_name) = map_name {
            let stale = self.legend_sent_map.get(session_id) != Some(&map_name);
            if stale
                && let Some(tmpl) = self.map_templates.get(&map_name)
            {
                let data = Self::terrain_legend(&map_name, tmpl);
                if let Some(session) = self.sessions.get(session_id) {
                    let _ = session.tx.send(ClientMessage::Game {
                        channel: "Terrain.Legend".into(),
                        data,
                    });
                }
                self.legend_sent_map.insert(session_id.to_string(), map_name);
            }
        }
    }

    fn send_commands(&mut self, session_id: &str, actor_ref: &str) {
        let is_builder = self.session_has_scope(session_id, Scope::Builder);
        let is_admin = self.session_has_scope(session_id, Scope::Admin);
        let room_ref = self.world.get(actor_ref).and_then(|a| a.location_ref.clone());

        // Reuse the cached list when nothing relevant changed: same room,
        // same scopes, and no world mutation since it was built.
        if let Some((cache_room, cache_builder, cache_admin, epoch, cmds)) = &self.commands_cache
            && *cache_room == room_ref
            && *cache_builder == is_builder
            && *cache_admin == is_admin
            && *epoch == self.world.version
        {
            if let Some(session) = self.sessions.get(session_id) {
                let _ = session.tx.send(ClientMessage::Commands { commands: cmds.clone() });
            }
            return;
        }

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
                "@tag", "@untag", "@script", "@scripts", "@rmscript",
                "@script-interval", "@lib", "@libs", "@rmlib",
                "@lock", "@unlock", "@locks", "@archetype", "@alias", "@clone",
                "@dialogue", "@test", "@reload", "@puppet", "@unpuppet", "@chown",
            ].iter().map(|s| String::from(*s)));
        }

        if is_admin {
            cmds.extend([
                "@force",
                "@grant", "@revoke", "@scopes", "@wall", "@boot",
                "@save", "@shutdown", "@reload-world", "@maxchars",
                "@eval", "@import", "@export",
            ].iter().map(|s| String::from(*s)));
        }

        if let Some(room_ref) = &room_ref {
            for exit in self.world.exits_from(room_ref) {
                cmds.push(exit.key.clone());
                for alias in &exit.aliases {
                    cmds.push(alias.clone());
                }
            }

            // Local objects (the room, its contents, the actor's inventory):
            // scan their resolved cmd_ hooks. Own hooks plus anything inherited
            // via `archetype_ref` — an instance that delegates its `cmd_*`
            // hooks should still show up in the command list.
            let room_itself = self.world.get(room_ref).into_iter();
            let room_objs = self.world.objects_in(room_ref).into_iter()
                .filter(|o| o.ref_id != actor_ref);
            let inv_objs = self.world.objects_in(actor_ref).into_iter();
            for obj in room_itself.chain(room_objs).chain(inv_objs) {
                for hook in hooks::resolve_hook_names(&self.world, obj) {
                    if let Some(cmd) = hook.strip_prefix("cmd_") {
                        cmds.push(cmd.to_string());
                    }
                }
            }

            // Global commands come straight from the derived index: its key is
            // the hook name (`cmd_<name>`), so the command list needs no
            // world scan and no per-object hook resolution here.
            let indexes = self.indexes();
            for hook in indexes.globals_by_hook.keys() {
                if let Some(cmd) = hook.strip_prefix("cmd_") {
                    cmds.push(cmd.to_string());
                }
            }
        }

        cmds.sort();
        cmds.dedup();

        self.commands_cache = Some((room_ref.clone(), is_builder, is_admin, self.world.version, cmds.clone()));

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
        let sender_name = self.world.display_name(actor);

        let lower_target = target_name.to_lowercase();
        let target_session = self.sessions.iter().find(|(sid, s)| {
            *sid != session_id
                && if let SessionState::Playing { actor_ref: ar, .. } = &s.state {
                    self.world
                        .get(ar)
                        .map(|o| {
                            o.location_ref.as_deref() == Some(&room_ref)
                                && (o.key.to_lowercase() == lower_target
                                    || self.world.display_name(o).to_lowercase() == lower_target)
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

        let name = self.world.display_name(actor);

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

    /// Find the ref of the `Kind::Code` object named `name` whose script
    /// defines `on_tick` (a global tick script), if any.
    fn find_script_object_ref(&self, name: &str) -> Option<String> {
        self.world
            .objects
            .values()
            .find(|o| {
                o.kind == Kind::Code
                    && o.key == name
                    && o.script.as_ref().is_some_and(|s| s.hooks.iter().any(|h| h == "on_tick"))
            })
            .map(|o| o.ref_id.clone())
    }

    /// Find the ref of the `Kind::Code` object named `name` that hosts the lib
    /// module `<name>`, if any.
    fn find_lib_object_ref(&self, name: &str) -> Option<String> {
        self.world
            .objects
            .values()
            .find(|o| o.kind == Kind::Code && o.key == name && o.libs.contains_key(name))
            .map(|o| o.ref_id.clone())
    }

    fn cmd_script(&mut self, session_id: &str, _actor_ref: &str, args: &str) -> String {
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
        let existing_ref = self.find_script_object_ref(name);
        if let Some(r) = &existing_ref
            && self.is_ref_locked(r)
        {
            return format!("{}\r\n", Self::locked_error(r));
        }
        let is_new = existing_ref.is_none();
        let ref_id = existing_ref.unwrap_or_else(|| {
            let ref_id = self.world.next_dbref();
            self.world.add_object(GameObject::new(&ref_id, name, Kind::Code));
            ref_id
        });
        if is_new {
            self.world
                .get_mut(&ref_id)
                .unwrap()
                .attrs
                .insert("tick_interval".into(), serde_json::json!(1));
        }
        self.set_object_script(&ref_id, source.to_string());
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
            .filter(|o| {
                o.kind == Kind::Code
                    && o.script.as_ref().is_some_and(|s| s.hooks.iter().any(|h| h == "on_tick"))
            })
            .collect();
        if scripts.is_empty() {
            return "No global scripts.\r\n".to_string();
        }
        scripts.sort_by(|a, b| a.key.cmp(&b.key));
        let mut out = "\r\nGlobal scripts:\r\n".to_string();
        for obj in scripts {
            let script = obj.script.as_ref().unwrap();
            let status = if script.enabled { "on" } else { "off" };
            let interval = obj
                .attrs
                .get("tick_interval")
                .and_then(|v| v.as_u64())
                .unwrap_or(1);
            let state_keys = script.state.len();
            out.push_str(&format!(
                "  {} [{}] interval={}  state_keys={}\r\n",
                obj.key, status, interval, state_keys
            ));
        }
        out
    }

    fn cmd_rmscript(&mut self, session_id: &str, _actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let name = args.trim();
        if name.is_empty() {
            return "Usage: @rmscript <name>\r\n".to_string();
        }
        match self.find_script_object_ref(name) {
            Some(ref_id) => {
                if self.is_ref_locked(&ref_id) {
                    return format!("{}\r\n", Self::locked_error(&ref_id));
                }
                let obj = self.world.get_mut(&ref_id).unwrap();
                hooks::clear_script(obj);
                let orphan = obj.script.is_none() && obj.libs.is_empty();
                self.world.bump_struct();
                if orphan {
                    self.world.remove_object(&ref_id);
                }
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
        match self.find_script_object_ref(name) {
            Some(ref_id) => {
                if self.is_ref_locked(&ref_id) {
                    return format!("{}\r\n", Self::locked_error(&ref_id));
                }
                self.world
                    .get_mut(&ref_id)
                    .unwrap()
                    .attrs
                    .insert("tick_interval".into(), serde_json::json!(interval));
                self.world.bump_struct();
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

    fn cmd_lib(&mut self, session_id: &str, _actor_ref: &str, args: &str) -> String {
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
        match self.upsert_library(name, source) {
            Ok((_, true)) => format!("Library '{}' created — require(\"{}\").\r\n", name, name),
            Ok((_, false)) => format!("Library '{}' updated.\r\n", name),
            Err(e) => format!("{}\r\n", e),
        }
    }

    /// Find-or-create the `Kind::Code` host for library `name` and set its
    /// source. Shared by the telnet `@lib` command and the REST `create_library`
    /// action, so both take the same guards: no shipped-module collision, valid
    /// syntax, and a locked host is refused. Returns `(host ref, was_created)`.
    fn upsert_library(&mut self, name: &str, source: &str) -> Result<(String, bool), String> {
        if self.softcode.is_shipped_module(name) {
            return Err(format!(
                "'{}' is a shipped module — choose a different name.",
                name
            ));
        }
        if let Err(e) = self.softcode.check_syntax(source) {
            return Err(format!("Syntax error: {}", e));
        }
        let existing_ref = self.find_lib_object_ref(name);
        if let Some(r) = &existing_ref
            && self.is_ref_locked(r)
        {
            return Err(Self::locked_error(r));
        }
        let is_new = existing_ref.is_none();
        let ref_id = existing_ref.unwrap_or_else(|| {
            let ref_id = self.world.next_dbref();
            self.world.add_object(GameObject::new(&ref_id, name, Kind::Code));
            ref_id
        });
        let obj = self.world.get_mut(&ref_id).unwrap();
        hooks::set_lib(obj, name, source.to_string(), hooks::ProgramOrigin::InGame);
        self.softcode.invalidate_module_cache();
        Ok((ref_id, is_new))
    }

    fn cmd_libs(&self, session_id: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let mut libs: Vec<(&str, &str)> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == Kind::Code)
            .flat_map(|o| o.libs.keys().map(move |name| (name.as_str(), o.key.as_str())))
            .collect();
        if libs.is_empty() {
            return "No libraries.\r\n".to_string();
        }
        libs.sort_by_key(|(name, _)| *name);
        let mut out = "\r\nLibraries:\r\n".to_string();
        for (name, key) in libs {
            out.push_str(&format!("  {} (object key: {})\r\n", name, key));
        }
        out
    }

    fn cmd_rmlib(&mut self, session_id: &str, _actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let name = args.trim();
        if name.is_empty() {
            return "Usage: @rmlib <name>\r\n".to_string();
        }
        match self.find_lib_object_ref(name) {
            Some(ref_id) => {
                if self.is_ref_locked(&ref_id) {
                    return format!("{}\r\n", Self::locked_error(&ref_id));
                }
                let obj = self.world.get_mut(&ref_id).unwrap();
                hooks::remove_lib(obj, name);
                if obj.script.is_none() && obj.libs.is_empty() {
                    self.world.remove_object(&ref_id);
                }
                self.softcode.invalidate_module_cache();
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
        if self.is_ref_locked(&resolved) {
            return format!("{}\r\n", Self::locked_error(&resolved));
        }
        if self.world.add_tag(&resolved, tag.clone()) {
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
        if self.is_ref_locked(&resolved) {
            return format!("{}\r\n", Self::locked_error(&resolved));
        }
        if self.world.get(&resolved).is_none() {
            return format!("No object with ref '{}'.\r\n", resolved);
        }
        if self.world.remove_tag(&resolved, &tag) {
            format!("Tag '{}' removed from {}.\r\n", tag.as_spec(), resolved)
        } else {
            format!("Object {} doesn't have tag '{}'.\r\n", resolved, tag.as_spec())
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
        {
            let mut host = self.softcode.wasm_host().borrow_mut();
            host.clear();
            host.load_dir(&game_path.join("wasm"));
        }
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
                // Re-apply config-driven locking to anything newly created or
                // re-imported this reload.
                crate::loader::stamp_locked(&mut self.world, &self.locked_prefixes);
                crate::loader::validate_attr_schemas(&self.world);
                // A bulk file reload can edit managed objects' scripts, tags,
                // and archetypes in place (not only through add/remove), so
                // force the derived indexes to rebuild from scratch rather than
                // relying on per-mutation `struct_version` bumps in the loader.
                self.derived = None;
                self.commands_cache = None;
                // Capture any file program changes into version history.
                self.record_file_program_versions();
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
                // Vocal divergence: name every object whose file script was
                // shadowed by an in-game edit, so the drop is never silent.
                if !result.diverged.is_empty() {
                    use std::fmt::Write;
                    let _ = write!(
                        msg,
                        "[yellow]{} file change(s) NOT applied — shadowed by in-game edits:[/]\r\n",
                        result.diverged.len()
                    );
                    for key in &result.diverged {
                        let _ = write!(msg, "  [yellow]diverged:[/] {} (file source ignored)\r\n", key);
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
                    // Imported objects may carry Programs (including
                    // `lib_*` user libraries) — drop stale compiled chunks,
                    // cached modules, and the user-lib sources table so the
                    // next run sees the imported state. Same reasoning as
                    // `cmd_reload_world`.
                    self.softcode.invalidate_cache();
                    self.reload_map_sources_from_db();
                    // Import can rewrite managed objects' scripts/tags/archetype
                    // in place; force the derived indexes to rebuild. (See the
                    // matching reset in `cmd_reload_world`.)
                    self.derived = None;
                    self.commands_cache = None;
                    self.record_file_program_versions();
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
                    || self.world.display_name(o).to_lowercase().contains(&target_lower)
            }) {
                Some(o) => o.ref_id.clone(),
                None => return format!("Cannot find '{}'.\r\n", target_input),
            }
        };

        // The mutating subcommands are authoring edits — refuse them on a
        // locked, file-authoritative object (show/test/export stay read-only).
        if matches!(subcommand, "edit" | "clear") && self.is_ref_locked(&target_ref) {
            return format!("{}\r\n", Self::locked_error(&target_ref));
        }

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
                // `_ink_editing` holds the target being edited (working data);
                // the routing flag is `Session.editor`.
                if let Some(actor) = self.world.get_mut(actor_ref) {
                    actor.attrs.insert(
                        "_ink_editing".into(),
                        serde_json::json!(target_ref),
                    );
                    actor
                        .attrs
                        .insert("_ink_buffer".into(), serde_json::json!(""));
                }
                self.set_editor(session_id, Some(EditorMode::Ink));
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
            self.set_editor(session_id, None);
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
            self.set_editor(session_id, None);
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
                    .insert("_eval_buffer".into(), serde_json::json!(""));
            }
            self.set_editor(session_id, Some(EditorMode::Eval));
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
            self.set_editor(session_id, None);
            if let Some(actor) = self.world.get_mut(actor_ref) {
                actor.attrs.remove("_eval_buffer");
            }
            if buffer.is_empty() {
                self.send(session_id, "Empty source, nothing run.\r\n");
                return;
            }
            let output = self.eval_and_report(actor_ref, &buffer);
            self.send(session_id, &output);
        } else if input == "@abort" {
            self.set_editor(session_id, None);
            if let Some(actor) = self.world.get_mut(actor_ref) {
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
        // @reload <ref>  — re-validate and re-enable an object's script
        if args.trim().is_empty() {
            return "Usage: @reload <ref>\r\n".to_string();
        }
        let target_ref = self.resolve_object_ref(actor_ref, args.trim());
        let source = match self.world.get(&target_ref).and_then(|o| o.script.as_ref()) {
            Some(s) => s.source.clone(),
            None => return format!("{} has no script.\r\n", target_ref),
        };
        if let Err(e) = self.softcode.check_syntax(&source) {
            return format!("Syntax error: {}\r\n", e);
        }
        if let Some(obj) = self.world.get_mut(&target_ref)
            && let Some(script) = obj.script.as_mut()
        {
            script.enabled = true;
        }
        format!("Script on {} reloaded and enabled.\r\n", target_ref)
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
            game_hour: self.current_game_hour(),
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
            .is_some_and(|o| hooks::object_responds(&self.world, o, "on_look"));
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
            .filter(|o| self.world.resolved_tags(o).contains(&hidden_tag) && o.ref_id != actor_ref)
            .map(|o| (o.ref_id.clone(), hooks::object_responds(&self.world, o, "can_see")))
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
                        || self.world.display_name(o).to_lowercase().contains(&target_name)
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

        // Check can_traverse hook on the exit itself (Luau), pairing with the
        // `traverse` lock above — both gate the exit. `can_enter` below is the
        // room's gate; firing this one on the destination room too would make
        // the two hooks identical and leave an exit's own hook never firing.
        match self.fire_hook(exit_ref, "can_traverse", actor_ref, Some(&old_room), None) {
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
        self.world.relocate(actor_ref, Some(target_ref.to_string()));

        // Move followers (troupe members tagged troupe:<actor_ref>) — from
        // the derived index, not a full-world scan.
        let followers = self.indexes().troupes.get(actor_ref).cloned().unwrap_or_default();
        for ref_id in followers {
            self.world.relocate(&ref_id, Some(target_ref.to_string()));
        }

        // Coordinate exit: an exit carrying `_dest_x`/`_dest_y` stamps the
        // arrival cell onto the actor (the grid/wilderness model is one room
        // with position tracked as `_x`/`_y` attrs — no room per cell). Applied
        // after the relocate and before `on_enter`/`on_look`, so the room's
        // hooks render the cell the actor actually landed on. Generic: the
        // engine only copies the exit's declared arrival coordinates; it knows
        // nothing about "wilderness". Followers land on the same cell so a
        // troupe stays together on the grid.
        let dest_xy = self.world.get(exit_ref).and_then(|e| {
            let dx = e.attrs.get("_dest_x").and_then(|v| v.as_i64())?;
            let dy = e.attrs.get("_dest_y").and_then(|v| v.as_i64())?;
            Some((dx, dy))
        });
        if let Some((dx, dy)) = dest_xy {
            let followers = self.indexes().troupes.get(actor_ref).cloned().unwrap_or_default();
            for ref_id in std::iter::once(actor_ref.to_string()).chain(followers) {
                if let Some(obj) = self.world.get_mut(&ref_id) {
                    obj.attrs.insert("_x".into(), serde_json::json!(dx));
                    obj.attrs.insert("_y".into(), serde_json::json!(dy));
                }
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
                        || self.world.display_name(o).to_lowercase().contains(&target_name))
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

        let name = self.world.display_name(self.world.get(&item_ref).unwrap());
        self.world.relocate(&item_ref, Some(room_ref.clone()));

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
                    || self.world.display_name(o).to_lowercase().contains(&target_name)
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
                    .map(|o| self.world.display_name(o))
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
            .is_some_and(|o| hooks::object_responds(&self.world, o, "on_say"));
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
            .map(|a| self.world.display_name(a))
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
        if self.is_ref_locked(&resolved) {
            return format!("{}\r\n", Self::locked_error(&resolved));
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

        if self.is_ref_locked(&resolved) {
            return format!("{}\r\n", Self::locked_error(&resolved));
        }
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

    /// `@alias <ref> = <a> <b> …` — replace an object's alias keywords.
    /// Builder-gated, owner-checked, refuses locked objects.
    fn cmd_alias(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let (path, alias_str) = match args.split_once('=') {
            Some((p, a)) => (p.trim(), a.trim()),
            None => return "Usage: @alias <ref> = <alias1> <alias2> ...\r\n".to_string(),
        };
        let resolved = if path == "here" {
            match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
                Some(r) => r,
                None => return "You're nowhere.\r\n".to_string(),
            }
        } else {
            path.to_string()
        };
        if !self.can_modify_object(session_id, actor_ref, &resolved) {
            return "Permission denied (not owner).\r\n".to_string();
        }
        if self.is_ref_locked(&resolved) {
            return format!("{}\r\n", Self::locked_error(&resolved));
        }
        let aliases: std::collections::HashSet<String> =
            alias_str.split_whitespace().map(|s| s.to_string()).collect();
        match self.world.get_mut(&resolved) {
            Some(obj) => {
                let n = aliases.len();
                obj.aliases = aliases;
                format!("Set {} alias(es) on {}.\r\n", n, resolved)
            }
            None => format!("No object with ref '{}'.\r\n", resolved),
        }
    }

    /// `@clone <ref>` — deep-copy an object into a new dbref owned by the
    /// cloner. Builder-gated; reuses the softcode clone path (strips system:*,
    /// refuses a locked source).
    fn cmd_clone(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let arg = args.trim();
        if arg.is_empty() {
            return "Usage: @clone <ref>\r\n".to_string();
        }
        let source = if arg == "here" {
            match self.world.get(actor_ref).and_then(|a| a.location_ref.clone()) {
                Some(r) => r,
                None => return "You're nowhere.\r\n".to_string(),
            }
        } else {
            arg.to_string()
        };
        let new_ref = self.world.next_dbref();
        let batch = softcode::IntentBatch::from_intents(vec![softcode::Intent::CloneObject {
            ref_id: new_ref.clone(),
            source: source.clone(),
            location: None,
            owner: Some(actor_ref.to_string()),
        }]);
        match softcode::apply_batch(&mut self.world, &batch) {
            Ok(_) => format!("Cloned {} → {}.\r\n", source, new_ref),
            Err(e) => format!("{}\r\n", e),
        }
    }

    /// `@force <player> = <command>` — run a command as another player (charm/
    /// puppet). Admin-gated; the same forced-command gate as softcode
    /// `run_command_as` (@-commands and quit refused, depth-bounded). The
    /// player must be online. `<player>` is a ref or an online player's name.
    fn cmd_force(&mut self, session_id: &str, actor_ref: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Admin) {
            return "Permission denied.\r\n".to_string();
        }
        let (who, command) = match args.split_once('=') {
            Some((w, c)) => (w.trim(), c.trim()),
            None => return "Usage: @force <player|npc> = <command>\r\n".to_string(),
        };
        if command.is_empty() {
            return "Usage: @force <player|npc> = <command>\r\n".to_string();
        }
        if !Self::forced_command_allowed(command) {
            return "That command cannot be forced (@-commands and quit are refused).\r\n"
                .to_string();
        }
        // Resolve the target: a #ref, an online player by name, or an NPC in
        // the forcer's current room by name/key.
        let target = if who.starts_with('#') {
            who.to_string()
        } else {
            let online = self.sessions.values().find_map(|s| match &s.state {
                SessionState::Playing { actor_ref, .. } => self.world.get(actor_ref).and_then(|o| {
                    (self.world.display_name(o).eq_ignore_ascii_case(who)
                        || o.key.eq_ignore_ascii_case(who))
                    .then(|| actor_ref.clone())
                }),
                _ => None,
            });
            match online {
                Some(r) => r,
                None => {
                    let room = self.world.get(actor_ref).and_then(|a| a.location_ref.clone());
                    let npc = room.and_then(|rm| {
                        self.world
                            .objects_in(&rm)
                            .into_iter()
                            .find(|o| {
                                o.kind == Kind::Npc
                                    && (self.world.display_name(o).eq_ignore_ascii_case(who)
                                        || o.key.eq_ignore_ascii_case(who))
                            })
                            .map(|o| o.ref_id.clone())
                    });
                    match npc {
                        Some(r) => r,
                        None => return format!("No player or NPC matching '{}'.\r\n", who),
                    }
                }
            }
        };
        let (kind, label) = match self.world.get(&target) {
            Some(o) => (o.kind.clone(), self.world.display_name(o)),
            None => return format!("No object with ref '{}'.\r\n", target),
        };
        if !matches!(kind, Kind::Player | Kind::Npc) {
            return "You can only force a player or an NPC.\r\n".to_string();
        }
        if kind == Kind::Player && self.session_for_actor(&target).is_none() {
            return "That player has no active session.\r\n".to_string();
        }
        if self.force_depth >= MAX_FORCE_DEPTH {
            return "Forced-command depth limit reached.\r\n".to_string();
        }
        self.dispatch_as_actor(&target, command);
        format!("You force {} to '{}'.\r\n", label, command)
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

    /// Run the `test_*` functions embedded in an object's own script (tests
    /// co-located with the hooks). `ctx.this` is bound to the object, so its
    /// tests run against itself. `None` if the object has no script.
    fn run_object_tests(
        &self,
        ref_id: &str,
    ) -> Option<Result<softcode::TestFileResult, softcode::SoftcodeError>> {
        let source = self
            .world
            .get(ref_id)
            .and_then(|o| o.script.as_ref())
            .map(|s| s.source.clone())?;
        Some(self.softcode.run_tests(
            &source,
            &format!("{} (embedded)", ref_id),
            Some(&self.world),
            Some(ref_id),
            &self.map_templates,
            softcode::Budget::default(),
        ))
    }

    /// Append a test-file/object result to `out`, tallying pass/fail.
    fn render_test_result(
        out: &mut String,
        label: &str,
        result: Result<softcode::TestFileResult, softcode::SoftcodeError>,
        passed: &mut usize,
        failed: &mut usize,
    ) {
        match result {
            Ok(fr) => {
                out.push_str(&format!("\r\n{}:\r\n", label));
                for tr in &fr.tests {
                    if tr.passed {
                        *passed += 1;
                        out.push_str(&format!("  PASS {}\r\n", tr.name));
                    } else {
                        *failed += 1;
                        out.push_str(&format!(
                            "  FAIL {} -- {}\r\n",
                            tr.name,
                            tr.error.as_deref().unwrap_or("?")
                        ));
                    }
                }
            }
            Err(e) => {
                *failed += 1;
                out.push_str(&format!("\r\n{}: ERROR -- {}\r\n", label, e));
            }
        }
    }

    fn cmd_test(&mut self, session_id: &str, args: &str) -> String {
        if !self.session_has_scope(session_id, Scope::Builder) {
            return "Permission denied.\r\n".to_string();
        }
        let arg = args.trim();
        let mut out = String::new();
        let mut passed = 0usize;
        let mut failed = 0usize;

        // `@test #<ref>` — run one object's embedded test_* functions. No
        // game_dir needed; the tests travel with the object's script.
        if arg.starts_with('#') {
            match self.run_object_tests(arg) {
                Some(result) => {
                    Self::render_test_result(
                        &mut out,
                        &format!("{} (embedded)", arg),
                        result,
                        &mut passed,
                        &mut failed,
                    );
                }
                None => return format!("{} has no script (no embedded tests).\r\n", arg),
            }
            out.push_str(&format!("\r\n{} passed, {} failed\r\n", passed, failed));
            return out;
        }

        // `@test <file>` — run one named .test.luau file (needs game_dir).
        if !arg.is_empty() {
            let game_dir = match &self.game_dir {
                Some(g) => g.clone(),
                None => return "No game_dir configured.\r\n".to_string(),
            };
            let game_path = std::path::Path::new(&game_dir);
            let path = game_path.join(arg);
            let source = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => return format!("Cannot read '{}': {}\r\n", arg, e),
            };
            let is_lib = path.starts_with(game_path.join("lib"));
            let world = if is_lib { None } else { Some(self.world.clone()) };
            let result = self.softcode.run_tests(
                &source,
                arg,
                world.as_ref(),
                None,
                &self.map_templates,
                softcode::Budget::default(),
            );
            Self::render_test_result(&mut out, arg, result, &mut passed, &mut failed);
            out.push_str(&format!("\r\n{} passed, {} failed\r\n", passed, failed));
            return out;
        }

        // `@test` (no args) — run every .test.luau file in the game dir.
        // (Embedded object tests are run per-object with `@test #<ref>`, never
        // swept globally.)
        let game_dir = match &self.game_dir {
            Some(g) => g.clone(),
            None => return "No game_dir configured.\r\n".to_string(),
        };
        let game_path = std::path::Path::new(&game_dir);
        let test_files = crate::loader::discover_test_files(game_path);
        if test_files.is_empty() {
            return "No .test.luau files found.\r\n".to_string();
        }
        for tf in &test_files {
            let world = if tf.is_lib { None } else { Some(self.world.clone()) };
            let result = self.softcode.run_tests(
                &tf.source,
                &tf.relative,
                world.as_ref(),
                None,
                &self.map_templates,
                softcode::Budget::default(),
            );
            Self::render_test_result(&mut out, &tf.relative, result, &mut passed, &mut failed);
        }
        out.push_str(&format!("\r\n{} passed, {} failed\r\n", passed, failed));
        out
    }

    /// Whether an object's DEFINITION is locked to in-game authoring — the
    /// `system:locked` OWN tag. Deliberately NOT resolved up the archetype
    /// chain (unlike `system:global`): a locked base must not lock its
    /// subtypes or instances, only itself. Locking refuses authoring edits
    /// (script, title, description, tags, archetype, location, delete, lib) at
    /// the REST + `@`-command entry points. It does NOT block runtime state:
    /// softcode hooks pushing intents during play still mutate a locked
    /// object, so gameplay is unaffected — only the authoring surface is
    /// closed. See `docs/plans/archetypes.md` and `crate::loader::stamp_locked`.
    fn is_object_locked(obj: &GameObject) -> bool {
        obj.tags
            .iter()
            .any(|t| t.category == "system" && t.key == "locked")
    }

    fn is_ref_locked(&self, ref_id: &str) -> bool {
        self.world.get(ref_id).is_some_and(Self::is_object_locked)
    }

    /// Whether an object resolves the `system:global` tag — own or inherited
    /// up its archetype chain (a shared "rules" object), matching how command
    /// dispatch resolves it (`DerivedIndexes::build`). Its `cmd_*`/`on_*` hooks
    /// run for every player, so authoring it is admin-only (RBAC audit H1).
    fn is_ref_global(&self, ref_id: &str) -> bool {
        self.world.get(ref_id).is_some_and(|o| {
            self.world
                .resolved_tags(o)
                .iter()
                .any(|t| t.category == "system" && t.key == "global")
        })
    }

    fn locked_error(ref_id: &str) -> String {
        format!(
            "{} is locked (system:locked) — edit the file and @reload-world",
            ref_id
        )
    }

    /// Candidate entities for a `ref`-typed attribute's dropdown. `source`
    /// vocabulary (extensible): `kind:<npc|item|room|exit|code>`,
    /// `tag:<cat>:<key>` (matched against RESOLVED tags, so an instance of a
    /// tagged archetype qualifies), or `archetype` (objects with live
    /// instances). Each candidate is `{ ref_id, label }` where label is the
    /// resolved title (falling back to the key), sorted by label. Unknown
    /// sources return nothing. See `crate::attr_schema`.
    fn ref_candidates(&self, source: &str) -> Vec<serde_json::Value> {
        let matches: Vec<&GameObject> = if let Some(kind_str) = source.strip_prefix("kind:") {
            match Kind::parse(kind_str) {
                Some(k) => self.world.objects.values().filter(|o| o.kind == k).collect(),
                None => Vec::new(),
            }
        } else if let Some(tag_spec) = source.strip_prefix("tag:") {
            match Tag::parse(tag_spec) {
                Ok(tag) => self
                    .world
                    .objects
                    .values()
                    .filter(|o| self.world.resolved_tags(o).contains(&tag))
                    .collect(),
                Err(_) => Vec::new(),
            }
        } else if source == "archetype" {
            self.world
                .objects
                .values()
                .filter(|o| self.world.has_archetype_instances(&o.ref_id))
                .collect()
        } else {
            Vec::new()
        };
        let mut out: Vec<serde_json::Value> = matches
            .into_iter()
            .map(|o| {
                serde_json::json!({
                    "ref_id": o.ref_id,
                    "label": self.world.resolved_title(o).unwrap_or_else(|| o.key.clone()),
                })
            })
            .collect();
        out.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
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
                .map(|o| self.world.display_name(o))
                .unwrap_or_else(|| ref_id.clone());
            let location = self
                .world
                .get(ref_id)
                .and_then(|o| o.location_ref.as_ref())
                .and_then(|loc| self.world.get(loc))
                .map(|r| self.world.display_name(r))
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
                        .map(|o| self.world.display_name(o).to_lowercase().contains(&name) || o.key.to_lowercase().contains(&name))
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
        self.world.add_tag(&current_ref, crate::world::Tag {
            category: "system".to_string(),
            key: "offline".to_string(),
        });

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
                        .map(|o| self.world.display_name(o).to_lowercase() == name || o.key.to_lowercase() == name)
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
            .map(|o| self.world.display_name(o))
            .unwrap_or_default();

        if let Some(account) = self.accounts.get_mut(&account_id) {
            account.characters.retain(|r| r != &target_ref);
        }
        self.world.remove_object(&target_ref);
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

        let target_name = self.world.display_name(target);

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
        if self.is_ref_locked(target_ref) {
            return format!("{}\r\n", Self::locked_error(target_ref));
        }
        if let Some(obj) = self.world.get_mut(target_ref) {
            obj.owner_ref = Some(new_owner.to_string());
            format!("Owner of {} set to {}.\r\n", target_ref, new_owner)
        } else {
            format!("No object with ref '{}'.\r\n", target_ref)
        }
    }
}

/// Render one queued [`softcode::Intent`] as a short, human-readable line for
/// the REPL's preview list — "set gem (#12).hp = 6", "spawn item torch (#42)
/// in Crossroads (#3)". Refs resolve to a title/key through the live world
/// where known; unknown refs (e.g. a not-yet-applied spawn) show raw.
fn describe_intent(intent: &softcode::Intent, world: &World) -> String {
    use softcode::Intent;
    let label = |r: &str| match world.get(r) {
        Some(o) => {
            let name = o.title.as_deref().filter(|t| !t.is_empty()).unwrap_or(&o.key);
            format!("{} ({})", name, r)
        }
        None => r.to_string(),
    };
    // Keep long values/messages from blowing out one line.
    let clip = |s: &str| {
        if s.chars().count() > 60 {
            format!("{}…", s.chars().take(60).collect::<String>())
        } else {
            s.to_string()
        }
    };
    match intent {
        Intent::SetAttr { target, key, value } => {
            format!("set {}.{} = {}", label(target), key, clip(&value.to_string()))
        }
        Intent::UnsetAttr { target, key } => format!("unset {}.{}", label(target), key),
        Intent::EmitActor { target, message } => {
            format!("emit to {}: {:?}", label(target), clip(message))
        }
        Intent::EmitRoom { room, message, .. } => {
            format!("emit to room {}: {:?}", label(room), clip(message))
        }
        Intent::Move { target, destination, .. } => {
            format!("move {} → {}", label(target), label(destination))
        }
        Intent::SetAliases { target, .. } => format!("set aliases of {}", label(target)),
        Intent::UpdateExit { target, direction, destination } => {
            let dir = direction.as_deref().unwrap_or("(unchanged)");
            match destination {
                Some(d) => format!("update exit {} dir '{}' → {}", label(target), dir, label(d)),
                None => format!("update exit {} dir '{}'", label(target), dir),
            }
        }
        Intent::ClearLock { target, hook } => format!("clear lock {}/{}", label(target), hook),
        Intent::CloneObject { ref_id, source, .. } => {
            format!("clone {} → {}", label(source), ref_id)
        }
        Intent::RunCommandAs { actor, command } => {
            format!("force {} to '{}'", label(actor), command)
        }
        Intent::SetTag { target, tag } => {
            format!("tag {} +{}:{}", label(target), tag.category, tag.key)
        }
        Intent::UnsetTag { target, tag } => {
            format!("tag {} -{}:{}", label(target), tag.category, tag.key)
        }
        Intent::Spawn { ref_id, key, kind, location, archetype, .. } => {
            let where_ = match location {
                Some(loc) => format!("in {}", label(loc)),
                None => "top-level".to_string(),
            };
            match archetype {
                Some(a) => format!(
                    "spawn {} {} ({}) {} (archetype {})",
                    kind, key, ref_id, where_, label(a)
                ),
                None => format!("spawn {} {} ({}) {}", kind, key, ref_id, where_),
            }
        }
        Intent::SetTitle { target, .. } => format!("set title of {}", label(target)),
        Intent::SetDescription { target, .. } => format!("set description of {}", label(target)),
        Intent::Destroy { target, cascade } => {
            if *cascade {
                format!("destroy {} (cascade)", label(target))
            } else {
                format!("destroy {}", label(target))
            }
        }
        Intent::Detach { target } => format!("clone/detach {}", label(target)),
        Intent::SetArchetype { target, archetype } => match archetype {
            Some(a) => format!("set archetype of {} to {}", label(target), label(a)),
            None => format!("clear archetype of {}", label(target)),
        },
        Intent::CreateExit { source, direction, target, .. } => {
            format!("exit '{}' from {} → {}", direction, label(source), label(target))
        }
        Intent::SetScript { target, .. } => format!("set script on {}", label(target)),
        Intent::SetLib { target, name, .. } => format!("set lib {}/{}", label(target), name),
        Intent::Trigger { target, hook, .. } => format!("trigger {}/{}", label(target), hook),
        Intent::EmitNearby { room, x, y, radius, .. } => {
            format!("emit near ({}, {}) r{} in {}", x, y, radius, label(room))
        }
        Intent::SetLock { target, hook, .. } => format!("lock {}/{}", label(target), hook),
        Intent::SetOwner { target, owner } => format!("chown {} → {}", label(target), label(owner)),
        Intent::After { target, hook, ticks, .. } => {
            format!("schedule {}/{} in {} ticks", label(target), hook, ticks)
        }
        Intent::CancelAfter { target, hook } => format!("cancel timer {}/{}", label(target), hook),
        Intent::EmitData { target, channel, .. } => {
            format!("emit data '{}' to {}", channel, label(target))
        }
        Intent::EmitRadius { room, radius, .. } => format!("emit radius r{} in {}", radius, label(room)),
        Intent::TransferAttr { from, to, key, amount } => {
            format!("transfer {} {} from {} → {}", amount, key, label(from), label(to))
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

    #[test]
    fn seeds_bootstrap_admin_from_env_on_fresh_store() {
        // SAFETY: single-threaded within this test; unique var names not read
        // by other tests. Edition 2024 marks env mutation unsafe.
        unsafe {
            std::env::set_var("HEARTH_ADMIN_USER", "bootadmin");
            std::env::set_var("HEARTH_ADMIN_PASSWORD", "bootpass123");
        }
        let db = crate::db::Database::open(Path::new(":memory:")).unwrap();
        let (_tx, rx) = mpsc::unbounded_channel();
        let engine = Engine::new(rx, db, &Config::default());
        unsafe {
            std::env::remove_var("HEARTH_ADMIN_USER");
            std::env::remove_var("HEARTH_ADMIN_PASSWORD");
        }
        let admin = engine
            .accounts
            .get_by_username("bootadmin")
            .expect("bootstrap admin should be seeded");
        assert!(admin.scopes.contains(&Scope::Admin));
        assert!(admin.scopes.contains(&Scope::Builder));
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
    async fn api_ink_playtest_roundtrip() {
        let (tx, handle) = test_engine().await;

        // A minimal branching conversation authored on an NPC.
        let resp = api_call(&tx, ApiRequest::CreateObject {
            area: "test".into(),
            key: "sage".into(),
            kind: "npc".into(),
            title: Some("the Sage".into()),
            description: None,
            location: Some("#1".into()),
        }).await;
        let npc = resp.data.unwrap()["ref_id"].as_str().unwrap().to_string();

        let source = "-> start\n\n=== start ===\nWell met.\n+ [Ask a question] -> answer\n+ [Leave] -> END\n\n=== answer ===\nThe path lies east. # hint:east\n-> END";
        let resp = api_call(&tx, ApiRequest::InkSave { ref_id: npc.clone(), source: source.into() }).await;
        assert!(resp.ok);
        assert_eq!(resp.data.unwrap()["valid"], true);

        // Start against the saved source (no explicit source passed).
        let resp = api_call(&tx, ApiRequest::InkPlayStart { ref_id: npc.clone(), source: None }).await;
        assert!(resp.ok, "start failed: {:?}", resp.error);
        let out = resp.data.unwrap();
        assert!(out["text"].as_str().unwrap().contains("Well met"));
        assert_eq!(out["choices"].as_array().unwrap().len(), 2);
        assert_eq!(out["ended"], false);

        // Choosing the first option advances to the answer and ends, and the
        // line's tag rides along.
        let resp = api_call(&tx, ApiRequest::InkPlayChoose { ref_id: npc.clone(), index: 0 }).await;
        assert!(resp.ok, "choose failed: {:?}", resp.error);
        let out = resp.data.unwrap();
        assert!(out["text"].as_str().unwrap().contains("path lies east"));
        assert_eq!(out["tags"][0], "hint:east");
        assert_eq!(out["ended"], true);

        // Ending is idempotent and clears the preview slot.
        let resp = api_call(&tx, ApiRequest::InkPlayEnd { ref_id: npc.clone() }).await;
        assert!(resp.ok);

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_ink_playtest_runs_unsaved_buffer() {
        let (tx, handle) = test_engine().await;

        let resp = api_call(&tx, ApiRequest::CreateObject {
            area: "test".into(),
            key: "mute".into(),
            kind: "npc".into(),
            title: Some("a stranger".into()),
            description: None,
            location: Some("#1".into()),
        }).await;
        let npc = resp.data.unwrap()["ref_id"].as_str().unwrap().to_string();

        // No _ink_source saved — but passing the buffer lets you playtest a
        // draft before committing it.
        let draft = "A draft line.\n-> END";
        let resp = api_call(&tx, ApiRequest::InkPlayStart {
            ref_id: npc.clone(),
            source: Some(draft.into()),
        }).await;
        assert!(resp.ok, "start failed: {:?}", resp.error);
        assert!(resp.data.unwrap()["text"].as_str().unwrap().contains("A draft line"));

        // With nothing saved and no buffer, there is nothing to play.
        let _ = api_call(&tx, ApiRequest::InkPlayEnd { ref_id: npc.clone() }).await;
        let resp = api_call(&tx, ApiRequest::InkPlayStart { ref_id: npc.clone(), source: None }).await;
        assert!(!resp.ok);

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

        let resp = api_call(&tx, ApiRequest::SetScript {
            ref_id: item_ref.clone(),
            source: "function on_get(this, actor, room) emit(actor, \"Hum!\") end".into(),
            base_version: None,
        }).await;
        assert!(resp.ok);

        let resp = api_call(&tx, ApiRequest::GetScript {
            ref_id: item_ref.clone(),
        }).await;
        assert!(resp.ok);
        let script = resp.data.unwrap();
        assert_eq!(script["hooks"], serde_json::json!(["on_get"]));

        let resp = api_call(&tx, ApiRequest::ClearScript {
            ref_id: item_ref.clone(),
        }).await;
        assert!(resp.ok);

        let resp = api_call(&tx, ApiRequest::GetScript {
            ref_id: item_ref.clone(),
        }).await;
        // A cleared script now returns an empty-script object (so version/lock
        // can still ride), not null — its source is empty and no hooks remain.
        let script = resp.data.unwrap();
        assert_eq!(script["source"], "");
        assert_eq!(script["hooks"], serde_json::json!([]));

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_delete_object() {
        let (tx, handle) = test_engine().await;
        let item_ref = create_test_item(&tx).await;

        let resp = api_call(&tx, ApiRequest::DeleteObject {
            ref_id: item_ref.clone(),
            cascade: false,
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

        let resp = api_call(&tx, ApiRequest::SetScript {
            ref_id: item_ref,
            source: "function on_get(this actor room) end".into(),
            base_version: None,
        }).await;
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("Syntax error"));

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_softcode_versioning_history_and_revert() {
        let (tx, handle) = test_engine().await;
        let item = create_test_item(&tx).await;

        // First versioned write → version 1.
        let v1 = api_call(&tx, ApiRequest::SetScript {
            ref_id: item.clone(),
            source: "local a = 1\nlocal b = 2\n".into(),
            base_version: Some(0),
        }).await;
        assert!(v1.ok, "{:?}", v1.error);
        assert_eq!(v1.data.unwrap()["version"], 1);

        // GetScript now carries the version.
        let got = api_call(&tx, ApiRequest::GetScript { ref_id: item.clone() }).await;
        assert_eq!(got.data.unwrap()["version"], 1);

        // History lists it, resolving the author to the account username.
        let hist = api_call(&tx, ApiRequest::ListScriptVersions {
            ref_id: item.clone(),
            name: None,
        }).await;
        let versions = hist.data.unwrap();
        let versions = versions["versions"].as_array().unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0]["author_name"], "test_builder");
        assert_eq!(versions[0]["origin"], "in_game");

        // Diverge to v2 so a revert has something to undo.
        let v2 = api_call(&tx, ApiRequest::SetScript {
            ref_id: item.clone(),
            source: "local a = 100\n".into(),
            base_version: Some(1),
        }).await;
        assert_eq!(v2.data.unwrap()["version"], 2);

        // Revert to v1 makes a NEW version (3) whose body matches v1.
        let reverted = api_call(&tx, ApiRequest::RevertScript {
            ref_id: item.clone(),
            name: None,
            version: 1,
        }).await;
        assert!(reverted.ok, "{:?}", reverted.error);
        let rv = reverted.data.unwrap();
        assert_eq!(rv["version"], 3);
        assert!(rv["source"].as_str().unwrap().contains("local a = 1"));

        // A no-op write (identical to the now-current source) does not append.
        let noop = api_call(&tx, ApiRequest::SetScript {
            ref_id: item.clone(),
            source: "local a = 1\nlocal b = 2\n".into(),
            base_version: Some(3),
        }).await;
        assert_eq!(noop.data.unwrap()["version"], 3, "identical source must not bump the version");

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_softcode_stale_base_merges_cleanly() {
        let (tx, handle) = test_engine().await;
        let item = create_test_item(&tx).await;

        // v1: baseline. The `pad` lines are unchanged anchors between the two
        // regions each side edits, so a line-based 3-way merge stays clean.
        let base = "local a = 1\nlocal pad1 = 0\nlocal pad2 = 0\nlocal pad3 = 0\nlocal b = 2\n";
        api_call(&tx, ApiRequest::SetScript {
            ref_id: item.clone(),
            source: base.into(),
            base_version: Some(0),
        }).await;

        // "Theirs": someone edits the `b` region → v2.
        api_call(&tx, ApiRequest::SetScript {
            ref_id: item.clone(),
            source: "local a = 1\nlocal pad1 = 0\nlocal pad2 = 0\nlocal pad3 = 0\nlocal b = 99\n".into(),
            base_version: Some(1),
        }).await;

        // "Ours": still based on v1, edits the far-apart `a` region. Non-overlapping
        // → the server merges cleanly and records what it merged across.
        let merged = api_call(&tx, ApiRequest::SetScript {
            ref_id: item.clone(),
            source: "local a = 42\nlocal pad1 = 0\nlocal pad2 = 0\nlocal pad3 = 0\nlocal b = 2\n".into(),
            base_version: Some(1),
        }).await;
        assert!(merged.ok, "{:?}", merged.error);
        let m = merged.data.unwrap();
        assert_eq!(m["merged_from"], 2);
        let src = m["source"].as_str().unwrap();
        assert!(src.contains("a = 42") && src.contains("b = 99"), "merged both edits: {src}");

        // A conflicting edit (also touches the `b` region) is refused with the 3 sides.
        let conflict = api_call(&tx, ApiRequest::SetScript {
            ref_id: item.clone(),
            source: "local a = 1\nlocal pad1 = 0\nlocal pad2 = 0\nlocal pad3 = 0\nlocal b = 7\n".into(),
            base_version: Some(1),
        }).await;
        assert!(!conflict.ok);
        assert_eq!(conflict.error.as_deref(), Some("conflict"));
        assert_eq!(conflict.data.unwrap()["conflict"], true);

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn api_edit_lock_claim_release_and_me() {
        let (tx, handle) = test_engine().await;
        let item = create_test_item(&tx).await;

        // whoami resolves the token's account.
        let me = api_call(&tx, ApiRequest::Me).await;
        let me = me.data.unwrap();
        assert_eq!(me["username"], "test_builder");
        let account_id = me["account_id"].as_str().unwrap().to_string();

        // Give the object a script so GetScript returns an object to carry the lock.
        api_call(&tx, ApiRequest::SetScript {
            ref_id: item.clone(),
            source: "function on_get() end\n".into(),
            base_version: None,
        }).await;

        // Claim the edit lock; it then shows on GetScript, held by us.
        let claim = api_call(&tx, ApiRequest::LockScript { ref_id: item.clone(), name: None }).await;
        assert!(claim.ok, "{:?}", claim.error);
        assert_eq!(claim.data.unwrap()["held_by"], account_id);

        let got = api_call(&tx, ApiRequest::GetScript { ref_id: item.clone() }).await;
        let lock = got.data.unwrap();
        assert_eq!(lock["lock"]["held_by"], account_id);
        assert_eq!(lock["lock"]["held_by_name"], "test_builder");

        // The holder can still write.
        let write = api_call(&tx, ApiRequest::SetScript {
            ref_id: item.clone(),
            source: "local x = 1\n".into(),
            base_version: None,
        }).await;
        assert!(write.ok, "holder must be able to publish: {:?}", write.error);

        // Release clears it.
        let unlock = api_call(&tx, ApiRequest::UnlockScript { ref_id: item.clone(), name: None }).await;
        assert!(unlock.ok);
        let got = api_call(&tx, ApiRequest::GetScript { ref_id: item.clone() }).await;
        assert!(got.data.unwrap()["lock"].is_null());

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
                editor: None,
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

    // -- Archetype (is-a) resolution seams — docs/plans/archetypes.md Stage 1 --

    /// An instance with no script of its own inherits its archetype's hook:
    /// `fire_hook` resolves the *archetype's* code but binds it to the
    /// *instance* — the write lands on the instance, not the archetype.
    #[test]
    fn archetype_instance_inherits_hook_fires_bound_to_instance() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_location(&room_ref);
        hooks::set_script(
            &mut archetype,
            r#"
                function on_get(this, actor, room)
                    set_attr(this, "got_by", actor.ref_id)
                end
            "#
            .to_string(),
        );
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        let result = engine.fire_hook(&instance_ref, "on_get", &actor_ref, Some(&room_ref), None);
        assert!(result.is_ok(), "hook should run: {:?}", result.err());

        assert_eq!(
            engine.world.get(&instance_ref).unwrap().attrs.get("got_by"),
            Some(&serde_json::json!(actor_ref)),
            "the archetype's code ran, bound to the instance"
        );
        assert!(
            engine.world.get(&archetype_ref).unwrap().attrs.get("got_by").is_none(),
            "the archetype itself must be untouched"
        );
    }

    /// When the resolved script errors, the message names both the instance
    /// the hook was fired on and the archetype whose code actually ran — see
    /// docs/plans/archetypes.md's "error attribution" integration note.
    #[test]
    fn archetype_hook_error_names_both_instance_and_archetype() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_location(&room_ref);
        hooks::set_script(
            &mut archetype,
            r#"function on_get(this, actor, room) error("kaboom") end"#.to_string(),
        );
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        let err = match engine.fire_hook(&instance_ref, "on_get", &actor_ref, Some(&room_ref), None) {
            Err(e) => e,
            Ok(_) => panic!("expected the script to error"),
        };
        assert!(err.contains(&instance_ref), "error should name the instance: {}", err);
        assert!(err.contains(&archetype_ref), "error should name the resolving archetype: {}", err);
    }

    /// `state` is never delegated: two instances sharing an inherited
    /// `on_tick` accumulate independent state, and the archetype itself
    /// never accumulates any.
    #[test]
    fn tick_state_stays_per_instance_when_the_hook_is_inherited() {
        let (mut engine, _session_id, _actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "ticking_monster", Kind::Npc)
            .with_location(&room_ref);
        hooks::set_script(
            &mut archetype,
            r#"
                function on_tick(this, state, room)
                    state.count = (state.count or 0) + 1
                end
            "#
            .to_string(),
        );
        engine.world.add_object(archetype);

        let a_ref = engine.world.next_dbref();
        let mut a = GameObject::new(&a_ref, "monster_a", Kind::Npc).with_location(&room_ref);
        a.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(a);

        let b_ref = engine.world.next_dbref();
        let mut b = GameObject::new(&b_ref, "monster_b", Kind::Npc).with_location(&room_ref);
        b.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(b);

        engine.fire_tick_hook_named(&a_ref, "on_tick").unwrap();
        engine.fire_tick_hook_named(&a_ref, "on_tick").unwrap();
        engine.fire_tick_hook_named(&b_ref, "on_tick").unwrap();

        let count_of = |engine: &Engine, r: &str| {
            engine.world.get(r).unwrap().script.as_ref().unwrap().state["count"]
                .as_u64()
                .unwrap()
        };
        assert_eq!(count_of(&engine, &a_ref), 2, "instance A ticked twice");
        assert_eq!(count_of(&engine, &b_ref), 1, "instance B's state is independent of A's");
        assert!(
            engine.world.get(&archetype_ref).unwrap().script.as_ref().unwrap().state.is_empty(),
            "the archetype's own script.state must never accumulate instance state"
        );
    }

    /// `dispatch_fallback`'s `cmd_*` resolution (`hooks::find_cmd_hook`)
    /// walks the archetype chain — a `system:global`-tagged instance with no
    /// script of its own still dispatches its archetype's `cmd_*` hook.
    #[test]
    fn global_instance_inherits_cmd_dispatch_from_archetype() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "rules", Kind::Item)
            .with_location(&room_ref);
        hooks::set_script(
            &mut archetype,
            r#"
                function cmd_greet(this, actor, room, args)
                    set_attr(actor, "greeted", true)
                end
            "#
            .to_string(),
        );
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "rules_instance", Kind::Item)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        instance.tags.insert(Tag { category: "system".into(), key: "global".into() });
        engine.world.add_object(instance);

        let output = engine.dispatch_fallback(&actor_ref, "greet", "");
        assert_eq!(output, "", "cmd_greet should have dispatched with no fallback error text");
        assert_eq!(
            engine.world.get(&actor_ref).unwrap().attrs.get("greeted"),
            Some(&serde_json::json!(true))
        );
    }

    /// `DerivedIndexes::build`'s `globals_by_hook` index — which drives
    /// `fire_global_hooks` (e.g. a world-wide `on_enter`) — also walks the
    /// chain, per the plan's explicit review-finding callout.
    #[test]
    fn derived_indexes_globals_by_hook_includes_inherited_hooks() {
        let (mut engine, _session_id, _actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "town_crier", Kind::Item)
            .with_location(&room_ref);
        hooks::set_script(
            &mut archetype,
            "function on_enter(this, actor, room) end".to_string(),
        );
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "crier_instance", Kind::Item)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        instance.tags.insert(Tag { category: "system".into(), key: "global".into() });
        engine.world.add_object(instance);

        let refs = engine.indexes().globals_by_hook.get("on_enter").cloned().unwrap_or_default();
        assert!(
            refs.contains(&instance_ref),
            "an instance that only inherits on_enter should still be indexed as global: {:?}",
            refs
        );
    }

    /// End-to-end through the real Luau API: `spawn({archetype = ...})`
    /// inherits the archetype's attrs (no override needed) and fires the
    /// constructor `on_create` bound to the new instance.
    #[test]
    fn spawn_via_luau_inherits_attrs_and_runs_constructor() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_location(&room_ref);
        archetype.attrs.insert("max_hp".into(), serde_json::json!(10));
        hooks::set_script(
            &mut archetype,
            r#"
                function on_create(this, actor, room)
                    set_attr(this, "welcomed", true)
                end
            "#
            .to_string(),
        );
        engine.world.add_object(archetype);

        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(
                r#"return spawn({{ key = "goblin1", kind = "npc", location = "{}", archetype = "{}" }})"#,
                room_ref, archetype_ref
            ),
        );
        assert!(out.contains("=>"), "spawn should return the new ref: {}", out);
        let instance_ref = engine
            .world
            .objects
            .values()
            .find(|o| o.key == "goblin1")
            .map(|o| o.ref_id.clone())
            .expect("instance should have been spawned");

        let instance = engine.world.get(&instance_ref).unwrap();
        assert_eq!(instance.archetype_ref.as_deref(), Some(archetype_ref.as_str()));
        // No max_hp override was given — resolves from the archetype.
        assert_eq!(
            engine.world.resolved_attr(instance, "max_hp"),
            Some(&serde_json::json!(10))
        );
        // Constructor seam: on_create ran bound to the instance.
        assert_eq!(instance.attrs.get("welcomed"), Some(&serde_json::json!(true)));
    }

    /// `clone(ref)` flattens an instance: resolved fields land on the
    /// object and `archetype_ref` clears, all through the real Luau API.
    #[test]
    fn clone_via_luau_detaches_the_instance() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_title("Goblin")
            .with_location(&room_ref);
        archetype.attrs.insert("max_hp".into(), serde_json::json!(10));
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        let out = engine.cmd_eval(&session_id, &actor_ref, &format!(r#"clone("{}")"#, instance_ref));
        assert!(out.starts_with("OK."), "unexpected output: {}", out);

        let instance = engine.world.get(&instance_ref).unwrap();
        assert!(instance.archetype_ref.is_none());
        assert_eq!(instance.title.as_deref(), Some("Goblin"));
        assert_eq!(instance.attrs.get("max_hp"), Some(&serde_json::json!(10)));
    }

    /// The native `@destroy` command enforces the same delete guard as
    /// `apply_to`'s `Intent::Destroy` — refuses an archetype with live
    /// instances unless `--cascade` is passed.
    #[test]
    fn cmd_destroy_refuses_archetype_with_live_instances_unless_cascaded() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_title("Goblin")
            .with_location(&room_ref)
            .with_owner(&actor_ref);
        archetype.attrs.insert("max_hp".into(), serde_json::json!(10));
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        let out = engine.cmd_destroy(&session_id, &actor_ref, &archetype_ref);
        assert!(out.contains("live instances"), "unexpected output: {}", out);
        assert!(engine.world.get(&archetype_ref).is_some(), "refused delete must not touch the world");

        let out = engine.cmd_destroy(&session_id, &actor_ref, &format!("{} --cascade", archetype_ref));
        assert!(out.starts_with("Destroyed"), "unexpected output: {}", out);
        assert!(engine.world.get(&archetype_ref).is_none());

        // cascade FLATTENS, it never orphans: the instance survives with its
        // resolved fields copied down and archetype_ref cleared.
        let instance = engine.world.get(&instance_ref).expect("instance must survive cascade delete");
        assert!(instance.archetype_ref.is_none());
        assert_eq!(instance.title.as_deref(), Some("Goblin"));
        assert_eq!(instance.attrs.get("max_hp"), Some(&serde_json::json!(10)));
    }

    /// The REST `DeleteObject` action enforces the same guard/cascade
    /// behavior as `apply_to`'s `Intent::Destroy` and `@destroy`.
    #[test]
    fn api_delete_object_cascades_by_flattening_instances() {
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder]);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_title("Goblin")
            .with_location(&room_ref);
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        let resp = engine.handle_api_request(
            ApiRequest::DeleteObject { ref_id: archetype_ref.clone(), cascade: false },
            Some(token.clone()),
        );
        assert!(!resp.ok, "should refuse without cascade");
        assert!(engine.world.get(&archetype_ref).is_some());

        let resp = engine.handle_api_request(
            ApiRequest::DeleteObject { ref_id: archetype_ref.clone(), cascade: true },
            Some(token),
        );
        assert!(resp.ok, "cascade should succeed: {:?}", resp.error);
        assert!(engine.world.get(&archetype_ref).is_none());

        let instance = engine.world.get(&instance_ref).expect("instance survives cascade delete");
        assert!(instance.archetype_ref.is_none());
        assert_eq!(instance.title.as_deref(), Some("Goblin"));
    }

    // -- Attr/tag/name resolution consistency — docs/plans/archetypes.md --

    /// `has_attr`, `pick`, and `find_by_attr` all resolve up the archetype
    /// chain, not just `get_attr`/`this.foo`.
    #[test]
    fn has_attr_pick_and_find_by_attr_resolve_inherited_attrs() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_location(&room_ref);
        archetype.attrs.insert(
            "stats".into(),
            serde_json::json!({"hp": 10, "def": 3}),
        );
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(r#"return has_attr("{}", "stats")"#, instance_ref),
        );
        assert!(out.contains("=> true"), "has_attr should see the inherited attr: {}", out);

        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(r#"return pick("{}", "stats", "hp")"#, instance_ref),
        );
        assert!(out.contains("=> 10"), "pick should read the inherited nested value: {}", out);

        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(
                r#"
                local found = find_by_attr("stats", {{ hp = 10, def = 3 }})
                for _, o in ipairs(found) do
                    if o.ref_id == "{}" then return true end
                end
                return false
                "#,
                instance_ref
            ),
        );
        assert!(out.contains("=> true"), "find_by_attr should find the instance via its inherited attr: {}", out);
    }

    /// `set_val` on an instance that inherits the whole attr (no override of
    /// its own) must copy the FULL resolved value down before editing the
    /// leaf — otherwise the un-edited siblings are dropped.
    #[test]
    fn set_val_on_an_inherited_attr_preserves_untouched_siblings() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_location(&room_ref);
        archetype.attrs.insert(
            "stats".into(),
            serde_json::json!({"hp": 10, "def": 3, "name": "Grunt"}),
        );
        engine.world.add_object(archetype);

        // No own "stats" attr — it's fully inherited.
        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(r#"set_val("{}", "stats", "hp", 99)"#, instance_ref),
        );
        assert!(out.starts_with("OK."), "unexpected output: {}", out);

        let instance = engine.world.get(&instance_ref).unwrap();
        let stats = instance.attrs.get("stats").expect("set_val must copy the whole value onto the instance");
        assert_eq!(stats["hp"], 99, "the edited leaf");
        assert_eq!(stats["def"], 3, "an untouched sibling must survive");
        assert_eq!(stats["name"], "Grunt", "another untouched sibling must survive");

        // The archetype's own copy is untouched.
        let archetype = engine.world.get(&archetype_ref).unwrap();
        assert_eq!(archetype.attrs.get("stats").unwrap()["hp"], 10);
    }

    /// `item:container` inherited from an archetype makes `is_container`
    /// true, and `has_tag` sees an inherited tag directly.
    #[test]
    fn is_container_and_has_tag_see_an_inherited_tag() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "bag", Kind::Item).with_location(&room_ref);
        archetype.tags.insert(Tag { category: "item".into(), key: "container".into() });
        engine.world.add_object(archetype);

        // No own tags — item:container is purely inherited.
        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "bag1", Kind::Item).with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        let out = engine.cmd_eval(&session_id, &actor_ref, &format!(r#"return is_container("{}")"#, instance_ref));
        assert!(out.contains("=> true"), "is_container should see the inherited tag: {}", out);

        let out = engine.cmd_eval(&session_id, &actor_ref, &format!(r#"return has_tag("{}", "item:container")"#, instance_ref));
        assert!(out.contains("=> true"), "has_tag should see the inherited tag: {}", out);

        // find_by_tag must also resolve up the chain — an instance that only
        // inherits the tag is still found (regression: it filtered raw tags).
        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(r#"for _, o in find_by_tag("item:container") do if o.ref_id == "{}" then return "found" end end return "missing""#, instance_ref),
        );
        assert!(out.contains("found"), "find_by_tag should return the inheriting instance: {}", out);
    }

    /// `system:global` inherited from an archetype (not set on the instance
    /// itself) still makes the instance count as global — both in
    /// `DerivedIndexes::build`'s `globals_by_hook` index and in actual
    /// `cmd_*` dispatch.
    #[test]
    fn system_global_tag_inherited_from_archetype_makes_instance_globally_dispatched() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "rules", Kind::Item).with_location(&room_ref);
        archetype.tags.insert(Tag { category: "system".into(), key: "global".into() });
        hooks::set_script(
            &mut archetype,
            r#"
                function cmd_greet(this, actor, room, args)
                    set_attr(actor, "greeted", true)
                end
            "#
            .to_string(),
        );
        engine.world.add_object(archetype);

        // No own system:global tag — only inherited from the archetype.
        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "rules_instance", Kind::Item)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        let refs = engine.indexes().globals_by_hook.get("cmd_greet").cloned().unwrap_or_default();
        assert!(
            refs.contains(&instance_ref),
            "instance should be indexed as global via its inherited system:global tag: {:?}",
            refs
        );

        let output = engine.dispatch_fallback(&actor_ref, "greet", "");
        assert_eq!(output, "", "cmd_greet should have dispatched with no fallback error text");
        assert_eq!(
            engine.world.get(&actor_ref).unwrap().attrs.get("greeted"),
            Some(&serde_json::json!(true))
        );
    }

    /// An instance with no title of its own inherits its archetype's, and
    /// the native `get` command's fuzzy name matcher resolves the chain too
    /// — so a player can `get goblin ear` even though only the archetype
    /// carries that title.
    #[test]
    fn get_command_matches_an_instance_by_its_inherited_title() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        // The archetype itself isn't placed anywhere in the world — like a
        // real prototype/blueprint object, only its instances are physically
        // present (giving it the same room location would make it a second,
        // ambiguous name-match for "goblin ear" alongside the instance).
        let archetype_ref = engine.world.next_dbref();
        let archetype = GameObject::new(&archetype_ref, "goblin_ear_archetype", Kind::Item)
            .with_title("Goblin Ear");
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "trophy1", Kind::Item)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        let out = engine.cmd_get(&actor_ref, "goblin ear");
        assert!(out.starts_with("You pick up"), "unexpected output: {}", out);
        assert_eq!(
            engine.world.get(&instance_ref).and_then(|o| o.location_ref.clone()),
            Some(actor_ref.clone()),
            "the instance (not the archetype) should have moved into the actor's inventory"
        );
    }

    // -- Stage 2: clear_attr / pass() — docs/plans/archetypes.md --

    /// `clear_attr` removes an instance's OWN attribute override so
    /// `get_attr` falls back to the archetype's value again — the exact
    /// effect `unset_attr` already has, under the archetype-facing name (MOO
    /// `clear_property`).
    #[test]
    fn clear_attr_reverts_instance_override_to_inherited_value() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_location(&room_ref);
        archetype.attrs.insert("hp".into(), serde_json::json!(10));
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        instance.attrs.insert("hp".into(), serde_json::json!(3));
        engine.world.add_object(instance);

        // The override wins while it's set.
        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(r#"return get_attr("{}", "hp")"#, instance_ref),
        );
        assert!(out.contains("=> 3"), "override should win: {}", out);

        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(r#"clear_attr("{}", "hp")"#, instance_ref),
        );
        assert!(out.starts_with("OK."), "unexpected output: {}", out);

        assert!(
            engine.world.get(&instance_ref).unwrap().attrs.get("hp").is_none(),
            "clear_attr must remove the instance's OWN attr"
        );
        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(r#"return get_attr("{}", "hp")"#, instance_ref),
        );
        assert!(out.contains("=> 10"), "get_attr should fall through to the archetype's value: {}", out);
    }

    #[test]
    fn clear_attr_then_read_in_the_same_run_sees_the_inherited_value() {
        // Regression: within ONE hook run, `clear_attr` then `get_attr` on the
        // same key must return the archetype's value (the override is gone),
        // not nil — the pending unset must fall through the chain, matching the
        // post-commit read.
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc).with_location(&room_ref);
        archetype.attrs.insert("hp".into(), serde_json::json!(10));
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc).with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        instance.attrs.insert("hp".into(), serde_json::json!(3));
        engine.world.add_object(instance);

        let out = engine.cmd_eval(
            &session_id,
            &actor_ref,
            &format!(r#"clear_attr("{r}", "hp"); return get_attr("{r}", "hp")"#, r = instance_ref),
        );
        assert!(
            out.contains("=> 10"),
            "same-run read after clear_attr must see the inherited value, not nil: {}",
            out
        );
    }

    #[test]
    fn examine_reports_archetype_delegation_and_instance_count() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let arch = engine.world.next_dbref();
        let mut a = GameObject::new(&arch, "goblin", Kind::Npc).with_title("Goblin");
        a.description = "A snarling goblin.".into();
        a.attrs.insert("armor".into(), serde_json::json!(2));
        a.tags.insert(Tag::parse("kind:monster").unwrap());
        hooks::set_script(&mut a, "function on_look(this, actor, room) end".to_string());
        engine.world.add_object(a);

        // A pure delegate: no own title or description, one own tag.
        let inst = engine.world.next_dbref();
        let mut i = GameObject::new(&inst, "grunt", Kind::Npc);
        i.archetype_ref = Some(arch.clone());
        i.attrs.insert("hp".into(), serde_json::json!(3)); // own
        i.tags.insert(Tag::parse("faction:raiders").unwrap());
        hooks::set_script(&mut i, "function on_death(this, actor, room) end".to_string());
        engine.world.add_object(i);

        let ex = engine
            .handle_api_request(ApiRequest::Examine { ref_id: inst.clone() }, Some(token.clone()))
            .data
            .unwrap();
        assert_eq!(ex["archetype_ref"].as_str(), Some(arch.as_str()));
        assert_eq!(ex["archetype"]["title"].as_str(), Some("Goblin"));
        // per-attr provenance
        assert_eq!(ex["resolved_attrs"]["hp"]["source"].as_str(), Some("own"));
        assert_eq!(ex["resolved_attrs"]["armor"]["source"].as_str(), Some(arch.as_str()));
        assert_eq!(ex["resolved_attrs"]["armor"]["value"], serde_json::json!(2));
        // per-hook origin
        let hooks_v = ex["resolved_hooks"].as_array().unwrap();
        let find = |h: &str| hooks_v.iter().find(|e| e["hook"] == h).map(|e| e["source"].as_str().unwrap().to_string());
        assert_eq!(find("on_death").as_deref(), Some("own"));
        assert_eq!(find("on_look").as_deref(), Some(arch.as_str()));

        // Effective title/description come from the archetype (own is unset).
        assert!(ex["title"].is_null());
        assert_eq!(ex["resolved_title"].as_str(), Some("Goblin"));
        assert_eq!(ex["description"].as_str(), Some(""));
        assert_eq!(ex["resolved_description"].as_str(), Some("A snarling goblin."));

        // Per-tag provenance: own tag "own", inherited tag names the ancestor.
        let tags_v = ex["resolved_tags"].as_array().unwrap();
        let tag_src = |t: &str| tags_v.iter().find(|e| e["tag"] == t).map(|e| e["source"].as_str().unwrap().to_string());
        assert_eq!(tag_src("faction:raiders").as_deref(), Some("own"));
        assert_eq!(tag_src("kind:monster").as_deref(), Some(arch.as_str()));

        // instance_count on the archetype
        let exa = engine
            .handle_api_request(ApiRequest::Examine { ref_id: arch.clone() }, Some(token.clone()))
            .data
            .unwrap();
        assert_eq!(exa["instance_count"].as_u64(), Some(1));
    }

    #[test]
    fn examine_reports_resolved_attr_schema_with_source() {
        use crate::attr_schema::{AttrDescriptor, AttrType};
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);

        // Archetype declares hp (int, min 0) + attack (int).
        let arch = engine.world.next_dbref();
        let mut a = GameObject::new(&arch, "monster", Kind::Npc).with_title("Monster");
        let mut hp = AttrDescriptor::new("hp", AttrType::Int);
        hp.label = Some("Hit points".into());
        hp.min = Some(0.0);
        a.attr_schema = vec![hp, AttrDescriptor::new("attack", AttrType::Int)];
        engine.world.add_object(a);

        // Instance inherits the schema and declares one more of its own.
        let inst = engine.world.next_dbref();
        let mut i = GameObject::new(&inst, "goblin", Kind::Npc);
        i.archetype_ref = Some(arch.clone());
        i.attr_schema = vec![AttrDescriptor::new("cunning", AttrType::Int)];
        engine.world.add_object(i);

        let ex = engine
            .handle_api_request(ApiRequest::Examine { ref_id: inst.clone() }, Some(token.clone()))
            .data
            .unwrap();
        let schema = ex["attr_schema"].as_array().unwrap();
        let entry = |k: &str| schema.iter().find(|e| e["key"] == k).unwrap();
        // Own descriptor marked "own"; type serializes under "type".
        assert_eq!(entry("cunning")["source"].as_str(), Some("own"));
        assert_eq!(entry("cunning")["type"].as_str(), Some("int"));
        // Inherited descriptors carry the archetype ref as source, with extras.
        assert_eq!(entry("hp")["source"].as_str(), Some(arch.as_str()));
        assert_eq!(entry("hp")["label"].as_str(), Some("Hit points"));
        assert_eq!(entry("hp")["min"].as_f64(), Some(0.0));
        assert_eq!(entry("attack")["source"].as_str(), Some(arch.as_str()));
        assert_eq!(schema.len(), 3);
    }

    #[test]
    fn list_ref_candidates_matches_kind_and_resolved_tag() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);

        let goblin = engine.world.next_dbref();
        let mut g = GameObject::new(&goblin, "goblin", Kind::Npc).with_title("Goblin");
        g.tags.insert(Tag::parse("kind:monster").unwrap());
        engine.world.add_object(g);

        // A pure delegate inherits the "kind:monster" tag (resolved), so a
        // tag: query must find it too.
        let grunt = engine.world.next_dbref();
        let mut gr = GameObject::new(&grunt, "grunt", Kind::Npc);
        gr.archetype_ref = Some(goblin.clone());
        engine.world.add_object(gr);

        let sword = engine.world.next_dbref();
        engine
            .world
            .add_object(GameObject::new(&sword, "sword", Kind::Item).with_title("a sword"));

        let mut refs = |src: &str| -> Vec<String> {
            engine
                .handle_api_request(
                    ApiRequest::ListRefCandidates { ref_source: src.to_string() },
                    Some(token.clone()),
                )
                .data
                .unwrap()["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["ref_id"].as_str().unwrap().to_string())
                .collect()
        };

        let npcs = refs("kind:npc");
        assert!(npcs.contains(&goblin) && npcs.contains(&grunt) && !npcs.contains(&sword));
        let items = refs("kind:item");
        assert!(items.contains(&sword) && !items.contains(&goblin));
        // Resolved tags: the grunt inherits kind:monster from its archetype.
        let monsters = refs("tag:kind:monster");
        assert!(monsters.contains(&goblin) && monsters.contains(&grunt));
        // Unknown source → no candidates.
        assert!(refs("nonsense").is_empty());
    }

    #[test]
    fn set_archetype_sets_refuses_cycle_and_clears() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let arch = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&arch, "goblin", Kind::Npc).with_title("Goblin"));
        let inst = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&inst, "grunt", Kind::Npc));

        // set
        let r = engine.handle_api_request(
            ApiRequest::SetArchetype { ref_id: inst.clone(), archetype_ref: Some(arch.clone()) },
            Some(token.clone()),
        );
        assert!(r.ok, "{:?}", r.error);
        assert_eq!(engine.world.get(&inst).unwrap().archetype_ref.as_deref(), Some(arch.as_str()));

        // cycle refused (arch -> inst, while inst -> arch)
        let r = engine.handle_api_request(
            ApiRequest::SetArchetype { ref_id: arch.clone(), archetype_ref: Some(inst.clone()) },
            Some(token.clone()),
        );
        assert!(!r.ok && r.error.unwrap().contains("cycle"));
        assert_eq!(engine.world.get(&arch).unwrap().archetype_ref, None);

        // clear
        let r = engine.handle_api_request(
            ApiRequest::SetArchetype { ref_id: inst.clone(), archetype_ref: None },
            Some(token),
        );
        assert!(r.ok);
        assert_eq!(engine.world.get(&inst).unwrap().archetype_ref, None);
    }

    /// Two-level chain: a child's `on_death` runs, calls `pass()`, and the
    /// archetype's `on_death` runs too — bound to the SAME instance, so both
    /// side effects land on the child, not the archetype.
    #[test]
    fn pass_runs_both_the_instance_and_the_archetypes_hook() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "monster", Kind::Npc)
            .with_location(&room_ref);
        hooks::set_script(
            &mut archetype,
            r#"function on_death(this, actor, room) set_attr(this, "parent_ran", true) end"#
                .to_string(),
        );
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        hooks::set_script(
            &mut instance,
            r#"
                function on_death(this, actor, room)
                    set_attr(this, "child_ran", true)
                    pass()
                end
            "#
            .to_string(),
        );
        engine.world.add_object(instance);

        let result = engine.fire_hook(&instance_ref, "on_death", &actor_ref, Some(&room_ref), None);
        assert!(result.is_ok(), "hook should run: {:?}", result.err());

        let instance = engine.world.get(&instance_ref).unwrap();
        assert_eq!(instance.attrs.get("child_ran"), Some(&serde_json::json!(true)), "the instance's own on_death should have run");
        assert_eq!(instance.attrs.get("parent_ran"), Some(&serde_json::json!(true)), "pass() should have run the archetype's on_death, bound to the instance");

        assert!(
            engine.world.get(&archetype_ref).unwrap().attrs.get("parent_ran").is_none(),
            "the archetype itself must be untouched — pass() binds `this` to the instance"
        );
    }

    /// Three-level chain where the middle level ALSO calls `pass()`: all
    /// three definitions run, each `pass()` searching further up from where
    /// IT resolved rather than back to the top.
    #[test]
    fn pass_chains_through_a_three_level_archetype_hierarchy() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let grandparent_ref = engine.world.next_dbref();
        let mut grandparent = GameObject::new(&grandparent_ref, "creature", Kind::Npc)
            .with_location(&room_ref);
        hooks::set_script(
            &mut grandparent,
            r#"function on_death(this, actor, room) set_attr(this, "grandparent_ran", true) end"#
                .to_string(),
        );
        engine.world.add_object(grandparent);

        let parent_ref = engine.world.next_dbref();
        let mut parent = GameObject::new(&parent_ref, "monster", Kind::Npc)
            .with_location(&room_ref);
        parent.archetype_ref = Some(grandparent_ref.clone());
        hooks::set_script(
            &mut parent,
            r#"
                function on_death(this, actor, room)
                    set_attr(this, "parent_ran", true)
                    pass()
                end
            "#
            .to_string(),
        );
        engine.world.add_object(parent);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc)
            .with_location(&room_ref);
        instance.archetype_ref = Some(parent_ref.clone());
        hooks::set_script(
            &mut instance,
            r#"
                function on_death(this, actor, room)
                    set_attr(this, "child_ran", true)
                    pass()
                end
            "#
            .to_string(),
        );
        engine.world.add_object(instance);

        let result = engine.fire_hook(&instance_ref, "on_death", &actor_ref, Some(&room_ref), None);
        assert!(result.is_ok(), "hook should run: {:?}", result.err());

        let instance = engine.world.get(&instance_ref).unwrap();
        assert_eq!(instance.attrs.get("child_ran"), Some(&serde_json::json!(true)));
        assert_eq!(instance.attrs.get("parent_ran"), Some(&serde_json::json!(true)));
        assert_eq!(instance.attrs.get("grandparent_ran"), Some(&serde_json::json!(true)));
    }

    /// `pass()` with no ancestor defining the hook is a safe no-op that
    /// returns nil — it must not error, and the calling hook keeps running
    /// after it.
    #[test]
    fn pass_with_no_ancestor_definer_is_a_safe_noop() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        // No archetype_ref at all — nothing to walk up to.
        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "lonely_goblin", Kind::Npc)
            .with_location(&room_ref);
        hooks::set_script(
            &mut instance,
            r#"
                function on_death(this, actor, room)
                    local result = pass()
                    set_attr(this, "pass_returned_nil", result == nil)
                    set_attr(this, "ran_after_pass", true)
                end
            "#
            .to_string(),
        );
        engine.world.add_object(instance);

        let result = engine.fire_hook(&instance_ref, "on_death", &actor_ref, Some(&room_ref), None);
        assert!(result.is_ok(), "pass() with no ancestor must not error: {:?}", result.err());

        let instance = engine.world.get(&instance_ref).unwrap();
        assert_eq!(instance.attrs.get("pass_returned_nil"), Some(&serde_json::json!(true)));
        assert_eq!(instance.attrs.get("ran_after_pass"), Some(&serde_json::json!(true)));
    }

    /// `pass()` forwards the calling hook's own args by default, but an
    /// explicit argument to `pass(...)` overrides what gets forwarded.
    #[test]
    fn pass_forwards_args_by_default_and_explicit_args_override() {
        let (mut engine, _session_id, actor_ref) = test_engine_with_session(true);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "monster", Kind::Npc)
            .with_location(&room_ref);
        hooks::set_script(
            &mut archetype,
            r#"function on_use(this, actor, room, args) set_attr(this, "parent_args", args) end"#
                .to_string(),
        );
        engine.world.add_object(archetype);

        // Default: pass() with no args forwards this hook's own args.
        let default_ref = engine.world.next_dbref();
        let mut default_instance = GameObject::new(&default_ref, "goblin_default", Kind::Npc)
            .with_location(&room_ref);
        default_instance.archetype_ref = Some(archetype_ref.clone());
        hooks::set_script(
            &mut default_instance,
            r#"function on_use(this, actor, room, args) pass() end"#.to_string(),
        );
        engine.world.add_object(default_instance);

        let result = engine.fire_hook(&default_ref, "on_use", &actor_ref, Some(&room_ref), Some("original"));
        assert!(result.is_ok(), "hook should run: {:?}", result.err());
        assert_eq!(
            engine.world.get(&default_ref).unwrap().attrs.get("parent_args"),
            Some(&serde_json::json!("original")),
            "pass() with no args should forward this call's own args"
        );

        // Explicit: pass("overridden") wins over the forwarded args.
        let explicit_ref = engine.world.next_dbref();
        let mut explicit_instance = GameObject::new(&explicit_ref, "goblin_explicit", Kind::Npc)
            .with_location(&room_ref);
        explicit_instance.archetype_ref = Some(archetype_ref.clone());
        hooks::set_script(
            &mut explicit_instance,
            r#"function on_use(this, actor, room, args) pass("overridden") end"#.to_string(),
        );
        engine.world.add_object(explicit_instance);

        let result = engine.fire_hook(&explicit_ref, "on_use", &actor_ref, Some(&room_ref), Some("original"));
        assert!(result.is_ok(), "hook should run: {:?}", result.err());
        assert_eq!(
            engine.world.get(&explicit_ref).unwrap().attrs.get("parent_args"),
            Some(&serde_json::json!("overridden")),
            "explicit pass(args) should override the forwarded args"
        );
    }

    /// The empty state-only stub `ensure_own_state_slot` leaves on an
    /// instance that only ticks an inherited `on_tick` must not read as "has
    /// a script" anywhere that's surfaced to a person.
    #[test]
    fn phantom_state_only_stub_does_not_count_as_has_script() {
        let (mut engine, token, _account_id) = engine_with_api_token(&[Scope::Builder]);
        let room_ref = engine.spawn_room_ref.clone();

        let archetype_ref = engine.world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "ticking_monster", Kind::Npc)
            .with_location(&room_ref);
        hooks::set_script(
            &mut archetype,
            "function on_tick(this, state, room) state.count = (state.count or 0) + 1 end".to_string(),
        );
        engine.world.add_object(archetype);

        let instance_ref = engine.world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "monster1", Kind::Npc).with_location(&room_ref);
        instance.archetype_ref = Some(archetype_ref.clone());
        engine.world.add_object(instance);

        // Firing the inherited tick creates the state-only stub.
        engine.fire_tick_hook_named(&instance_ref, "on_tick").unwrap();
        assert!(
            engine.world.get(&instance_ref).unwrap().script.is_some(),
            "sanity: the stub was created"
        );

        let resp = engine.handle_api_request(
            ApiRequest::Examine { ref_id: instance_ref.clone() },
            Some(token.clone()),
        );
        assert!(resp.ok);
        assert_eq!(
            resp.data.unwrap()["has_script"],
            false,
            "a state-only stub is not an authored script"
        );

        let resp = engine.handle_api_request(ApiRequest::ListProgramsAll, Some(token));
        assert!(resp.ok);
        let listed = resp.data.unwrap();
        let refs: Vec<String> = listed
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["ref_id"].as_str().unwrap().to_string())
            .collect();
        assert!(
            !refs.contains(&instance_ref),
            "an instance with only a state stub shouldn't appear in the program list: {:?}",
            refs
        );
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

        // Bare `@eval` enters the multi-line editor — engine-owned session
        // state now, not an object attr.
        engine.handle_input(&session_id, "@eval");
        assert_eq!(
            engine.sessions.get(&session_id).unwrap().editor,
            Some(EditorMode::Eval)
        );

        engine.handle_input(&session_id, &format!(r#"set_attr("{}", "line_one", 1)"#, target));
        engine.handle_input(&session_id, &format!(r#"set_attr("{}", "line_two", 2)"#, target));
        engine.handle_input(&session_id, ".");

        // Editor state is cleared once the buffer runs.
        assert_eq!(engine.sessions.get(&session_id).unwrap().editor, None);
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

        assert_eq!(engine.sessions.get(&session_id).unwrap().editor, None);
    }

    // -- Puppeting: the effective-actor routing that ADR-0008 completes --

    /// Build an NPC in `room` and return its ref.
    fn add_npc(engine: &mut Engine, room: &str, key: &str) -> String {
        let npc_ref = engine.world.next_dbref();
        let npc = GameObject::new(&npc_ref, key, Kind::Npc)
            .with_title(key)
            .with_location(room);
        engine.world.add_object(npc);
        npc_ref
    }

    #[test]
    fn session_accessors_split_character_from_effective_actor() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room = engine.world.get(&actor_ref).unwrap().location_ref.clone().unwrap();
        let npc = add_npc(&mut engine, &room, "goblin");

        // Not puppeting: effective actor == character.
        {
            let s = engine.sessions.get(&session_id).unwrap();
            assert_eq!(s.character(), Some(actor_ref.as_str()));
            assert_eq!(s.effective_actor(), Some(actor_ref.as_str()));
            assert_eq!(s.puppet(), None);
        }

        engine.handle_input(&session_id, &format!("@puppet {}", npc));

        // Puppeting: character is unchanged, effective actor is the NPC.
        let s = engine.sessions.get(&session_id).unwrap();
        assert_eq!(s.character(), Some(actor_ref.as_str()));
        assert_eq!(s.effective_actor(), Some(npc.as_str()));
        assert_eq!(s.puppet(), Some(npc.as_str()));
    }

    #[test]
    fn puppet_routes_gameplay_to_the_npc_not_the_character() {
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room1 = engine.world.get(&actor_ref).unwrap().location_ref.clone().unwrap();

        // A second room, an exit "north" from room1 to it, and an NPC in room1.
        let room2 = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&room2, "north_room", Kind::Room).with_title("North"));
        let exit_ref = engine.world.next_dbref();
        engine.world.add_object(
            GameObject::new(&exit_ref, "north", Kind::Exit)
                .with_location(&room1)
                .with_target(&room2),
        );
        let npc = add_npc(&mut engine, &room1, "goblin");

        engine.handle_input(&session_id, &format!("@puppet {}", npc));
        engine.handle_input(&session_id, "go north");

        // The PUPPET moved; the character stayed put. This is the behavior the
        // feature described but never delivered before ADR-0008.
        assert_eq!(
            engine.world.get(&npc).unwrap().location_ref.as_deref(),
            Some(room2.as_str()),
            "gameplay routes to the effective actor"
        );
        assert_eq!(
            engine.world.get(&actor_ref).unwrap().location_ref.as_deref(),
            Some(room1.as_str()),
            "the character does not move while puppeting"
        );
    }

    #[test]
    fn charswitch_would_drop_the_puppet() {
        // enter_world rebuilds Playing with puppet_ref: None, so any path that
        // re-enters the world (charswitch, reconnect) releases the puppet.
        let (mut engine, session_id, actor_ref) = test_engine_with_session(true);
        let room = engine.world.get(&actor_ref).unwrap().location_ref.clone().unwrap();
        let npc = add_npc(&mut engine, &room, "goblin");
        engine.handle_input(&session_id, &format!("@puppet {}", npc));
        assert_eq!(engine.sessions.get(&session_id).unwrap().puppet(), Some(npc.as_str()));

        engine.handle_input(&session_id, "@unpuppet");
        assert_eq!(engine.sessions.get(&session_id).unwrap().puppet(), None);
        // After releasing, gameplay is the character again.
        assert_eq!(
            engine.sessions.get(&session_id).unwrap().effective_actor(),
            Some(actor_ref.as_str())
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
        engine.world.relocate(&code_ref, Some(actor_ref.clone()));
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
        hooks::set_script(
            &mut obj,
            "function on_tick(this, state, room) state.ticks = (state.ticks or 0) + 1 end".into(),
        );
        engine.world.add_object(obj);

        engine.do_tick();
        engine.do_tick();

        let ticks = engine.world.get(&ref_id).unwrap().script.as_ref().unwrap()
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
            engine.world.get(&ref_id).unwrap().script.as_ref().unwrap().state.get("ticks"),
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
    fn format_epoch_secs_matches_known_values() {
        assert_eq!(format_epoch_secs(0), "1970-01-01 00:00:00 UTC");
        // 2023-11-14T22:13:20Z
        assert_eq!(format_epoch_secs(1_700_000_000), "2023-11-14 22:13:20 UTC");
        // 2000-02-29 exercises the leap-day branch of civil_from_days.
        assert_eq!(format_epoch_secs(951_782_400), "2000-02-29 00:00:00 UTC");
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

    #[test]
    fn run_tests_api_reports_pass_and_fail_under_builder_scope() {
        // The builder's test panel runs an ad-hoc `source` (no game_dir needed)
        // and only needs Builder scope — it executes on a world clone, so it is
        // strictly less privileged than the `set_program` a Builder already has.
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let source = "\
function test_math_ok()
  assert_eq(1 + 1, 2)
end
function test_math_bad()
  assert_eq(1 + 1, 3)
end
";
        let resp = engine.handle_api_request(
            ApiRequest::RunTests { source: Some(source.into()), file: None, ref_id: None },
            Some(token),
        );
        assert!(resp.ok, "{:?}", resp.error);
        let data = resp.data.unwrap();
        assert_eq!(data["passed"], 1);
        assert_eq!(data["failed"], 1);
        let files = data["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["tests"].as_array().unwrap().len(), 2);
    }

    /// Tests co-located in an object's own script (`test_*` alongside the
    /// hooks) run via `RunTests { ref_id }`, with `ctx.this` bound to that
    /// object so its tests exercise itself.
    #[test]
    fn embedded_object_tests_run_with_this_bound() {
        let (mut engine, token, _) = engine_with_api_token(&[Scope::Builder]);
        let room = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&room, "room", Kind::Room));
        let obj = engine.world.next_dbref();
        let mut o = GameObject::new(&obj, "widget", Kind::Item).with_location(&room);
        crate::softcode::hooks::set_script(
            &mut o,
            "function on_use(this, actor, room) end\n\
             function test_this_is_the_object(ctx)\n\
               local me = get_object(ctx.this)\n\
               assert_eq(me.key, \"widget\")\n\
             end\n\
             function test_intentional_fail()\n\
               assert_true(false)\n\
             end\n"
                .to_string(),
        );
        engine.world.add_object(o);

        let resp = engine.handle_api_request(
            ApiRequest::RunTests { source: None, file: None, ref_id: Some(obj.clone()) },
            Some(token.clone()),
        );
        assert!(resp.ok, "{:?}", resp.error);
        let data = resp.data.unwrap();
        assert_eq!(data["passed"], 1, "the this-bound test passes");
        assert_eq!(data["failed"], 1, "the intentional failure is reported");
        let tests = data["files"][0]["tests"].as_array().unwrap();
        assert!(tests.iter().any(|t| t["name"] == "test_this_is_the_object" && t["passed"] == true));
        assert!(tests.iter().any(|t| t["name"] == "test_intentional_fail" && t["passed"] == false));

        // An object with no script reports a clear error.
        let bare = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&bare, "bare", Kind::Item));
        let none = engine.handle_api_request(
            ApiRequest::RunTests { source: None, file: None, ref_id: Some(bare) },
            Some(token),
        );
        assert!(!none.ok);
    }

    #[test]
    fn eval_preview_reports_writes_without_applying() {
        // The REPL previews: it runs the line, reports the writes it WOULD make,
        // and leaves the live world untouched — Builder scope, no admin.
        let (mut engine, token, account_id) = engine_with_api_token(&[Scope::Builder]);
        // Give the account a character to act as, in a room.
        let room = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&room, "hall", Kind::Room));
        let pc = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&pc, "hero", Kind::Player));
        engine.world.relocate(&pc, Some(room.clone()));
        if let Some(a) = engine.accounts.get_mut(&account_id) { a.active_character = Some(pc.clone()); }

        let resp = engine.handle_api_request(
            ApiRequest::EvalPreview {
                source: format!("set_attr(\"{}\", \"hp\", 5)\nreturn 42", pc),
            },
            Some(token),
        );
        assert!(resp.ok, "{:?}", resp.error);
        let data = resp.data.unwrap();
        assert_eq!(data["returned"], "42");
        assert_eq!(data["write_count"], 1);
        assert_eq!(data["writes"].as_array().unwrap().len(), 1);
        // Crucially: the live world was NOT mutated by the preview.
        assert!(
            engine.world.get(&pc).unwrap().attrs.get("hp").is_none(),
            "preview must not apply its writes to the live world"
        );
    }

    #[test]
    fn preview_hook_fires_and_reports_emits_without_applying() {
        // Preview-fire an on_enter as a chosen actor: the emits + writes show up
        // as would-apply intents, and the live world stays untouched.
        let (mut engine, token, account_id) = engine_with_api_token(&[Scope::Builder]);
        let room = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&room, "hall", Kind::Room));
        let pc = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&pc, "hero", Kind::Player));
        engine.world.relocate(&pc, Some(room.clone()));
        if let Some(a) = engine.accounts.get_mut(&account_id) { a.active_character = Some(pc.clone()); }

        let source = "\
function on_enter(this, actor, room)
  if not is_player(actor) then return end
  emit(actor, \"Cold air.\")
  set_attr(this, \"entered\", true)
end
";
        let resp = engine.handle_api_request(
            ApiRequest::PreviewHook {
                ref_id: room.clone(),
                hook: "on_enter".into(),
                source: Some(source.into()),
                actor_ref: Some(pc.clone()),
                room_ref: None,
            },
            Some(token),
        );
        assert!(resp.ok, "{:?}", resp.error);
        let data = resp.data.unwrap();
        assert_eq!(data["write_count"], 2); // the emit + the set_attr
        assert_eq!(data["denied"], false);
        assert!(
            engine.world.get(&room).unwrap().attrs.get("entered").is_none(),
            "preview-fire must not apply its writes to the live world"
        );
    }

    #[test]
    fn run_tests_api_requires_builder_scope() {
        // A bare player can't run tests — the whole authoring surface is
        // Builder-gated (see the auth block at the top of handle_api_request).
        let (mut engine, token, _) = engine_with_api_token(&[]);
        let resp = engine.handle_api_request(
            ApiRequest::RunTests { source: Some("function test_x() end".into()), file: None, ref_id: None },
            Some(token),
        );
        assert!(!resp.ok);
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
        assert_eq!(prefixes, ["on_", "cmd_"]);
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
            ApiRequest::SetScript { ref_id: r.clone(), source: "function on_enter(this, actor, room) end".into(), base_version: None },
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
        engine.handle_api_request(ApiRequest::DeleteObject { ref_id: b.clone(), cascade: false }, Some(token.clone()));
        // Inject a script that doesn't compile (bypassing SetScript's own check).
        if let Some(obj) = engine.world.get_mut(&a) {
            hooks::set_script(obj, "local x = (".to_string());
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
        // Routed through Intent::UpdateExit, whose refusal names the missing
        // destination (the unified authoring/softcode message).
        assert!(bad.error.as_deref().unwrap().contains("no destination"));
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
        hooks::set_script(&mut obj, "function on_use(this, actor, room)\n  -- secret source\nend".to_string());
        engine.world.add_object(obj);

        let unauthenticated = engine.handle_api_request(ApiRequest::GetScript { ref_id: ref_id.clone() }, None);
        assert!(!unauthenticated.ok, "GetScript must not serve script source with no token");
        assert_eq!(unauthenticated.error.as_deref(), Some("Authentication required"));

        let authenticated = engine.handle_api_request(ApiRequest::GetScript { ref_id }, Some(token));
        assert!(authenticated.ok, "{:?}", authenticated.error);
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

    /// RBAC audit M2: room/object/exit enumeration leaks area layout and, by
    /// ignoring `system:hidden`/`can_see`, hidden content — so it requires a
    /// valid token, though any authenticated account (not Builder) suffices.
    /// `ListHooks` (static engine vocabulary) stays public.
    #[test]
    fn world_reads_require_a_token_but_not_builder() {
        let (mut engine, _admin, _) = engine_with_api_token(&[Scope::Admin]);

        // A second, Player-only account (subsequent accounts get Player only)
        // with its own token.
        let player_id = engine.accounts.create("plainplayer", "password123").unwrap().id.clone();
        let player_token = "player-token".to_string();
        engine.api_tokens.insert(
            Engine::hash_token(&player_token),
            TokenInfo { account_id: player_id, label: "p".into(), persistent: false, expires_at: None },
        );

        let anon = engine.handle_api_request(ApiRequest::ListRooms, None);
        assert!(!anon.ok, "world enumeration must not be anonymous");
        assert_eq!(anon.error.as_deref(), Some("Authentication required"));

        let as_player =
            engine.handle_api_request(ApiRequest::ListRooms, Some(player_token));
        assert!(as_player.ok, "a plain Player token must suffice to read: {:?}", as_player.error);

        let hooks = engine.handle_api_request(ApiRequest::ListHooks, None);
        assert!(hooks.ok, "ListHooks is static schema and must stay public");
    }

    /// RBAC audit H1: a `system:global` object's `cmd_*` hooks run for every
    /// player, so a non-admin Builder must not rewrite one or promote an object
    /// into one — but ordinary objects stay Builder-editable (the guard is
    /// narrow; content editing is unaffected).
    #[test]
    fn only_admins_can_author_the_system_global_surface() {
        let (mut engine, admin, _) = engine_with_api_token(&[Scope::Admin]);

        let builder_id =
            engine.accounts.create("plainbuilder", "password123").unwrap().id.clone();
        engine.accounts.grant_scope(&builder_id, Scope::Builder);
        let builder_token = "builder-token".to_string();
        engine.api_tokens.insert(
            Engine::hash_token(&builder_token),
            TokenInfo { account_id: builder_id, label: "b".into(), persistent: false, expires_at: None },
        );

        // A global "rules" object and an ordinary object.
        let global_ref = engine.world.next_dbref();
        let mut global = GameObject::new(&global_ref, "rules", Kind::Item);
        global.tags.insert(Tag { category: "system".into(), key: "global".into() });
        engine.world.add_object(global);

        let plain_ref = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&plain_ref, "widget", Kind::Item));

        // Builder cannot rewrite the global object...
        let denied = engine.handle_api_request(
            ApiRequest::SetAttribute { ref_id: global_ref.clone(), key: "pwned".into(), value: serde_json::json!(true) },
            Some(builder_token.clone()),
        );
        assert_eq!(denied.error.as_deref(), Some("system:global objects are admin-only to edit"));

        // ...but the admin can.
        let allowed = engine.handle_api_request(
            ApiRequest::SetAttribute { ref_id: global_ref, key: "ok".into(), value: serde_json::json!(1) },
            Some(admin),
        );
        assert!(allowed.ok, "admin must still author the global object: {:?}", allowed.error);

        // Builder cannot promote a plain object into the global surface...
        let escalate = engine.handle_api_request(
            ApiRequest::AddTag { ref_id: plain_ref.clone(), tag: "system:global".into() },
            Some(builder_token.clone()),
        );
        assert_eq!(escalate.error.as_deref(), Some("Only admins can set or remove the system:global tag"));

        // ...but editing an ordinary object still works.
        let normal = engine.handle_api_request(
            ApiRequest::SetAttribute { ref_id: plain_ref, key: "color".into(), value: serde_json::json!("red") },
            Some(builder_token),
        );
        assert!(normal.ok, "builder editing a non-global object must still work: {:?}", normal.error);
    }

    /// The `run_command_as` security gate: `@`-commands (all authoring/admin
    /// verbs) and quit are refused on the forced path; ordinary gameplay verbs
    /// pass and run under the target's own scopes.
    #[test]
    fn forced_commands_ban_at_prefixed_and_quit() {
        assert!(Engine::forced_command_allowed("drop sword"));
        assert!(Engine::forced_command_allowed("say hello there"));
        assert!(Engine::forced_command_allowed("go north"));
        assert!(Engine::forced_command_allowed("attack goblin"));
        // Authoring/admin verbs are all @-prefixed — never forceable.
        assert!(!Engine::forced_command_allowed("@program #5"));
        assert!(!Engine::forced_command_allowed("   @grant admin bob"));
        assert!(!Engine::forced_command_allowed("@eval destroy('#1')"));
        // Forced disconnect is banned too.
        assert!(!Engine::forced_command_allowed("quit"));
        assert!(!Engine::forced_command_allowed("Q"));
    }

    /// REST parity for the new intents: clone + lock are Builder-gated and honor
    /// the locked guard; RunCommandAs is Admin-gated and honors the forced gate.
    #[test]
    fn rest_clone_lock_and_force_gating() {
        let (mut engine, admin, _) = engine_with_api_token(&[Scope::Admin]);

        // A Builder (non-admin) token.
        let builder_id = engine.accounts.create("restbuilder", "password123").unwrap().id.clone();
        engine.accounts.grant_scope(&builder_id, Scope::Builder);
        let builder = "rest-builder-token".to_string();
        engine.api_tokens.insert(
            Engine::hash_token(&builder),
            TokenInfo { account_id: builder_id, label: "b".into(), persistent: false, expires_at: None },
        );

        let src = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&src, "widget", Kind::Item));

        // CloneObject: a Builder can clone; the copy exists.
        let resp = engine.handle_api_request(
            ApiRequest::CloneObject { source: src.clone(), location: None, owner: None },
            Some(builder.clone()),
        );
        assert!(resp.ok, "builder clone: {:?}", resp.error);
        let new_ref = resp.data.unwrap()["ref_id"].as_str().unwrap().to_string();
        assert!(engine.world.get(&new_ref).is_some());

        // SetLock / ClearLock: Builder-gated round trip.
        assert!(engine
            .handle_api_request(
                ApiRequest::SetLock { ref_id: src.clone(), hook: "get".into(), expr: "perm(admin)".into() },
                Some(builder.clone()),
            )
            .ok);
        assert!(engine.world.get(&src).unwrap().locks.contains_key("get"));
        assert!(engine
            .handle_api_request(
                ApiRequest::ClearLock { ref_id: src.clone(), hook: "get".into() },
                Some(builder.clone()),
            )
            .ok);
        assert!(!engine.world.get(&src).unwrap().locks.contains_key("get"));

        // Lock the source: cloning it and locking it are both refused.
        engine
            .world
            .get_mut(&src)
            .unwrap()
            .tags
            .insert(Tag { category: "system".into(), key: "locked".into() });
        assert!(!engine
            .handle_api_request(
                ApiRequest::CloneObject { source: src.clone(), location: None, owner: None },
                Some(builder.clone()),
            )
            .ok, "a locked source must not be cloneable");
        assert!(!engine
            .handle_api_request(
                ApiRequest::SetLock { ref_id: src.clone(), hook: "get".into(), expr: "perm(admin)".into() },
                Some(builder.clone()),
            )
            .ok, "a locked object refuses SetLock");

        // RunCommandAs is Admin-only.
        let denied = engine.handle_api_request(
            ApiRequest::RunCommandAs { ref_id: src.clone(), command: "look".into() },
            Some(builder),
        );
        assert_eq!(denied.error.as_deref(), Some("Admin scope required"));

        // Even for an admin, the forced gate refuses @-commands.
        let gated = engine.handle_api_request(
            ApiRequest::RunCommandAs { ref_id: src, command: "@grant admin bob".into() },
            Some(admin),
        );
        assert!(!gated.ok);
        assert!(gated.error.as_deref().unwrap().contains("cannot be forced"));
    }

    /// Build an engine with a fast game clock (1 game-hour per tick) plus a
    /// `system:global` object whose rollover hooks record what happened.
    fn clock_engine() -> (Engine, String) {
        let config = Config {
            clock: Some(crate::clock::ClockConfig {
                minutes_per_tick: 60.0,
                ..crate::clock::ClockConfig::default()
            }),
            ..Config::default()
        };
        let db = crate::db::Database::open(Path::new(":memory:")).unwrap();
        let (_tx, rx) = mpsc::unbounded_channel();
        let mut engine = Engine::new(rx, db, &config);
        let gref = engine.world.next_dbref();
        let mut g = GameObject::new(&gref, "worldclock", Kind::Item);
        g.tags.insert(Tag { category: "system".into(), key: "global".into() });
        crate::softcode::hooks::set_script(
            &mut g,
            "function on_hour(this, actor, room)\n\
               local t = get_time()\n\
               set_attr(this, \"hour\", t.hour)\n\
               set_attr(this, \"hours_fired\", (get_attr(this, \"hours_fired\") or 0) + 1)\n\
             end\n\
             function on_dawn(this)\n set_attr(this, \"dawned\", true)\n end\n\
             function on_dusk(this)\n set_attr(this, \"dusked\", true)\n end\n\
             function on_day(this)\n set_attr(this, \"days\", (get_attr(this, \"days\") or 0) + 1)\n end\n"
                .to_string(),
        );
        engine.world.add_object(g);
        (engine, gref)
    }

    #[test]
    fn game_clock_advances_and_fires_rollovers_with_get_time() {
        let (mut engine, gref) = clock_engine();
        // Six ticks → 06:00. on_hour fired each tick; dawn (hour 6) fired once.
        for _ in 0..6 {
            engine.do_tick();
        }
        let g = engine.world.get(&gref).unwrap();
        assert_eq!(g.attrs.get("hour").and_then(|v| v.as_i64()), Some(6), "get_time().hour inside on_hour");
        assert_eq!(g.attrs.get("hours_fired").and_then(|v| v.as_i64()), Some(6));
        assert_eq!(g.attrs.get("dawned").and_then(|v| v.as_bool()), Some(true), "on_dawn fired at hour 6");
        assert!(g.attrs.get("days").is_none(), "no day rollover yet");

        // Advance to hour 24 → day rolls to day 2 (on_day once), dusk (20) fired.
        for _ in 6..24 {
            engine.do_tick();
        }
        let g = engine.world.get(&gref).unwrap();
        assert_eq!(g.attrs.get("days").and_then(|v| v.as_i64()), Some(1), "on_day fired once at the day boundary");
        assert_eq!(g.attrs.get("dusked").and_then(|v| v.as_bool()), Some(true), "on_dusk fired at hour 20");
    }

    #[test]
    fn get_time_is_nil_without_a_clock() {
        // Default config has no [clock]; a global on_tick sees get_time() == nil.
        let db = crate::db::Database::open(Path::new(":memory:")).unwrap();
        let (_tx, rx) = mpsc::unbounded_channel();
        let mut engine = Engine::new(rx, db, &Config::default());
        let gref = engine.world.next_dbref();
        let mut g = GameObject::new(&gref, "probe", Kind::Item);
        g.tags.insert(Tag { category: "system".into(), key: "global".into() });
        crate::softcode::hooks::set_script(
            &mut g,
            "function on_tick(this, state, room)\n set_attr(this, \"nilclock\", get_time() == nil)\n end\n".to_string(),
        );
        engine.world.add_object(g);
        engine.do_tick();
        let g = engine.world.get(&gref).unwrap();
        assert_eq!(g.attrs.get("nilclock").and_then(|v| v.as_bool()), Some(true));
    }

    /// `run_command_as` drives NPCs through the session-less dispatch: a forced
    /// gameplay command actually executes against the NPC's body.
    #[test]
    fn forcing_an_npc_runs_gameplay_and_hooks() {
        let (mut engine, admin, _) = engine_with_api_token(&[Scope::Admin]);

        // Two rooms joined by a north exit, and an NPC standing in room A whose
        // room defines a `cmd_wave` hook that marks the actor.
        let a = engine.world.next_dbref();
        let mut room_a = GameObject::new(&a, "rooma", Kind::Room);
        crate::softcode::hooks::set_script(
            &mut room_a,
            "function cmd_wave(this, actor, room, args)\n  set_attr(actor, \"waved\", true)\nend\n"
                .to_string(),
        );
        engine.world.add_object(room_a);
        let b = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&b, "roomb", Kind::Room));
        let exit = engine.world.next_dbref();
        engine.world.add_object(
            GameObject::new(&exit, "north", Kind::Exit).with_location(&a).with_target(&b),
        );
        let npc = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&npc, "goblin", Kind::Npc).with_location(&a));

        // Force the NPC to run a game cmd_ hook (on the room, since dispatch
        // excludes the actor's own hooks) — it runs with the NPC as actor.
        let waved = engine.handle_api_request(
            ApiRequest::RunCommandAs { ref_id: npc.clone(), command: "wave".into() },
            Some(admin.clone()),
        );
        assert!(waved.ok, "{:?}", waved.error);
        assert_eq!(
            engine.world.get(&npc).unwrap().attrs.get("waved"),
            Some(&serde_json::json!(true)),
            "the room's cmd_wave should have fired with the NPC as actor"
        );

        // Force the NPC to walk north — its location actually changes.
        let moved = engine.handle_api_request(
            ApiRequest::RunCommandAs { ref_id: npc.clone(), command: "go north".into() },
            Some(admin.clone()),
        );
        assert!(moved.ok, "{:?}", moved.error);
        assert_eq!(
            engine.world.get(&npc).unwrap().location_ref.as_deref(),
            Some(b.as_str()),
            "the forced NPC should have moved to room B"
        );

        // The `@`-gate still holds for NPC targets.
        let gated = engine.handle_api_request(
            ApiRequest::RunCommandAs { ref_id: npc.clone(), command: "@grant admin bob".into() },
            Some(admin.clone()),
        );
        assert!(!gated.ok);
        assert!(gated.error.as_deref().unwrap().contains("cannot be forced"));

        // A non-player/non-npc target (an item) is still refused.
        let item = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&item, "rock", Kind::Item));
        let bad = engine.handle_api_request(
            ApiRequest::RunCommandAs { ref_id: item, command: "go north".into() },
            Some(admin),
        );
        assert!(!bad.ok);
        assert_eq!(bad.error.as_deref(), Some("Target is not a player or NPC"));
    }

    /// A coordinate exit (`_dest_x`/`_dest_y` on the exit) stamps the arrival
    /// cell onto the actor as `_x`/`_y` when it is traversed — the room→map-cell
    /// entry for the one-room grid/wilderness model. Followers land there too.
    #[test]
    fn coordinate_exit_sets_actor_grid_position_on_traverse() {
        let (mut engine, _admin, _) = engine_with_api_token(&[Scope::Admin]);

        // Room A → wilderness room B via a north exit carrying arrival coords.
        let a = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&a, "rooma", Kind::Room));
        let b = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&b, "wilds", Kind::Room));
        let exit = engine.world.next_dbref();
        let mut exit_obj =
            GameObject::new(&exit, "north", Kind::Exit).with_location(&a).with_target(&b);
        exit_obj.attrs.insert("_dest_x".into(), serde_json::json!(5));
        exit_obj.attrs.insert("_dest_y".into(), serde_json::json!(12));
        engine.world.add_object(exit_obj);

        // A player in A, plus a troupe follower tagged to them.
        let pc = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&pc, "hero", Kind::Player).with_location(&a));
        let hench = engine.world.next_dbref();
        let mut hench_obj = GameObject::new(&hench, "squire", Kind::Npc).with_location(&a);
        hench_obj.tags.insert(crate::world::Tag {
            category: "troupe".into(),
            key: pc.clone(),
        });
        engine.world.add_object(hench_obj);

        engine.do_move(&pc, &exit, &b);

        let hero = engine.world.get(&pc).unwrap();
        assert_eq!(hero.location_ref.as_deref(), Some(b.as_str()));
        assert_eq!(hero.attrs.get("_x"), Some(&serde_json::json!(5)));
        assert_eq!(hero.attrs.get("_y"), Some(&serde_json::json!(12)));

        let squire = engine.world.get(&hench).unwrap();
        assert_eq!(squire.location_ref.as_deref(), Some(b.as_str()));
        assert_eq!(squire.attrs.get("_x"), Some(&serde_json::json!(5)),
            "a troupe follower should land on the same cell");
        assert_eq!(squire.attrs.get("_y"), Some(&serde_json::json!(12)));

        // A plain exit (no dest coords) leaves position untouched.
        let c = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&c, "roomc", Kind::Room));
        let plain = engine.world.next_dbref();
        engine.world.add_object(
            GameObject::new(&plain, "south", Kind::Exit).with_location(&b).with_target(&c),
        );
        engine.do_move(&pc, &plain, &c);
        let hero = engine.world.get(&pc).unwrap();
        assert_eq!(hero.attrs.get("_x"), Some(&serde_json::json!(5)),
            "a plain exit must not disturb grid position");
    }

    /// `can_traverse` belongs to the **exit**, pairing with the exit's
    /// `traverse` lock; `can_enter` belongs to the destination room. Both were
    /// once fired on the destination room with identical arguments, which made
    /// the two hooks indistinguishable and meant an exit's own `can_traverse`
    /// — the documented contract — silently never ran.
    #[test]
    fn can_traverse_fires_on_the_exit_not_the_destination_room() {
        let (mut engine, _admin, _) = engine_with_api_token(&[Scope::Admin]);

        let new_room = |engine: &mut Engine, key: &str| {
            let r = engine.world.next_dbref();
            engine.world.add_object(GameObject::new(&r, key, Kind::Room));
            r
        };
        let a = new_room(&mut engine, "rooma");
        let b = new_room(&mut engine, "roomb");
        let c = new_room(&mut engine, "roomc");
        let d = new_room(&mut engine, "roomd");
        let e = new_room(&mut engine, "roome");

        let pc = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&pc, "hero", Kind::Player).with_location(&a));

        // 1. A can_traverse hook on the EXIT runs, and `this` is that exit.
        let exit_ab = engine.world.next_dbref();
        let mut ab = GameObject::new(&exit_ab, "north", Kind::Exit)
            .with_location(&a)
            .with_target(&b);
        hooks::set_script(
            &mut ab,
            r#"function can_traverse(this, actor, room)
                 set_attr(actor, "saw_this", this.ref_id)
                 return true
               end"#
                .to_string(),
        );
        engine.world.add_object(ab);

        engine.do_move(&pc, &exit_ab, &b);
        let hero = engine.world.get(&pc).unwrap();
        assert_eq!(
            hero.location_ref.as_deref(),
            Some(b.as_str()),
            "an allowing can_traverse must not block the move"
        );
        assert_eq!(
            hero.attrs.get("saw_this"),
            Some(&serde_json::json!(exit_ab)),
            "can_traverse must fire on the exit, with `this` bound to the exit"
        );

        // 2. A can_traverse hook on the exit can veto.
        let exit_bc = engine.world.next_dbref();
        let mut bc = GameObject::new(&exit_bc, "north", Kind::Exit)
            .with_location(&b)
            .with_target(&c);
        hooks::set_script(
            &mut bc,
            "function can_traverse(this, actor, room) return false end".to_string(),
        );
        engine.world.add_object(bc);

        engine.do_move(&pc, &exit_bc, &c);
        assert_eq!(
            engine.world.get(&pc).unwrap().location_ref.as_deref(),
            Some(b.as_str()),
            "can_traverse returning false must block the move"
        );

        // 3. can_traverse on the destination ROOM is NOT a movement gate — that
        //    is can_enter's job. Under the old wiring this denied the move.
        let mut c_obj = engine.world.get(&c).unwrap().clone();
        hooks::set_script(
            &mut c_obj,
            "function can_traverse(this, actor, room) return false end".to_string(),
        );
        engine.world.add_object(c_obj);

        let exit_bc2 = engine.world.next_dbref();
        engine.world.add_object(
            GameObject::new(&exit_bc2, "east", Kind::Exit).with_location(&b).with_target(&c),
        );
        engine.do_move(&pc, &exit_bc2, &c);
        assert_eq!(
            engine.world.get(&pc).unwrap().location_ref.as_deref(),
            Some(c.as_str()),
            "a room's can_traverse must not gate movement into it"
        );

        // 4. can_enter on the destination room still gates it.
        let mut d_obj = engine.world.get(&d).unwrap().clone();
        hooks::set_script(
            &mut d_obj,
            "function can_enter(this, actor, room) return false end".to_string(),
        );
        engine.world.add_object(d_obj);

        let exit_cd = engine.world.next_dbref();
        engine.world.add_object(
            GameObject::new(&exit_cd, "north", Kind::Exit).with_location(&c).with_target(&d),
        );
        engine.do_move(&pc, &exit_cd, &d);
        assert_eq!(
            engine.world.get(&pc).unwrap().location_ref.as_deref(),
            Some(c.as_str()),
            "can_enter on the destination room must still block the move"
        );

        // ...and an unguarded room still admits.
        let exit_ce = engine.world.next_dbref();
        engine.world.add_object(
            GameObject::new(&exit_ce, "south", Kind::Exit).with_location(&c).with_target(&e),
        );
        engine.do_move(&pc, &exit_ce, &e);
        assert_eq!(
            engine.world.get(&pc).unwrap().location_ref.as_deref(),
            Some(e.as_str()),
            "an unguarded exit and room must allow the move"
        );
    }

    // -- GMCP Terrain.Legend --

    #[test]
    fn terrain_legend_assigns_stable_env_ids_and_colors() {
        use crate::map_template::{MapHeader, MapTemplateFile, TerrainDef, TileRotation};
        let mk = |color: Option<&str>| TerrainDef {
            theme: "plains".into(),
            title_prefix: None,
            passable: true,
            color: color.map(String::from),
            tile_image: None,
            tile_rotation: TileRotation::default(),
            archetype: None,
            attrs: HashMap::new(),
        };
        let mut terrain = HashMap::new();
        terrain.insert("a".to_string(), mk(Some("#3a6a2e")));
        terrain.insert("b".to_string(), mk(None)); // no color → neutral gray
        let tmpl = MapTemplateFile {
            map: MapHeader { name: "iron_hills".into(), grid: "ab".into() },
            terrain,
            cells: HashMap::new(),
        };

        let legend = Engine::terrain_legend("iron_hills", &tmpl);
        assert_eq!(legend["map"], "iron_hills");
        // env_ids are 1000 + sorted-index, stable per char.
        assert_eq!(legend["terrains"]["a"]["env_id"], 1000);
        assert_eq!(legend["terrains"]["a"]["color"], "#3a6a2e");
        assert_eq!(legend["terrains"]["a"]["passable"], true);
        assert_eq!(legend["terrains"]["b"]["env_id"], 1001);
        assert_eq!(legend["terrains"]["b"]["color"], "#808080");
        // Deterministic across calls.
        assert_eq!(legend, Engine::terrain_legend("iron_hills", &tmpl));
    }

    #[test]
    fn terrain_legend_rides_map_entry_once_per_session() {
        use crate::map_template::{MapHeader, MapTemplateFile, TerrainDef, TileRotation};
        let db = crate::db::Database::open(Path::new(":memory:")).unwrap();
        let config = Config::default();
        let (_tx, rx) = mpsc::unbounded_channel();
        let mut engine = Engine::new(rx, db, &config);

        let mk = |color: &str| TerrainDef {
            theme: "plains".into(),
            title_prefix: None,
            passable: true,
            color: Some(color.into()),
            tile_image: None,
            tile_rotation: TileRotation::default(),
            archetype: None,
            attrs: HashMap::new(),
        };
        let mut terrain = HashMap::new();
        terrain.insert("a".to_string(), mk("#3a6a2e"));
        terrain.insert("b".to_string(), mk("#8a8a8a"));
        engine.map_templates.insert(
            "iron_hills".into(),
            MapTemplateFile {
                map: MapHeader { name: "iron_hills".into(), grid: "ab".into() },
                terrain,
                cells: HashMap::new(),
            },
        );

        // Two rooms on that map, and a player standing in the first.
        let r1 = engine.world.next_dbref();
        let mut room1 = GameObject::new(&r1, "hill1", Kind::Room).with_title("Hill One");
        room1.attrs.insert("map_name".into(), serde_json::json!("iron_hills"));
        room1.attrs.insert("terrain".into(), serde_json::json!("a"));
        engine.world.add_object(room1);
        let r2 = engine.world.next_dbref();
        let mut room2 = GameObject::new(&r2, "hill2", Kind::Room).with_title("Hill Two");
        room2.attrs.insert("map_name".into(), serde_json::json!("iron_hills"));
        room2.attrs.insert("terrain".into(), serde_json::json!("b"));
        engine.world.add_object(room2);

        let actor = engine.world.next_dbref();
        engine
            .world
            .add_object(GameObject::new(&actor, "p", Kind::Player).with_location(&r1));

        let (client_tx, mut client_rx) = mpsc::unbounded_channel();
        let sid = "legend-test".to_string();
        engine.sessions.insert(
            sid.clone(),
            Session {
                tx: client_tx,
                state: SessionState::Playing {
                    actor_ref: actor.clone(),
                    account_id: "acct".into(),
                    puppet_ref: None,
                },
                editor: None,
            },
        );

        // First mapped room → the legend rides along.
        engine.send_room_data(&sid, &actor);
        let mut first = Vec::new();
        while let Ok(m) = client_rx.try_recv() {
            first.push(m);
        }
        let legend = first
            .iter()
            .find_map(|m| match m {
                ClientMessage::Game { channel, data } if channel == "Terrain.Legend" => {
                    Some(data.clone())
                }
                _ => None,
            })
            .expect("entering a mapped room should send Terrain.Legend");
        assert_eq!(legend["map"], "iron_hills");
        assert_eq!(legend["terrains"]["a"]["color"], "#3a6a2e");

        // Move to another room on the SAME map → no re-send (deduped).
        if let Some(o) = engine.world.get_mut(&actor) {
            o.location_ref = Some(r2.clone());
        }
        engine.send_room_data(&sid, &actor);
        let mut second = Vec::new();
        while let Ok(m) = client_rx.try_recv() {
            second.push(m);
        }
        assert!(
            !second.iter().any(|m| matches!(
                m,
                ClientMessage::Game { channel, .. } if channel == "Terrain.Legend"
            )),
            "same-map move must not re-send the legend"
        );
    }

    /// The improved `trigger()`: `data` reaches the hook as its 4th argument (a
    /// real table, not the old `_trigger_data` attr), and the optional 4th arg
    /// fires the hook *as* a chosen actor rather than the ambient one.
    #[test]
    fn trigger_delivers_data_as_arg_and_honors_actor_override() {
        let (mut engine, _t, _) = engine_with_api_token(&[Scope::Builder]);

        // Target: on_ping records the payload amount + who acted.
        let a = engine.world.next_dbref();
        let mut oa = GameObject::new(&a, "target", Kind::Item);
        crate::softcode::hooks::set_script(
            &mut oa,
            "function on_ping(this, actor, room, data)\n\
               set_attr(this, \"saw_amount\", data.amount)\n\
               set_attr(this, \"saw_actor\", actor and actor.ref_id or \"none\")\n\
             end"
            .to_string(),
        );
        engine.world.add_object(oa);

        // Two distinct actors: the ambient one that fires the driver, and the
        // override the trigger names.
        let ambient = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&ambient, "ambient", Kind::Player));
        let override_actor = engine.world.next_dbref();
        engine.world.add_object(GameObject::new(&override_actor, "chosen", Kind::Player));

        // Driver: on_use triggers on_ping on the target with data + the override.
        let b = engine.world.next_dbref();
        let mut ob = GameObject::new(&b, "wand", Kind::Item);
        crate::softcode::hooks::set_script(
            &mut ob,
            format!(
                "function on_use(this, actor, room)\n  trigger(\"{}\", \"on_ping\", {{ amount = 7 }}, \"{}\")\nend",
                a, override_actor
            ),
        );
        engine.world.add_object(ob);

        // Fire the driver as the AMBIENT actor; the trigger should still land as
        // the override.
        engine.fire_hook(&b, "on_use", &ambient, None, None).unwrap();

        let t = engine.world.get(&a).unwrap();
        assert_eq!(
            t.attrs.get("saw_amount"),
            Some(&serde_json::json!(7)),
            "data reached the hook as its 4th arg (not _trigger_data)"
        );
        assert_eq!(
            t.attrs.get("saw_actor").and_then(|v| v.as_str()),
            Some(override_actor.as_str()),
            "the trigger fired as the override actor, not the ambient one"
        );
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

    // -- system:locked (file-authoritative object) --

    /// Create an item and stamp `system:locked` on it. The `AddTag` succeeds
    /// because the object is not yet locked when the tag is added; every edit
    /// after that is refused.
    async fn create_locked_item(tx: &mpsc::UnboundedSender<EngineMessage>) -> String {
        let resp = api_call(tx, ApiRequest::CreateObject {
            area: "test".into(),
            key: "relic".into(),
            kind: "item".into(),
            title: Some("a relic".into()),
            description: None,
            location: Some("#1".into()),
        }).await;
        let ref_id = resp.data.unwrap()["ref_id"].as_str().unwrap().to_string();
        let resp = api_call(tx, ApiRequest::AddTag {
            ref_id: ref_id.clone(),
            tag: "system:locked".into(),
        }).await;
        assert!(resp.ok, "locking a not-yet-locked object should succeed");
        ref_id
    }

    #[tokio::test]
    async fn locked_object_refuses_authoring_edits() {
        let (tx, handle) = test_engine().await;
        let locked = create_locked_item(&tx).await;

        let edits = [
            ApiRequest::SetScript { ref_id: locked.clone(), source: "function on_get() end".into(), base_version: None },
            ApiRequest::SetTitle { ref_id: locked.clone(), title: "hax".into() },
            ApiRequest::SetDescription { ref_id: locked.clone(), description: "hax".into() },
            ApiRequest::SetAttribute { ref_id: locked.clone(), key: "hp".into(), value: serde_json::json!(10) },
            ApiRequest::AddTag { ref_id: locked.clone(), tag: "foo:bar".into() },
            // Self-protecting: cannot strip system:locked via the builder.
            ApiRequest::RemoveTag { ref_id: locked.clone(), tag: "system:locked".into() },
            ApiRequest::SetArchetype { ref_id: locked.clone(), archetype_ref: None },
            ApiRequest::DeleteObject { ref_id: locked.clone(), cascade: false },
        ];
        for req in edits {
            let resp = api_call(&tx, req).await;
            assert!(!resp.ok, "a locked object must refuse the edit");
            assert!(
                resp.error.as_deref().unwrap_or("").contains("locked"),
                "expected a locked error, got {:?}",
                resp.error
            );
        }

        // Examine reports the locked state, and self-protection held.
        let resp = api_call(&tx, ApiRequest::Examine { ref_id: locked.clone() }).await;
        let data = resp.data.unwrap();
        assert_eq!(data["locked"], true);
        assert!(
            data["tags"].as_array().unwrap().iter().any(|t| t == "system:locked"),
            "system:locked survives the refused RemoveTag"
        );

        // A non-locked object still allows the same edits.
        let free = create_test_item(&tx).await;
        let resp = api_call(&tx, ApiRequest::SetTitle { ref_id: free.clone(), title: "renamed".into() }).await;
        assert!(resp.ok, "a non-locked object still accepts edits");

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn locked_object_still_accepts_runtime_state() {
        let (tx, handle) = test_engine().await;
        let locked = create_locked_item(&tx).await;

        // A softcode hook's `set_attr` on a locked object still applies —
        // runtime state is never blocked, only the authoring surface.
        let resp = api_call(&tx, ApiRequest::Eval {
            source: format!("set_attr(\"{}\", \"hp\", 42)", locked),
        }).await;
        assert!(resp.ok, "runtime set_attr on a locked object must apply: {:?}", resp.error);

        let resp = api_call(&tx, ApiRequest::Examine { ref_id: locked.clone() }).await;
        assert_eq!(resp.data.unwrap()["attrs"]["hp"], 42);

        drop(tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn locked_is_own_tag_not_inherited() {
        let (tx, handle) = test_engine().await;

        // A locked base archetype.
        let base = api_call(&tx, ApiRequest::CreateObject {
            area: "test".into(),
            key: "goblin".into(),
            kind: "npc".into(),
            title: Some("Goblin".into()),
            description: None,
            location: Some("#1".into()),
        }).await.data.unwrap()["ref_id"].as_str().unwrap().to_string();
        assert!(api_call(&tx, ApiRequest::AddTag { ref_id: base.clone(), tag: "system:locked".into() }).await.ok);

        // An instance delegating to it — not itself locked.
        let inst = api_call(&tx, ApiRequest::CreateObject {
            area: "test".into(),
            key: "grunt".into(),
            kind: "npc".into(),
            title: None,
            description: None,
            location: Some("#1".into()),
        }).await.data.unwrap()["ref_id"].as_str().unwrap().to_string();
        assert!(api_call(&tx, ApiRequest::SetArchetype {
            ref_id: inst.clone(),
            archetype_ref: Some(base.clone()),
        }).await.ok, "reparenting a non-locked instance is allowed");

        // The instance is NOT locked even though its archetype is (own tag only).
        let resp = api_call(&tx, ApiRequest::Examine { ref_id: inst.clone() }).await;
        assert_eq!(resp.data.unwrap()["locked"], false);

        // ...and the instance still accepts authoring edits.
        let resp = api_call(&tx, ApiRequest::SetTitle { ref_id: inst.clone(), title: "Grunt".into() }).await;
        assert!(resp.ok, "a non-locked instance of a locked archetype is still editable");

        drop(tx);
        let _ = handle.await;
    }

    /// A cascade delete flattens each instance (rewriting its definition), so it
    /// must refuse when any instance is locked — otherwise a locked definition
    /// would be mutated indirectly, behind the target-only guard.
    #[tokio::test]
    async fn cascade_delete_refuses_when_an_instance_is_locked() {
        let (tx, handle) = test_engine().await;

        // An UNLOCKED archetype with a LOCKED instance delegating to it.
        let base = api_call(&tx, ApiRequest::CreateObject {
            area: "test".into(), key: "goblin".into(), kind: "npc".into(),
            title: Some("Goblin".into()), description: None, location: Some("#1".into()),
        }).await.data.unwrap()["ref_id"].as_str().unwrap().to_string();
        let inst = api_call(&tx, ApiRequest::CreateObject {
            area: "test".into(), key: "grunt".into(), kind: "npc".into(),
            title: None, description: None, location: Some("#1".into()),
        }).await.data.unwrap()["ref_id"].as_str().unwrap().to_string();
        assert!(api_call(&tx, ApiRequest::SetArchetype {
            ref_id: inst.clone(), archetype_ref: Some(base.clone()),
        }).await.ok);
        assert!(api_call(&tx, ApiRequest::AddTag { ref_id: inst.clone(), tag: "system:locked".into() }).await.ok);

        // Cascading the delete would flatten the locked instance — refuse.
        let resp = api_call(&tx, ApiRequest::DeleteObject { ref_id: base.clone(), cascade: true }).await;
        assert!(!resp.ok, "cascade must not flatten a locked instance");

        // Both objects survive and the delegation is intact.
        assert!(api_call(&tx, ApiRequest::Examine { ref_id: base.clone() }).await.ok, "archetype survives");
        let inst_resp = api_call(&tx, ApiRequest::Examine { ref_id: inst.clone() }).await;
        assert_eq!(inst_resp.data.unwrap()["archetype_ref"].as_str(), Some(base.as_str()),
            "the locked instance still delegates — it was not flattened");

        drop(tx);
        let _ = handle.await;
    }

    /// `create_library` (the builder-facing, file-free `@lib`) creates a
    /// require()able module and refuses a duplicate name.
    #[tokio::test]
    async fn create_library_makes_a_module_and_refuses_duplicates() {
        let (tx, handle) = test_engine().await;

        let resp = api_call(&tx, ApiRequest::CreateLibrary { name: "myutils".into() }).await;
        assert!(resp.ok, "create_library succeeds");
        let ref_id = resp.data.unwrap()["ref_id"].as_str().unwrap().to_string();

        // The host object carries the lib module.
        let libs = api_call(&tx, ApiRequest::ListLibs { ref_id: ref_id.clone() }).await;
        let arr = libs.data.unwrap();
        assert!(
            arr.as_array().unwrap().iter().any(|m| m["name"] == "myutils"),
            "the created host carries the 'myutils' module"
        );

        // Creating the same name again is refused (edit it via set_lib instead).
        let dup = api_call(&tx, ApiRequest::CreateLibrary { name: "myutils".into() }).await;
        assert!(!dup.ok, "duplicate library name refused");

        drop(tx);
        let _ = handle.await;
    }
}

/// Normalize emitted text for the wire: interior newlines become CRLF, and the
/// message gets a trailing CRLF.
///
/// Softcode composes multi-line text all the time — `str.wrap` returns one
/// newline-joined string, and any table or box helper builds one. Without this
/// a bare LF went out mid-message and staircased on telnet, so every recipe had
/// to split the string and loop over `emit`. Splitting on `\n` after stripping
/// `\r` handles CRLF, bare LF, and mixed input identically.
fn wire_text(message: &str) -> String {
    let normalized = message.replace('\r', "");
    let mut out = normalized.replace('\n', "\r\n");
    out.push_str("\r\n");
    out
}
