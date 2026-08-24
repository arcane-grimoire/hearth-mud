---
target: builder IDE (/builder/workspace)
total_score: 22
max_score: 40
na_heuristics: 
p0_count: 1
p1_count: 2
timestamp: 2026-08-24T00-56-52Z
slug: web-src-components-builderworkspace-svelte
---
# Design Critique — Hearth Builder IDE (/builder/workspace)

Method: dual-agent (A: design-review sub-agent · B: detector + browser-overlay sub-agent, isolated). Mode: Operate.

## Design Health Score

| # | Heuristic | Score | Key Issue |
|---|-----------|-------|-----------|
| 1 | Visibility of System Status | 3 | Dirty-dot + "Saved X" good; Properties fields save silently on blur, no per-field confirmation. |
| 2 | Match System / Real World | 3 | Fluent dbref/hook/tag for veterans; raw _rx/_ry/_file_key and "Unfiled" leak internals to newcomers. |
| 3 | User Control and Freedom | 1 | No undo anywhere; exit-delete and contents-remove instant + permanent. |
| 4 | Consistency and Standards | 2 | Three safety levels behind same trash icon (object=2-step, hook=inline, exit/content=none). |
| 5 | Error Prevention | 2 | Hook-name validation strong; _-attrs freely overwritable, exit target free-text. |
| 6 | Recognition Rather Than Recall | 2 | Tree/table/finder good; move/exit fields ask you to recall a dbref. |
| 7 | Flexibility and Efficiency | 3 | Cmd-K/B/S, #ref open, reverse-exit auto-create; no in-tree keyboard nav, no multi-select. |
| 8 | Aesthetic and Minimalist Design | 3 | Clean/restrained; functional text runs too small (see P1-text). |
| 9 | Error Recovery | 2 | Transient generic toasts ("Failed"); no inline field errors. |
| 10 | Help and Documentation | 1 | No onboarding, no concept help for newcomers. |
| Total | | 22/40 | Acceptable–low (below 28 "solid Operate") |

## Design Specificity Verdict
Category-interchangeable with a thin MUD skin: explorer tree + tabbed editor + flat table + card forms = any headless-CMS/DB admin. Only MUD identity: dbref chips, kind color-code, direction badges, room graph, Playtest. Biggest miss: builds prose rooms but Description is a bare textarea with no BBCode preview / "read as player" view.

Deterministic scan: CLI detector CLEAN (0 findings, exit 0) in builder scope — verified real (control scan of wider web/src emitted 2 findings elsewhere). In-page overlay: 32 elements / 44 findings on the live workspace (static scan can't see computed styles).
- undersized-ui-text ×31 (labels @10px, count chips @9px, kind badges @8px)
- ai-color-palette ×12 (cyan #22d3ee room badges) — FALSE POSITIVE: deliberate semantic kind-palette, not slop
- tiny-text ×1 (11.5px subtitle)

## What's Working
1. Unified selection + tabbed model (BuilderWorkspace.svelte:148-216) — one selection.ref, no forked editing paths.
2. Hook-name validation before edit (HooksPanel.svelte:33-40) — engine vocab + custom names; serves veterans and newcomers.
3. Table as ground-truth, map as a view (ObjectTable.svelte) — correct IA.

## Priority Issues

[P0] Destructive actions silent & irreversible
- Exit-delete (StructurePanel.svelte:75) and Contents-remove (:95, hard-deletes object incl NPC/player) fire on one click, no confirm/undo. Sinks H3/H4.
- Fix: route all deletes through the 2-step confirm (PropertiesPanel.svelte:244); toast-level Undo; make "remove from room" unlink not delete, gate NPC/player deletes.
- Command: /impeccable harden

[P1] Undersized functional text systemic (detector-caught)
- 31 elements below 11px floor; kind badges at 8px. Taxes H6 + low-vision.
- Fix: raise functional-text floor to >=11-12px; audit token scale.
- Command: /impeccable typeset

[P1] "Area" grouping broken — 83% of objects in "Unfiled"
- Area derived from program records (BuilderWorkspace.svelte:125), not _file_key; The Crossroads sits in Unfiled despite _file_key town/crossroads.
- Fix: derive area from _file_key (AREA_OF helper exists StructurePanel.svelte:27-30), program-area fallback.
- Command: /impeccable clarify

[P2] No newcomer scaffolding; internals leak
- Empty state teaches nothing; _rx/_ry/_file_key editable; hook/tag/dbref unexplained; Hooks subtab unlabeled combobox. Defaults to veteran-raw everywhere.
- Fix: fold _-attrs behind Advanced disclosure; first-run tips + inline concept hints; label Hooks picker.
- Command: /impeccable onboard

[P2] Accessibility gaps in core interactions
- Tabs role="tab" on div, no tablist/aria-selected/aria-controls, no arrow traversal (BuilderWorkspace.svelte:311-323); tree mouse-only; dblclick-to-edit keyboard-inaccessible; color-only focus (PropertiesPanel.svelte:269). With 8px badges, Sam locked out.
- Command: /impeccable audit

Folded to minor: room description generic treatment (/impeccable bolder); toast-only errors (/impeccable polish).

## Persona Red Flags
- Alex: no in-tree keyboard nav; no multi-select/bulk; Properties exit/move fields lack the dbref datalist StructurePanel has; no Cmd-1..9 tab switch.
- Jordan: "Nothing open" no path; unexplained _rx/_file_key/system:managed; blank Hooks dropdown; "Unfiled" looks broken.
- Sam: incomplete tab ARIA; tree non-navigable; color-only focus; 8px badges; toast-only errors.
- Mixed-range builder: surfaces that satisfy veterans strand newcomers; serves one half at the other's expense instead of layering.

## Minor Observations
- Only responsive step sidebar 250->180 then Cmd-B hide; .row crowds below ~500px.
- Components hardcode dark-only amber fallbacks in var(--x,#hex); no own light overrides.
- Stock world test cruft (two "New Room", "Renamed via Property") undisambiguated.
- Live player state ("nipper") appears deletable in Contents; build vs runtime conflated.
- Map graph labels unreadable until zoomed.

## Questions to Consider
1. What would Properties look like if "how does this room read?" were primary instead of a buried textarea?
2. Why default every surface to veteran-raw instead of a newcomer-safe default the veteran peels back?
3. Why open on an empty editor + broken tree instead of the Table (declared ground-truth)?
4. Is the absence of undo a bug, or a statement the builder doesn't trust itself to be forgiving?
