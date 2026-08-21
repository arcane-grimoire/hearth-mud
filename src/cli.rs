//! `hearth eval` / `hearth program ...` — a synchronous CLI over the REST
//! API (`POST /api`), so softcode can be authored in a normal editor and
//! pushed to a running server without a restart or `@reload-world`.
//!
//! This is a thin HTTP client, not an alternate engine entry point: every
//! subcommand here sends one `ApiRequest` (see `engine::ApiRequest`) and
//! prints the result. It talks to a server that is already running — it
//! does not start one, and deliberately does not pull in a tokio runtime
//! (`main.rs` only builds one for the server path).
//!
//! Output is meant to be piped: `program get` writes exactly the program's
//! source to stdout with nothing else, `eval`'s output is the same text
//! `@eval` would show a telnet user, and every failure goes to stderr with
//! a non-zero exit code.

use std::io::Read;
use std::time::Duration;

use crate::config::Config;

const DEFAULT_ADDR: &str = "localhost:8000";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

const USAGE: &str = "\
Usage: hearth <subcommand> [args...] [--addr HOST:PORT | --config PATH] [--token TOKEN]

Subcommands:
  eval [FILE]                       Run a one-shot Luau script against the live world.
                                     Reads stdin when FILE is '-' or omitted.
  program get <ref>/<hook>          Print a program's current source to stdout.
  program set <ref>/<hook> [FILE]   Set a program's source. Reads stdin when FILE is '-' or omitted.
  program history <ref>/<hook>      List a program's version history.
  program restore <ref>/<hook> <n>  Restore version <n>'s source as a new version.
  import <path> [--dry-run]         Install a TOML+.luau bundle into the DB.
                                     <path> is resolved on the SERVER's filesystem.
  export <path>                     Write DB-owned content back to files.
                                     <path> is resolved on the SERVER's filesystem.

Connection (flags go after the subcommand, and may appear in any order):
  --addr HOST:PORT   Server address (default: localhost:8000)
  --config PATH      Read the address from a hearth.toml-style config's web_addr
  --token TOKEN      API token (default: $HEARTH_TOKEN)

Every subcommand here requires an API token with at least builder scope
(eval requires admin). Mint one in-game with `@token create <label>`
(telnet, or the web client's command bar), or from the web client's
Settings drawer under API Tokens -> Create. The token is shown once.";

const PROGRAM_USAGE: &str = "\
Usage: hearth program <get|set|history|restore> <ref>/<hook> [...] [--addr ...] [--token ...]

  hearth program get <ref>/<hook>
  hearth program set <ref>/<hook> [FILE]
  hearth program history <ref>/<hook>
  hearth program restore <ref>/<hook> <n>";

/// Whether `name` is a `hearth` CLI subcommand — used by `main.rs` to
/// decide between CLI dispatch and the existing config-path-and-run-server
/// behaviour. Kept to an explicit allowlist rather than "anything that
/// isn't a file" so back-compat is a property of this list, not of what
/// happens to exist on disk.
pub fn is_known_subcommand(name: &str) -> bool {
    matches!(name, "eval" | "program" | "import" | "export")
}

/// Entry point from `main.rs`. `args` is the full argv tail, e.g.
/// `["eval", "script.luau", "--token", "abc"]`. Returns the process exit
/// code: 0 on success, 2 on a usage error, 1 on anything else (auth
/// rejected, server unreachable, the eval/write itself failing).
pub fn run(args: &[String]) -> i32 {
    match args.first().map(|s| s.as_str()) {
        Some("eval") => cmd_eval(&args[1..]),
        Some("program") => cmd_program(&args[1..]),
        Some("import") => cmd_import(&args[1..]),
        Some("export") => cmd_export(&args[1..]),
        _ => {
            eprintln!("{}", USAGE);
            2
        }
    }
}

#[derive(Default)]
struct GlobalOpts {
    addr: Option<String>,
    config: Option<String>,
    token: Option<String>,
}

/// Pull `--addr`/`--config`/`--token` out of `args`, wherever they appear,
/// and return them alongside the remaining positional arguments in order.
/// Hand-rolled rather than a `clap` dependency: the surface here is three
/// flags and a couple of positional args per subcommand, and clap's
/// derive-based subcommand dispatch doesn't compose cleanly with `main.rs`
/// needing to fall through to "config path, run the server" for anything
/// that isn't one of these subcommands.
fn extract_global_opts(args: &[String]) -> (GlobalOpts, Vec<String>) {
    let mut opts = GlobalOpts::default();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--addr" => {
                opts.addr = args.get(i + 1).cloned();
                i += 2;
            }
            "--config" => {
                opts.config = args.get(i + 1).cloned();
                i += 2;
            }
            "--token" => {
                opts.token = args.get(i + 1).cloned();
                i += 2;
            }
            other => {
                rest.push(other.to_string());
                i += 1;
            }
        }
    }
    (opts, rest)
}

