local C = require("collections")

function test_set_add_and_has()
    local s = C.Set.new()
    s:add("a")
    s:add("b")
    assert_true(s:has("a"))
    assert_true(s:has("b"))
    assert_false(s:has("c"))
end

function test_set_size()
    local s = C.Set.new({"x", "y", "z"})
    assert_eq(s:size(), 3)
    s:add("x")
    assert_eq(s:size(), 3)
end

function test_set_remove()
    local s = C.Set.new({"a", "b", "c"})
    s:remove("b")
    assert_eq(s:size(), 2)
    assert_false(s:has("b"))
end

function test_set_values()
    local s = C.Set.new({"a", "b"})
    local vals = s:values()
    assert_eq(#vals, 2)
end

function test_array_map()
    local result = C.Array.map({1, 2, 3}, function(v) return v * 2 end)
    assert_eq(result, {2, 4, 6})
end

function test_array_filter()
    local result = C.Array.filter({1, 2, 3, 4}, function(v) return v % 2 == 0 end)
    assert_eq(result, {2, 4})
end

function test_array_contains()
    assert_true(C.Array.contains({1, 2, 3}, 2))
    assert_false(C.Array.contains({1, 2, 3}, 4))
end

function test_array_find()
    local result = C.Array.find({1, 2, 3}, function(v) return v > 1 end)
    assert_eq(result, 2)
    local none = C.Array.find({1, 2, 3}, function(v) return v > 10 end)
    assert_nil(none)
end

function test_array_reduce()
    local sum = C.Array.reduce({1, 2, 3, 4}, function(acc, v) return acc + v end, 0)
    assert_eq(sum, 10)
end

function test_array_reverse()
    local result = C.Array.reverse({1, 2, 3})
    assert_eq(result, {3, 2, 1})
end

function test_array_flat()
    local result = C.Array.flat({{1, 2}, {3}, {4, 5}})
    assert_eq(result, {1, 2, 3, 4, 5})
end
