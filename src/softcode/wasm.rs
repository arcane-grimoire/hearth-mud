//! Compute-only WASM plugins.
//!
//! A second sandboxed language alongside Luau, for pure data-in → data-out
//! work an author would rather write in Rust/AssemblyScript/etc. and ship as a
//! compiled `.wasm` binary. Plugins **never touch world state**: they take a
//! JSON payload and return a JSON payload. Luau stays the only thing that
//! emits `Intent`s, so the single-writer engine's invariants are untouched.
//!
//! Modules are CODE, not editable content — like `lib/*.luau` and `*.ink`,
//! they load from `<game_dir>/wasm/*.wasm` on every boot / `@reload-world` and
//! are never persisted to the database (see CLAUDE.md, "game_dir is image
//! content").
//!
//! ## Guest ABI (core WASM, no WASI / Component Model)
//!
//! A guest module exports:
//! - `memory` — its linear memory,
//! - `alloc(len: u32) -> u32` — reserve `len` bytes, return a pointer,
//! - `<func>(ptr: u32, len: u32) -> u64` — read the input JSON from
//!   `[ptr, ptr+len)` and return the result packed as `(out_ptr << 32) |
//!   out_len`, pointing at JSON bytes the guest still owns in its memory.
//!
//! The host allocates, writes the input, calls the function, and reads the
//! result back out. Every call runs under a fuel budget (mirroring the Luau
//! `Budget`), so a runaway plugin traps instead of hanging the engine.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use wasmi::{Config, Engine, ExternType, Linker, Module, Store, ValType};

