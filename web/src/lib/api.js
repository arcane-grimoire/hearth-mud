let token = localStorage.getItem('hearth_token');

export function setToken(t) {
  token = t;
  if (t) {
    localStorage.setItem('hearth_token', t);
  } else {
    localStorage.removeItem('hearth_token');
  }
}

export function getSavedToken() {
  return localStorage.getItem('hearth_token');
}

export async function api(action, data = {}) {
  const headers = { 'Content-Type': 'application/json' };
  if (token) headers['Authorization'] = `Bearer ${token}`;
  let res;
  try {
    res = await fetch('/api', {
      method: 'POST',
      headers,
      body: JSON.stringify({ action, ...data }),
    });
  } catch (e) {
    return { ok: false, error: `Network error: ${e.message}` };
  }
  // The body is normally our ApiResponse JSON. A request rejected *before* the
  // handler — an unknown action against an older backend, say — comes back as a
  // plain-text extractor rejection instead; parse defensively so it surfaces as
  // an error rather than throwing and hanging the caller (never resetting a
  // "…"/busy flag). Read as text first, then try JSON.
  const body = await res.text();
  try {
    return JSON.parse(body);
  } catch {
    return { ok: false, error: (body || '').trim() || `HTTP ${res.status}` };
  }
}
