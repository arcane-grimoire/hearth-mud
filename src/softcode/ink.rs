use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use bladeink::story::Story;
use bladeink::value_type::ValueType;
use bladeink_compiler::Compiler;

type ConversationKey = (String, String);

pub struct InkOutput {
    pub text: String,
    pub choices: Vec<InkChoice>,
    pub tags: Vec<String>,
    pub can_continue: bool,
    pub ended: bool,
}

pub struct InkChoice {
    pub index: usize,
    pub text: String,
    pub tags: Vec<String>,
}

struct ActiveConversation {
    compiled_json: String,
    state_json: String,
}

pub struct InkRuntime {
    compile_cache: HashMap<u64, String>,
    active: HashMap<ConversationKey, ActiveConversation>,
    ink_dir: Option<PathBuf>,
}

impl InkRuntime {
    pub fn new() -> Self {
        Self {
            compile_cache: HashMap::new(),
            active: HashMap::new(),
            ink_dir: None,
        }
    }

    pub fn set_ink_dir(&mut self, dir: PathBuf) {
        self.ink_dir = Some(dir);
    }

    pub fn compile(&mut self, source: &str) -> Result<String, String> {
        let hash = Self::source_hash(source);
        if let Some(json) = self.compile_cache.get(&hash) {
            return Ok(json.clone());
        }
        let compiler = Compiler::new();
        let json = compiler.compile(source).map_err(|e| format!("{e}"))?;
        self.compile_cache.insert(hash, json.clone());
        Ok(json)
    }

    pub fn read_ink_file(&self, name: &str) -> Result<String, String> {
        let dir = self
            .ink_dir
            .as_ref()
            .ok_or_else(|| "no game_dir configured".to_string())?;
        let path = dir.join(name);
        let path = if path.extension().is_none() {
            path.with_extension("ink")
        } else {
            path
        };
        std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))
    }

    pub fn start_conversation(
        &mut self,
        player_ref: &str,
        npc_ref: &str,
        source: &str,
        saved_state: Option<&str>,
    ) -> Result<InkOutput, String> {
        let compiled_json = self.compile(source)?;
        let mut story = Story::new(&compiled_json).map_err(|e| format!("{e}"))?;
        if let Some(state) = saved_state {
            story.load_state(state).map_err(|e| format!("{e}"))?;
        }

        let output = run_story_to_output(&mut story)?;
        let state_json = story.save_state().map_err(|e| format!("{e}"))?;

        let key = (player_ref.to_string(), npc_ref.to_string());
        self.active.insert(
            key,
            ActiveConversation {
                compiled_json,
                state_json,
            },
        );
        Ok(output)
    }

    pub fn continue_story(
        &mut self,
        player_ref: &str,
        npc_ref: &str,
    ) -> Result<InkOutput, String> {
        let key = (player_ref.to_string(), npc_ref.to_string());
        let conv = self
            .active
            .get_mut(&key)
            .ok_or_else(|| "no active conversation".to_string())?;

        let mut story = Story::new(&conv.compiled_json).map_err(|e| format!("{e}"))?;
        story
            .load_state(&conv.state_json)
            .map_err(|e| format!("{e}"))?;

        let output = run_story_to_output(&mut story)?;
        conv.state_json = story.save_state().map_err(|e| format!("{e}"))?;
        Ok(output)
    }

    pub fn choose(
        &mut self,
        player_ref: &str,
        npc_ref: &str,
        choice_index: usize,
    ) -> Result<InkOutput, String> {
        let key = (player_ref.to_string(), npc_ref.to_string());
        let conv = self
            .active
            .get_mut(&key)
            .ok_or_else(|| "no active conversation".to_string())?;

        let mut story = Story::new(&conv.compiled_json).map_err(|e| format!("{e}"))?;
        story
            .load_state(&conv.state_json)
            .map_err(|e| format!("{e}"))?;
        story
            .choose_choice_index(choice_index)
            .map_err(|e| format!("{e}"))?;

        let output = run_story_to_output(&mut story)?;
        conv.state_json = story.save_state().map_err(|e| format!("{e}"))?;
        Ok(output)
    }

    pub fn get_variable(
        &self,
        player_ref: &str,
        npc_ref: &str,
        name: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let key = (player_ref.to_string(), npc_ref.to_string());
        let conv = self
            .active
            .get(&key)
            .ok_or_else(|| "no active conversation".to_string())?;

        let story = Story::new(&conv.compiled_json).map_err(|e| format!("{e}"))?;
        // Note: must load state to see variable values set during conversation
        // But load_state takes &mut, so we need a mutable story
        // For get_variable we can just create a fresh story with state loaded
        let mut story = story;
        story
            .load_state(&conv.state_json)
            .map_err(|e| format!("{e}"))?;
        match story.get_variable(name) {
            Some(v) => Ok(Some(value_type_to_json(&v))),
            None => Ok(None),
        }
    }

    pub fn set_variable(
        &mut self,
        player_ref: &str,
        npc_ref: &str,
        name: &str,
        value: &serde_json::Value,
    ) -> Result<(), String> {
        let key = (player_ref.to_string(), npc_ref.to_string());
        let conv = self
            .active
            .get_mut(&key)
            .ok_or_else(|| "no active conversation".to_string())?;

        let mut story = Story::new(&conv.compiled_json).map_err(|e| format!("{e}"))?;
        story
            .load_state(&conv.state_json)
            .map_err(|e| format!("{e}"))?;
        let vt = json_to_value_type(value)?;
        story.set_variable(name, &vt).map_err(|e| format!("{e}"))?;
        conv.state_json = story.save_state().map_err(|e| format!("{e}"))?;
        Ok(())
    }

    pub fn end_conversation(
        &mut self,
        player_ref: &str,
        npc_ref: &str,
        save: bool,
    ) -> Result<Option<String>, String> {
        let key = (player_ref.to_string(), npc_ref.to_string());
        let state = if save {
            self.active.get(&key).map(|c| c.state_json.clone())
        } else {
            None
        };
        self.active.remove(&key);
        Ok(state)
    }

    pub fn goto(
        &mut self,
        player_ref: &str,
        npc_ref: &str,
        path: &str,
    ) -> Result<InkOutput, String> {
        let key = (player_ref.to_string(), npc_ref.to_string());
        let conv = self
            .active
            .get_mut(&key)
            .ok_or_else(|| "no active conversation".to_string())?;

        let mut story = Story::new(&conv.compiled_json).map_err(|e| format!("{e}"))?;
        story
            .load_state(&conv.state_json)
            .map_err(|e| format!("{e}"))?;
        story
            .choose_path_string(path, true, None)
            .map_err(|e| format!("{e}"))?;

        let output = run_story_to_output(&mut story)?;
        conv.state_json = story.save_state().map_err(|e| format!("{e}"))?;
        Ok(output)
    }

    pub fn cleanup_player(&mut self, player_ref: &str) {
        self.active.retain(|(p, _), _| p != player_ref);
    }

    pub fn invalidate_cache(&mut self) {
        self.compile_cache.clear();
    }

    pub fn has_active(&self, player_ref: &str, npc_ref: &str) -> bool {
        let key = (player_ref.to_string(), npc_ref.to_string());
        self.active.contains_key(&key)
    }

    fn source_hash(source: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        hasher.finish()
    }
}

