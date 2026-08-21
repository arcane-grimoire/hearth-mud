mod object;
mod tag;

pub use object::{GameObject, Kind};
pub use tag::Tag;

use std::collections::HashMap;

#[derive(Clone)]
pub struct World {
    pub objects: HashMap<String, GameObject>,
    pub next_id: u64,
}

impl World {
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
            next_id: 0,
        }
    }

    /// Allocate and return the next dbref, formatted as `"#N"`.
    pub fn next_dbref(&mut self) -> String {
        self.next_id += 1;
        format!("#{}", self.next_id)
    }

    pub fn add_object(&mut self, obj: GameObject) {
        self.objects.insert(obj.ref_id.clone(), obj);
    }

    pub fn get(&self, ref_id: &str) -> Option<&GameObject> {
        self.objects.get(ref_id)
    }

    pub fn get_mut(&mut self, ref_id: &str) -> Option<&mut GameObject> {
        self.objects.get_mut(ref_id)
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
}
