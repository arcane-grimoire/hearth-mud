-- luau-signal by seaofvoices — MIT License
-- https://github.com/seaofvoices/luau-signal
-- Adapted: bundled into single file, task.spawn replaced with direct calls

-- Connection
local Connection = {}
local ConnectionMetatable = { __index = Connection }

function Connection.new(unsubscribe)
    return setmetatable({ _unsubscribe = unsubscribe }, ConnectionMetatable)
end

function Connection:disconnect()
    if self._unsubscribe == nil then
        error("attempt to disconnect a connection twice")
    end
    local unsubscribe = self._unsubscribe
    self._unsubscribe = nil
    unsubscribe()
end

function Connection:disconnectOnceFn()
    return function()
        if self._unsubscribe ~= nil then
            self:disconnect()
        end
    end
end

-- Signal
local Signal = {}
local SignalMetatable = { __index = Signal }

function Signal.new()
    return setmetatable({ _callbacks = {} }, SignalMetatable)
end

function Signal:fire(...)
    for _, callback in self._callbacks do
        callback(...)
    end
end

function Signal:connect(callback)
    table.insert(self._callbacks, callback)
    local callbacks = self._callbacks
    local function unsubscribe()
        local index = table.find(callbacks, callback)
        if index then
            table.remove(callbacks, index)
        end
    end
    return Connection.new(unsubscribe)
end

-- Subject (replays last value to new subscribers)
local Subject = setmetatable({}, { __index = Signal })
local SubjectMetatable = { __index = Subject }

function Subject.new(...)
    local self = Signal.new()
    self._last = { ... }
    self._lastLength = select("#", ...)
    return setmetatable(self, SubjectMetatable)
end

function Subject:fire(...)
    self._last = { ... }
    self._lastLength = select("#", ...)
    Signal.fire(self, ...)
end

function Subject:connect(callback)
    callback(unpack(self._last, 1, self._lastLength))
    return Signal.connect(self, callback)
end

return {
    new = Signal.new,
    newSubject = Subject.new,
    Connection = Connection,
}
