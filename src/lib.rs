//! Hearth MUD — a Rust MUD framework with Luau softcode.
//!
//! This library crate exposes the engine's internals so integration tests,
//! benchmarks (`benches/`), and the `hearth-mud` binary (`src/main.rs`) can all
//! build against one compiled copy of the modules. The binary is a thin
//! entrypoint that wires `engine` + `net` together; everything reusable lives
//! here.

#![allow(dead_code)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]

pub mod accounts;
pub mod ansi;
pub mod cli;
pub mod config;
pub mod db;
pub mod dungeon;
pub mod engine;
pub mod grid;
pub mod import_export;
pub mod loader;
pub mod locks;
pub mod map_template;
pub mod markup;
pub mod net;
pub mod noise;
pub mod softcode;
pub mod theme;
pub mod world;
