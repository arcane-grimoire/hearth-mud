use crate::world::{GameObject, Kind, Tag, World};

#[derive(Debug, Clone)]
pub enum LockExpr {
    True,
    False,
    Perm(String),
    HasTag(Tag),
    HasAttr(String, Option<serde_json::Value>),
    InInventory(Tag),
    IsKind(Kind),
    IsOwner,
    TimeBetween(u32, u32),
    GameTimeBetween(u32, u32),
    And(Box<LockExpr>, Box<LockExpr>),
    Or(Box<LockExpr>, Box<LockExpr>),
    Not(Box<LockExpr>),
}

pub struct AccessContext<'a> {
    pub actor: &'a GameObject,
    pub world: &'a World,
    pub account_scopes: &'a [String],
    pub target: Option<&'a GameObject>,
    /// The current in-world clock hour, when a game clock is configured. Backs
    /// `game_time_between()`; `None` (no clock) makes that predicate false.
    pub game_hour: Option<u32>,
}

pub fn evaluate(expr: &LockExpr, ctx: &AccessContext) -> bool {
    match expr {
        LockExpr::True => true,
        LockExpr::False => false,
        LockExpr::Perm(scope) => ctx.account_scopes.iter().any(|s| s == scope || s == "admin"),
        LockExpr::HasTag(tag) => ctx.actor.tags.contains(tag),
        LockExpr::HasAttr(key, expected) => match ctx.actor.attrs.get(key) {
            Some(val) => match expected {
                Some(expected_val) => val == expected_val,
                None => true,
            },
            None => false,
        },
        LockExpr::InInventory(tag) => ctx
            .world
            .objects_in(&ctx.actor.ref_id)
            .iter()
            .any(|o| o.tags.contains(tag)),
        LockExpr::IsKind(kind) => ctx.actor.kind == *kind,
        LockExpr::IsOwner => ctx
            .target
            .and_then(|t| t.owner_ref.as_ref())
            .is_some_and(|owner| owner == &ctx.actor.ref_id),
        LockExpr::TimeBetween(start, end) => {
            let hour = chrono_hour_utc();
            if start <= end {
                hour >= *start && hour < *end
            } else {
                hour >= *start || hour < *end
            }
        }
        // In-world clock, not wall-clock. False when no game clock is
        // configured (`ctx.game_hour` is None).
        LockExpr::GameTimeBetween(start, end) => match ctx.game_hour {
            Some(hour) => {
                if start <= end {
                    hour >= *start && hour < *end
                } else {
                    hour >= *start || hour < *end
                }
            }
            None => false,
        },
        LockExpr::And(a, b) => evaluate(a, ctx) && evaluate(b, ctx),
        LockExpr::Or(a, b) => evaluate(a, ctx) || evaluate(b, ctx),
        LockExpr::Not(inner) => !evaluate(inner, ctx),
    }
}

fn chrono_hour_utc() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    ((secs % 86400) / 3600) as u32
}

pub fn parse(input: &str) -> Result<LockExpr, String> {
    let tokens = tokenize(input)?;
    let mut pos = 0;
    let expr = parse_or(&tokens, &mut pos)?;
    if pos < tokens.len() {
        return Err(format!("Unexpected token: '{}'", tokens[pos]));
    }
    Ok(expr)
}

pub fn evaluate_lock_string(
    lock_str: &str,
    ctx: &AccessContext,
) -> Result<bool, String> {
    let expr = parse(lock_str)?;
    Ok(evaluate(&expr, ctx))
}

fn tokenize(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '(' || c == ')' || c == ',' {
            tokens.push(c.to_string());
            chars.next();
            continue;
        }
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == '(' || c == ')' || c == ',' {
                break;
            }
            word.push(c);
            chars.next();
        }
        if !word.is_empty() {
            tokens.push(word);
        }
    }
    Ok(tokens)
}

