use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Tag {
    pub category: String,
    pub key: String,
}

impl Tag {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let raw = spec.trim();
        if raw.is_empty() {
            return Err("Tag cannot be empty".into());
        }
        if let Some((category, key)) = raw.split_once(':') {
            Ok(Self {
                category: category.trim().to_string(),
                key: key.trim().to_string(),
            })
        } else {
            Ok(Self {
                category: String::new(),
                key: raw.to_string(),
            })
        }
    }

    pub fn as_spec(&self) -> String {
        if self.category.is_empty() {
            self.key.clone()
        } else {
            format!("{}:{}", self.category, self.key)
        }
    }
}
