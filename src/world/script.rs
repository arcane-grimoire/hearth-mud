use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Script {
    pub name: String,
    pub source: String,
    pub entry: String,
    pub interval: u64,
    pub enabled: bool,
    #[serde(default)]
    pub state: HashMap<String, serde_json::Value>,
}

impl Script {
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: source.into(),
            entry: "on_tick".into(),
            interval: 1,
            enabled: true,
            state: HashMap::new(),
        }
    }
}
