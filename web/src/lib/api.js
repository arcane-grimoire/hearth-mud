let token = null;

export function setToken(t) {
  token = t;
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
