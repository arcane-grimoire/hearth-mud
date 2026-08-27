---
description: Write and run Luau test files for Hearth MUD softcode. Use when creating .test.luau files, debugging test failures, or testing lib modules and game hooks.
---

# Writing softcode tests

Test files use the `.test.luau` suffix and live alongside the code they test in the game directory. Tests are discovered and run by the engine's test runner via `cargo test` or the in-game `@test` command.

## Two modes

**Unit tests** — files under `lib/` get stdlib + `require()` + assertions only. No world, no softcode API. Test functions take no arguments.

**Integration tests** — files outside `lib/` get the full softcode API with a test world. Test functions receive a `ctx` table with `actor`, `room`, and `this` refs.

## Conventions

- Test files: `<name>.test.luau` (e.g., `str.test.luau`, `cmd_hero.test.luau`)
- Test functions: top-level globals named `test_*`
- Tests run alphabetically within each file
- Each integration test gets a fresh world clone for isolation
- A test passes if it returns without error; it fails if any assertion fails

## Assertion API

```lua
assert_eq(actual, expected)           -- deep equality (tables, nested)
assert_eq(actual, expected, "msg")    -- with custom message
assert_true(value)                    -- must be literal true
assert_false(value)                   -- must be literal false
assert_nil(value)                     -- must be nil
assert_not_nil(value)                 -- must not be nil
assert_error(fn)                      -- fn must throw
assert_error(fn, "pattern")           -- error message must contain pattern
```

`assert_eq` does deep table comparison — `assert_eq({1, 2}, {1, 2})` passes. All assertions accept an optional trailing message string for context.

## Unit test example

```lua
-- lib/str.test.luau
local str = require("str")

function test_split()
    local parts = str.split("a,b,c", ",")
    assert_eq(#parts, 3)
    assert_eq(parts[1], "a")
end

function test_trim()
    assert_eq(str.trim("  hello  "), "hello")
end

function test_error_handling()
    assert_error(function()
        str.split(nil, ",")
    end)
end
```

## Integration test example

```lua
-- system/cmd_hero.test.luau
function test_get_object(ctx)
    local actor = get_object(ctx.actor)
    assert_not_nil(actor)
    assert_eq(actor.kind, "player")
end

function test_room_contents(ctx)
    local contents = get_room_contents(ctx.room)
    assert_true(#contents > 0)
end

function test_spawn_and_destroy(ctx)
    local ref = spawn({ key = "test_item", kind = "item", location = ctx.room })
    assert_not_nil(ref)
end
```

Integration tests have the full softcode API available as globals: `get_object`, `set_attr`, `emit`, `spawn`, `move_object`, `get_room_contents`, `get_inventory`, `has_tag`, etc.

## Running tests

```sh
cargo test run_game_softcode_tests        # run all .test.luau files
cargo test run_game_softcode_tests -- --nocapture  # with output
```

In-game (as a builder):
```
@test                          -- run all test files
@test lib/str.test.luau        -- run one file
```

The `cargo test` harness looks for the game directory at `../the-last-stag-mud/game/` or the `HEARTH_GAME_DIR` environment variable. It skips gracefully if not found.

## When writing tests

- Test one thing per function — keep tests focused
- Name tests descriptively: `test_split_empty_string`, not `test_1`
- For lib modules, test edge cases: empty input, nil, boundary values
- For integration tests, use `ctx.actor`/`ctx.room`/`ctx.this` for object refs
- Integration tests can use the write API (set_attr, spawn, etc.) but mutations are discarded after each test
- Use `assert_error` to verify error handling paths

## File locations

- `src/softcode/mod.rs` — `SoftcodeRuntime::run_tests()`, assertion helpers, Rust test harness
- `src/loader.rs` — `discover_test_files()`, `TestFile` struct
- `src/engine/mod.rs` — `@test` builder command
- Game test files live in the game directory (e.g., `../the-last-stag-mud/game/lib/str.test.luau`)
