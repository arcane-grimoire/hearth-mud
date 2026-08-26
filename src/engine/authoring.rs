//! Authoring → Intent translation.
//!
//! The REST/telnet authoring surface and softcode share **one** mutation
//! mechanism: [`crate::softcode::apply_batch`]. They do **not** share
//! authorization. Softcode is ownership-gated (`may_modify` inside `apply_to`:
//! a Program may only mutate what its object owns). Authoring is scope- and
//! lock-gated at the transport edge — the preamble of
//! [`crate::engine::Engine::handle_api_request`] enforces Builder/Admin scope,
//! `system:locked`, and `system:global` before any mutation is built.
//!
//! Once that preamble has authorized a request, authoring applies its batch
//! with `authority = None` (system-trusted, the default for
//! [`IntentBatch::from_intents`]): `apply_batch` supplies *integrity*
//! (atomicity, rollback, existence/containment/parse validation), not a second
//! authorization pass. This is the split recorded in ADR-0007 — "authoring
//! shares the Intent mechanism, not its authorization."
//!
//! Most write actions have an exact [`Intent`] twin whose `apply_to` arm
//! already carries the same validation the REST arm used to inline (the Intent
//! definitions even say "Mirrors the REST `SetAliases`/`UpdateExit`"). Those
//! collapse into [`write_batch`] below. The exceptions stay in
//! `handle_api_request`'s match and are listed there:
//!
//! - **`SetLocation`** — carries an authoring-only guard (refuse relocating a
//!   player by ref) that `Intent::Move` deliberately lacks, and needs the
//!   world to check kind, so it builds its `Move` batch in the arm.
//! - **`DeleteObject`** — its cascade refuses flattening a `system:locked`
//!   instance, an authoring-tier policy `Intent::Destroy` does not model.
//! - **`CreateObject`/`CreateRoom`/`CreateExit`/`CloneObject`** — creation
//!   pre-mints a dbref and returns it, so they build their batch in the arm.
//! - **`SetScript`/`ClearScript`/`SetLib`/`RemoveLib`/`CreateLibrary`/`InkSave`**
//!   — program and narrative authoring with syntax checks, version history,
//!   shipped-name collision, and locked-host rules; not a clean Intent twin.

use crate::engine::ApiRequest;
use crate::softcode::{Intent, IntentBatch};
use crate::world::Tag;

