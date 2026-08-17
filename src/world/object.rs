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
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::Room => write!(f, "room"),
            Kind::Item => write!(f, "item"),
            Kind::Npc => write!(f, "npc"),
            Kind::Player => write!(f, "player"),
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
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attribute {
    pub key: String,
    pub value: serde_json::Value,
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameObject {
    pub ref_id: String,
    pub key: String,
    pub kind: Kind,
    pub title: Option<String>,
    pub description: String,
    pub location_ref: Option<String>,
    pub attrs: HashMap<String, serde_json::Value>,
    pub tags: HashSet<Tag>,
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
            attrs: HashMap::new(),
            tags: HashSet::new(),
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

    pub fn display_name(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.key)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exit {
    pub ref_id: String,
    pub source_ref: String,
    pub target_ref: String,
    pub key: String,
    pub aliases: Vec<String>,
    #[serde(default)]
    pub locks: HashMap<String, String>,
    pub id: String,
}

impl Exit {
    pub fn new(
        ref_id: impl Into<String>,
        source: impl Into<String>,
        target: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        Self {
            ref_id: ref_id.into(),
            source_ref: source.into(),
            target_ref: target.into(),
            key: key.into(),
            aliases: Vec::new(),
            locks: HashMap::new(),
            id: Uuid::new_v4().to_string(),
        }
    }

    pub fn with_aliases(mut self, aliases: Vec<&str>) -> Self {
        self.aliases = aliases.into_iter().map(String::from).collect();
        self
    }
}
