# Simple DSL for object locks, with future script escape hatch

Locks are string expressions evaluated against an AccessContext (actor, target object, room, account, actor's inventory). The built-in DSL covers common patterns:

```
traverse: perm(builder) OR has_tag(vip)
get: in_inventory(quest:key_holder)
enter: perm(admin)
```

Built-in functions: `perm(scope)`, `has_tag(spec)`, `has_attr(key)`, `has_attr(key, value)`, `in_inventory(tag_spec)`, `is_kind(kind)`, `time_between(start_hour, end_hour)`, `true`, `false`. Combinators: `AND`, `OR`, `NOT`.

Lock points the engine checks: traverse (exits), get, drop, enter (rooms), use, look, teleport.

We chose a DSL over Luau-evaluated locks because lock checks happen frequently (every movement, every interaction) and running the Luau VM for each would be expensive. A `can_` hook escape hatch (calling a Luau program for complex lock logic) is a planned future extension but not part of the initial implementation.