fn parse_or(tokens: &[String], pos: &mut usize) -> Result<LockExpr, String> {
    let mut left = parse_and(tokens, pos)?;
    while *pos < tokens.len() && tokens[*pos].eq_ignore_ascii_case("OR") {
        *pos += 1;
        let right = parse_and(tokens, pos)?;
        left = LockExpr::Or(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_and(tokens: &[String], pos: &mut usize) -> Result<LockExpr, String> {
    let mut left = parse_not(tokens, pos)?;
    while *pos < tokens.len() && tokens[*pos].eq_ignore_ascii_case("AND") {
        *pos += 1;
        let right = parse_not(tokens, pos)?;
        left = LockExpr::And(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_not(tokens: &[String], pos: &mut usize) -> Result<LockExpr, String> {
    if *pos < tokens.len() && tokens[*pos].eq_ignore_ascii_case("NOT") {
        *pos += 1;
        let inner = parse_atom(tokens, pos)?;
        return Ok(LockExpr::Not(Box::new(inner)));
    }
    parse_atom(tokens, pos)
}

fn parse_atom(tokens: &[String], pos: &mut usize) -> Result<LockExpr, String> {
    if *pos >= tokens.len() {
        return Err("Unexpected end of lock expression".into());
    }

    let token = &tokens[*pos];

    if token == "(" {
        *pos += 1;
        let expr = parse_or(tokens, pos)?;
        if *pos >= tokens.len() || tokens[*pos] != ")" {
            return Err("Expected ')'".into());
        }
        *pos += 1;
        return Ok(expr);
    }

    let lower = token.to_lowercase();

    if lower == "true" {
        *pos += 1;
        return Ok(LockExpr::True);
    }
    if lower == "false" {
        *pos += 1;
        return Ok(LockExpr::False);
    }

    // Function calls: name(args...)
    if *pos + 1 < tokens.len() && tokens[*pos + 1] == "(" {
        let func_name = lower.clone();
        *pos += 2; // skip name and (
        let args = parse_args(tokens, pos)?;

        match func_name.as_str() {
            "perm" => {
                if args.len() != 1 {
                    return Err("perm() takes 1 argument".into());
                }
                Ok(LockExpr::Perm(args[0].clone()))
            }
            "has_tag" => {
                if args.len() != 1 {
                    return Err("has_tag() takes 1 argument".into());
                }
                let tag = Tag::parse(&args[0])?;
                Ok(LockExpr::HasTag(tag))
            }
            "has_attr" => {
                if args.is_empty() || args.len() > 2 {
                    return Err("has_attr() takes 1-2 arguments".into());
                }
                let value = if args.len() == 2 {
                    Some(parse_json_value(&args[1]))
                } else {
                    None
                };
                Ok(LockExpr::HasAttr(args[0].clone(), value))
            }
            "in_inventory" => {
                if args.len() != 1 {
                    return Err("in_inventory() takes 1 argument".into());
                }
                let tag = Tag::parse(&args[0])?;
                Ok(LockExpr::InInventory(tag))
            }
            "is_kind" => {
                if args.len() != 1 {
                    return Err("is_kind() takes 1 argument".into());
                }
                let kind = Kind::parse(&args[0])
                    .ok_or_else(|| format!("Unknown kind: '{}'", args[0]))?;
                Ok(LockExpr::IsKind(kind))
            }
            "is_owner" => {
                if !args.is_empty() {
                    return Err("is_owner() takes no arguments".into());
                }
                Ok(LockExpr::IsOwner)
            }
            "time_between" => {
                if args.len() != 2 {
                    return Err("time_between() takes 2 arguments".into());
                }
                let start: u32 = args[0]
                    .parse()
                    .map_err(|_| format!("Invalid hour: '{}'", args[0]))?;
                let end: u32 = args[1]
                    .parse()
                    .map_err(|_| format!("Invalid hour: '{}'", args[1]))?;
                if start > 23 || end > 23 {
                    return Err("Hours must be 0-23".into());
                }
                Ok(LockExpr::TimeBetween(start, end))
            }
            "game_time_between" => {
                if args.len() != 2 {
                    return Err("game_time_between() takes 2 arguments".into());
                }
                let start: u32 = args[0]
                    .parse()
                    .map_err(|_| format!("Invalid hour: '{}'", args[0]))?;
                let end: u32 = args[1]
                    .parse()
                    .map_err(|_| format!("Invalid hour: '{}'", args[1]))?;
                if start > 23 || end > 23 {
                    return Err("Hours must be 0-23".into());
                }
                Ok(LockExpr::GameTimeBetween(start, end))
            }
            _ => Err(format!("Unknown lock function: '{}'", func_name)),
        }
    } else {
        Err(format!(
            "Expected a lock function or keyword, got '{}'",
            token
        ))
    }
}

fn parse_args(tokens: &[String], pos: &mut usize) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    if *pos < tokens.len() && tokens[*pos] == ")" {
        *pos += 1;
        return Ok(args);
    }
    loop {
        if *pos >= tokens.len() {
            return Err("Expected ')' in function call".into());
        }
        // Collect everything up to , or ) as a single arg
        let mut arg = String::new();
        while *pos < tokens.len() && tokens[*pos] != "," && tokens[*pos] != ")" {
            if !arg.is_empty() {
                arg.push(' ');
            }
            arg.push_str(&tokens[*pos]);
            *pos += 1;
        }
        args.push(arg);

        if *pos >= tokens.len() {
            return Err("Expected ')' in function call".into());
        }
        if tokens[*pos] == ")" {
            *pos += 1;
            return Ok(args);
        }
        if tokens[*pos] == "," {
            *pos += 1;
        }
    }
}

fn parse_json_value(s: &str) -> serde_json::Value {
    let trimmed = s.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return v;
    }
    let lower = trimmed.to_lowercase();
    if lower == "true" {
        return serde_json::Value::Bool(true);
    }
    if lower == "false" {
        return serde_json::Value::Bool(false);
    }
    if lower == "null" {
        return serde_json::Value::Null;
    }
    if let Ok(n) = trimmed.parse::<i64>() {
        return serde_json::Value::Number(n.into());
    }
    if let Ok(n) = trimmed.parse::<f64>()
        && let Some(n) = serde_json::Number::from_f64(n) {
            return serde_json::Value::Number(n);
        }
    serde_json::Value::String(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    

    fn test_actor(tags: &[&str], attrs: &[(&str, serde_json::Value)]) -> GameObject {
        let mut actor = GameObject::new("player/test", "test", Kind::Player);
        for spec in tags {
            actor.tags.insert(Tag::parse(spec).unwrap());
        }
        for (k, v) in attrs {
            actor.attrs.insert(k.to_string(), v.clone());
        }
        actor
    }

    fn ctx<'a>(actor: &'a GameObject, world: &'a World, scopes: &'a [String]) -> AccessContext<'a> {
        AccessContext {
            actor,
            world,
            account_scopes: scopes,
            target: None,
            game_hour: None,
        }
    }

    #[test]
    fn game_time_between_reads_game_hour_and_needs_a_clock() {
        let actor = test_actor(&[], &[]);
        let world = World::new();
        let scopes: Vec<String> = vec![];
        let mk = |gh: Option<u32>| AccessContext {
            actor: &actor,
            world: &world,
            account_scopes: &scopes,
            target: None,
            game_hour: gh,
        };
        // Daytime window 6..20.
        assert!(evaluate_lock_string("game_time_between(6, 20)", &mk(Some(12))).unwrap());
        assert!(!evaluate_lock_string("game_time_between(6, 20)", &mk(Some(3))).unwrap());
        // Overnight wrap 20..6.
        assert!(evaluate_lock_string("game_time_between(20, 6)", &mk(Some(23))).unwrap());
        assert!(!evaluate_lock_string("game_time_between(20, 6)", &mk(Some(12))).unwrap());
        // No clock configured → always false, never an error.
        assert!(!evaluate_lock_string("game_time_between(6, 20)", &mk(None)).unwrap());
    }

    #[test]
    fn test_true_false() {
        let actor = test_actor(&[], &[]);
        let world = World::new();
        let scopes = vec![];
        assert!(evaluate_lock_string("true", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(!evaluate_lock_string("false", &ctx(&actor, &world, &scopes)).unwrap());
    }

    #[test]
    fn test_perm() {
        let actor = test_actor(&[], &[]);
        let world = World::new();
        let scopes = vec!["builder".to_string()];
        assert!(evaluate_lock_string("perm(builder)", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(!evaluate_lock_string("perm(admin)", &ctx(&actor, &world, &scopes)).unwrap());
    }

    #[test]
    fn test_admin_implies_all_perms() {
        let actor = test_actor(&[], &[]);
        let world = World::new();
        let scopes = vec!["admin".to_string()];
        assert!(evaluate_lock_string("perm(builder)", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(evaluate_lock_string("perm(admin)", &ctx(&actor, &world, &scopes)).unwrap());
    }

    #[test]
    fn test_has_tag() {
        let actor = test_actor(&["quest:worthy"], &[]);
        let world = World::new();
        let scopes = vec![];
        assert!(evaluate_lock_string("has_tag(quest:worthy)", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(!evaluate_lock_string("has_tag(quest:unworthy)", &ctx(&actor, &world, &scopes)).unwrap());
    }

    #[test]
    fn test_has_attr() {
        let actor = test_actor(&[], &[("level", serde_json::json!(5))]);
        let world = World::new();
        let scopes = vec![];
        assert!(evaluate_lock_string("has_attr(level)", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(!evaluate_lock_string("has_attr(missing)", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(evaluate_lock_string("has_attr(level, 5)", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(!evaluate_lock_string("has_attr(level, 10)", &ctx(&actor, &world, &scopes)).unwrap());
    }

    #[test]
    fn test_is_kind() {
        let actor = test_actor(&[], &[]);
        let world = World::new();
        let scopes = vec![];
        assert!(evaluate_lock_string("is_kind(player)", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(!evaluate_lock_string("is_kind(npc)", &ctx(&actor, &world, &scopes)).unwrap());
    }

    #[test]
    fn test_combinators() {
        let actor = test_actor(&["vip"], &[]);
        let world = World::new();
        let scopes = vec!["player".to_string()];
        assert!(evaluate_lock_string("perm(player) AND has_tag(vip)", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(!evaluate_lock_string("perm(admin) AND has_tag(vip)", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(evaluate_lock_string("perm(admin) OR has_tag(vip)", &ctx(&actor, &world, &scopes)).unwrap());
        assert!(evaluate_lock_string("NOT perm(admin)", &ctx(&actor, &world, &scopes)).unwrap());
    }

    #[test]
    fn test_in_inventory() {
        let mut world = World::new();
        let actor = test_actor(&[], &[]);
        world.add_object(actor.clone());
        let mut key = GameObject::new("item/key", "key", Kind::Item)
            .with_location("player/test");
        key.tags.insert(Tag::parse("quest:skeleton_key").unwrap());
        world.add_object(key);

        let actor = world.get("player/test").unwrap();
        let scopes = vec![];
        assert!(evaluate_lock_string(
            "in_inventory(quest:skeleton_key)",
            &ctx(actor, &world, &scopes)
        )
        .unwrap());
        assert!(!evaluate_lock_string(
            "in_inventory(quest:missing)",
            &ctx(actor, &world, &scopes)
        )
        .unwrap());
    }

    #[test]
    fn test_parens() {
        let actor = test_actor(&["vip"], &[]);
        let world = World::new();
        let scopes = vec![];
        assert!(evaluate_lock_string(
            "(perm(admin) OR has_tag(vip)) AND NOT perm(builder)",
            &ctx(&actor, &world, &scopes)
        )
        .unwrap());
    }
}