/// Split `<ref>/<hook>` on the *last* `/`, matching
/// `Engine::resolve_ref_hook_path` — a dbref can itself contain slashes
/// (`area/town/room/crossroads`), the hook name cannot.
fn split_ref_hook(path: &str) -> Result<(String, String), String> {
    match path.rsplit_once('/') {
        Some((r, h)) if !r.is_empty() && !h.is_empty() => Ok((r.to_string(), h.to_string())),
        _ => Err(format!("Expected <ref>/<hook>, got '{}'.", path)),
    }
}

fn read_source(file: Option<&str>) -> Result<String, String> {
    match file {
        None | Some("-") => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("Failed to read stdin: {}", e))?;
            Ok(buf)
        }
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read '{}': {}", path, e)),
    }
}

/// `web_addr` in `hearth.toml` (default `0.0.0.0:8000`) is a *bind*
/// address, not reachable as a connect address on most systems — swap an
/// unspecified host for `localhost`, keep the port.
fn connectable_addr(bind_addr: &str) -> String {
    match bind_addr.rsplit_once(':') {
        Some((host, port)) if host.is_empty() || host == "0.0.0.0" || host == "::" || host == "[::]" => {
            format!("localhost:{}", port)
        }
        _ => bind_addr.to_string(),
    }
}

fn resolve_connection(opts: &GlobalOpts) -> (String, Option<String>) {
    let addr = opts
        .addr
        .clone()
        .or_else(|| opts.config.as_deref().map(|p| connectable_addr(&Config::load(std::path::Path::new(p)).web_addr)))
        .unwrap_or_else(|| DEFAULT_ADDR.to_string());
    let base = if addr.contains("://") {
        addr.trim_end_matches('/').to_string()
    } else {
        format!("http://{}", addr)
    };
    let token = opts.token.clone().or_else(|| std::env::var("HEARTH_TOKEN").ok());
    (base, token)
}

fn require_token(token: &Option<String>) -> Result<String, String> {
    match token.as_deref() {
        Some(t) if !t.trim().is_empty() => Ok(t.to_string()),
        _ => Err("No API token provided. Pass --token <token>, or set the HEARTH_TOKEN \
             environment variable.\n\nMint one in-game with `@token create <label>` \
             (telnet, or the web client's command bar), or from the web client's \
             Settings drawer under API Tokens -> Create. The token is shown once."
            .to_string()),
    }
}

#[derive(serde::Deserialize)]
struct ApiResponseDto {
    ok: bool,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

/// POST `body` to `<base>/api` with the given bearer token and return the
/// parsed `{ok, data, error}` envelope. Every failure here is a *transport*
/// failure (unreachable server, malformed response) — the engine always
/// answers with HTTP 200 even for a rejected token or a failed write, so
/// those surface through the `ok`/`error` fields, not as a `ureq::Error`.
fn call_api(base: &str, token: &str, body: serde_json::Value) -> Result<ApiResponseDto, String> {
    let url = format!("{}/api", base);
    let result = ureq::post(&url)
        .timeout(REQUEST_TIMEOUT)
        .set("Authorization", &format!("Bearer {}", token))
        .send_json(body);

    match result {
        Ok(resp) => resp
            .into_json::<ApiResponseDto>()
            .map_err(|e| format!("Server at {} sent a response we couldn't parse: {}", url, e)),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(format!("Server at {} returned HTTP {}: {}", url, code, body.trim()))
        }
        Err(ureq::Error::Transport(t)) => Err(format!(
            "Could not reach a Hearth server at {}: {}\n\n\
             Is the server running, and is this the right address? Check \
             --addr/--config (default: {}).",
            url, t, DEFAULT_ADDR
        )),
    }
}

