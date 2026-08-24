<script>
  import { Button, showFlash, Tooltip } from '@kenn-io/kit-ui';
  import SaveIcon from '@lucide/svelte/icons/save';
  import DownloadIcon from '@lucide/svelte/icons/download';
  import UploadIcon from '@lucide/svelte/icons/upload';
  import { api } from '../../../lib/api.js';
  import {
    freshFromSample, dims, parseTOML, importMap, importPalette,
    serializeMap, serializePalette,
  } from '../../../lib/mapwright-toml.js';
  import TerrainPalette from './TerrainPalette.svelte';
  import MapGrid from './MapGrid.svelte';
  import RoomInspector from './RoomInspector.svelte';
  import TerrainModal from './TerrainModal.svelte';
  import SchemaModal from './SchemaModal.svelte';
  import ImportExportModal from './ImportExportModal.svelte';

  // Native map builder — a Svelte port of mapwright.html, themed with kit-ui.
  // `name` deep-links to a specific server map; null falls back to the first (or
  // the Iron Hills sample when there's no server).
  let { name = null } = $props();

  let m = $state(freshFromSample());
  let server = $state(false);
  let mapList = $state([]);
  let loading = $state(true);

  let terrainOpen = $state(false);
  let editingCh = $state(null);
  let schemaOpen = $state(false);
  let ioMode = $state(null); // null | 'import' | 'export'

  const persistKey = () => 'mapwright:map:' + (m.name || 'untitled');
  function persist() {
    try {
      const { name: nm, palette, grid, cells, schema } = m;
      localStorage.setItem(persistKey(), JSON.stringify({ name: nm, palette, grid, cells, schema, brush: m.brush }));
    } catch (e) { /* ignore */ }
  }

  $effect(() => { boot(); });

  async function boot() {
    loading = true;
    let list;
    try { list = await api('list_maps'); } catch (e) { list = null; }
    if (list?.ok && Array.isArray(list.data?.maps)) {
      server = true;
      mapList = list.data.maps;
      // shared palette + schema (list_maps returns terrain toml; fall back to get_terrain)
      let terrToml = list.data.terrain;
      if (!terrToml) { try { const t = await api('get_terrain'); if (t?.ok) terrToml = t.data?.toml; } catch (e) { /* ignore */ } }
      if (terrToml) {
        try {
          const { palette, schema } = importPalette(parseTOML(terrToml));
          m.palette = palette;
          if (schema) m.schema = schema;
          if (!m.palette[m.brush]) m.brush = Object.keys(palette)[0];
        } catch (e) { /* keep sample palette */ }
      }
      const target = name && mapList.includes(name) ? name : mapList.includes(m.name) ? m.name : mapList[0];
      if (target) await loadMap(target);
    } else {
      server = false; // standalone/no server — stay on the sample
    }
    loading = false;
  }

  async function loadMap(target) {
    const j = await api('get_map', { name: target });
    if (!j?.ok) { showFlash(j?.error || `couldn't open ${target}`, { tone: 'danger' }); return; }
    try {
      const parsed = importMap(parseTOML(j.data.toml));
      m.name = j.data.name || target;
      m.grid = parsed.grid;
      m.cells = parsed.cells;
      m.selected = null;
      persist();
    } catch (e) { showFlash('Parse error: ' + (e.message || e), { tone: 'danger' }); }
  }

  async function saveGame() {
    const nm = (m.name || 'untitled').trim().replace(/[^A-Za-z0-9_-]/g, '_');
    const j = await api('put_map', { name: nm, toml: serializeMap(m) });
    if (j?.ok) {
      showFlash(nm + '.toml saved to the game', { tone: 'success' });
      try { const l = await api('list_maps'); if (l?.ok) mapList = l.data.maps; } catch (e) { /* ignore */ }
    } else showFlash('Save failed: ' + (j?.error || '?'), { tone: 'danger' });
  }
  async function saveDoc(doc) {
    if (doc === 'palette') {
      const j = await api('put_terrain', { toml: serializePalette(m) });
      showFlash(j?.ok ? 'terrain.toml saved to the game' : 'Save failed: ' + (j?.error || '?'), { tone: j?.ok ? 'success' : 'danger' });
    } else { await saveGame(); }
  }

  // ── grid interaction ─────────────────────────────────────────────
  function applyAt(x, y) {
    const { w, h } = dims(m.grid);
    if (x < 0 || y < 0 || x >= w || y >= h) return;
    if (!m.grid[y]) m.grid[y] = [];
    while (m.grid[y].length < w) m.grid[y].push(null);
    if (m.tool === 'paint') m.grid[y][x] = m.brush;
    else if (m.tool === 'erase') m.grid[y][x] = null;
    persist();
  }
  function selectCell(x, y) { m.selected = x + ',' + y; }

  function pruneCells(w, h) {
    for (const k of Object.keys(m.cells)) { const [x, y] = k.split(',').map(Number); if (x >= w || y >= h) delete m.cells[k]; }
    if (m.selected) { const [x, y] = m.selected.split(',').map(Number); if (x >= w || y >= h) m.selected = null; }
  }
  function onDim(dim, d) {
    const { w, h } = dims(m.grid);
    if (dim === 'w') {
      const nw = Math.max(1, Math.min(40, w + d));
      m.grid.forEach((r) => { if (d > 0) r.push(null); else if (r.length >= w) r.length = nw; });
      if (d < 0) pruneCells(nw, h);
    } else {
      const nh = Math.max(1, Math.min(40, h + d));
      if (d > 0) m.grid.push(new Array(w).fill(null));
      else { m.grid.length = nh; pruneCells(w, nh); }
    }
    persist();
  }
  function reset() {
    if (!confirm('Discard the current map and reload the Iron Hills sample?')) return;
    const f = freshFromSample();
    m.name = f.name; m.palette = f.palette; m.grid = f.grid; m.cells = f.cells; m.schema = f.schema;
    m.brush = f.brush; m.tool = 'paint'; m.selected = null;
    persist();
    showFlash('Reset to sample', { tone: 'info' });
  }

  // ── terrain modal ────────────────────────────────────────────────
  function onSaveTerrain(char, def, oldCh) {
    if (oldCh && oldCh !== char) {
      delete m.palette[oldCh];
      for (const row of m.grid) for (let x = 0; x < row.length; x++) if (row[x] === oldCh) row[x] = char;
    }
    m.palette[char] = def; m.brush = char; m.tool = 'paint';
    persist(); terrainOpen = false;
  }
  function onDeleteTerrain(ch) {
    delete m.palette[ch];
    if (m.brush === ch) m.brush = Object.keys(m.palette)[0] || 'f';
    persist(); terrainOpen = false;
    showFlash(`Terrain ‘${ch}’ removed`, { tone: 'info' });
  }

  // ── import / export ──────────────────────────────────────────────
  function onImport(doc, root) {
    if (doc === 'map') {
      const parsed = importMap(root);
      m.name = parsed.name; m.grid = parsed.grid; m.cells = parsed.cells; m.selected = null;
    } else {
      const { palette, schema } = importPalette(root);
      m.palette = palette; if (schema) m.schema = schema;
      if (!m.palette[m.brush]) m.brush = Object.keys(palette)[0];
    }
    persist();
  }

  function onKey(e) {
    if ((e.metaKey || e.ctrlKey) && (e.key === 's' || e.key === 'S')) { e.preventDefault(); if (server) saveGame(); else (ioMode = 'export'); return; }
    if (e.target.matches('input,textarea,select')) return;
    if (e.key === '1') m.tool = 'paint';
    if (e.key === '2') m.tool = 'erase';
    if (e.key === '3') m.tool = 'inspect';
  }

  const hint = $derived(m.tool === 'inspect' ? 'click a tile to edit its room details' : m.tool === 'erase' ? 'click & drag to clear tiles' : 'click & drag to paint · switch to Inspect to edit a room');
