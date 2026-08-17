# Commands extend via cmd_ hooks on objects, not CmdSets

Player-typed commands resolve by searching for `cmd_` hooks on objects in the room and the player's inventory. There is no layer/CmdSet system for contextual command replacement.

We considered Evennia-style CmdSets (mergeable command bags that stack on a player) for situations like combat, but the edge cases are heavy: who owns the layer, where does the code live, how do conflicts resolve, do layers persist across disconnects. The `cmd_` hook pattern handles these naturally — combat commands live as `cmd_attack` etc. on a combat state object in the player's inventory. When combat ends, remove the object or its hooks. No new concepts needed.

If contextual command overrides become necessary later, a layer system can be added on top — `cmd_` resolution already searches a set of objects, and layers would just change which objects are searched.
