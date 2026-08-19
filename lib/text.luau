-- Rich text formatting with accessible/visual modes.
-- Uses BBCode markup for transport-neutral styling (works on both telnet and web).
-- MIT License
--
-- Usage:
--   local text = require("text")
--   local fmt = text.for_mode(get_attr(actor, "_display_mode") or "visual")
--   emit(actor, fmt.bar(hp, max_hp, 20))
--   emit(actor, fmt.header("Combat Status"))

local str = require("str")

local T = {}

function T.strip_markup(s)
	return s:gsub("%[/?[%w=_ ]*%]", "")
end

function T.visible_len(s)
	return #T.strip_markup(s)
end

function T.color(s, color_name, mode)
	if mode == "accessible" then return s end
	return "[" .. color_name .. "]" .. s .. "[/]"
end

function T.bold(s, mode)
	if mode == "accessible" then return s end
	return "[b]" .. s .. "[/b]"
end

function T.dim(s, mode)
	if mode == "accessible" then return s end
	return "[dim]" .. s .. "[/dim]"
end

function T.underline(s, mode)
	if mode == "accessible" then return s end
	return "[u]" .. s .. "[/u]"
end

-- HP / progress bar.
-- Visual:     [████████░░░░] 8/12
-- Accessible: HP: 8/12 (67%)
function T.bar(current, max, width, mode, label)
	width = width or 20
	label = label or ""
	local pct = max > 0 and (current / max) or 0
	local filled = math.floor(pct * width + 0.5)

	if mode == "accessible" then
		local prefix = label ~= "" and (label .. ": ") or ""
		return prefix .. current .. "/" .. max .. " (" .. math.floor(pct * 100) .. "%)"
	end

	local bar_filled = string.rep("\xe2\x96\x88", filled)
	local bar_empty = string.rep("\xe2\x96\x91", width - filled)

	local bar_color
	if pct > 0.6 then
		bar_color = "green"
	elseif pct > 0.3 then
		bar_color = "yellow"
	else
		bar_color = "red"
	end

	local bar = "[" .. bar_color .. "]" .. bar_filled .. "[/]" .. "[dim]" .. bar_empty .. "[/dim]"
	local text = " " .. current .. "/" .. max
	if label ~= "" then
		return label .. " [" .. bar .. "]" .. text
	end
	return "[" .. bar .. "]" .. text
end

-- Section header.
-- Visual:     ═══ Combat Status ═══
-- Accessible: --- Combat Status ---
function T.header(title, mode, width)
	width = width or 40
	if mode == "accessible" then
		return "--- " .. title .. " ---"
	end
	local pad = width - #title - 2
	local left = math.floor(pad / 2)
	local right = pad - left
	return "[b]" .. string.rep("\xe2\x95\x90", left) .. " " .. title .. " " .. string.rep("\xe2\x95\x90", right) .. "[/b]"
end

-- Horizontal divider.
-- Visual:     ─────────────
-- Accessible: -------------
function T.divider(width, mode)
	width = width or 40
	if mode == "accessible" then
		return string.rep("-", width)
	end
	return "[dim]" .. string.rep("\xe2\x94\x80", width) .. "[/dim]"
end

