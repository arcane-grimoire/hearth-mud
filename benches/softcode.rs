//! Performance benchmarks for the Luau softcode runtime.
//!
//! Softcode runs synchronously on the engine's single writer thread, so every
//! microsecond spent in a hook is a microsecond the world tick is blocked. The
//! instruction [`Budget`] caps how *many* VM interrupts a script may fire, but
//! it is a count, not a clock — a script that regresses to slow-but-bounded
//! passes the budget and still stalls the tick. These benchmarks measure the
//! wall-clock cost the budget can't see, so a regression in hook dispatch, the
//! bytecode cache, the budget interrupt, the Intent batch, or the read/write
//! API shows up as a number instead of a mystery lag.
//!
//! Each scenario isolates one cost center so a regression points somewhere:
//!
//! - `runtime_new`      — cost of standing up a fresh Luau VM.
//! - `compile_cold`     — compiling a never-before-seen chunk (cache miss).
//! - `dispatch_trivial` — fixed per-call overhead of a warm, near-empty hook.
//! - `compute_loop`     — the budget interrupt's per-back-edge cost, no world I/O.
//! - `read_heavy`       — the read API (`get_attr`/`has_tag`/`get_room_contents`).
//! - `write_heavy`      — building + applying an Intent batch of mutations.
//! - `mixed_realistic`  — a representative command hook: read + write together.
//!
//! Run with `cargo bench`. See issue #20.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use hearth_mud::map_template::MapTemplateFile;
use hearth_mud::softcode::hooks::ProgramRecord;
use hearth_mud::softcode::{apply_batch, Budget, ScheduledHook, SoftcodeRuntime};
use hearth_mud::theme::Theme;
use hearth_mud::world::{GameObject, Kind, Tag, World};

/// A small but non-trivial world, mirroring the `test_world()` fixture used by
/// the softcode unit tests. Fixed creation order gives predictable dbrefs:
/// room "#1", room2 "#2", alice "#3", bob "#4", sword "#5", shield "#6",
/// guard "#7", exit "#8".
fn bench_world() -> World {
    let mut world = World::new();

    let room_ref = world.next_dbref(); // "#1"
    let mut room = GameObject::new(&room_ref, "room", Kind::Room).with_title("A Room");
    room.description = "A plain room.".into();
    world.add_object(room);

    let room2_ref = world.next_dbref(); // "#2"
    let room2 = GameObject::new(&room2_ref, "room2", Kind::Room).with_title("Another Room");
    world.add_object(room2);

    let alice_ref = world.next_dbref(); // "#3"
    let mut alice = GameObject::new(&alice_ref, "alice", Kind::Player)
        .with_title("Alice")
        .with_location(&room_ref);
    alice.tags.insert(Tag { category: "quest".into(), key: "hero".into() });
    world.add_object(alice);

    let bob_ref = world.next_dbref(); // "#4"
    let bob = GameObject::new(&bob_ref, "bob", Kind::Player)
        .with_title("Bob")
        .with_location(&room_ref);
    world.add_object(bob);

    let sword_ref = world.next_dbref(); // "#5"
    let mut sword = GameObject::new(&sword_ref, "sword", Kind::Item)
        .with_title("a rusty sword")
        .with_location(&room_ref);
    sword.tags.insert(Tag { category: "loot".into(), key: "weapon".into() });
    sword.attrs.insert("damage".into(), serde_json::json!(10));
    world.add_object(sword);

    let shield_ref = world.next_dbref(); // "#6"
    let shield = GameObject::new(&shield_ref, "shield", Kind::Item)
        .with_title("a wooden shield")
        .with_location(&alice_ref);
    world.add_object(shield);

    let guard_ref = world.next_dbref(); // "#7"
    let guard = GameObject::new(&guard_ref, "guard", Kind::Npc)
        .with_title("A Town Guard")
        .with_location(&room_ref);
    world.add_object(guard);

    let exit_ref = world.next_dbref(); // "#8"
    let exit = GameObject::new(&exit_ref, "north", Kind::Exit)
        .with_location(&room_ref)
        .with_target(&room2_ref)
        .with_aliases(vec!["n"]);
    world.add_object(exit);

    world
}

/// A fresh dbref counter seeded from the world's next id — matches what the
/// engine hands `run_hook` so `spawn` produces non-colliding refs.
fn counter(world: &World) -> Rc<Cell<u64>> {
    Rc::new(Cell::new(world.next_id))
}

/// Runs one hook against `world` and returns the resulting batch, panicking on
/// any softcode error so a broken benchmark fails loudly instead of silently
/// measuring an error path. `this`/`actor`/`room` are the standard sword/alice/
/// room fixture refs.
fn run(
    runtime: &SoftcodeRuntime,
    world: &World,
    program: &ProgramRecord,
    themes: &HashMap<String, Theme>,
    maps: &HashMap<String, MapTemplateFile>,
    no_hooks: &[ScheduledHook],
) -> hearth_mud::softcode::ProgramResult {
    runtime
        .run_hook(
            world,
            program,
            "#5",
            "#3",
            Some("#1"),
            None,
            Budget::default(),
            counter(world),
            themes,
            maps,
            no_hooks,
            0,
        )
        .expect("hook should run")
}

