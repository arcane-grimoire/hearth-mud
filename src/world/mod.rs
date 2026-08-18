mod object;
mod script;
mod tag;

pub use object::{GameObject, Kind};
pub use script::Script;
pub use tag::Tag;

use std::collections::HashMap;

#[derive(Clone)]
pub struct World {
    pub objects: HashMap<String, GameObject>,
    pub scripts: HashMap<String, Script>,
    pub next_id: u64,
}

impl World {
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
            scripts: HashMap::new(),
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

    pub fn objects_in(&self, location_ref: &str) -> Vec<&GameObject> {
        self.objects
            .values()
            .filter(|o| {
                o.location_ref.as_deref() == Some(location_ref) && o.kind != Kind::Exit
            })
            .collect()
    }
}
