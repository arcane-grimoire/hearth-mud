mod object;
mod tag;

pub use object::{Exit, GameObject, Kind};
pub use tag::Tag;

use std::collections::HashMap;

#[derive(Clone)]
pub struct World {
    pub objects: HashMap<String, GameObject>,
    pub exits: Vec<Exit>,
}

impl World {
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
            exits: Vec::new(),
        }
    }

    pub fn add_object(&mut self, obj: GameObject) {
        self.objects.insert(obj.ref_id.clone(), obj);
    }

    pub fn add_exit(&mut self, exit: Exit) {
        self.exits.push(exit);
    }

    pub fn get(&self, ref_id: &str) -> Option<&GameObject> {
        self.objects.get(ref_id)
    }

    pub fn get_mut(&mut self, ref_id: &str) -> Option<&mut GameObject> {
        self.objects.get_mut(ref_id)
    }

    pub fn exits_from(&self, room_ref: &str) -> Vec<&Exit> {
        self.exits
            .iter()
            .filter(|e| e.source_ref == room_ref)
            .collect()
    }

    pub fn objects_in(&self, location_ref: &str) -> Vec<&GameObject> {
        self.objects
            .values()
            .filter(|o| o.location_ref.as_deref() == Some(location_ref))
            .collect()
    }

    pub fn find_exit(&self, room_ref: &str, direction: &str) -> Option<&Exit> {
        let dir = direction.to_lowercase();
        self.exits.iter().find(|e| {
            e.source_ref == room_ref
                && (e.key.to_lowercase() == dir
                    || e.aliases.iter().any(|a| a.to_lowercase() == dir))
        })
    }
}
