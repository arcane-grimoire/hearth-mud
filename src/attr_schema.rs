//! Declared attribute schemas — game-authored, typed metadata describing the
//! custom attributes an object (or, later, terrain/room) uses, so tooling can
//! render a real typed form instead of untyped key/value text boxes.
//!
//! The engine's job is **expose, don't enforce** (see
//! `docs/plans/attribute-schema.md`): descriptors are carried opaquely, resolved
//! up the archetype chain like attrs, and handed to the builder over `examine`.
//! Attribute values stay free-form; the schema is descriptive.
//!
//! The type set is a **closed** enum with an [`AttrType::Unknown`] catch-all so
//! an unrecognized `type` degrades to a raw field (a warn at load) rather than
//! failing the load — the same non-fatal contract the loaders use elsewhere.
//! This module is deliberately kind-agnostic: objects/archetypes are the first
//! consumer, terrain adopts the same `AttrType`/`AttrDescriptor` later.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A closed set of attribute types. Each maps to a TOML value, a JSON value,
/// and an editor widget. `List` is a homogeneous list whose element type lives
/// in [`AttrDescriptor::item_type`]. `Unknown` preserves an unrecognized type
/// string so it round-trips and the editor can fall back to a raw field.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AttrType {
    #[default]
    String,
    Text,
    Int,
    Float,
    Bool,
    Enum,
    Color,
    Ref,
    List,
    Unknown(String),
}

impl AttrType {
    /// The canonical tag string (`"int"`, `"list"`, …). An `Unknown` yields its
    /// original string so it round-trips unchanged.
    pub fn tag(&self) -> &str {
        match self {
            AttrType::String => "string",
            AttrType::Text => "text",
            AttrType::Int => "int",
            AttrType::Float => "float",
            AttrType::Bool => "bool",
            AttrType::Enum => "enum",
            AttrType::Color => "color",
            AttrType::Ref => "ref",
            AttrType::List => "list",
            AttrType::Unknown(s) => s,
        }
    }

    /// Parse a tag string. An unrecognized tag becomes [`AttrType::Unknown`]
    /// (never an error) — the closed-set-with-fallback contract.
    pub fn from_tag(s: &str) -> Self {
        match s {
            "string" => AttrType::String,
            "text" => AttrType::Text,
            "int" => AttrType::Int,
            "float" => AttrType::Float,
            "bool" => AttrType::Bool,
            "enum" => AttrType::Enum,
            "color" => AttrType::Color,
            "ref" => AttrType::Ref,
            "list" => AttrType::List,
            other => AttrType::Unknown(other.to_string()),
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, AttrType::Unknown(_))
    }
}

impl Serialize for AttrType {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.tag())
    }
}

impl<'de> Deserialize<'de> for AttrType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(AttrType::from_tag(&s))
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One declared attribute: its key, type, and the metadata a form generator
/// needs. Per-type extras (`min`/`max`/`step`, `values`, `ref_source`,
/// `pattern`, `item_type`) are carried opaquely; the editor interprets them for
/// the type at hand and ignores the rest. All fields but `key` are optional so
/// descriptors stay terse in TOML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttrDescriptor {
    pub key: String,
    #[serde(rename = "type", default)]
    pub ty: AttrType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Authoring/display default shown in the form (and written on save). NOT a
    /// runtime resolution layer — `World::resolved_attr` is unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    // int/float
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    // enum
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    // ref — the source vocabulary the editor enumerates candidates from
    // (e.g. "kind:npc", "tag:loot:weapon", "archetype").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_source: Option<String>,
    // string
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    // list — the element type
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_type: Option<AttrType>,
}

impl AttrDescriptor {
    /// A minimal descriptor (test/programmatic construction).
    pub fn new(key: impl Into<String>, ty: AttrType) -> Self {
        Self {
            key: key.into(),
            ty,
            label: None,
            help: None,
            default: None,
            required: false,
            min: None,
            max: None,
            step: None,
            values: Vec::new(),
            ref_source: None,
            pattern: None,
            item_type: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attr_type_round_trips_through_its_tag() {
        for t in [
            AttrType::String,
            AttrType::Text,
            AttrType::Int,
            AttrType::Float,
            AttrType::Bool,
            AttrType::Enum,
            AttrType::Color,
            AttrType::Ref,
            AttrType::List,
        ] {
            assert_eq!(AttrType::from_tag(t.tag()), t);
        }
    }

    #[test]
    fn unknown_type_degrades_and_round_trips() {
        let t = AttrType::from_tag("hologram");
        assert!(t.is_unknown());
        assert_eq!(t.tag(), "hologram");
        // Serializes back to its original string.
        assert_eq!(serde_json::to_value(&t).unwrap(), serde_json::json!("hologram"));
    }

    #[test]
    fn descriptor_deserializes_from_terse_toml() {
        let d: AttrDescriptor =
            toml::from_str(r#"key = "hp"
type = "int"
label = "Hit points"
min = 0
default = 1
"#)
            .unwrap();
        assert_eq!(d.key, "hp");
        assert_eq!(d.ty, AttrType::Int);
        assert_eq!(d.label.as_deref(), Some("Hit points"));
        assert_eq!(d.min, Some(0.0));
        assert_eq!(d.default, Some(serde_json::json!(1)));
        assert!(!d.required);
    }

    #[test]
    fn enum_and_ref_descriptors_carry_their_extras() {
        let d: AttrDescriptor =
            toml::from_str(r#"key = "biome"
type = "enum"
values = ["arid", "alpine"]
"#)
            .unwrap();
        assert_eq!(d.ty, AttrType::Enum);
        assert_eq!(d.values, vec!["arid".to_string(), "alpine".to_string()]);

        let r: AttrDescriptor =
            toml::from_str(r#"key = "boss"
type = "ref"
ref_source = "kind:npc"
"#)
            .unwrap();
        assert_eq!(r.ty, AttrType::Ref);
        assert_eq!(r.ref_source.as_deref(), Some("kind:npc"));
    }
}
