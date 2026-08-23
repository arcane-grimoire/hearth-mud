// Shared builder selection — the spine of the unified builder workspace.
//
// Every panel (object table, map, properties, hooks, dialogue) reads the SAME
// selection from here instead of each re-deciding "which object am I editing?".
// Select a row in the table, click "Full editor…" on a map node, or pick from
// ⌘K — all of them route through selectRef(), and the whole workspace reflects
// it. This is what turns N loosely-linked tools into one IDE: one selection,
// many views.
//
// Runes live in a .svelte.js module so reads are reactive across components.

let _ref = $state(null);   // currently selected object ref, e.g. "#12"
let _hook = $state(null);  // optionally, a specific hook being edited on it

/** Reactive view of the current builder selection. */
export const selection = {
  get ref() { return _ref; },
  get hook() { return _hook; },
};

/** Select an object; clears any hook focus. */
export function selectRef(ref) {
  _ref = ref;
  _hook = null;
}

/** Select an object and jump straight to one of its hooks (deep link / map). */
export function selectHook(ref, hook) {
  _ref = ref;
  _hook = hook;
}

export function clearSelection() {
  _ref = null;
  _hook = null;
}
