-- Synchronous finite state machine.
-- MIT License

local StateMachine = {}
StateMachine.__index = StateMachine

function StateMachine.new(config)
    local self = setmetatable({}, StateMachine)
    self._state = config.initial
    self._transitions = {}
    self._on_enter = config.on_enter or {}
    self._on_exit = config.on_exit or {}

    for _, t in ipairs(config.transitions or {}) do
        if not self._transitions[t.from] then
            self._transitions[t.from] = {}
        end
        self._transitions[t.from][t.event] = {
            to = t.to,
            guard = t.guard,
            action = t.action,
        }
    end

    return self
end

function StateMachine:state()
    return self._state
end

function StateMachine:can(event)
    local edges = self._transitions[self._state]
    if not edges then return false end
    local edge = edges[event]
    if not edge then return false end
    if edge.guard and not edge.guard(self._state) then return false end
    return true
end

function StateMachine:send(event, ...)
    local edges = self._transitions[self._state]
    if not edges then return false end
    local edge = edges[event]
    if not edge then return false end
    if edge.guard and not edge.guard(self._state) then return false end

    local old = self._state

    local exit_fn = self._on_exit[old]
    if exit_fn then exit_fn(old, event) end

    self._state = edge.to

    if edge.action then edge.action(old, self._state, ...) end

    local enter_fn = self._on_enter[self._state]
    if enter_fn then enter_fn(self._state, event) end

    return true
end

function StateMachine:is(state)
    return self._state == state
end

return StateMachine
