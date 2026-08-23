// Pure map-builder logic ported verbatim from src/net/mapwright.html.
// No DOM, no globals: the serialize/import helpers that used to read a module
// `state` now take the map state (or its `schema`) as an explicit argument, so
// the Svelte components can own the reactive state. Behavior is identical.

// ---------------- sample data (the real Iron Hills, slimmed schema) ----------------
export const SAMPLE_PALETTE = {
  '.': { theme: 'plains', title_prefix: 'Plains', passable: true, color: '#9bbf6a' },
  f: { theme: 'forest', title_prefix: 'Forest', passable: true, color: '#3a6a2e' },
  m: { theme: 'mountain', title_prefix: 'Mountain', passable: true, color: '#7d756b' },
  r: { theme: 'river', title_prefix: 'River', passable: false, color: '#3a6ea5' },
  T: { theme: 'town', title_prefix: 'Town', passable: true, color: '#b08d57' },
};
export const SAMPLE_GRID = ['..fmmm', '.ff.mm', 'f..T.m', '...rr.', '..rrr.', '.....f'];
export const SAMPLE_CELLS = {
  '3,2': { title: 'The Crossroads', description: 'A weathered crossroads where four dirt paths converge beneath an ancient oak. A carved wooden stag watches from atop a leaning signpost.', fixed_room: 'town/crossroads' },
  '2,0': { title: 'The Whispering Pines', description: 'Tall pines sway and creak in the wind. Their whispered conversations sound almost like words.' },
  '4,0': { title: 'Iron Peak Pass', description: 'A narrow pass between jagged peaks. The wind howls through the gap, carrying flecks of rust-colored stone.', encounters: [{ monster: 'orc', count: [2, 3] }] },
  '5,0': { title: 'The High Crag', description: 'A windswept crag overlooking the world below. An old watchtower, half-collapsed, clings to the edge.', objects: [{ key: 'watchtower', kind: 'item', title: 'a crumbling watchtower', description: 'Stone walls worn smooth by centuries of wind. Someone carved tally marks near the door — hundreds of them.' }] },
  '0,2': { title: 'The Old Grove', description: 'Ancient oaks with trunks wider than a cart. The canopy is so thick it feels like twilight even at noon.', objects: [{ key: 'hermit', kind: 'npc', title: 'Moss the Hermit', description: 'A gaunt figure wrapped in bark-cloth. They watch you with bright, curious eyes.' }] },
  '0,5': { title: 'The Standing Stone', description: 'A single standing stone rises from the grass, covered in spiraling runes. The air hums faintly.' },
  '5,5': { title: 'The Sunken Grove', description: 'The forest floor dips into a mossy hollow. A spring bubbles up from between the roots of a dead tree.' },
};

export const ATTR_TYPES = ['text', 'int', 'float', 'bool', 'enum', 'list'];

export function freshFromSample() {
  return {
    name: 'iron_hills',
    palette: structuredClone(SAMPLE_PALETTE),
    grid: SAMPLE_GRID.map((r) => r.split('').map((c) => (c === ' ' ? null : c))),
    cells: structuredClone(SAMPLE_CELLS),
    schema: [
      { key: 'spawn_table', type: 'text' },
      { key: 'danger', type: 'int' },
      { key: 'biome', type: 'enum', values: ['plains', 'forest', 'mountain', 'river', 'marsh'] },
    ],
    tool: 'paint',
    brush: 'f',
    selected: null,
  };
}

// ---------------- helpers ----------------
// choose dark/light glyph ink for a tile color
export function readable(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex || '');
  if (!m) return 'rgba(0,0,0,.5)';
  const n = parseInt(m[1], 16), r = (n >> 16) & 255, g = (n >> 8) & 255, b = n & 255;
  const L = 0.299 * r + 0.587 * g + 0.114 * b;
  return L > 150 ? 'rgba(0,0,0,.62)' : 'rgba(255,255,255,.82)';
}

export function dims(grid) {
  return { h: grid.length, w: grid.reduce((mx, r) => Math.max(mx, r.length), 0) };
}

export function schemaFor(schema, key) {
  return (schema || []).find((d) => d.key === key);
}

// ---------------- TOML serialize ----------------
const bareKey = (k) => /^[A-Za-z0-9_-]+$/.test(k);
const tkey = (k) => (bareKey(k) ? k : `"${String(k).replace(/"/g, '\\"')}"`);
const tstr = (s) => `"${String(s).replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n')}"`;