/// Translate an authoring write request into a system-authority [`IntentBatch`].
///
/// - `Some(Ok(batch))` — a translatable write; the caller applies it via
///   `apply_batch` and answers `ok()`.
/// - `Some(Err(msg))` — a translatable write that failed a pure pre-guard
///   (e.g. an unparseable tag); the caller answers `error(msg)`.
/// - `None` — not a pure authoring write (a read, a creation returning a ref,
///   program/asset authoring, or `DeleteObject`); the caller's `match` handles
///   it as before.
///
/// This function is pure: it borrows the request and consults no world state,
/// so it is unit-testable without constructing an [`crate::engine::Engine`].
/// All world-dependent validation (existence, archetype cycles, lock-expr
/// parsing, containment) lives in `apply_to` and runs when the batch is applied.
pub(crate) fn write_batch(req: &ApiRequest) -> Option<Result<IntentBatch, String>> {
    let intent = match req {
        ApiRequest::SetAttribute { ref_id, key, value } => {
            // A null value means "remove" — the same convention the softcode
            // `set_attr` API uses (nil → UnsetAttr).
            if value.is_null() {
                Intent::UnsetAttr { target: ref_id.clone(), key: key.clone() }
            } else {
                Intent::SetAttr {
                    target: ref_id.clone(),
                    key: key.clone(),
                    value: value.clone(),
                }
            }
        }
        ApiRequest::SetTitle { ref_id, title } => {
            Intent::SetTitle { target: ref_id.clone(), title: title.clone() }
        }
        ApiRequest::SetDescription { ref_id, description } => {
            Intent::SetDescription { target: ref_id.clone(), description: description.clone() }
        }
        ApiRequest::SetAliases { ref_id, aliases } => {
            Intent::SetAliases { target: ref_id.clone(), aliases: aliases.clone() }
        }
        ApiRequest::AddTag { ref_id, tag } => match Tag::parse(tag) {
            Ok(parsed) => Intent::SetTag { target: ref_id.clone(), tag: parsed },
            Err(e) => return Some(Err(e)),
        },
        ApiRequest::RemoveTag { ref_id, tag } => match Tag::parse(tag) {
            Ok(parsed) => Intent::UnsetTag { target: ref_id.clone(), tag: parsed },
            Err(e) => return Some(Err(e)),
        },
        ApiRequest::SetLock { ref_id, hook, expr } => Intent::SetLock {
            target: ref_id.clone(),
            hook: hook.clone(),
            expr: expr.clone(),
        },
        ApiRequest::ClearLock { ref_id, hook } => {
            Intent::ClearLock { target: ref_id.clone(), hook: hook.clone() }
        }
        ApiRequest::UpdateExit { ref_id, direction, target } => Intent::UpdateExit {
            target: ref_id.clone(),
            direction: direction.clone(),
            destination: target.clone(),
        },
        ApiRequest::SetArchetype { ref_id, archetype_ref } => Intent::SetArchetype {
            target: ref_id.clone(),
            archetype: archetype_ref.clone(),
        },
        ApiRequest::DetachObject { ref_id } => Intent::Detach { target: ref_id.clone() },
        _ => return None,
    };
    Some(Ok(IntentBatch::from_intents(vec![intent])))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The whole point of the seam: assert on the translation directly, no
    // Engine, no world, no async runtime. `apply_batch`'s own tests (in
    // `softcode`) cover what each Intent does once applied.

    fn one(req: ApiRequest) -> Intent {
        let batch = write_batch(&req).expect("translatable").expect("no pre-guard error");
        assert_eq!(batch.intents.len(), 1);
        batch.intents.into_iter().next().unwrap()
    }

    #[test]
    fn set_attribute_with_value_is_set_attr() {
        let i = one(ApiRequest::SetAttribute {
            ref_id: "#1".into(),
            key: "hp".into(),
            value: serde_json::json!(10),
        });
        assert!(matches!(i, Intent::SetAttr { target, key, value }
            if target == "#1" && key == "hp" && value == serde_json::json!(10)));
    }

    #[test]
    fn set_attribute_null_removes() {
        let i = one(ApiRequest::SetAttribute {
            ref_id: "#1".into(),
            key: "hp".into(),
            value: serde_json::Value::Null,
        });
        assert!(matches!(i, Intent::UnsetAttr { target, key }
            if target == "#1" && key == "hp"));
    }

    #[test]
    fn add_tag_parses_then_sets() {
        let i = one(ApiRequest::AddTag { ref_id: "#1".into(), tag: "color:red".into() });
        assert!(matches!(i, Intent::SetTag { target, .. } if target == "#1"));
    }

    #[test]
    fn add_tag_rejects_unparseable() {
        // An empty/whitespace tag is the one `Tag::parse` rejects (a bare word
        // is a valid keyless tag). The refusal surfaces as a pure pre-guard.
        let out = write_batch(&ApiRequest::AddTag {
            ref_id: "#1".into(),
            tag: "   ".into(),
        });
        assert!(matches!(out, Some(Err(_))), "a blank tag is a pure pre-guard refusal");
    }

    #[test]
    fn update_exit_maps_target_to_destination() {
        let i = one(ApiRequest::UpdateExit {
            ref_id: "#5".into(),
            direction: Some("north".into()),
            target: Some("#9".into()),
        });
        assert!(matches!(i, Intent::UpdateExit { target, direction, destination }
            if target == "#5" && direction.as_deref() == Some("north")
                && destination.as_deref() == Some("#9")));
    }

    #[test]
    fn batch_carries_no_authority() {
        // Authoring is system-trusted after the preamble: authority is None so
        // `may_modify` no-ops. See ADR-0007.
        let batch = write_batch(&ApiRequest::SetTitle {
            ref_id: "#1".into(),
            title: "Hall".into(),
        })
        .unwrap()
        .unwrap();
        assert!(batch.authority.is_none());
    }

    #[test]
    fn reads_and_creation_are_not_translated() {
        assert!(write_batch(&ApiRequest::ListRooms).is_none());
        assert!(write_batch(&ApiRequest::SetLocation {
            ref_id: "#1".into(),
            location: "#2".into(),
        })
        .is_none());
    }
}
