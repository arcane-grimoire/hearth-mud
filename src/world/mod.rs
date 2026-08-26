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
    /// `get_mut`, `relocate`). This is the *save/dirty* epoch: it drives
    /// incremental persistence, and being conservative here only ever costs a
    /// redundant save, never correctness.
    pub version: u64,
    /// Bumped only when a *structurally* relevant change happens — an object
    /// added or removed, or one of the fields the engine's derived indexes
    /// read (tags, script/hooks, archetype, the `tick_interval` attr). A plain
    /// attribute write or a [`Self::relocate`] does **not** bump it, so the
    /// engine's `DerivedIndexes` (tickables / globals-by-hook / troupes) cache
    /// survives ordinary gameplay mutation instead of being rebuilt on every
    /// write. Correctness rule: every mutation that could change what those
    /// indexes contain must call [`Self::bump_struct`]. Location is handled by
    /// the separate `children` index and is deliberately *not* structural.
    pub struct_version: u64,
    /// Refs written via [`Self::get_mut`] / added via [`Self::add_object`]
    /// since the last drain — used for incremental persistence. Removals are
    /// recorded here with an empty entry (see [`Self::remove_object`]).
    /// Drained by the engine's save path (`db::save_world_delta`).
    pub(crate) dirty: HashMap<String, bool>,
    /// Location → the refs contained there, for *every* kind (rooms hold
    /// items/npcs/exits, actors hold inventory, containers hold contents).
    /// Maintained incrementally by [`Self::add_object`], [`Self::remove_object`],
    /// and [`Self::relocate`] so `objects_in` / `exits_from` / `find_exit`
    /// answer in O(occupants) instead of scanning every object in the world.
    /// Objects with no `location_ref` are absent from every bucket.
    children: HashMap<String, HashSet<String>>,
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
            struct_version: 0,
            dirty: HashMap::new(),
            children: HashMap::new(),
        }
    }

    /// Allocate and return the next dbref, formatted as `"#N"`.
    pub fn next_dbref(&mut self) -> String {
        self.next_id += 1;
        format!("#{}", self.next_id)
    }

    pub fn add_object(&mut self, obj: GameObject) {
        self.version += 1;
        self.struct_version += 1;
        self.dirty.insert(obj.ref_id.clone(), true);
        self.index_insert(&obj.ref_id, obj.location_ref.as_deref());
        self.objects.insert(obj.ref_id.clone(), obj);
    }

    /// Remove an object, recording the removal for incremental saves.
    pub fn remove_object(&mut self, ref_id: &str) -> Option<GameObject> {
        self.version += 1;
        self.struct_version += 1;
        self.dirty.insert(ref_id.to_string(), false);
        let removed = self.objects.remove(ref_id);
        if let Some(obj) = &removed {
            self.index_remove(ref_id, obj.location_ref.as_deref());
        }
        removed
    }

    pub fn get(&self, ref_id: &str) -> Option<&GameObject> {
        self.objects.get(ref_id)
    }

    /// Mutable access. Marks the object dirty for the next save. Does **not**
    /// touch `struct_version` or the `children` index: a caller changing a
    /// structural field must call [`Self::bump_struct`], and a caller changing
    /// location must go through [`Self::relocate`] rather than assigning
    /// `location_ref` directly (a direct assignment silently desyncs the
    /// children index — see the `children_index_survives_*` tests).
    pub fn get_mut(&mut self, ref_id: &str) -> Option<&mut GameObject> {
        self.version += 1;
        if let Some(obj) = self.objects.get(ref_id) {
            self.dirty.insert(obj.ref_id.clone(), true);
        }
        self.objects.get_mut(ref_id)
    }

    /// Signal that a structural field the engine's derived indexes read
    /// (tags, script/hooks, archetype, `tick_interval`) has changed on some
    /// object, so the `DerivedIndexes` cache rebuilds on next access. Call
    /// this alongside the `get_mut` that made such a change.
    pub fn bump_struct(&mut self) {
        self.struct_version += 1;
    }

    /// The structural epoch the engine's `DerivedIndexes` compares against.
    pub fn struct_version(&self) -> u64 {
        self.struct_version
    }

    /// Move `ref_id` to `new_loc` (or nowhere, if `None`), keeping the
    /// `children` index in sync. This is the *only* correct way to change an
    /// object's `location_ref` once it is in the world. Bumps `version` (for
    /// the save delta) but not `struct_version` — a move never changes what the
    /// derived indexes contain.
    pub fn relocate(&mut self, ref_id: &str, new_loc: Option<String>) {
        let old = self.objects.get(ref_id).and_then(|o| o.location_ref.clone());
        if old.as_deref() == new_loc.as_deref() {
            return;
        }
        self.version += 1;
        self.dirty.insert(ref_id.to_string(), true);
        self.index_remove(ref_id, old.as_deref());
        self.index_insert(ref_id, new_loc.as_deref());
        if let Some(obj) = self.objects.get_mut(ref_id) {
            obj.location_ref = new_loc;
        }
    }

    /// Add a tag to an object, bumping the structural epoch (a `system:global`
    /// or `troupe:*` tag changes what the derived indexes contain). Returns
    /// false if the ref doesn't resolve. Prefer this over `get_mut().tags`.
    pub fn add_tag(&mut self, ref_id: &str, tag: Tag) -> bool {
        self.struct_version += 1;
        match self.get_mut(ref_id) {
            Some(obj) => {
                obj.tags.insert(tag);
                true
            }
            None => false,
        }
    }

    /// Remove a tag from an object, bumping the structural epoch. Returns
    /// whether the tag was present.
    pub fn remove_tag(&mut self, ref_id: &str, tag: &Tag) -> bool {
        self.struct_version += 1;
        self.get_mut(ref_id).is_some_and(|obj| obj.tags.remove(tag))
    }

    /// Set (or clear) an object's `archetype_ref`, bumping the structural
    /// epoch — the archetype chain decides which hooks/tags an object resolves.
    pub fn set_object_archetype(&mut self, ref_id: &str, archetype: Option<String>) -> bool {
        self.struct_version += 1;
        match self.get_mut(ref_id) {
            Some(obj) => {
                obj.archetype_ref = archetype;
                true
            }
            None => false,
        }
    }

    fn index_insert(&mut self, ref_id: &str, loc: Option<&str>) {
        if let Some(l) = loc {
            self.children.entry(l.to_string()).or_default().insert(ref_id.to_string());
        }
    }

    fn index_remove(&mut self, ref_id: &str, loc: Option<&str>) {
        if let Some(l) = loc
            && let Some(set) = self.children.get_mut(l)
        {
            set.remove(ref_id);
            if set.is_empty() {
                self.children.remove(l);
            }
        }
    }

    /// Rebuild the `children` index from scratch. Only needed if an object's
    /// `location_ref` was changed by bypassing [`Self::relocate`]; kept for the
    /// consistency tests and as a repair hatch, not the hot path.
    pub fn rebuild_children_index(&mut self) {
        let mut children: HashMap<String, HashSet<String>> = HashMap::new();
        for obj in self.objects.values() {
            if let Some(l) = &obj.location_ref {
                children.entry(l.clone()).or_default().insert(obj.ref_id.clone());
            }
        }
        self.children = children;
    }

    /// Refs of every object located at `location_ref`, any kind. The backing
    /// store for the kind-filtered public queries below.
    fn children_of(&self, location_ref: &str) -> impl Iterator<Item = &GameObject> {
        self.children
            .get(location_ref)
            .into_iter()
            .flat_map(|set| set.iter())
            .filter_map(|ref_id| self.objects.get(ref_id))
    }

    /// Take the pending change set for incremental saves: refs to upsert
    /// (value `true`) and refs to delete (value `false`).
    pub fn drain_dirty(&mut self) -> HashMap<String, bool> {
        std::mem::take(&mut self.dirty)
    }

    pub fn exits_from(&self, room_ref: &str) -> Vec<&GameObject> {
        self.children_of(room_ref)
            .filter(|o| o.kind == Kind::Exit)
            .collect()
    }

    pub fn find_exit(&self, room_ref: &str, direction: &str) -> Option<&GameObject> {
        self.children_of(room_ref)
            .find(|o| o.kind == Kind::Exit && o.matches_direction(direction))
    }

    /// Objects located at `location_ref` — a room, an actor's inventory, or
    /// a container. Excludes `Exit` (navigation, not contents) and `Code`
    /// (never a physical thing — see [`Kind::Code`]) so every caller that
    /// builds room contents, inventory, or container listings gets the
    /// exclusion for free.
    pub fn objects_in(&self, location_ref: &str) -> Vec<&GameObject> {
        self.children_of(location_ref)
            .filter(|o| o.kind != Kind::Exit && o.kind != Kind::Code)
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
    /// The effective attribute schema: `obj`'s own declared descriptors first,
    /// then each archetype ancestor (nearest first) contributes any `key` not
    /// already declared — nearest-wins per key, mirroring [`Self::resolved_attrs`].
    /// Each descriptor is paired with its source: `"own"` or the ancestor ref it
    /// is inherited from. So an archetype declares `hp`/`attack` once and every
    /// instance renders the same typed fields.
    pub fn resolved_attr_schema(
        &self,
        obj: &GameObject,
    ) -> Vec<(crate::attr_schema::AttrDescriptor, String)> {
        let mut out: Vec<(crate::attr_schema::AttrDescriptor, String)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for d in &obj.attr_schema {
            if seen.insert(d.key.clone()) {
                out.push((d.clone(), "own".to_string()));
            }
        }
        for anc in self.archetype_ancestors(obj) {
            for d in &anc.attr_schema {
                if seen.insert(d.key.clone()) {
                    out.push((d.clone(), anc.ref_id.clone()));
                }
            }
        }
        out
    }

    pub fn resolved_tags(&self, obj: &GameObject) -> HashSet<Tag> {
        let mut tags: HashSet<Tag> = obj.tags.clone();
        for anc in self.archetype_ancestors(obj) {
            // `system:locked` is an OWN-only lock (see `loader::stamp_locked`
            // and `Engine::is_object_locked`): a locked base must not make its
            // subtypes or instances appear or behave as locked. Enforcement
            // already checks own tags only; excluding it here keeps every
            // display/query surface (examine, `find_by_tag`, the builder's
            // inherited-tags) consistent with that. Every other tag inherits.
            tags.extend(
                anc.tags
                    .iter()
                    .filter(|t| !(t.category == "system" && t.key == "locked"))
                    .cloned(),
            );
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

    // -- Children (location) index — F1: O(occupants) spatial queries --

    /// Brute-force reference: the refs a full-world scan says are located at
    /// `loc`, matching the semantics the incremental `children` index must
    /// preserve exactly.
    fn scan_children(world: &World, loc: &str) -> HashSet<String> {
        world
            .objects
            .values()
            .filter(|o| o.location_ref.as_deref() == Some(loc))
            .map(|o| o.ref_id.clone())
            .collect()
    }

    fn indexed_children(world: &World, loc: &str) -> HashSet<String> {
        world.children_of(loc).map(|o| o.ref_id.clone()).collect()
    }

    #[test]
    fn children_index_matches_a_full_scan_through_add_move_and_remove() {
        let mut world = World::new();
        let room_a = world.next_dbref();
        let room_b = world.next_dbref();
        world.add_object(GameObject::new(&room_a, "a", Kind::Room));
        world.add_object(GameObject::new(&room_b, "b", Kind::Room));

        let sword = world.next_dbref();
        world.add_object(GameObject::new(&sword, "sword", Kind::Item).with_location(&room_a));
        let goblin = world.next_dbref();
        world.add_object(GameObject::new(&goblin, "goblin", Kind::Npc).with_location(&room_a));

        // add: both are in room A by the index and by a scan.
        assert_eq!(indexed_children(&world, &room_a), scan_children(&world, &room_a));
        assert_eq!(indexed_children(&world, &room_a).len(), 2);

        // move: the sword goes to room B.
        world.relocate(&sword, Some(room_b.clone()));
        assert_eq!(indexed_children(&world, &room_a), scan_children(&world, &room_a));
        assert_eq!(indexed_children(&world, &room_b), scan_children(&world, &room_b));
        assert!(indexed_children(&world, &room_b).contains(&sword));

        // move to nowhere: the goblin leaves the world's locations entirely.
        world.relocate(&goblin, None);
        assert_eq!(indexed_children(&world, &room_a), scan_children(&world, &room_a));
        assert!(indexed_children(&world, &room_a).is_empty());

        // remove: the sword vanishes from room B's bucket.
        world.remove_object(&sword);
        assert_eq!(indexed_children(&world, &room_b), scan_children(&world, &room_b));
        assert!(indexed_children(&world, &room_b).is_empty());
    }

    #[test]
    fn rebuild_children_index_reproduces_the_incremental_one() {
        let mut world = World::new();
        let room = world.next_dbref();
        world.add_object(GameObject::new(&room, "room", Kind::Room));
        for i in 0..5 {
            let r = world.next_dbref();
            world.add_object(
                GameObject::new(&r, &format!("item{i}"), Kind::Item).with_location(&room),
            );
        }
        let before = indexed_children(&world, &room);
        world.rebuild_children_index();
        assert_eq!(indexed_children(&world, &room), before);
        assert_eq!(indexed_children(&world, &room), scan_children(&world, &room));
    }

    #[test]
    fn objects_in_and_exits_from_agree_with_a_scan() {
        let mut world = World::new();
        let room = world.next_dbref();
        let other = world.next_dbref();
        world.add_object(GameObject::new(&room, "room", Kind::Room));
        world.add_object(GameObject::new(&other, "other", Kind::Room));
        let item = world.next_dbref();
        world.add_object(GameObject::new(&item, "sword", Kind::Item).with_location(&room));
        let code = world.next_dbref();
        world.add_object(GameObject::new(&code, "weather", Kind::Code).with_location(&room));
        let exit = world.next_dbref();
        world.add_object(
            GameObject::new(&exit, "north", Kind::Exit)
                .with_location(&room)
                .with_target(&other)
                .with_aliases(vec!["n"]),
        );

        // objects_in excludes Exit + Code; exits_from is only exits.
        let contents: HashSet<&str> = world.objects_in(&room).iter().map(|o| o.ref_id.as_str()).collect();
        assert_eq!(contents, HashSet::from([item.as_str()]));
        let exits: HashSet<&str> = world.exits_from(&room).iter().map(|o| o.ref_id.as_str()).collect();
        assert_eq!(exits, HashSet::from([exit.as_str()]));

        // find_exit resolves by direction and alias.
        assert_eq!(world.find_exit(&room, "north").map(|o| o.ref_id.clone()), Some(exit.clone()));
        assert_eq!(world.find_exit(&room, "n").map(|o| o.ref_id.clone()), Some(exit.clone()));
        assert!(world.find_exit(&room, "south").is_none());
    }

    // -- Structural epoch — F2: derived indexes survive ordinary mutation --

    #[test]
    fn a_move_bumps_version_but_not_the_structural_epoch() {
        let mut world = World::new();
        let room_a = world.next_dbref();
        let room_b = world.next_dbref();
        world.add_object(GameObject::new(&room_a, "a", Kind::Room));
        world.add_object(GameObject::new(&room_b, "b", Kind::Room));
        let item = world.next_dbref();
        world.add_object(GameObject::new(&item, "sword", Kind::Item).with_location(&room_a));

        let struct_before = world.struct_version();
        let ver_before = world.version;
        world.relocate(&item, Some(room_b.clone()));
        assert!(world.version > ver_before, "a move must dirty the object for saving");
        assert_eq!(
            world.struct_version(),
            struct_before,
            "a move must NOT invalidate the derived indexes"
        );
    }

    #[test]
    fn a_plain_attr_write_does_not_bump_the_structural_epoch() {
        let mut world = World::new();
        let item = world.next_dbref();
        world.add_object(GameObject::new(&item, "sword", Kind::Item));
        let struct_before = world.struct_version();
        world.get_mut(&item).unwrap().attrs.insert("sharp".into(), serde_json::json!(true));
        assert_eq!(world.struct_version(), struct_before);
    }

    #[test]
    fn tag_and_archetype_changes_bump_the_structural_epoch() {
        let mut world = World::new();
        let item = world.next_dbref();
        world.add_object(GameObject::new(&item, "sword", Kind::Item));

        let e0 = world.struct_version();
        world.add_tag(&item, Tag { category: "system".into(), key: "global".into() });
        let e1 = world.struct_version();
        assert!(e1 > e0, "adding a tag must invalidate the derived indexes");

        world.remove_tag(&item, &Tag { category: "system".into(), key: "global".into() });
        let e2 = world.struct_version();
        assert!(e2 > e1, "removing a tag must invalidate the derived indexes");

        let base = world.next_dbref();
        world.add_object(GameObject::new(&base, "base", Kind::Item));
        let e3 = world.struct_version();
        world.set_object_archetype(&item, Some(base.clone()));
        assert!(world.struct_version() > e3, "reparenting must invalidate the derived indexes");
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

    /// `system:locked` is an own-only lock: a locked base must not make its
    /// subtypes/instances appear locked via `resolved_tags` (which every
    /// display/query surface uses). Every other tag still inherits, and an
    /// object's OWN `system:locked` is kept.
    #[test]
    fn resolved_tags_does_not_inherit_system_locked() {
        let locked = Tag { category: "system".into(), key: "locked".into() };
        let global = Tag { category: "system".into(), key: "global".into() };

        let mut world = World::new();
        let base_ref = world.next_dbref();
        let mut base = GameObject::new(&base_ref, "monster", Kind::Npc);
        base.tags.insert(locked.clone());
        base.tags.insert(global.clone());
        world.add_object(base);

        let child_ref = world.next_dbref();
        let mut child = GameObject::new(&child_ref, "grunt", Kind::Npc);
        child.archetype_ref = Some(base_ref.clone());
        world.add_object(child);

        let resolved = world.resolved_tags(world.get(&child_ref).unwrap());
        assert!(!resolved.contains(&locked), "inherited system:locked must not leak to subtypes");
        assert!(resolved.contains(&global), "other system tags still inherit");

        // An object's OWN system:locked is kept.
        world.get_mut(&child_ref).unwrap().tags.insert(locked.clone());
        assert!(
            world.resolved_tags(world.get(&child_ref).unwrap()).contains(&locked),
            "own system:locked is kept",
        );
    }

    /// A declared attribute schema inherits down the archetype chain: an
    /// instance sees its archetype's descriptors, its own descriptors win on a
    /// key collision (nearest-first), and each carries its source.
    #[test]
    fn resolved_attr_schema_inherits_own_wins() {
        use crate::attr_schema::{AttrDescriptor, AttrType};

        let mut world = World::new();
        let base_ref = world.next_dbref();
        let mut base = GameObject::new(&base_ref, "monster", Kind::Npc);
        base.attr_schema = vec![
            AttrDescriptor::new("hp", AttrType::Int),
            AttrDescriptor::new("armor", AttrType::Int),
        ];
        world.add_object(base);

        let child_ref = world.next_dbref();
        let mut child = GameObject::new(&child_ref, "goblin", Kind::Npc);
        child.archetype_ref = Some(base_ref.clone());
        // Own `hp` overrides the inherited one; `attack` is new.
        let mut own_hp = AttrDescriptor::new("hp", AttrType::Int);
        own_hp.label = Some("Goblin HP".into());
        child.attr_schema = vec![own_hp, AttrDescriptor::new("attack", AttrType::Int)];
        world.add_object(child);

        let resolved = world.resolved_attr_schema(world.get(&child_ref).unwrap());
        let by_key: std::collections::HashMap<&str, &(AttrDescriptor, String)> =
            resolved.iter().map(|e| (e.0.key.as_str(), e)).collect();

        // Own descriptors are marked "own"; own hp wins over the inherited one.
        assert_eq!(by_key["hp"].1, "own");
        assert_eq!(by_key["hp"].0.label.as_deref(), Some("Goblin HP"));
        assert_eq!(by_key["attack"].1, "own");
        // The inherited-only descriptor carries its source ref.
        assert_eq!(by_key["armor"].1, base_ref);
        // Each key appears exactly once.
        assert_eq!(resolved.len(), 3);
    }
}
