-- Lua-native collection utilities: Set, Array helpers.
-- MIT License

local Collections = {}

-- Set: unique values, insertion-ordered iteration.
local Set = {}
Set.__index = Set

function Set.new(items)
    local self = setmetatable({}, Set)
    self._items = {}
    self._order = {}
    if items then
        for _, v in ipairs(items) do
            self:add(v)
        end
    end
    return self
end

function Set:add(value)
    if not self._items[value] then
        self._items[value] = true
        table.insert(self._order, value)
    end
end

function Set:remove(value)
    if self._items[value] then
        self._items[value] = nil
        for i, v in ipairs(self._order) do
            if v == value then
                table.remove(self._order, i)
                break
            end
        end
    end
end

function Set:has(value)
    return self._items[value] == true
end

function Set:size()
    return #self._order
end

function Set:values()
    local copy = {}
    for _, v in ipairs(self._order) do
        table.insert(copy, v)
    end
    return copy
end

function Set:union(other)
    local result = Set.new(self._order)
    for _, v in ipairs(other._order) do
        result:add(v)
    end
    return result
end

function Set:intersection(other)
    local result = Set.new()
    for _, v in ipairs(self._order) do
        if other:has(v) then
            result:add(v)
        end
    end
    return result
end

function Set:difference(other)
    local result = Set.new()
    for _, v in ipairs(self._order) do
        if not other:has(v) then
            result:add(v)
        end
    end
    return result
end

Collections.Set = Set

-- Array helpers (operate on plain Lua tables).
local Array = {}

function Array.map(list, fn)
    local out = {}
    for i, v in ipairs(list) do
        out[i] = fn(v, i)
    end
    return out
end

function Array.filter(list, fn)
    local out = {}
    for i, v in ipairs(list) do
        if fn(v, i) then
            table.insert(out, v)
        end
    end
    return out
end

function Array.find(list, fn)
    for i, v in ipairs(list) do
        if fn(v, i) then
            return v
        end
    end
    return nil
end

function Array.reduce(list, fn, initial)
    local acc = initial
    local start = 1
    if acc == nil then
        acc = list[1]
        start = 2
    end
    for i = start, #list do
        acc = fn(acc, list[i], i)
    end
    return acc
end

function Array.flat(list)
    local out = {}
    for _, v in ipairs(list) do
        if type(v) == "table" then
            for _, inner in ipairs(v) do
                table.insert(out, inner)
            end
        else
            table.insert(out, v)
        end
    end
    return out
end

function Array.contains(list, value)
    for _, v in ipairs(list) do
        if v == value then return true end
    end
    return false
end

function Array.reverse(list)
    local out = {}
    for i = #list, 1, -1 do
        table.insert(out, list[i])
    end
    return out
end

function Array.slice(list, start, stop)
    start = start or 1
    stop = stop or #list
    local out = {}
    for i = start, stop do
        table.insert(out, list[i])
    end
    return out
end

function Array.every(list, fn)
    for i, v in ipairs(list) do
        if not fn(v, i) then return false end
    end
    return true
end

function Array.some(list, fn)
    for i, v in ipairs(list) do
        if fn(v, i) then return true end
    end
    return false
end

Collections.Array = Array

return Collections