/// A sidecar manifest (`<stem>.toml` next to `<stem>.wasm`) declaring which of
/// a plugin's exports to bind as Luau functions. Optional — a module with no
/// manifest is still callable via the low-level `wasm_call`.
///
/// ```toml
/// # names.toml — manifest for names.wasm
/// description = "Fantasy name generator"
///
/// [[functions]]
/// export = "generate"          # the wasm export
/// lua = "generate"             # Luau name (defaults to `export`)
/// description = "Roll a name from { seed, kind }."
/// ```
///
/// The functions are installed under a table named after the module, so the
/// example above becomes `names.generate({ seed = 1, kind = "elf" })`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Manifest {
    /// Overrides the Luau table name (defaults to the module/file stem).
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub functions: Vec<FnDecl>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FnDecl {
    /// The wasm export to call.
    pub export: String,
    /// The Luau function name (defaults to `export`).
    #[serde(default)]
    pub lua: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// One module's Luau binding: the table name (`ns`), the wasm module key to
/// call, and each `(lua_name, export_name)` pair. Produced by
/// [`WasmHost::binding_specs`] for `api::install` to turn into real functions.
#[derive(Debug, Clone)]
pub struct BindingSpec {
    pub ns: String,
    pub module: String,
    pub functions: Vec<(String, String)>,
}

/// Loaded, compiled WASM plugins, keyed by bare module name (the `.wasm`
/// file stem). Compiled once at load; each `call` spins up a fresh `Store`
/// so plugins never share mutable state across invocations.
pub struct WasmHost {
    engine: Engine,
    modules: HashMap<String, Module>,
    /// Per-module manifests, keyed by module name. Absent = no Luau bindings.
    manifests: HashMap<String, Manifest>,
}

impl Default for WasmHost {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmHost {
    pub fn new() -> Self {
        let mut config = Config::default();
        // Fuel metering is what makes an untrusted plugin safe to run in the
        // engine's single task — same role as the Luau instruction budget.
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        Self {
            engine,
            modules: HashMap::new(),
            manifests: HashMap::new(),
        }
    }

    /// Register a manifest for `module` (see [`Manifest`]). Overwrites any
    /// prior manifest for that module.
    pub fn set_manifest(&mut self, module: impl Into<String>, manifest: Manifest) {
        self.manifests.insert(module.into(), manifest);
    }

    /// The Luau bindings to install, one [`BindingSpec`] per module, owned so
    /// callers can drop the host borrow before building Lua functions.
    ///
    /// The wasm module is the source of truth for *what exists*: every export
    /// matching the plugin ABI (`(i32, i32) -> i64`) is bound automatically —
    /// `alloc` (`(i32) -> i32`) and `memory` don't match, so they're excluded
    /// for free. An optional manifest only *annotates* those real exports (a
    /// renamed Luau name, a different table namespace); a manifest entry whose
    /// `export` isn't actually in the module is ignored, so the two can't drift.
    pub fn binding_specs(&self) -> Vec<BindingSpec> {
        self.modules
            .iter()
            .filter_map(|(module, m)| {
                let manifest = self.manifests.get(module);
                let ns = manifest
                    .and_then(|x| x.name.clone())
                    .unwrap_or_else(|| module.clone());
                let overrides: HashMap<&str, &FnDecl> = manifest
                    .map(|x| x.functions.iter().map(|f| (f.export.as_str(), f)).collect())
                    .unwrap_or_default();

                let mut functions: Vec<(String, String)> = m
                    .exports()
                    .filter_map(|exp| {
                        let ExternType::Func(ft) = exp.ty() else {
                            return None;
                        };
                        if ft.params() != [ValType::I32, ValType::I32]
                            || ft.results() != [ValType::I64]
                        {
                            return None;
                        }
                        let export = exp.name().to_string();
                        let lua = overrides
                            .get(export.as_str())
                            .and_then(|d| d.lua.clone())
                            .unwrap_or_else(|| export.clone());
                        Some((lua, export))
                    })
                    .collect();
                functions.sort(); // deterministic install order

                if functions.is_empty() {
                    None
                } else {
                    Some(BindingSpec {
                        ns,
                        module: module.clone(),
                        functions,
                    })
                }
            })
            .collect()
    }

    /// Compile and register a module from raw bytes (`.wasm` or, with the
    /// `wat` feature, `.wat` text). Overwrites any module of the same name.
    pub fn add_module(&mut self, name: impl Into<String>, bytes: &[u8]) -> Result<(), String> {
        let module = Module::new(&self.engine, bytes)
            .map_err(|e| format!("failed to compile wasm module: {e}"))?;
        self.modules.insert(name.into(), module);
        Ok(())
    }

    /// Drop all modules and manifests (used before a reload re-populates).
    pub fn clear(&mut self) {
        self.modules.clear();
        self.manifests.clear();
    }

    pub fn has_module(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    /// Load every `<dir>/*.wasm` file, keyed by file stem. Missing dir is not
    /// an error (a game need not ship any plugins). A file that fails to
    /// compile is logged and skipped, never fatal.
    pub fn load_dir(&mut self, dir: &Path) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => {
                tracing::debug!(?dir, "no wasm plugin directory");
                return;
            }
        };
        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            match std::fs::read(&path) {
                Ok(bytes) => match self.add_module(stem, &bytes) {
                    Ok(()) => {
                        count += 1;
                        // Optional sidecar manifest: <stem>.toml beside <stem>.wasm.
                        let manifest_path = path.with_extension("toml");
                        match std::fs::read_to_string(&manifest_path) {
                            Ok(text) => match toml::from_str::<Manifest>(&text) {
                                Ok(manifest) => self.set_manifest(stem, manifest),
                                Err(e) => tracing::warn!(
                                    ?manifest_path, error = %e,
                                    "invalid wasm plugin manifest; binding exports without it",
                                ),
                            },
                            Err(_) => { /* no manifest — bind exports as-is */ }
                        }
                    }
                    Err(e) => tracing::warn!(?path, error = %e, "failed to load wasm plugin"),
                },
                Err(e) => tracing::warn!(?path, error = %e, "failed to read wasm plugin"),
            }
        }
        if count > 0 {
            tracing::info!(count, "Loaded wasm plugins");
        }
    }

    /// Invoke `module.func(input)` under a `fuel` budget, moving `input` bytes
    /// in and the result bytes out through the guest's linear memory per the
    /// ABI documented above.
    pub fn call(
        &self,
        module: &str,
        func: &str,
        input: &[u8],
        fuel: u64,
    ) -> Result<Vec<u8>, String> {
        let module = self
            .modules
            .get(module)
            .ok_or_else(|| format!("no wasm module '{module}'"))?;

        // Pure-compute: no host imports. A fresh store per call means no
        // state leaks between invocations.
        let mut store = Store::new(&self.engine, ());
        store
            .set_fuel(fuel)
            .map_err(|e| format!("failed to set fuel: {e}"))?;

        let linker: Linker<()> = Linker::new(&self.engine);
        let instance = linker
            .instantiate_and_start(&mut store, module)
            .map_err(|e| format!("failed to instantiate wasm module: {e}"))?;

        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| "wasm module has no 'memory' export".to_string())?;

        let alloc = instance
            .get_typed_func::<u32, u32>(&store, "alloc")
            .map_err(|_| "wasm module has no 'alloc(u32) -> u32' export".to_string())?;
        let ptr = alloc
            .call(&mut store, input.len() as u32)
            .map_err(|e| format!("wasm alloc trapped: {e}"))?;

        memory
            .write(&mut store, ptr as usize, input)
            .map_err(|e| format!("failed to write wasm input: {e}"))?;

        let run = instance
            .get_typed_func::<(u32, u32), u64>(&store, func)
            .map_err(|_| format!("wasm module has no '{func}(u32, u32) -> u64' export"))?;
        let packed = run
            .call(&mut store, (ptr, input.len() as u32))
            .map_err(|e| format!("wasm '{func}' trapped: {e}"))?;

        let out_ptr = (packed >> 32) as usize;
        let out_len = (packed & 0xffff_ffff) as usize;
        let mut buf = vec![0u8; out_len];
        memory
            .read(&store, out_ptr, &mut buf)
            .map_err(|e| format!("failed to read wasm output: {e}"))?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal identity-ish guest, hand-written in WAT: echoes the input
    /// bytes back. Exercises the full ABI (alloc + packed pointer return)
    /// without needing a compiled Rust crate.
    const ECHO_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (global $bump (mut i32) (i32.const 1024))
          (func (export "alloc") (param $len i32) (result i32)
            (local $ptr i32)
            (local.set $ptr (global.get $bump))
            (global.set $bump (i32.add (global.get $bump) (local.get $len)))
            (local.get $ptr))
          ;; echo(ptr, len) -> (ptr << 32) | len  — return the input as-is
          (func (export "echo") (param $ptr i32) (param $len i32) (result i64)
            (i64.or
              (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
              (i64.extend_i32_u (local.get $len)))))
    "#;

    #[test]
    fn echo_roundtrips_through_memory() {
        let mut host = WasmHost::new();
        host.add_module("echo", ECHO_WAT.as_bytes()).unwrap();
        let out = host.call("echo", "echo", b"{\"hi\":1}", 1_000_000).unwrap();
        assert_eq!(out, b"{\"hi\":1}");
    }

    #[test]
    fn binding_specs_introspect_exports() {
        let mut host = WasmHost::new();
        host.add_module("echo", ECHO_WAT.as_bytes()).unwrap();
        let specs = host.binding_specs();
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.ns, "echo");
        // `echo` matches the ABI and is bound; `alloc` ((i32)->i32) is not.
        assert_eq!(spec.functions, vec![("echo".to_string(), "echo".to_string())]);
    }

    #[test]
    fn manifest_renames_and_renamespaces() {
        let mut host = WasmHost::new();
        host.add_module("echo", ECHO_WAT.as_bytes()).unwrap();
        host.set_manifest(
            "echo",
            Manifest {
                name: Some("mirror".to_string()),
                description: None,
                functions: vec![FnDecl {
                    export: "echo".to_string(),
                    lua: Some("reflect".to_string()),
                    description: None,
                }],
            },
        );
        let spec = &host.binding_specs()[0];
        assert_eq!(spec.ns, "mirror");
        assert_eq!(spec.functions, vec![("reflect".to_string(), "echo".to_string())]);
        // A manifest entry for a non-existent export can't invent a binding.
        assert!(!spec.functions.iter().any(|(_, e)| e == "does_not_exist"));
    }

    #[test]
    fn missing_module_is_an_error() {
        let host = WasmHost::new();
        assert!(host.call("nope", "echo", b"", 1000).is_err());
    }

    #[test]
    fn fuel_exhaustion_traps() {
        let mut host = WasmHost::new();
        host.add_module("echo", ECHO_WAT.as_bytes()).unwrap();
        // One unit of fuel can't get through instantiation + calls.
        assert!(host.call("echo", "echo", b"x", 1).is_err());
    }

    /// The real compiled-Rust demo plugin (`plugins/names`), loaded from its
    /// committed fixture — proves the full wasm32 → WasmHost path, not just WAT.
    fn names_host() -> WasmHost {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/wasm/names.wasm"
        ))
        .expect("names.wasm fixture present");
        let mut host = WasmHost::new();
        host.add_module("names", &bytes).unwrap();
        host
    }

    fn gen_name(host: &WasmHost, seed: u64, kind: &str) -> String {
        let input = serde_json::json!({ "seed": seed, "kind": kind });
        let out = host
            .call(
                "names",
                "generate",
                &serde_json::to_vec(&input).unwrap(),
                50_000_000,
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        v["name"].as_str().unwrap().to_string()
    }

    #[test]
    #[ignore = "demo output, run with --nocapture"]
    fn names_plugin_samples() {
        let host = names_host();
        for kind in ["elf", "dwarf", "human"] {
            let names: Vec<String> = (0..6).map(|s| gen_name(&host, s, kind)).collect();
            println!("{kind}: {}", names.join(", "));
        }
    }

    #[test]
    fn names_plugin_is_deterministic() {
        let host = names_host();
        let a = gen_name(&host, 42, "elf");
        let b = gen_name(&host, 42, "elf");
        assert_eq!(a, b, "same seed+kind must reproduce the same name");
        assert!(!a.is_empty());
        assert!(a.chars().next().unwrap().is_uppercase());
    }

    #[test]
    fn names_plugin_varies_by_seed() {
        let host = names_host();
        let names: std::collections::HashSet<String> =
            (0..8).map(|s| gen_name(&host, s, "elf")).collect();
        // The generator should not collapse every seed onto one name.
        assert!(names.len() > 1, "expected variety across seeds: {names:?}");
    }
}
