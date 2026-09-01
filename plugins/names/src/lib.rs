//! Deterministic, seedable order-2 character Markov name generator — a demo
//! compute-only Hearth WASM plugin.
//!
//! ABI (see `src/softcode/wasm.rs`): the host calls `alloc(len)` to reserve a
//! buffer, writes the input JSON there, then calls `generate(ptr, len)` which
//! returns the result location packed as `(out_ptr << 32) | out_len`. Linear
//! memory is exported as `memory` automatically for a `cdylib` wasm target.
//!
//! Input:  `{ "seed": <u64>, "kind": "elf" | "dwarf" | "human" }`
//! Output: `{ "name": "<generated>" }`
//!
//! Same `(seed, kind)` always yields the same name, so results are safe to
//! persist and to assert on in tests.

use std::collections::HashMap;

use serde::Deserialize;

// -- Arena allocator + `reset` export ------------------------------------
//
// Opts this plugin into host-side instance pooling (see `src/softcode/wasm.rs`):
// the host keeps one instance resident and calls `reset` before each `generate`
// instead of re-instantiating. A per-call bump arena makes that safe — every
// allocation (input buffer, the Markov model, the output) comes from one arena
// that `reset` rewinds to the start, so memory can't grow across calls. `dealloc`
// is a no-op; the whole arena is reclaimed wholesale on the next `reset`.

const ARENA_SIZE: usize = 4 * 1024 * 1024;

#[repr(C, align(16))]
struct Arena(core::cell::UnsafeCell<[u8; ARENA_SIZE]>);
// Single-threaded wasm: no real concurrency to guard against.
unsafe impl Sync for Arena {}

static ARENA: Arena = Arena(core::cell::UnsafeCell::new([0; ARENA_SIZE]));
static OFFSET: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

struct BumpAlloc;

unsafe impl core::alloc::GlobalAlloc for BumpAlloc {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let base = ARENA.0.get() as usize;
        let cur = OFFSET.load(core::sync::atomic::Ordering::Relaxed);
        let aligned = (base + cur + layout.align() - 1) & !(layout.align() - 1);
        let new_off = aligned - base + layout.size();
        if new_off > ARENA_SIZE {
            return core::ptr::null_mut();
        }
        OFFSET.store(new_off, core::sync::atomic::Ordering::Relaxed);
        aligned as *mut u8
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

#[global_allocator]
static ALLOC: BumpAlloc = BumpAlloc;

/// Rewind the arena — the host calls this before each `generate` so a pooled
/// instance starts every call with a clean heap.
#[no_mangle]
pub extern "C" fn reset() {
    OFFSET.store(0, core::sync::atomic::Ordering::Relaxed);
}

#[derive(Deserialize)]
struct Input {
    #[serde(default)]
    seed: u64,
    #[serde(default)]
    kind: String,
}

// Start/end sentinels kept out of the visible alphabet.
const START: char = '\u{2}';
const END: char = '\u{3}';

fn corpus(kind: &str) -> &'static [&'static str] {
    match kind {
        "dwarf" => &[
            "thorin", "balin", "dwalin", "gloin", "durin", "nain", "borin", "farin", "grimm",
            "brok", "thrain", "dain", "bofur", "bombur", "kili", "fili", "oin", "gror",
        ],
        "human" => &[
            "aldric", "bran", "cedric", "rowan", "edmund", "godwin", "harald", "leofric", "osric",
            "wulfstan", "alistair", "gareth", "tristan", "roland", "hugh", "walter",
        ],
        // default: elf
        _ => &[
            "aelrindel", "elrond", "galadriel", "legolas", "thranduil", "arwen", "celeborn",
            "elladan", "elrohir", "finrod", "luthien", "elenwe", "idril", "aegnor", "curufin",
            "fingon",
        ],
    }
}

/// Order-2 char model: `(c1, c2) -> possible next chars`, with two START
/// sentinels prefixed and one END sentinel appended to each training word.
fn build_model(words: &[&str]) -> HashMap<(char, char), Vec<char>> {
    let mut model: HashMap<(char, char), Vec<char>> = HashMap::new();
    for word in words {
        let chars: Vec<char> = std::iter::once(START)
            .chain(std::iter::once(START))
            .chain(word.chars())
            .chain(std::iter::once(END))
            .collect();
        for w in chars.windows(3) {
            model.entry((w[0], w[1])).or_default().push(w[2]);
        }
    }
    model
}

/// Small deterministic LCG so a given seed reproduces a given walk.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn generate_name(kind: &str, seed: u64) -> String {
    let model = build_model(corpus(kind));
    // Nudge the seed so seed=0 still produces a lively walk.
    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
    let mut c1 = START;
    let mut c2 = START;
    let mut out = String::new();
    for _ in 0..24 {
        let Some(choices) = model.get(&(c1, c2)) else {
            break;
        };
        let next = choices[(rng.next() as usize) % choices.len()];
        if next == END {
            break;
        }
        out.push(next);
        c1 = c2;
        c2 = next;
    }
    if out.is_empty() {
        out.push_str("nameless");
    }
    // Capitalize.
    let mut chars = out.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => out,
    }
}

/// Reserve `len` bytes in the guest and hand back a pointer the host writes to.
#[no_mangle]
pub extern "C" fn alloc(len: u32) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Read input JSON from `[ptr, ptr+len)`, return `{ "name": ... }` JSON packed
/// as `(out_ptr << 32) | out_len`.
///
/// # Safety
/// The host guarantees `[ptr, ptr+len)` is a buffer it just wrote via `alloc`.
#[no_mangle]
pub extern "C" fn generate(ptr: *const u8, len: u32) -> u64 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let parsed: Input = serde_json::from_slice(input).unwrap_or(Input {
        seed: 0,
        kind: String::new(),
    });
    let name = generate_name(&parsed.kind, parsed.seed);
    let out = serde_json::to_vec(&serde_json::json!({ "name": name }))
        .unwrap_or_else(|_| b"{\"name\":\"error\"}".to_vec());

    let out_len = out.len() as u64;
    let out_ptr = out.as_ptr() as u64;
    std::mem::forget(out);
    (out_ptr << 32) | out_len
}
