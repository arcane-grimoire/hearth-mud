-- Hearth MUD → Mudlet mapper bridge.
--
-- Drives Mudlet's built-in mapper from the GMCP packages a Hearth server
-- sends. Paste this whole file into a Mudlet Script (Scripts → Add Item),
-- save, then reconnect. Open the mapper (Toolbox → Mapper) and walk around —
-- rooms appear, link, and (in mapped areas) paint by terrain as you move.
--
-- Room.Info payload (per look/move, from src/net/telnet.rs):
--   { num, name, area?, map?, environment?, coords?:{x,y}, exits:{<dir>:<num>} }
-- Terrain.Legend payload (once per map on entry):
--   { map, terrains: { "<char>": { env_id, color:"#rrggbb", passable, … } } }
-- `num`/exit targets are Hearth dbrefs like "#42"; we map "#42" → integer 42.

-- Long name → Mudlet short direction. Unknown dirs pass through unchanged
-- (Mudlet also accepts custom exit names).
local DIRS = {
  north = "n", south = "s", east = "e", west = "w",
  up = "up", down = "down",
  northeast = "ne", northwest = "nw", southeast = "se", southwest = "sw",
  ["in"] = "in", out = "out",
}

-- Terrain char → env_id, populated from Terrain.Legend. Lets Room.Info paint a
-- room by its `environment` char once the legend for that map has arrived.
hearthEnv = hearthEnv or {}

local function toId(ref)
  if ref == nil or ref == "" then return nil end
  return tonumber((tostring(ref):gsub("#", "")))
end

local function hexToRGB(hex)
  if type(hex) ~= "string" then return nil end
  hex = hex:gsub("#", "")
  if #hex ~= 6 then return nil end
  return tonumber(hex:sub(1, 2), 16),
         tonumber(hex:sub(3, 4), 16),
         tonumber(hex:sub(5, 6), 16)
end

-- Register each terrain's color as a custom Mudlet environment, and remember
-- the char → env_id map so rooms can be assigned their environment.
function hearthTerrainLegend()
  local leg = gmcp and gmcp.Terrain and gmcp.Terrain.Legend
  if not leg or not leg.terrains then return end
  for char, t in pairs(leg.terrains) do
    if t.env_id then
      hearthEnv[char] = t.env_id
      local r, g, b = hexToRGB(t.color)
      if r then setCustomEnvColor(t.env_id, r, g, b, 255) end
    end
  end
end

function hearthRoomInfo()
  local info = gmcp and gmcp.Room and gmcp.Room.Info
  if not info then return end
  local id = toId(info.num)
  if not id then return end

  if not roomExists(id) then addRoom(id) end
  setRoomName(id, info.name or ("room " .. id))

  -- Group rooms into a Mudlet area matching the game area.
  if info.area and info.area ~= "" then
    local areas = getAreaTable() or {}
    local aid = areas[info.area] or addAreaName(info.area)
    if aid and aid > 0 then setRoomArea(id, aid) end
  end

  -- Paint the room by terrain, if the legend for this map has been received.
  local env = info.environment and hearthEnv[info.environment]
  if env then setRoomEnv(id, env) end

  -- Absolute grid coordinates for map-instantiated rooms; hand-authored rooms
  -- carry none, so Mudlet lays them out from their exits instead.
  if info.coords and info.coords.x and info.coords.y then
    setRoomCoordinates(id, info.coords.x, info.coords.y, 0)
  end

  if info.exits then
    for dir, target in pairs(info.exits) do
      local d = DIRS[tostring(dir):lower()] or dir
      local tid = toId(target)
      if tid then
        if not roomExists(tid) then addRoom(tid) end
        setExit(id, tid, d)
      else
        -- No known destination yet — leave a stub so the exit is visible; it
        -- resolves once the player walks through and that room reports in.
        setExitStub(id, d, true)
      end
    end
  end

  centerview(id)
  updateMap()
end

-- Re-register cleanly so saving the script twice doesn't stack handlers.
if hearthRoomHandler then killAnonymousEventHandler(hearthRoomHandler) end
if hearthLegendHandler then killAnonymousEventHandler(hearthLegendHandler) end
hearthRoomHandler = registerAnonymousEventHandler("gmcp.Room.Info", "hearthRoomInfo")
hearthLegendHandler = registerAnonymousEventHandler("gmcp.Terrain.Legend", "hearthTerrainLegend")