// infer a TOML value from a free-typed attribute string
export function tval(v) {
  const s = String(v == null ? '' : v).trim();
  if (s === 'true' || s === 'false') return s;
  if (/^-?\d+$/.test(s)) return s;
  if (/^-?(\d+\.\d*|\.\d+)$/.test(s)) return s;
  if (/^[[{"]/.test(s)) return s; // raw TOML: array, inline table, or already quoted
  return tstr(s);
}

export function emitAttrs(attrs, schema) {
  let o = '';
  for (const [k, v] of Object.entries(attrs || {})) {
    if (!k) continue;
    const def = schemaFor(schema, k);
    if (!def) { o += `${tkey(k)} = ${tval(v)}\n`; continue; } // ad-hoc: infer
    if (def.type === 'int') o += `${tkey(k)} = ${parseInt(v, 10) || 0}\n`;
    else if (def.type === 'float') o += `${tkey(k)} = ${parseFloat(v) || 0}\n`;
    else if (def.type === 'bool') o += `${tkey(k)} = ${String(v) === 'true' ? 'true' : 'false'}\n`;
    else if (def.type === 'list') o += `${tkey(k)} = ${/^\s*\[/.test(String(v)) ? String(v).trim() : '[' + String(v).split(',').filter((s) => s.trim()).map((s) => tstr(s.trim())).join(', ') + ']'}\n`;
    else o += `${tkey(k)} = ${tstr(v)}\n`; // text, enum → quoted string
  }
  return o;
}

// turn a parsed TOML value back into the string the attribute editor shows
export function fromVal(v) {
  if (Array.isArray(v)) return '[' + v.map((x) => (typeof x === 'string' ? tstr(x) : x)).join(', ') + ']';
  if (typeof v === 'boolean') return v ? 'true' : 'false';
  if (v == null) return '';
  return String(v);
}

export function serializeMap(state) {
  const { w, h } = dims(state.grid);
  const rows = [];
  for (let y = 0; y < h; y++) {
    let s = '';
    for (let x = 0; x < w; x++) { const c = (state.grid[y] || [])[x]; s += c == null ? '.' : c; }
    rows.push(s.replace(/\s+$/, ''));
  }
  let out = `[map]\nname = ${tstr(state.name || 'untitled')}\ngrid = """\n${rows.join('\n')}\n"""\n`;
  const keys = Object.keys(state.cells).sort((a, b) => { const [ax, ay] = a.split(',').map(Number), [bx, by] = b.split(',').map(Number); return ay - by || ax - bx; });
  for (const k of keys) {
    const c = state.cells[k]; const q = `cells.${tkey(k)}`;
    out += `\n[${q}]\n`;
    if (c.title) out += `title = ${tstr(c.title)}\n`;
    if (c.description) out += `description = ${tstr(c.description)}\n`;
    if (c.fixed_room) out += `fixed_room = ${tstr(c.fixed_room)}\n`;
    if (c.passable === false) out += `passable = false\n`;
    out += emitAttrs(c.attrs, state.schema);
    (c.objects || []).forEach((o) => {
      out += `\n[[${q}.objects]]\n`;
      out += `key = ${tstr(o.key || '')}\n`;
      out += `kind = ${tstr(o.kind || 'npc')}\n`;
      if (o.title) out += `title = ${tstr(o.title)}\n`;
      if (o.description) out += `description = ${tstr(o.description)}\n`;
    });
    (c.encounters || []).forEach((e) => {
      out += `\n[[${q}.encounters]]\n`;
      out += `monster = ${tstr(e.monster || '')}\n`;
      out += `count = [${e.count ? e.count[0] : 1}, ${e.count ? e.count[1] : 1}]\n`;
    });
  }
  return out;
}

export function serializePalette(state) {
  let out = `# Game-level terrain palette — every map inherits these square types.\n`;
  for (const d of state.schema || []) {
    out += `\n[[terrain_attr]]\nkey = ${tstr(d.key)}\ntype = ${tstr(d.type)}\n`;
    if (d.type === 'enum' && d.values && d.values.length) out += `values = [${d.values.map(tstr).join(', ')}]\n`;
  }
  for (const [ch, t] of Object.entries(state.palette)) {
    out += `\n[terrain.${tkey(ch)}]\n`;
    out += `theme = ${tstr(t.theme || '')}\n`;
    if (t.title_prefix) out += `title_prefix = ${tstr(t.title_prefix)}\n`;
    if (t.passable === false) out += `passable = false\n`;
    if (t.color) out += `color = ${tstr(t.color)}\n`;
    out += emitAttrs(t.attrs, state.schema);
  }
  return out;
}

// ---------------- TOML parse (focused subset) ----------------
export function parseTOML(text) {
  const root = {}; let cur = root; const lines = text.split(/\r?\n/);
  const getObj = (path, arr) => {
    let o = root;
    for (let i = 0; i < path.length; i++) {
      const last = i === path.length - 1; const k = path[i];
      if (last && arr) { o[k] = o[k] || []; const n = {}; o[k].push(n); return n; }
      o[k] = o[k] || (Array.isArray(o[k]) ? o[k] : {});
      if (Array.isArray(o[k])) o = o[k][o[k].length - 1]; else o = o[k];
    }
    return o;
  };
  const parsePath = (s) => {
    // split on dots but respect quotes
    const out = []; let buf = '', q = false;
    for (let i = 0; i < s.length; i++) { const c = s[i]; if (c === '"') { q = !q; buf += c; } else if (c === '.' && !q) { out.push(buf); buf = ''; } else buf += c; }
    out.push(buf);
    return out.map((p) => { p = p.trim(); if (p.startsWith('"') && p.endsWith('"')) return p.slice(1, -1).replace(/\\"/g, '"'); return p; });
  };
  const parseVal = (raw) => {
    raw = raw.trim();
    if (raw.startsWith('"""')) return raw; // handled elsewhere
    if (raw.startsWith('"')) return raw.slice(1, -1).replace(/\\n/g, '\n').replace(/\\"/g, '"').replace(/\\\\/g, '\\');
    if (raw === 'true') return true; if (raw === 'false') return false;
    if (raw.startsWith('[')) { const inner = raw.slice(1, -1).trim(); if (!inner) return []; return inner.split(',').map((v) => parseVal(v.trim())); }
    if (/^-?\d+$/.test(raw)) return parseInt(raw, 10);
    if (/^-?\d*\.\d+$/.test(raw)) return parseFloat(raw);
    return raw;
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]; const t = line.trim();
    if (!t || t.startsWith('#')) continue;
    let m;
    if ((m = /^\[\[(.+)\]\]$/.exec(t))) { cur = getObj(parsePath(m[1]), true); continue; }
    if ((m = /^\[(.+)\]$/.exec(t))) { cur = getObj(parsePath(m[1]), false); continue; }
    if ((m = /^([^=]+?)\s*=\s*(.*)$/.exec(t))) {
      const key = m[1].trim().replace(/^"|"$/g, ''); const val = m[2];
      if (val.trim().startsWith('"""')) {
        // multi-line basic string
        let acc = val.trim().slice(3); let closed = acc.endsWith('"""');
        if (closed) { acc = acc.slice(0, -3); }
        else { acc = acc.replace(/^\n/, ''); i++; for (; i < lines.length; i++) { if (lines[i].trim().endsWith('"""') || lines[i].includes('"""')) { acc += (acc ? '\n' : '') + lines[i].replace(/"""\s*$/, ''); closed = true; break; } acc += (acc ? '\n' : '') + lines[i]; } }
        acc = acc.replace(/^\n/, '');
        cur[key] = acc;
      } else {
        cur[key] = parseVal(val);
      }
    }
  }
  return root;
}

// importMap/importPalette are pure: they return the parsed slices for the
// caller to assign into reactive state (rather than mutating a global).
export function importMap(root) {
  if (!root.map || !root.map.grid) throw new Error('No [map] table with a grid found.');
  const gridRows = root.map.grid.replace(/^\n+|\n+$/g, '').split('\n');
  const name = root.map.name || 'untitled';
  const grid = gridRows.map((r) => r.split('').map((c) => (c === ' ' ? null : c)));
  const cells = {};
  const cellsTbl = root.cells || {};
  for (const [k, v] of Object.entries(cellsTbl)) {
    const c = {};
    if (v.title) c.title = v.title; if (v.description) c.description = v.description;
    if (v.fixed_room) c.fixed_room = v.fixed_room; if (v.passable === false) c.passable = false;
    if (Array.isArray(v.encounters)) c.encounters = v.encounters.map((e) => ({ monster: e.monster, count: Array.isArray(e.count) ? e.count : [1, 1] }));
    if (Array.isArray(v.objects)) c.objects = v.objects.map((o) => ({ key: o.key, kind: o.kind || 'npc', title: o.title, description: o.description }));
    const KNOWN_C = new Set(['title', 'description', 'fixed_room', 'passable', 'lock', 'encounters', 'objects']);
    const ca = {}; for (const [ak, av] of Object.entries(v)) if (!KNOWN_C.has(ak)) ca[ak] = fromVal(av);
    if (Object.keys(ca).length) c.attrs = ca;
    if (Object.keys(c).length) cells[k] = c;
  }
  return { name, grid, cells };
}

export function importPalette(root) {
  if (!root.terrain) throw new Error('No [terrain.*] tables found.');
  const p = {}; const KNOWN_T = new Set(['theme', 'title_prefix', 'passable', 'color']);
  for (const [ch, t] of Object.entries(root.terrain)) {
    const attrs = {}; for (const [k, v] of Object.entries(t)) if (!KNOWN_T.has(k)) attrs[k] = fromVal(v);
    p[ch] = { theme: t.theme || '', title_prefix: t.title_prefix || '', passable: t.passable !== false, color: t.color || '#88aa66', attrs };
  }
  let schema;
  if (Array.isArray(root.terrain_attr)) {
    schema = root.terrain_attr
      .filter((d) => d && d.key)
      .map((d) => ({ key: d.key, type: ATTR_TYPES.includes(d.type) ? d.type : 'text', values: Array.isArray(d.values) ? d.values.map(String) : undefined }));
  }
  return { palette: p, schema };
}
