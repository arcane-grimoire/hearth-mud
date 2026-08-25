mod object;
mod tag;

pub use object::{GameObject, Kind};
pub use tag::Tag;

use std::collections::{HashMap, HashSet};

/// Maximum archetype chain depth walked when resolving a delegated field or
/// hook. Guards a malformed chain — a cycle that somehow slipped past the
/// `apply_batch` guard, or an absurdly deep chain — against turning a resolve
/// into an infinite loop. MUD archetype chains are expected to be a handful
/// of levels deep at most (`goblin_chief -> goblin -> monster`), so this is
/// generous headroom, not a real limit.
pub const MAX_ARCHETYPE_DEPTH: usize = 32;

#[derive(Clone)]
pub struct World {
    pub objects: HashMap<String, GameObject>,
    pub next_id: u64,
    /// Bumped on every potential mutation (`add_object`, `remove_object`,
    /// `get_mut`). Derived indexes elsewhere (engine-side tick/global/troupe
    /// caches) compare their epoch against this to know when to rebuild.
    /// Deliberately conservative: `get_mut` may be called without an actual
    /// write, but a spurious rebuild is only a perf cost, never a bug.
    pub version: u64,
    /// Refs written via [`Self::get_mut`] / added via [`Self::add_object`]
    /// since the last drain — used for incremental persistence. Removals are
    /// recorded here with an empty entry (see [`Self::remove_object`]).
    /// Drained by the engine's save path (`db::save_world_delta`).
    pub(crate) dirty: HashMap<String, bool>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
            next_id: 0,
            version: 0,
            dirty: HashMap::new(),
        }
    }

    /// Allocate and return the next dbref, formatted as `"#N"`.
    pub fn next_dbref(&mut self) -> String {
        self.next_id += 1;
        format!("#{}", self.next_id)
    }

    pub fn add_object(&mut self, obj: GameObject) {
        self.version += 1;
        self.dirty.insert(obj.ref_id.clone(), true);
        self.objects.insert(obj.ref_id.clone(), obj);
    }

    /// Remove an object, recording the removal for incremental saves.
    pub fn remove_object(&mut self, ref_id: &str) -> Option<GameObject> {
        self.version += 1;
        self.dirty.insert(ref_id.to_string(), false);
        self.objects.remove(ref_id)
    }

    pub fn get(&self, ref_id: &str) -> Option<&GameObject> {
        self.objects.get(ref_id)
    }

    pub fn get_mut(&mut self, ref_id: &str) -> Option<&mut GameObject> {
        self.version += 1;
        if let Some(obj) = self.objects.get(ref_id) {
            self.dirty.insert(obj.ref_id.clone(), true);
        }
        self.objects.get_mut(ref_id)
    }

    /// Take the pending change set for incremental saves: refs to upsert
    /// (value `true`) and refs to delete (value `false`).
    pub fn drain_dirty(&mut self) -> HashMap<String, bool> {
        std::mem::take(&mut self.dirty)
    }

    pub fn exits_from(&self, room_ref: &str) -> Vec<&GameObject> {
        self.objects
            .values()
            .filter(|o| o.kind == Kind::Exit && o.location_ref.as_deref() == Some(room_ref))
            .collect()
    }

    pub fn find_exit(&self, room_ref: &str, direction: &str) -> Option<&GameObject> {
        self.objects.values().find(|o| {
            o.kind == Kind::Exit
                && o.location_ref.as_deref() == Some(room_ref)
                && o.matches_direction(direction)
        })
    }

    /// Objects located at `location_ref` — a room, an actor's inventory, or
    /// a container. Excludes `Exit` (navigation, not contents) and `Code`
    /// (never a physical thing — see [`Kind::Code`]) so every caller that
    /// builds room contents, inventory, or container listings gets the
    /// exclusion for free.
    pub fn objects_in(&self, location_ref: &str) -> Vec<&GameObject> {
        self.objects
            .values()
            .filter(|o| {
                o.location_ref.as_deref() == Some(location_ref)
                    && o.kind != Kind::Exit
                    && o.kind != Kind::Code
            })
            .collect()
    }

    // -- Archetype (is-a) resolution — see docs/plans/archetypes.md --
    //
    // An instance delegates unset fields (title, description, attrs) and its
    // script up its `archetype_ref` chain, but never its `state` (see
    // `softcode::hooks`, which handles script/hook resolution). These
    // resolvers are the one mechanism both the engine's native reads and the
    // Lua object snapshot (`softcode::api::object_to_table`) go through, so
    // "does this field delegate" never drifts between the two.

    /// `obj`'s ancestors, nearest first, walking `archetype_ref` upward.
    /// Bounded and cycle-safe: a chain longer than [`MAX_ARCHETYPE_DEPTH`] or
    /// one that revisits a ref (a malformed chain that slipped past the
    /// `apply_batch` cycle guard) stops rather than looping forever.
    pub(crate) fn archetype_ancestors(&self, obj: &GameObject) -> Vec<&GameObject> {
        let mut out = Vec::new();
        let mut next = obj.archetype_ref.clone();
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(obj.ref_id.clone());
        while let Some(aref) = next {
            if !visited.insert(aref.clone()) || visited.len() > MAX_ARCHETYPE_DEPTH {
                break;
            }
            let Some(anc) = self.get(&aref) else { break };
            out.push(anc);
            next = anc.archetype_ref.clone();
        }
        out
    }

    /// Whether setting `target`'s `archetype_ref` to `candidate` would create
    /// a cycle: `candidate` is `target` itself, or `target` already appears
    /// in `candidate`'s (current) ancestor chain. Used by `apply_batch` —
    /// callers should check this *before* writing the new `archetype_ref`.
    pub fn would_cycle_archetype(&self, target: &str, candidate: &str) -> bool {
        if candidate == target {
            return true;
        }
        let mut next = Some(candidate.to_string());
        let mut visited: HashSet<String> = HashSet::new();
        while let Some(cur) = next {
            if !visited.insert(cur.clone()) || visited.len() > MAX_ARCHETYPE_DEPTH {
                break;
            }
            if cur == target {
                return true;
            }
            next = self.get(&cur).and_then(|o| o.archetype_ref.clone());
        }
        false
    }

    /// Whether any object's `archetype_ref` points at `ref_id` — i.e.
    /// whether `ref_id` is an archetype with live instances. Used to refuse
    /// deleting an archetype out from under its instances.
    pub fn has_archetype_instances(&self, ref_id: &str) -> bool {
        self.objects
            .values()
            .any(|o| o.archetype_ref.as_deref() == Some(ref_id))
    }

    /// `obj`'s title if it has one, else the first ancestor's, walking up the
    /// archetype chain. `None` if nothing in the chain has a title.
    pub fn resolved_title(&self, obj: &GameObject) -> Option<String> {
        if let Some(t) = &obj.title {
            return Some(t.clone());
        }
        self.archetype_ancestors(obj)
            .into_iter()
            .find_map(|anc| anc.title.clone())
    }

    /// The display name an instance shows — its own title, else the nearest
    /// ancestor's, else its own key (identity is never inherited, only
    /// behavior and defaults are). The archetype-aware counterpart to
    /// [`GameObject::display_name`], which cannot resolve the chain because
    /// it has no `World` to walk it with.
    pub fn display_name(&self, obj: &GameObject) -> String {
        self.resolved_title(obj).unwrap_or_else(|| obj.key.clone())
    }

    /// `obj`'s description if non-empty, else the first ancestor's non-empty
    /// description, walking up the archetype chain. Empty string if nothing
    /// in the chain has one — same fallback as the raw field.
    pub fn resolved_description(&self, obj: &GameObject) -> String {
        if !obj.description.is_empty() {
            return obj.description.clone();
        }
        self.archetype_ancestors(obj)
            .into_iter()
            .find_map(|anc| (!anc.description.is_empty()).then(|| anc.description.clone()))
            .unwrap_or_default()
    }

    /// `obj`'s own value for `key` if it has one, else the first ancestor's,
    /// walking up the archetype chain. Writing an attr on an instance always
    /// sets it on the instance — this only affects reads.
    pub fn resolved_attr<'a>(&'a self, obj: &'a GameObject, key: &str) -> Option<&'a serde_json::Value> {
        if let Some(v) = obj.attrs.get(key) {
            return Some(v);
        }
        self.archetype_ancestors(obj)
            .into_iter()
            .find_map(|anc| anc.attrs.get(key))
    }

    /// The full merged attr map: every key reachable anywhere in the
    /// archetype chain, with a nearer definition (the instance's own, then
    /// its nearest ancestor, ...) winning over a farther one. Used where the
    /// whole set matters (attr iteration, `clone`/`detach`'s "copy the
    /// resolved fields onto the object") rather than a single-key lookup.
    pub fn resolved_attrs(&self, obj: &GameObject) -> HashMap<String, serde_json::Value> {
        let mut merged: HashMap<String, serde_json::Value> = HashMap::new();
        let mut ancestors = self.archetype_ancestors(obj);
        ancestors.reverse(); // farthest first, so nearer inserts overwrite
        for anc in ancestors {
            merged.extend(anc.attrs.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        merged.extend(obj.attrs.iter().map(|(k, v)| (k.clone(), v.clone())));
        merged
    }

    /// The union of `obj`'s own tags with every ancestor's, up the archetype
    /// chain. Tags are additive-only — there is no per-instance "clear an
    /// inherited tag" in Stage 1 (that's `clear_attr`'s Stage 2 sibling, per
    /// docs/plans/archetypes.md), so this is a plain union rather than an
    /// override-per-key merge like `resolved_attrs`.
    pub fn resolved_tags(&self, obj: &GameObject) -> HashSet<Tag> {
        let mut tags: HashSet<Tag> = obj.tags.clone();
        for anc in self.archetype_ancestors(obj) {
            tags.extend(anc.tags.iter().cloned());
        }
        tags
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Kind::Code` objects are never physical things — a global script or
    /// a library must never show up in room contents, inventory, or a
    /// container, no matter how its `location_ref` ends up set. This is the
    /// single choke point every such caller (`do_look`, `do_inventory`,
    /// `get`, container listings, the web client's Room message, ...)
    /// relies on — see docs/plans/program-authoring.md Stage 2.
    #[test]
    fn objects_in_excludes_code_objects() {
        let mut world = World::new();
        let room_ref = world.next_dbref();
        world.add_object(GameObject::new(&room_ref, "room", Kind::Room));

        let item_ref = world.next_dbref();
        world.add_object(
            GameObject::new(&item_ref, "sword", Kind::Item).with_location(&room_ref),
        );

        // A Code object should never appear in objects_in, even if
        // something mistakenly gives it a location_ref matching a room.
        let code_ref = world.next_dbref();
        world.add_object(
            GameObject::new(&code_ref, "weather", Kind::Code).with_location(&room_ref),
        );

        let contents = world.objects_in(&room_ref);
        let refs: Vec<&str> = contents.iter().map(|o| o.ref_id.as_str()).collect();
        assert!(refs.contains(&item_ref.as_str()), "ordinary item should still be listed");
        assert!(!refs.contains(&code_ref.as_str()), "Code object must be excluded");
    }

    #[test]
    fn dirty_tracking_records_writes_and_removals() {
        let mut w = World::new();
        let obj = GameObject::new("#1", "rock", Kind::Item);
        w.add_object(obj.clone());
        assert_eq!(w.drain_dirty().get("#1"), Some(&true));

        // Clean after drain.
        assert!(w.drain_dirty().is_empty());

        w.get_mut("#1").unwrap().title = Some("a rock".into());
        let dirty = w.drain_dirty();
        assert_eq!(dirty.get("#1"), Some(&true));

        w.remove_object("#1");
        let dirty = w.drain_dirty();
        assert_eq!(dirty.get("#1"), Some(&false));

        // Unknown ref: no dirty entry, no panic.
        assert!(w.get_mut("#999").is_none());
        assert!(w.drain_dirty().is_empty());
    }

    // -- Archetype (is-a) resolution — docs/plans/archetypes.md Stage 1 --

    /// An instance with no title/description/attr of its own resolves them
    /// from its archetype; setting one on the instance shadows the
    /// archetype's (copy-on-write, per the plan's Decision 2).
    #[test]
    fn resolved_fields_fall_through_to_the_archetype_and_instance_overrides_shadow() {
        let mut world = World::new();
        let archetype_ref = world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc)
            .with_title("Goblin")
            .with_description("A snarling goblin.");
        archetype.attrs.insert("max_hp".into(), serde_json::json!(10));
        world.add_object(archetype);

        let instance_ref = world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc);
        instance.archetype_ref = Some(archetype_ref.clone());
        world.add_object(instance);

        let instance = world.get(&instance_ref).unwrap();
        assert_eq!(world.resolved_title(instance), Some("Goblin".to_string()));
        assert_eq!(world.display_name(instance), "Goblin");
        assert_eq!(world.resolved_description(instance), "A snarling goblin.");
        assert_eq!(
            world.resolved_attr(instance, "max_hp"),
            Some(&serde_json::json!(10))
        );
        assert_eq!(world.resolved_attr(instance, "no_such_attr"), None);

        // Override on the instance shadows the archetype — copy-on-write,
        // instance-first.
        world.get_mut(&instance_ref).unwrap().title = Some("Grubnak".into());
        world
            .get_mut(&instance_ref)
            .unwrap()
            .attrs
            .insert("max_hp".into(), serde_json::json!(6));
        let instance = world.get(&instance_ref).unwrap();
        assert_eq!(world.resolved_title(instance), Some("Grubnak".to_string()));
        assert_eq!(world.display_name(instance), "Grubnak");
        assert_eq!(
            world.resolved_attr(instance, "max_hp"),
            Some(&serde_json::json!(6))
        );
        // The archetype itself is untouched by the instance's override.
        let archetype = world.get(&archetype_ref).unwrap();
        assert_eq!(
            world.resolved_attr(archetype, "max_hp"),
            Some(&serde_json::json!(10))
        );
    }

    /// A chain deeper than one level resolves correctly
    /// (`goblin_chief -> goblin -> monster`), and `display_name` falls back
    /// to the instance's own key when nothing in the chain has a title.
    #[test]
    fn resolved_title_walks_a_multi_level_chain() {
        let mut world = World::new();
        let monster_ref = world.next_dbref();
        world.add_object(GameObject::new(&monster_ref, "monster", Kind::Npc));

        let goblin_ref = world.next_dbref();
        let mut goblin = GameObject::new(&goblin_ref, "goblin", Kind::Npc).with_title("Goblin");
        goblin.archetype_ref = Some(monster_ref.clone());
        world.add_object(goblin);

        let chief_ref = world.next_dbref();
        let mut chief = GameObject::new(&chief_ref, "goblin_chief", Kind::Npc);
        chief.archetype_ref = Some(goblin_ref.clone());
        world.add_object(chief);

        let chief = world.get(&chief_ref).unwrap();
        assert_eq!(world.resolved_title(chief), Some("Goblin".to_string()));

        // Nothing anywhere in the chain has a title: falls back to the
        // instance's own key (never an ancestor's key — identity isn't
        // inherited).
        let monster = world.get(&monster_ref).unwrap();
        assert_eq!(world.resolved_title(monster), None);
        assert_eq!(world.display_name(monster), "monster");
    }

    /// `resolved_attrs` merges the whole chain, nearer values winning —
    /// used by `clone`/`detach` and attr iteration.
    #[test]
    fn resolved_attrs_merges_the_chain_nearest_wins() {
        let mut world = World::new();
        let archetype_ref = world.next_dbref();
        let mut archetype = GameObject::new(&archetype_ref, "goblin", Kind::Npc);
        archetype.attrs.insert("max_hp".into(), serde_json::json!(10));
        archetype.attrs.insert("defense".into(), serde_json::json!(3));
        world.add_object(archetype);

        let instance_ref = world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc);
        instance.archetype_ref = Some(archetype_ref.clone());
        instance.attrs.insert("max_hp".into(), serde_json::json!(6)); // override
        instance.attrs.insert("name_tag".into(), serde_json::json!("Grubnak")); // own-only
        world.add_object(instance);

        let instance = world.get(&instance_ref).unwrap();
        let merged = world.resolved_attrs(instance);
        assert_eq!(merged.get("max_hp"), Some(&serde_json::json!(6)));
        assert_eq!(merged.get("defense"), Some(&serde_json::json!(3)));
        assert_eq!(merged.get("name_tag"), Some(&serde_json::json!("Grubnak")));
    }

    /// The cycle guard `apply_batch` relies on: `candidate` is `target`
    /// itself, or `target` already sits somewhere in `candidate`'s current
    /// ancestor chain.
    #[test]
    fn would_cycle_archetype_detects_self_and_indirect_cycles() {
        let mut world = World::new();
        let a_ref = world.next_dbref();
        world.add_object(GameObject::new(&a_ref, "a", Kind::Npc));
        let b_ref = world.next_dbref();
        let mut b = GameObject::new(&b_ref, "b", Kind::Npc);
        b.archetype_ref = Some(a_ref.clone());
        world.add_object(b);
        let c_ref = world.next_dbref();
        let mut c = GameObject::new(&c_ref, "c", Kind::Npc);
        c.archetype_ref = Some(b_ref.clone());
        world.add_object(c);
        // Chain so far: c -> b -> a

        // Self-parenting is always a cycle.
        assert!(world.would_cycle_archetype(&a_ref, &a_ref));

        // Setting a's archetype to c would close c -> b -> a -> c.
        assert!(world.would_cycle_archetype(&a_ref, &c_ref));

        // Setting a's archetype to b is also a cycle (b -> a -> b).
        assert!(world.would_cycle_archetype(&a_ref, &b_ref));

        // Setting c's archetype to a is fine — shortens the chain, no cycle
        // (a doesn't appear anywhere in a's own chain).
        assert!(!world.would_cycle_archetype(&c_ref, &a_ref));
    }

    #[test]
    fn has_archetype_instances_finds_live_dependents() {
        let mut world = World::new();
        let archetype_ref = world.next_dbref();
        world.add_object(GameObject::new(&archetype_ref, "goblin", Kind::Npc));
        assert!(!world.has_archetype_instances(&archetype_ref));

        let instance_ref = world.next_dbref();
        let mut instance = GameObject::new(&instance_ref, "goblin1", Kind::Npc);
        instance.archetype_ref = Some(archetype_ref.clone());
        world.add_object(instance);

        assert!(world.has_archetype_instances(&archetype_ref));
        assert!(!world.has_archetype_instances(&instance_ref));
    }
}