/// `call_api`, plus unwrapping the `{ok, data, error}` envelope into a
/// plain `Result` — an auth rejection gets a hint appended, since that is
/// the failure people will hit most and the raw server message alone
/// ("Authentication required" / "Admin scope required") doesn't say what to
/// do about it.
fn api_call(base: &str, token: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    let resp = call_api(base, token, body)?;
    if resp.ok {
        Ok(resp.data.unwrap_or(serde_json::Value::Null))
    } else {
        let msg = resp.error.unwrap_or_else(|| "Unknown error".to_string());
        if msg.contains("Authentication required") || msg.contains("scope required") {
            Err(format!(
                "Server rejected the request: {}\n\n\
                 Check that --token/HEARTH_TOKEN is a valid, unrevoked token \
                 with the right scope (`@scopes` in-game shows yours; `eval` \
                 needs admin, everything else needs builder).",
                msg
            ))
        } else {
            Err(msg)
        }
    }
}

fn cmd_eval(args: &[String]) -> i32 {
    let (opts, rest) = extract_global_opts(args);
    let source = match read_source(rest.first().map(|s| s.as_str())) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}", e);
            return 2;
        }
    };
    if source.trim().is_empty() {
        eprintln!("No source given (empty file/stdin).");
        return 2;
    }
    let (base, token) = resolve_connection(&opts);
    let token = match require_token(&token) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    match api_call(&base, &token, serde_json::json!({ "action": "eval", "source": source })) {
        Ok(data) => {
            let output = data.get("output").and_then(|v| v.as_str()).unwrap_or("");
            print!("{}", output);
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

/// `hearth import <path> [--dry-run]` — the primary dev-loop interface for
/// installing/upgrading a bundle without a restart or `@reload-world`; see
/// docs/plans/program-authoring.md Stage 4's "The dev loop: a CLI, not an
/// overwrite mode." `<path>` is resolved on the *server's* filesystem, not
/// the machine running this CLI — same as `hearth program set` pushes local
/// source to a remote object, `hearth import` here tells an already-running
/// server which of its own local paths to read. This only works when the
/// CLI and the server share a filesystem (the common case: running the CLI
/// against your own dev server), which is the dev loop this is for.
fn cmd_import(args: &[String]) -> i32 {
    let (opts, rest) = extract_global_opts(args);
    let mut path: Option<String> = None;
    let mut dry_run = false;
    for a in &rest {
        if a == "--dry-run" {
            dry_run = true;
        } else if path.is_none() {
            path = Some(a.clone());
        }
    }
    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("Usage: hearth import <path> [--dry-run]");
            return 2;
        }
    };
    let (base, token) = resolve_connection(&opts);
    let token = match require_token(&token) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    match api_call(
        &base,
        &token,
        serde_json::json!({ "action": "import", "path": path, "dry_run": dry_run }),
    ) {
        Ok(data) => {
            let output = data.get("output").and_then(|v| v.as_str()).unwrap_or("");
            print!("{}", output);
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

/// `hearth export <path>` — same server-side-path convention as `hearth
/// import`. See docs/plans/program-authoring.md Stage 4.
fn cmd_export(args: &[String]) -> i32 {
    let (opts, rest) = extract_global_opts(args);
    let path = match rest.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("Usage: hearth export <path>");
            return 2;
        }
    };
    let (base, token) = resolve_connection(&opts);
    let token = match require_token(&token) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    match api_call(&base, &token, serde_json::json!({ "action": "export", "path": path })) {
        Ok(data) => {
            let output = data.get("output").and_then(|v| v.as_str()).unwrap_or("");
            print!("{}", output);
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

fn cmd_program(args: &[String]) -> i32 {
    match args.first().map(|s| s.as_str()) {
        Some("get") => cmd_program_get(&args[1..]),
        Some("set") => cmd_program_set(&args[1..]),
        Some("history") => cmd_program_history(&args[1..]),
        Some("restore") => cmd_program_restore(&args[1..]),
        Some(other) => {
            eprintln!("Unknown 'hearth program' subcommand '{}'.\n\n{}", other, PROGRAM_USAGE);
            2
        }
        None => {
            eprintln!("{}", PROGRAM_USAGE);
            2
        }
    }
}

fn cmd_program_get(args: &[String]) -> i32 {
    let (opts, rest) = extract_global_opts(args);
    let path = match rest.first() {
        Some(p) => p,
        None => {
            eprintln!("Usage: hearth program get <ref>/<hook>");
            return 2;
        }
    };
    let (ref_id, hook) = match split_ref_hook(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 2;
        }
    };
    let (base, token) = resolve_connection(&opts);
    let token = match require_token(&token) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    // No single-program REST action exists — reuse `list_programs` (the
    // same call `hearth program set` would need to confirm a write, and
    // what `@programs` uses in-game) and pick out the one hook.
    match api_call(&base, &token, serde_json::json!({ "action": "list_programs", "ref_id": ref_id })) {
        Ok(data) => {
            let found = data
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| p.get("hook").and_then(|h| h.as_str()) == Some(hook.as_str()))
                .and_then(|p| p.get("source").and_then(|s| s.as_str()).map(|s| s.to_string()));
            match found {
                Some(source) => {
                    print!("{}", source);
                    0
                }
                None => {
                    eprintln!("{}/{} has no program.", ref_id, hook);
                    1
                }
            }
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

fn cmd_program_set(args: &[String]) -> i32 {
    let (opts, rest) = extract_global_opts(args);
    let path = match rest.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("Usage: hearth program set <ref>/<hook> [FILE]");
            return 2;
        }
    };
    let (ref_id, hook) = match split_ref_hook(&path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 2;
        }
    };
    let source = match read_source(rest.get(1).map(|s| s.as_str())) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}", e);
            return 2;
        }
    };
    let (base, token) = resolve_connection(&opts);
    let token = match require_token(&token) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    match api_call(
        &base,
        &token,
        serde_json::json!({ "action": "set_program", "ref_id": ref_id, "hook": hook, "source": source }),
    ) {
        Ok(_) => {
            println!("Set {}/{}.", ref_id, hook);
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

fn cmd_program_history(args: &[String]) -> i32 {
    let (opts, rest) = extract_global_opts(args);
    let path = match rest.first() {
        Some(p) => p,
        None => {
            eprintln!("Usage: hearth program history <ref>/<hook>");
            return 2;
        }
    };
    let (ref_id, hook) = match split_ref_hook(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 2;
        }
    };
    let (base, token) = resolve_connection(&opts);
    let token = match require_token(&token) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    match api_call(
        &base,
        &token,
        serde_json::json!({ "action": "program_history", "ref_id": ref_id, "hook": hook }),
    ) {
        Ok(data) => {
            let versions = data.as_array().cloned().unwrap_or_default();
            if versions.is_empty() {
                println!("No version history for {}/{}.", ref_id, hook);
                return 0;
            }
            println!("History for {}/{}:", ref_id, hook);
            for v in &versions {
                let n = v.get("version").and_then(|x| x.as_u64()).unwrap_or(0);
                let created_at = v.get("created_at").and_then(|x| x.as_i64()).unwrap_or(0);
                let author = v.get("author").and_then(|x| x.as_str()).unwrap_or("?");
                let deleted = v.get("deleted").and_then(|x| x.as_bool()).unwrap_or(false);
                println!(
                    "  {:>3}  {}  {}{}",
                    n,
                    crate::engine::format_epoch_secs(created_at),
                    author,
                    if deleted { "  (deleted)" } else { "" }
                );
            }
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

fn cmd_program_restore(args: &[String]) -> i32 {
    let (opts, rest) = extract_global_opts(args);
    if rest.len() < 2 {
        eprintln!("Usage: hearth program restore <ref>/<hook> <n>");
        return 2;
    }
    let (ref_id, hook) = match split_ref_hook(&rest[0]) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 2;
        }
    };
    let version: usize = match rest[1].parse() {
        Ok(v) if v > 0 => v,
        _ => {
            eprintln!("Version number must be a positive integer.");
            return 2;
        }
    };
    let (base, token) = resolve_connection(&opts);
    let token = match require_token(&token) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    match api_call(
        &base,
        &token,
        serde_json::json!({ "action": "program_restore", "ref_id": ref_id, "hook": hook, "version": version }),
    ) {
        Ok(data) => {
            if data.get("restored_deleted").and_then(|v| v.as_bool()).unwrap_or(false) {
                println!(
                    "Version {} of {}/{} was a deletion — restored as a new deletion.",
                    version, ref_id, hook
                );
            } else {
                println!("Restored version {} of {}/{} as a new version.", version, ref_id, hook);
            }
            0
        }
        Err(e) => {
            eprintln!("{}", e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_subcommands_are_exactly_eval_program_import_export() {
        assert!(is_known_subcommand("eval"));
        assert!(is_known_subcommand("program"));
        assert!(is_known_subcommand("import"));
        assert!(is_known_subcommand("export"));
        // A game config path must never accidentally match — this is the
        // property back-compat with `cargo run -- <config.toml>` rests on.
        assert!(!is_known_subcommand("../the-last-stag-mud/hearth.toml"));
        assert!(!is_known_subcommand("hearth.toml"));
        assert!(!is_known_subcommand(""));
        assert!(!is_known_subcommand("Eval"));
    }

    #[test]
    fn split_ref_hook_splits_on_last_slash() {
        assert_eq!(
            split_ref_hook("area/town/room/crossroads/on_look"),
            Ok(("area/town/room/crossroads".to_string(), "on_look".to_string()))
        );
        assert_eq!(split_ref_hook("#5/on_tick"), Ok(("#5".to_string(), "on_tick".to_string())));
    }

    #[test]
    fn split_ref_hook_rejects_missing_hook_or_ref() {
        assert!(split_ref_hook("no_slash_here").is_err());
        assert!(split_ref_hook("/on_tick").is_err());
        assert!(split_ref_hook("#5/").is_err());
    }

    #[test]
    fn extract_global_opts_pulls_flags_from_anywhere() {
        let args: Vec<String> = vec!["#5/on_tick", "--token", "abc", "--addr", "example.com:9000"]
            .into_iter()
            .map(String::from)
            .collect();
        let (opts, rest) = extract_global_opts(&args);
        assert_eq!(opts.token.as_deref(), Some("abc"));
        assert_eq!(opts.addr.as_deref(), Some("example.com:9000"));
        assert_eq!(rest, vec!["#5/on_tick".to_string()]);
    }

    #[test]
    fn extract_global_opts_preserves_positional_order() {
        let args: Vec<String> = vec!["--addr", "localhost:8000", "#5/on_tick", "script.luau"]
            .into_iter()
            .map(String::from)
            .collect();
        let (_, rest) = extract_global_opts(&args);
        assert_eq!(rest, vec!["#5/on_tick".to_string(), "script.luau".to_string()]);
    }

    #[test]
    fn connectable_addr_swaps_unspecified_host_for_localhost() {
        assert_eq!(connectable_addr("0.0.0.0:8000"), "localhost:8000");
        assert_eq!(connectable_addr(":8000"), "localhost:8000");
        assert_eq!(connectable_addr("example.com:8000"), "example.com:8000");
        assert_eq!(connectable_addr("192.168.1.5:8000"), "192.168.1.5:8000");
    }

    #[test]
    fn resolve_connection_defaults_to_localhost_8000() {
        let opts = GlobalOpts::default();
        let (base, token) = resolve_connection(&opts);
        assert_eq!(base, "http://localhost:8000");
        assert_eq!(token, std::env::var("HEARTH_TOKEN").ok());
    }

    #[test]
    fn resolve_connection_prefers_explicit_addr_and_token() {
        let opts = GlobalOpts {
            addr: Some("example.com:9000".to_string()),
            config: None,
            token: Some("mytoken".to_string()),
        };
        let (base, token) = resolve_connection(&opts);
        assert_eq!(base, "http://example.com:9000");
        assert_eq!(token.as_deref(), Some("mytoken"));
    }

    #[test]
    fn resolve_connection_leaves_a_full_url_alone() {
        let opts = GlobalOpts {
            addr: Some("https://hearth.example.com".to_string()),
            config: None,
            token: None,
        };
        let (base, _) = resolve_connection(&opts);
        assert_eq!(base, "https://hearth.example.com");
    }

    #[test]
    fn require_token_rejects_missing_or_blank() {
        assert!(require_token(&None).is_err());
        assert!(require_token(&Some(String::new())).is_err());
        assert!(require_token(&Some("   ".to_string())).is_err());
        assert!(require_token(&Some("abc".to_string())).is_ok());
    }
}
