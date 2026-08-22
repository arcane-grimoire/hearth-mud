// Directional layout for the room graph: rooms have no inherent coordinates,
// so we walk the exits from a root and place each target in the compass
// direction its exit names (n = up, e = right, …). Rooms with a saved
// position (persisted as _rx/_ry) keep it; otherwise the whole slice is
// auto-arranged. Svelte Flow owns pan/zoom/drag on top of these positions.

export const DIRS = {
  n: [0, -1], s: [0, 1], e: [1, 0], w: [-1, 0],
  ne: [1, -1], nw: [-1, -1], se: [1, 1], sw: [-1, 1],
  up: [0.7, -0.7], down: [-0.7, 0.7], in: [0.6, 0.6], out: [-0.6, -0.6],
};

export const REV = {
  n: 's', s: 'n', e: 'w', w: 'e',
  ne: 'sw', sw: 'ne', nw: 'se', se: 'nw',
  up: 'down', down: 'up', in: 'out', out: 'in',
};

// The world uses full direction words (north, southeast, up…); the builder's
// picker uses short codes (n, se, up). Normalise either to the short code so
// layout + handle routing speak one vocabulary.
const DIR_ALIAS = {
  north: 'n', south: 's', east: 'e', west: 'w',
  northeast: 'ne', northwest: 'nw', southeast: 'se', southwest: 'sw',
  up: 'up', down: 'down', in: 'in', out: 'out', u: 'up', d: 'down',
};
// Short code → the world's full word, for exits the builder creates.
export const DIR_FULL = {
  n: 'north', s: 'south', e: 'east', w: 'west',
  ne: 'northeast', nw: 'northwest', se: 'southeast', sw: 'southwest',
  up: 'up', down: 'down', in: 'in', out: 'out',
};
export function normDir(dir) {
  const k = String(dir || '').trim().toLowerCase();
  return DIR_ALIAS[k] || k;
}

// Handle geometry: 8 compass points on every node. Non-compass exits still
// need a handle and a layout offset — up/down/in/out go to distinct corners,
// and anything unknown (a custom named exit like `enter` or `gate`) is spread
// deterministically by name so they never all collapse onto one side.
export const HANDLE_VEC = {
  n: [0, -1], ne: [1, -1], e: [1, 0], se: [1, 1],
  s: [0, 1], sw: [-1, 1], w: [-1, 0], nw: [-1, -1],
};
export const HANDLE_OPP = { n: 's', s: 'n', e: 'w', w: 'e', ne: 'sw', sw: 'ne', nw: 'se', se: 'nw' };
const COMPASS8 = ['n', 'ne', 'e', 'se', 's', 'sw', 'w', 'nw'];
const DIR_HANDLE = {
  n: 'n', s: 's', e: 'e', w: 'w', ne: 'ne', nw: 'nw', se: 'se', sw: 'sw',
  up: 'ne', down: 'sw', in: 'se', out: 'nw',
};
const COMPASS_SET = new Set(COMPASS8);
function hashStr(s) {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
  return Math.abs(h);
}
/** The node-side handle an exit leaves from (one of the 8 compass points). */
export function dirToHandle(dir) {
  const nd = normDir(dir);
  return DIR_HANDLE[nd] || COMPASS8[hashStr(nd) % 8];
}
/** A true compass bearing gets a solid spatial edge; everything else is dashed. */
export function isCompass(dir) {
  return COMPASS_SET.has(normDir(dir));
}

// Compass order for the direction picker grid (nulls are layout gaps).
export const DIR_GRID = ['nw', 'n', 'ne', 'w', null, 'e', 'sw', 's', 'se'];
export const DIR_EXTRA = ['up', 'down', 'in', 'out'];

const GX = 250; // world units between columns
const GY = 175; // world units between rows

export function computeLayout(rooms, exits, saved = {}) {
  // If every room already has a saved position, honour the hand-laid graph.
  if (rooms.length && rooms.every((r) => saved[r.ref])) {
    const pos = {};
    for (const r of rooms) pos[r.ref] = { ...saved[r.ref] };
    return pos;
  }

  const ids = new Set(rooms.map((r) => r.ref));
  const out = {};
  const deg = {};
  for (const e of exits) {
    if (ids.has(e.from)) (out[e.from] ||= []).push(e);
    deg[e.from] = (deg[e.from] || 0) + 1;
    deg[e.to] = (deg[e.to] || 0) + 1;
  }
  // Most-connected rooms make the most stable roots.
  const order = [...ids].sort((a, b) => (deg[b] || 0) - (deg[a] || 0));

  const occupied = new Map(); // "gx,gy" -> ref
  const grid = {}; // ref -> {gx,gy}
  const pos = {};
  const key = (x, y) => `${x},${y}`;

  const place = (ref, gx, gy) => {
    while (occupied.has(key(gx, gy))) gx += 1; // nudge east off collisions
    occupied.set(key(gx, gy), ref);
    grid[ref] = { gx, gy };
    pos[ref] = { x: gx * GX, y: gy * GY };
  };

  for (const root of order) {
    if (grid[root]) continue;
    place(root, 0, 0);
    const queue = [root];
    while (queue.length) {
      const cur = queue.shift();
      const cg = grid[cur];
      for (const e of out[cur] || []) {
        if (!ids.has(e.to) || grid[e.to]) continue;
        const d = DIRS[normDir(e.dir)] || HANDLE_VEC[dirToHandle(e.dir)] || [1, 0];
        place(e.to, cg.gx + Math.round(d[0]), cg.gy + Math.round(d[1]));
        queue.push(e.to);
      }
    }
  }

  // Any isolated rooms with no exits: line them up along the top.
  let scatter = 0;
  for (const r of rooms) {
    if (!pos[r.ref]) { pos[r.ref] = { x: scatter * GX, y: -2 * GY }; scatter += 1; }
  }
  return pos;
}