-- Simple table renderer.
-- Visual:     aligned columns with dim separators
-- Accessible: label: value pairs, one per line
function T.table(rows, headers, mode)
	if mode == "accessible" then
		local lines = {}
		for _, row in ipairs(rows) do
			local parts = {}
			for i, cell in ipairs(row) do
				local label = headers and headers[i] or ""
				if label ~= "" then
					table.insert(parts, label .. ": " .. tostring(cell))
				else
					table.insert(parts, tostring(cell))
				end
			end
			table.insert(lines, table.concat(parts, ", "))
		end
		return table.concat(lines, "\n")
	end

	local col_count = 0
	if headers then col_count = #headers end
	for _, row in ipairs(rows) do
		if #row > col_count then col_count = #row end
	end

	local widths = {}
	for i = 1, col_count do widths[i] = 0 end
	if headers then
		for i, h in ipairs(headers) do
			if #tostring(h) > widths[i] then widths[i] = #tostring(h) end
		end
	end
	for _, row in ipairs(rows) do
		for i, cell in ipairs(row) do
			local len = #tostring(cell)
			if len > widths[i] then widths[i] = len end
		end
	end

	local lines = {}
	local sep = "[dim] \xe2\x94\x82 [/dim]"

	if headers then
		local parts = {}
		for i, h in ipairs(headers) do
			table.insert(parts, "[b]" .. str.pad_right(tostring(h), widths[i]) .. "[/b]")
		end
		table.insert(lines, table.concat(parts, sep))
		local rule_parts = {}
		for i = 1, col_count do
			table.insert(rule_parts, string.rep("\xe2\x94\x80", widths[i]))
		end
		table.insert(lines, "[dim]" .. table.concat(rule_parts, "\xe2\x94\xbc\xe2\x94\x80") .. "[/dim]")
	end

	for _, row in ipairs(rows) do
		local parts = {}
		for i = 1, col_count do
			local cell = row[i] or ""
			table.insert(parts, str.pad_right(tostring(cell), widths[i]))
		end
		table.insert(lines, table.concat(parts, sep))
	end

	return table.concat(lines, "\n")
end

-- Box around text lines.
-- Visual:     ╭──────╮
--             │ text │
--             ╰──────╯
-- Accessible: [text]
function T.box(lines_or_text, mode, width)
	local lines
	if type(lines_or_text) == "string" then
		lines = {}
		for line in lines_or_text:gmatch("[^\n]+") do
			table.insert(lines, line)
		end
	else
		lines = lines_or_text
	end

	if mode == "accessible" then
		local out = {}
		for _, line in ipairs(lines) do
			table.insert(out, "  " .. line)
		end
		return table.concat(out, "\n")
	end

	local max_w = width or 0
	for _, line in ipairs(lines) do
		local len = T.visible_len(line)
		if len > max_w then max_w = len end
	end
	max_w = max_w + 2

	local out = {}
	table.insert(out, "[dim]\xe2\x95\xad" .. string.rep("\xe2\x94\x80", max_w) .. "\xe2\x95\xae[/dim]")
	for _, line in ipairs(lines) do
		local pad = max_w - T.visible_len(line) - 2
		table.insert(out, "[dim]\xe2\x94\x82[/dim] " .. line .. string.rep(" ", pad) .. " [dim]\xe2\x94\x82[/dim]")
	end
	table.insert(out, "[dim]\xe2\x95\xb0" .. string.rep("\xe2\x94\x80", max_w) .. "\xe2\x95\xaf[/dim]")
	return table.concat(out, "\n")
end

-- Inline label: value with color.
function T.stat(label, value, color_name, mode)
	if mode == "accessible" then
		return label .. ": " .. tostring(value)
	end
	return "[dim]" .. label .. ": [/dim]" .. T.color(tostring(value), color_name or "white", mode)
end

-- Convenience: create a formatter pre-bound to a mode.
function T.for_mode(mode)
	mode = mode or "visual"
	return {
		color = function(s, c) return T.color(s, c, mode) end,
		bold = function(s) return T.bold(s, mode) end,
		dim = function(s) return T.dim(s, mode) end,
		underline = function(s) return T.underline(s, mode) end,
		bar = function(cur, max, w, label) return T.bar(cur, max, w, mode, label) end,
		header = function(title, w) return T.header(title, mode, w) end,
		divider = function(w) return T.divider(w, mode) end,
		table = function(rows, hdrs) return T.table(rows, hdrs, mode) end,
		box = function(lines, w) return T.box(lines, mode, w) end,
		stat = function(label, value, c) return T.stat(label, value, c, mode) end,
		strip_markup = T.strip_markup,
		wrap = str.wrap,
		pluralize = str.pluralize,
		mode = mode,
	}
end

return T
