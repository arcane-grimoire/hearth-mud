-- String utilities missing from Luau stdlib.
-- MIT License

local S = {}

function S.split(s, delim)
    delim = delim or " "
    local parts = {}
    local pattern = "([^" .. delim .. "]+)"
    for part in s:gmatch(pattern) do
        table.insert(parts, part)
    end
    return parts
end

function S.trim(s)
    return s:match("^%s*(.-)%s*$")
end

function S.ltrim(s)
    return s:match("^%s*(.*)")
end

function S.rtrim(s)
    return s:match("(.-)%s*$")
end

function S.starts_with(s, prefix)
    return s:sub(1, #prefix) == prefix
end

function S.ends_with(s, suffix)
    return suffix == "" or s:sub(-#suffix) == suffix
end

function S.title_case(s)
    return s:gsub("(%a)([%w]*)", function(first, rest)
        return first:upper() .. rest:lower()
    end)
end

function S.pad_right(s, width, char)
    char = char or " "
    if #s >= width then return s end
    return s .. string.rep(char, width - #s)
end

function S.pad_left(s, width, char)
    char = char or " "
    if #s >= width then return s end
    return string.rep(char, width - #s) .. s
end

function S.center(s, width, char)
    char = char or " "
    if #s >= width then return s end
    local pad = width - #s
    local left = math.floor(pad / 2)
    local right = pad - left
    return string.rep(char, left) .. s .. string.rep(char, right)
end

function S.truncate(s, width, suffix)
    suffix = suffix or "..."
    if #s <= width then return s end
    return s:sub(1, width - #suffix) .. suffix
end

function S.pluralize(n, singular, plural)
    plural = plural or (singular .. "s")
    if n == 1 then
        return n .. " " .. singular
    end
    return n .. " " .. plural
end

function S.wrap(s, width)
    width = width or 78
    local lines = {}
    local line = ""
    for word in s:gmatch("%S+") do
        if #line == 0 then
            line = word
        elseif #line + 1 + #word > width then
            table.insert(lines, line)
            line = word
        else
            line = line .. " " .. word
        end
    end
    if #line > 0 then
        table.insert(lines, line)
    end
    return table.concat(lines, "\n")
end

return S
