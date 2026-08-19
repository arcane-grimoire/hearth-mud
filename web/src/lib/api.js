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
  const res = await fetch('/api', {
    method: 'POST',
    headers,
    body: JSON.stringify({ action, ...data }),
  });
  return res.json();
}
