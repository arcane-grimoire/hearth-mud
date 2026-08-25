-- Hearth MUD → Mudlet mapper bridge.
--
-- Drives Mudlet's built-in mapper from the GMCP `Room.Info` package a Hearth
-- server sends on look/movement. Paste this whole file into a Mudlet Script
-- (Scripts → Add Item), save, then reconnect. Open the mapper (Toolbox →
-- Mapper) and walk around — rooms appear and link as you move.
--
-- Room.Info payload shape (from src/net/telnet.rs):
--   { num, name, area?, map?, environment?, coords?:{x,y}, exits:{<dir>:<num>} }
-- `num`/exit targets are Hearth dbrefs like "#42"; we map "#42" → integer 42.

-- Long name → Mudlet short direction. Unknown dirs pass through unchanged
-- (Mudlet also accepts custom exit names).
local DIRS = {
  north = "n", south = "s", east = "e", west = "w",
  up = "up", down = "down",
  northeast = "ne", northwest = "nw", southeast = "se", southwest = "sw",
  ["in"] = "in", out = "out",
}

local function toId(ref)
  if ref == nil or ref == "" then return nil end
  return tonumber((tostring(ref):gsub("#", "")))
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
if hearthMapperHandler then killAnonymousEventHandler(hearthMapperHandler) end
hearthMapperHandler = registerAnonymousEventHandler("gmcp.Room.Info", "hearthRoomInfo")
