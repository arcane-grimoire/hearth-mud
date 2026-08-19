-- Random utilities for game scripting.
-- MIT License

local Random = {}

function Random.roll(sides)
    return math.random(1, sides)
end

function Random.dice(count, sides)
    local total = 0
    for _ = 1, count do
        total = total + math.random(1, sides)
    end
    return total
end

function Random.chance(percent)
    return math.random(1, 100) <= percent
end

function Random.pick(list)
    if #list == 0 then return nil end
    return list[math.random(1, #list)]
end

function Random.weighted(choices)
    local total = 0
    for _, entry in ipairs(choices) do
        total = total + entry.weight
    end
    local roll = math.random() * total
    local cumulative = 0
    for _, entry in ipairs(choices) do
        cumulative = cumulative + entry.weight
        if roll <= cumulative then
            return entry.value
        end
    end
    return choices[#choices].value
end

function Random.shuffle(list)
    local out = table.clone(list)
    for i = #out, 2, -1 do
        local j = math.random(1, i)
        out[i], out[j] = out[j], out[i]
    end
    return out
end

function Random.range(min, max)
    return min + math.random() * (max - min)
end

function Random.sample(list, count)
    local shuffled = Random.shuffle(list)
    local out = {}
    for i = 1, math.min(count, #shuffled) do
        table.insert(out, shuffled[i])
    end
    return out
end

return Random
