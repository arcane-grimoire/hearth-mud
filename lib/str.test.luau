local str = require("str")

function test_split()
    local parts = str.split("a,b,c", ",")
    assert_eq(#parts, 3)
    assert_eq(parts[1], "a")
    assert_eq(parts[2], "b")
    assert_eq(parts[3], "c")
end

function test_split_spaces()
    local parts = str.split("hello world foo", " ")
    assert_eq(#parts, 3)
    assert_eq(parts[1], "hello")
end

function test_trim()
    assert_eq(str.trim("  hello  "), "hello")
    assert_eq(str.trim("no-spaces"), "no-spaces")
end

function test_starts_with()
    assert_true(str.starts_with("hello world", "hello"))
    assert_false(str.starts_with("hello world", "world"))
end

function test_ends_with()
    assert_true(str.ends_with("hello world", "world"))
    assert_false(str.ends_with("hello world", "hello"))
end

function test_title_case()
    assert_eq(str.title_case("hello world"), "Hello World")
end

function test_pad_right()
    assert_eq(str.pad_right("hi", 5), "hi   ")
    assert_eq(str.pad_right("hello", 3), "hello")
end

function test_truncate()
    assert_eq(str.truncate("hello world", 8), "hello...")
    assert_eq(str.truncate("short", 10), "short")
end

function test_pluralize()
    assert_eq(str.pluralize(1, "sword"), "1 sword")
    assert_eq(str.pluralize(3, "sword"), "3 swords")
    assert_eq(str.pluralize(0, "wolf", "wolves"), "0 wolves")
end

function test_wrap()
    local result = str.wrap("a b c d e", 5)
    assert_eq(result, "a b c\nd e")
end
