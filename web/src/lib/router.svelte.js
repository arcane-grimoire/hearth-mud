// A minimal History-API router for the Hearth web client.
//
// The app is a Vite + Svelte 5 SPA served by the engine, whose static handler
// already falls back to index.html for unknown paths (src/net/web.rs) — so
// clean deep links like /builder/rooms load the app and resolve here, with no
// hash needed. We only have a handful of top-level surfaces (the game, the
// builder tools), so a full router framework would be more than this earns.
//
// Reactive: reads go through a $state rune, so any component reading
// route.path / route.query re-renders on navigation. Runes live in a
// .svelte.js module (Svelte 5).

function snapshot() {
  return {
    path: window.location.pathname || '/',
    query: Object.fromEntries(new URLSearchParams(window.location.search)),
    hash: window.location.hash.slice(1),
  };
}

let current = $state(snapshot());

function sync() {
  current = snapshot();
}

/** Reactive view of the current location. */
export const route = {
  get path() { return current.path; },
  get query() { return current.query; },
  get hash() { return current.hash; },
};

/** True when the current path is `prefix` exactly, or nested under it. */
export function matches(prefix) {
  const p = prefix.replace(/\/$/, '');
  return current.path === p || current.path === prefix || current.path.startsWith(p + '/');
}

/** Navigate to an internal path. External URLs fall through to the browser. */
export function navigate(to, { replace = false, state = null } = {}) {
  const url = new URL(to, window.location.origin);
  if (url.origin !== window.location.origin) {
    window.location.href = to;
    return;
  }
  history[replace ? 'replaceState' : 'pushState'](state, '', url.pathname + url.search + url.hash);
  sync();
}

/**
 * Patch the query string in place without changing the path. Defaults to
 * replaceState so filter tweaks (area / focus / depth) don't spam history.
 * A null/undefined/'' value removes the key.
 */
export function setQuery(patch, { replace = true } = {}) {
  const params = new URLSearchParams(window.location.search);
  for (const [k, v] of Object.entries(patch)) {
    if (v === null || v === undefined || v === '') params.delete(k);
    else params.set(k, String(v));
  }
  const qs = params.toString();
  navigate(window.location.pathname + (qs ? '?' + qs : '') + window.location.hash, { replace });
}

if (typeof window !== 'undefined') {
  window.addEventListener('popstate', sync);

  // Let plain internal left-clicks on <a href> act as SPA navigation, so
  // ordinary links work without every call site wiring onclick. Anything the
  // browser should handle itself — modified clicks, new-tab targets, downloads,
  // cross-origin, or an explicit data-native opt-out — is left alone.
  window.addEventListener('click', (e) => {
    if (e.defaultPrevented || e.button !== 0) return;
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const a = e.target.closest?.('a');
    if (!a) return;
    if (a.target && a.target !== '_self') return;
    if (a.hasAttribute('download') || a.dataset.native !== undefined) return;
    const href = a.getAttribute('href');
    if (!href || href.startsWith('#')) return;
    const url = new URL(href, window.location.origin);
    if (url.origin !== window.location.origin) return;
    e.preventDefault();
    navigate(url.pathname + url.search + url.hash);
  });
}
