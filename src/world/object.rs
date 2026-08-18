use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Tag;
use crate::softcode::hooks::ProgramRecord;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Room,
    Item,
    Npc,
    Player,
    Exit,
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::Room => write!(f, "room"),
            Kind::Item => write!(f, "item"),
            Kind::Npc => write!(f, "npc"),
            Kind::Player => write!(f, "player"),
            Kind::Exit => write!(f, "exit"),
        }
    }
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "room" => Some(Kind::Room),
            "item" => Some(Kind::Item),
            "npc" => Some(Kind::Npc),
            "player" => Some(Kind::Player),
            "exit" => Some(Kind::Exit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameObject {
    pub ref_id: String,
    pub key: String,
    pub kind: Kind,
    pub title: Option<String>,
    pub description: String,
    pub location_ref: Option<String>,
    #[serde(default)]
    pub owner_ref: Option<String>,
    #[serde(default)]
    pub target_ref: Option<String>,
    pub attrs: HashMap<String, serde_json::Value>,
    pub tags: HashSet<Tag>,
    #[serde(default)]
    pub aliases: HashSet<String>,
    #[serde(default)]
    pub programs: HashMap<String, ProgramRecord>,
    #[serde(default)]
    pub locks: HashMap<String, String>,
    pub id: String,
}

impl GameObject {
    pub fn new(ref_id: impl Into<String>, key: impl Into<String>, kind: Kind) -> Self {
        Self {
            ref_id: ref_id.into(),
            key: key.into(),
            kind,
            title: None,
            description: String::new(),
            location_ref: None,
            owner_ref: None,
            target_ref: None,
            attrs: HashMap::new(),
            tags: HashSet::new(),
            aliases: HashSet::new(),
            programs: HashMap::new(),
            locks: HashMap::new(),
            id: Uuid::new_v4().to_string(),
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    pub fn with_location(mut self, loc: impl Into<String>) -> Self {
        self.location_ref = Some(loc.into());
        self
    }

    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner_ref = Some(owner.into());
        self
    }

    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target_ref = Some(target.into());
        self
    }

    pub fn with_aliases(mut self, aliases: Vec<&str>) -> Self {
        self.aliases = aliases.into_iter().map(String::from).collect();
        self
    }

    pub fn display_name(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.key)
    }

    pub fn matches_direction(&self, direction: &str) -> bool {
        let dir = direction.to_lowercase();
        self.key.to_lowercase() == dir || self.aliases.iter().any(|a| a.to_lowercase() == dir)
    }
}
