import { api } from './api.js';

// The engine's hook vocabulary, for the builder's hook pickers. The engine is
// the single source of truth (GET list_hooks → KNOWN_HOOKS + descriptions +
// the open-ended prefixes), so the client never hard-codes and drifts from the
// list. Cached module-wide: the vocabulary is static for a running engine.
//
// Shape: { known: [{ name, describes }], openPrefixes: ['on_','cmd_','lib_'] }

const FALLBACK_PREFIXES = ['on_', 'cmd_', 'lib_'];
let _cache = null;
let _inflight = null;

export async function loadHooks() {
  if (_cache) return _cache;
  if (_inflight) return _inflight;
  _inflight = (async () => {
    try {
      const r = await api('list_hooks');
      if (r?.ok && Array.isArray(r.data?.known)) {
        _cache = { known: r.data.known, openPrefixes: r.data.open_prefixes || FALLBACK_PREFIXES };
        return _cache;
      }
    } catch (e) { /* offline — fall through */ }
    // No list available: keep `known` empty so validation defers to the server
    // (see isValidHookName) rather than mis-flagging a legitimate can_* hook.
    _cache = { known: [], openPrefixes: FALLBACK_PREFIXES };
    return _cache;
  })();
  return _inflight;
}

// Mirrors the engine's `is_valid_hook_name`: a name passes if it's a KNOWN_HOOK
// or starts with an open prefix. When the vocabulary hasn't loaded (offline),
// `known` is empty and we defer to the server — a bare word still fails, but we
// don't wrongly reject a can_* we simply can't see the list for.
export function isValidHookName(name, data) {
  const n = (name || '').trim();
  if (!n) return false;
  const known = data?.known || [];
  const prefixes = data?.openPrefixes || FALLBACK_PREFIXES;
  if (prefixes.some((p) => n.startsWith(p) && n.length > p.length)) return true;
  if (known.length) return known.some((h) => h.name === n);
  // Vocabulary unknown: allow anything non-empty; the engine has the final say.
  return true;
}

// Grouped Typeahead options: events (on_*) and guards (can_*). Custom on_/cmd_/
// lib_ names come through the widget's allowCustom row, not this list.
export function hookOptions(data) {
  const known = data?.known || [];
  const toOpt = (h) => ({ name: h.name, label: h.name, meta: h.describes || '' });
  const guards = known.filter((h) => h.name.startsWith('can_')).map(toOpt);
  const events = known.filter((h) => !h.name.startsWith('can_')).map(toOpt);
  const groups = [];
  if (events.length) groups.push({ name: '__events', label: `Events · on_ (${events.length})`, children: events });
  if (guards.length) groups.push({ name: '__guards', label: `Guards · can_ (${guards.length})`, children: guards });
  return groups;
}