</script>

<svelte:window onkeydown={onKey} />

<div class="mb">
  <header class="bar">
    <div class="mapname">
      <label for="mb-name">map</label>
      <input id="mb-name" type="text" spellcheck="false" bind:value={m.name} oninput={persist} />
      {#if server}<span class="live">● live</span>{/if}
    </div>
    <span class="spacer"></span>
    {#if server}<Tooltip text="Save map to the game (⌘S)"><Button size="sm" tone="accent" onclick={saveGame}><SaveIcon size={13} /> Save to game</Button></Tooltip>{/if}
    <Button size="sm" onclick={() => (ioMode = 'import')}><UploadIcon size={13} /> Import</Button>
    <Button size="sm" tone="accent" surface="soft" onclick={() => (ioMode = 'export')}><DownloadIcon size={13} /> Export TOML</Button>
  </header>

  {#if loading}
    <div class="loading">Loading map…</div>
  {:else}
    <div class="cols">
      <TerrainPalette {m}
        ontool={(t) => (m.tool = t)}
        onbrush={(ch) => { m.brush = ch; m.tool = 'paint'; }}
        onedit={(ch) => { editingCh = ch; terrainOpen = true; }}
        onadd={() => { editingCh = null; terrainOpen = true; }}
        onschema={() => (schemaOpen = true)}
        ondim={onDim}
        onreset={reset} />
      <section class="stage">
        <MapGrid {m} onapply={applyAt} onselect={selectCell} />
        <p class="hint">{hint}</p>
      </section>
      <RoomInspector {m} onchange={persist} />
    </div>
  {/if}
</div>

{#if terrainOpen}
  {#key editingCh}
    <TerrainModal {editingCh} palette={m.palette} schema={m.schema}
      onsave={onSaveTerrain} ondelete={onDeleteTerrain} onclose={() => (terrainOpen = false)} />
  {/key}
{/if}
{#if schemaOpen}
  <SchemaModal schema={m.schema} onchange={(s) => { m.schema = s; persist(); }} onclose={() => (schemaOpen = false)} />
{/if}
{#if ioMode}
  <ImportExportModal mode={ioMode} mapState={m} {server} {onimport} onsavedoc={saveDoc} onclose={() => (ioMode = null)} />
{/if}

<style>
  .mb { display: flex; flex-direction: column; height: 100%; min-height: 0; background: var(--bg-primary); }
  .bar { display: flex; align-items: center; gap: 10px; padding: 9px 14px; border-bottom: 1px solid var(--border-default); background: var(--bg-surface); }
  .mapname { display: flex; align-items: center; gap: 8px; }
  .mapname label { font-size: 11px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-muted); }
  .mapname input { font-family: var(--font-mono); font-size: 13px; font-weight: 500; background: var(--bg-inset); border: 1px solid var(--border-default); color: var(--text-primary); border-radius: 6px; padding: 5px 9px; width: 150px; outline: none; }
  .mapname input:focus { border-color: var(--accent-amber); }
  .live { font-family: var(--font-mono); font-size: var(--fs-meta); letter-spacing: 0.1em; text-transform: uppercase; color: var(--accent-green); }
  .spacer { flex: 1; }
  .loading { padding: 24px; color: var(--text-muted); font-style: italic; }
  .cols { flex: 1; min-height: 0; display: grid; grid-template-columns: 236px minmax(0, 1fr) 340px; }
  .stage { min-width: 0; overflow: auto; padding: 28px; display: grid; place-items: center; align-content: center; gap: 12px;
    background: radial-gradient(circle at 1px 1px, color-mix(in srgb, var(--text-muted) 18%, transparent) 1px, transparent 0) 0 0/22px 22px, var(--bg-primary); }
  .hint { font-family: var(--font-mono); font-size: 11px; color: var(--text-muted); text-align: center; margin: 0; }

  @media (max-width: 900px) {
    .cols { grid-template-columns: 1fr; grid-template-rows: auto auto auto; }
  }
</style>
