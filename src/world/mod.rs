mod object;
mod tag;

pub use object::{GameObject, Kind};
pub use tag::Tag;

use std::collections::HashMap;

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
}