fn softcode_benches(c: &mut Criterion) {
    let world = bench_world();
    let themes: HashMap<String, Theme> = HashMap::new();
    let maps: HashMap<String, MapTemplateFile> = HashMap::new();
    let no_hooks: [ScheduledHook; 0] = [];

    // Standing up a fresh Luau VM (stdlib install, registry setup). The engine
    // does this once at boot, but a regression here slows every test run and
    // every `@reload-world`.
    c.bench_function("runtime_new", |b| {
        b.iter(|| black_box(SoftcodeRuntime::new()));
    });

    // Cache miss: a chunk the runtime has never seen. A unique comment per
    // iteration changes the source hash so `get_or_compile` always misses and
    // recompiles, isolating compile cost from VM setup (the VM is reused).
    c.bench_function("compile_cold", |b| {
        let runtime = SoftcodeRuntime::new();
        let mut n: u64 = 0;
        b.iter(|| {
            n += 1;
            let source = format!("-- {n}\nfunction on_get(this, actor, room) end");
            let program = ProgramRecord::new("on_get", source);
            black_box(run(&runtime, &world, &program, &themes, &maps, &no_hooks));
        });
    });

    // Warm cache: the fixed cost of firing a near-empty hook — arg marshalling,
    // budget interrupt install, env setup, function lookup. This number is paid
    // by *every* script, so it matters most.
    c.bench_function("dispatch_trivial", |b| {
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new("on_get", "function on_get(this, actor, room) end");
        // Prime the chunk cache so we measure warm dispatch, not first compile.
        run(&runtime, &world, &program, &themes, &maps, &no_hooks);
        b.iter(|| black_box(run(&runtime, &world, &program, &themes, &maps, &no_hooks)));
    });

    // A tight compute loop with no world I/O. The budget interrupt fires on
    // every loop back-edge, so this isolates the per-interrupt overhead.
    c.bench_function("compute_loop", |b| {
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local sum = 0
                    for i = 1, 1000 do sum = sum + i end
                    return sum
                end
            "#,
        );
        run(&runtime, &world, &program, &themes, &maps, &no_hooks);
        b.iter(|| black_box(run(&runtime, &world, &program, &themes, &maps, &no_hooks)));
    });

    // Read-API heavy: many `get_attr`/`has_tag`/`get_room_contents` calls
    // against the populated world. Guards the read path and snapshotting.
    c.bench_function("read_heavy", |b| {
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local total = 0
                    for i = 1, 100 do
                        if has_tag(this, "loot:weapon") then total = total + 1 end
                        total = total + (get_attr(this, "damage") or 0)
                        local contents = get_room_contents(room)
                        total = total + #contents
                    end
                    set_attr(this, "total", total)
                end
            "#,
        );
        run(&runtime, &world, &program, &themes, &maps, &no_hooks);
        b.iter(|| black_box(run(&runtime, &world, &program, &themes, &maps, &no_hooks)));
    });

    // Write-API heavy: queue many Intents, then apply the batch to a clone of
    // the world. Measures batch build + validate + apply together.
    c.bench_function("write_heavy", |b| {
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    for i = 1, 100 do
                        set_attr(this, "k" .. i, i)
                    end
                end
            "#,
        );
        run(&runtime, &world, &program, &themes, &maps, &no_hooks);
        b.iter(|| {
            let result = run(&runtime, &world, &program, &themes, &maps, &no_hooks);
            let mut w = world.clone();
            apply_batch(&mut w, &result.batch).expect("batch should apply");
            black_box(w);
        });
    });

    // A representative command hook: read some state, branch, emit, mutate —
    // the closest single-benchmark proxy for real game-hook cost.
    c.bench_function("mixed_realistic", |b| {
        let runtime = SoftcodeRuntime::new();
        let program = ProgramRecord::new(
            "on_get",
            r#"
                function on_get(this, actor, room)
                    local dmg = get_attr(this, "damage") or 0
                    if has_tag(this, "loot:weapon") then
                        emit(actor, this.display_name .. " hums, dealing " .. dmg .. " damage.")
                        emit_room(room, actor.display_name .. " hefts " .. this.display_name .. ".", {actor.ref_id})
                        set_attr(this, "held_by", actor.ref_id)
                        set_attr(actor, "wielding", this.ref_id)
                    end
                    return true
                end
            "#,
        );
        run(&runtime, &world, &program, &themes, &maps, &no_hooks);
        b.iter(|| {
            let result = run(&runtime, &world, &program, &themes, &maps, &no_hooks);
            let mut w = world.clone();
            apply_batch(&mut w, &result.batch).expect("batch should apply");
            black_box(w);
        });
    });
}

criterion_group!(benches, softcode_benches);
criterion_main!(benches);