fn run_story_to_output(story: &mut Story) -> Result<InkOutput, String> {
    let mut text = String::new();
    while story.can_continue() {
        let line = story.cont().map_err(|e| format!("{e}"))?;
        text.push_str(&line);
    }

    let tags = story.get_current_tags().map_err(|e| format!("{e}"))?;

    let choices: Vec<InkChoice> = story
        .get_current_choices()
        .iter()
        .map(|c| InkChoice {
            index: *c.index.borrow(),
            text: c.text.clone(),
            tags: c.tags.clone(),
        })
        .collect();

    let ended = !story.can_continue() && choices.is_empty();

    Ok(InkOutput {
        text,
        choices,
        tags,
        can_continue: false,
        ended,
    })
}

fn value_type_to_json(vt: &ValueType) -> serde_json::Value {
    match vt {
        ValueType::Bool(b) => serde_json::Value::Bool(*b),
        ValueType::Int(i) => serde_json::json!(*i),
        ValueType::Float(f) => serde_json::json!(*f),
        ValueType::String(s) => serde_json::Value::String(s.string.clone()),
        _ => serde_json::Value::Null,
    }
}

fn json_to_value_type(v: &serde_json::Value) -> Result<ValueType, String> {
    match v {
        serde_json::Value::Bool(b) => Ok(ValueType::from(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(ValueType::from(i as i32))
            } else if let Some(f) = n.as_f64() {
                Ok(ValueType::from(f as f32))
            } else {
                Err("unsupported number type".into())
            }
        }
        serde_json::Value::String(s) => Ok(ValueType::from(s.as_str())),
        _ => Err(format!("cannot convert {} to Ink value", v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_STORY: &str = r#"
-> start

=== start ===
Hello, traveler.
* [Ask about rumors]
    I heard something stirs in the dungeon...
    -> END
* [Leave]
    Safe travels.
    -> END
"#;

    const VAR_STORY: &str = r#"
VAR player_name = "stranger"
VAR has_key = false

-> start

=== start ===
Welcome, {player_name}.
{has_key:
    I see you have the key!
    -> key_path
}
What brings you here?
* [Nothing]
    -> END

=== key_path ===
The door is yours.
-> END
"#;

    #[test]
    fn compile_simple_story() {
        let mut rt = InkRuntime::new();
        let result = rt.compile(SIMPLE_STORY);
        assert!(result.is_ok(), "compile failed: {:?}", result.err());
    }

    #[test]
    fn compile_caches_by_hash() {
        let mut rt = InkRuntime::new();
        rt.compile(SIMPLE_STORY).unwrap();
        assert_eq!(rt.compile_cache.len(), 1);
        rt.compile(SIMPLE_STORY).unwrap();
        assert_eq!(rt.compile_cache.len(), 1);
        rt.compile(VAR_STORY).unwrap();
        assert_eq!(rt.compile_cache.len(), 2);
    }

    #[test]
    fn start_and_get_choices() {
        let mut rt = InkRuntime::new();
        let output = rt
            .start_conversation("player1", "npc1", SIMPLE_STORY, None)
            .unwrap();
        assert!(output.text.contains("Hello, traveler"));
        assert_eq!(output.choices.len(), 2);
        assert!(!output.ended);
    }

    #[test]
    fn choose_and_continue() {
        let mut rt = InkRuntime::new();
        rt.start_conversation("p1", "n1", SIMPLE_STORY, None)
            .unwrap();
        let output = rt.choose("p1", "n1", 0).unwrap();
        assert!(output.text.contains("dungeon"));
        assert!(output.ended);
    }

    #[test]
    fn variables() {
        let mut rt = InkRuntime::new();
        rt.start_conversation("p1", "n1", VAR_STORY, None).unwrap();

        let name = rt.get_variable("p1", "n1", "player_name").unwrap();
        assert_eq!(name, Some(serde_json::json!("stranger")));

        rt.end_conversation("p1", "n1", false).unwrap();

        rt.start_conversation("p1", "n1", VAR_STORY, None).unwrap();
        rt.set_variable("p1", "n1", "player_name", &serde_json::json!("Aria"))
            .unwrap();

        let name = rt.get_variable("p1", "n1", "player_name").unwrap();
        assert_eq!(name, Some(serde_json::json!("Aria")));

        rt.end_conversation("p1", "n1", false).unwrap();
    }

    #[test]
    fn save_and_restore() {
        let mut rt = InkRuntime::new();
        rt.start_conversation("p1", "n1", SIMPLE_STORY, None)
            .unwrap();
        let state = rt.end_conversation("p1", "n1", true).unwrap();
        assert!(state.is_some());

        let output = rt
            .start_conversation("p1", "n1", SIMPLE_STORY, state.as_deref())
            .unwrap();
        assert_eq!(output.choices.len(), 2);
    }

    #[test]
    fn cleanup_player() {
        let mut rt = InkRuntime::new();
        rt.start_conversation("p1", "n1", SIMPLE_STORY, None)
            .unwrap();
        rt.start_conversation("p1", "n2", SIMPLE_STORY, None)
            .unwrap();
        rt.start_conversation("p2", "n1", SIMPLE_STORY, None)
            .unwrap();
        assert!(rt.has_active("p1", "n1"));
        assert!(rt.has_active("p1", "n2"));

        rt.cleanup_player("p1");
        assert!(!rt.has_active("p1", "n1"));
        assert!(!rt.has_active("p1", "n2"));
        assert!(rt.has_active("p2", "n1"));
    }

    #[test]
    fn compile_error() {
        let mut rt = InkRuntime::new();
        let result = rt.compile("this is not valid ink {{{}}}");
        assert!(result.is_err());
    }

    #[test]
    fn goto_knot() {
        let mut rt = InkRuntime::new();
        rt.start_conversation("p1", "n1", VAR_STORY, None).unwrap();
        let output = rt.goto("p1", "n1", "key_path").unwrap();
        assert!(output.text.contains("door"));
        assert!(output.ended);
    }
}
