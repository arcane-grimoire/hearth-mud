<script>
  import { SvelteFlow, Background, Controls, MiniMap, ConnectionMode, MarkerType } from '@xyflow/svelte';
  import '@xyflow/svelte/dist/style.css';
  import { Button } from '@kenn-io/kit-ui';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import SaveIcon from '@lucide/svelte/icons/save';
  import XIcon from '@lucide/svelte/icons/x';
  import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';
  import RoomNode from './RoomNode.svelte';
  import StubNode from './StubNode.svelte';
  import {
    DIRS, REV, DIR_GRID, DIR_EXTRA, DIR_FULL, normDir, computeLayout,
    dirToHandle, HANDLE_OPP, HANDLE_VEC, isCompass,
  } from './layout.js';
  import { sampleWorld, sampleAreas } from './sample.js';
  import { api } from '../../lib/api.js';
  import { route, setQuery } from '../../lib/router.svelte.js';

  let { onedit = () => {}, reloadSignal = 0 } = $props();

  const nodeTypes = { room: RoomNode, stub: StubNode };

  let nodes = $state.raw([]);
  let edges = $state.raw([]);
  let areas = $state([]);
  let live = $state(false);
  let loading = $state(true);
  let truncated = $state(false);
  let reloadKey = $state(0); // bump remounts SvelteFlow so it re-fits on scope change

  let sel = $state(null); // selected room's data (inspector open when set)
  let iTitle = $state('');
  let iDesc = $state('');
  let dirty = $state(false);
  let saving = $state(false);
  let posDirty = $state(false);
  let toast = $state('');
  let picker = $state(null); // { source, target, dir, reverse }

  const scopeArea = $derived(route.query.area || '');
  const scopeNear = $derived(route.query.focus || '');
  const scopeDepth = $derived(route.query.depth || '2');

  // The area <select> binds to a local mirror of the route, kept in sync below.
  // Binding (vs. value={scopeArea}) avoids Svelte's controlled-select race,
  // where the box could snap back before the route round-trips.
  let areaSel = $state(route.query.area || '');
  $effect(() => { areaSel = scopeArea; });

  let toastT;
  function flash(msg) {
    toast = msg;
    clearTimeout(toastT);
    toastT = setTimeout(() => { toast = ''; }, 2400);
  }

  // Route each edge through the handle its direction points at (8-way), so an
  // east exit leaves the east side and enters the target's west side.
  // Non-compass exits (up/down/in/out, custom names) get a distinct handle and
  // a dashed style so they don't converge or masquerade as a bearing.
  function mkEdge(id, source, target, dir) {
    const sh = dirToHandle(dir);
    return {
      id: String(id), source, target,
      sourceHandle: sh, targetHandle: HANDLE_OPP[sh],
      label: dir, data: { dir },
      markerEnd: { type: MarkerType.ArrowClosed, width: 14, height: 14 },
      class: isCompass(dir) ? 'exit-edge' : 'exit-edge exit-special',
    };
  }

  function ingest(slice) {
    // The live API keys entities as `ref_id`; the sample world uses `ref`.
    // Normalise to `ref` so everything downstream is uniform.
    const norm = (o) => ({ ...o, ref: o.ref ?? o.ref_id });
    const rooms = (slice.rooms || []).map(norm);
    const exits = (slice.exits || []).map(norm);
    const boundary = (slice.boundary || []).map(norm);
    // Honour saved layout positions (_rx/_ry) the slice returns.
    const saved = {};
    for (const r of rooms) {
      if (typeof r.rx === 'number' && typeof r.ry === 'number') saved[r.ref] = { x: r.rx, y: r.ry };
    }
    const pos = computeLayout(rooms, exits, saved);
    const roomIds = new Set(rooms.map((r) => r.ref));

    const stubPos = {};
    for (const b of boundary) {
      const link = exits.find((e) => e.to === b.ref && pos[e.from]);
      if (link) {
        const d = DIRS[normDir(link.dir)] || HANDLE_VEC[dirToHandle(link.dir)] || [1, 0];
        const p = pos[link.from];
        stubPos[b.ref] = { x: p.x + d[0] * 150, y: p.y + d[1] * 130 };
      } else {
        stubPos[b.ref] = { x: 0, y: 0 };
      }
    }

    nodes = [
      ...rooms.map((r) => ({ id: r.ref, type: 'room', position: pos[r.ref] || { x: 0, y: 0 }, data: { ...r } })),
      ...boundary.map((b) => ({ id: b.ref, type: 'stub', position: stubPos[b.ref], data: { ...b } })),
    ];
    edges = exits
      .filter((e) => roomIds.has(e.from))
      .map((e) => mkEdge(e.ref, e.from, e.to, e.dir));
    truncated = !!slice.truncated;
    reloadKey += 1;
  }

  async function load() {
    loading = true;
    sel = null;
    dirty = false;
    // Clear the old slice so a scope change never lingers as stale-but-current.
    nodes = [];
    edges = [];
    try {
      const ar = await api('list_areas');
      if (ar?.ok) areas = ar.data;
    } catch (e) { /* offline */ }

    const params = {};
    if (scopeNear) { params.near = scopeNear; params.depth = Number(scopeDepth) || 2; }
    else if (scopeArea) { params.area = scopeArea; }

    try {
      const res = await api('list_world_slice', params);
      if (res?.ok) { live = true; ingest(res.data); loading = false; return; }
    } catch (e) { /* offline */ }

    // Fallback: the sample hamlet, so this is a live demo with no engine.
    live = false;
    if (!areas.length) areas = sampleAreas;
    ingest(sampleWorld);
    loading = false;
  }

  let lastScope = null;
  $effect(() => {
    const k = `${scopeArea}|${scopeNear}|${scopeDepth}|${reloadSignal}`;
    if (k !== lastScope) { lastScope = k; load(); }
  });

  // --- scope bar ---
  function onAreaChange(e) { setQuery({ area: e.target.value || null, focus: null }); }
  function clearFocus() { setQuery({ focus: null }); }

  // --- node interactions ---
  function handleNodeClick(ev) {
    const node = ev?.node || ev?.targetNode || ev?.detail?.node || ev;
    if (!node || !node.type) return;
    if (node.type === 'stub') { setQuery({ focus: node.id, area: null, depth: scopeDepth || '2' }); return; }
    selectRoom(node);
  }
  function selectRoom(node) {
    sel = { ...node.data };
    iTitle = node.data.title || '';
    iDesc = node.data.description || '';
    dirty = false;
  }
  function closeInspector() { sel = null; }
  function markDirty() { dirty = true; }

  // --- connect: drag from a handle (its side names the direction) ---
  function handleConnect(conn) {
    if (!conn?.source || !conn?.target || conn.source === conn.target) return;
    if (!nodes.some((n) => n.id === conn.target && n.type === 'room')) return; // don't link into a stub
    picker = { source: conn.source, target: conn.target, dir: conn.sourceHandle || 'e', reverse: true };
  }
  async function commitExit(code) {
    const { source, target, reverse } = picker;
    picker = null;
    // Create exits with the world's full direction word (north, not n) so
    // they match existing content and respond to full-word movement.
    const dir = DIR_FULL[code] || code;
    const revCode = REV[code];
    const revDir = revCode ? DIR_FULL[revCode] || revCode : null;
    const add = [];
    if (live) {
      const r = await api('create_exit', { source, direction: dir, target, aliases: null });
      if (!r?.ok) { flash('Exit failed: ' + (r?.error || '?')); return; }
      add.push(mkEdge(r.data.ref_id, source, target, dir));
      if (reverse && revDir) {
        const r2 = await api('create_exit', { source: target, direction: revDir, target: source, aliases: null });
        if (r2?.ok) add.push(mkEdge(r2.data.ref_id, target, source, revDir));
      }
    } else {
      add.push(mkEdge('demo-' + Math.random().toString(36).slice(2), source, target, dir));
      if (reverse && revDir) add.push(mkEdge('demo-' + Math.random().toString(36).slice(2), target, source, revDir));
    }
    edges = [...edges, ...add];
    flash(live ? `Exit ${dir} created` : `Exit ${dir} drawn (demo)`);
  }

  // --- add room (stamped into the scoped area) ---
  async function addRoom() {
    const rs = nodes.filter((n) => n.type === 'room');
    const cx = rs.length ? rs.reduce((s, n) => s + n.position.x, 0) / rs.length : 0;
    const cy = rs.length ? rs.reduce((s, n) => s + n.position.y, 0) / rs.length : 0;
    const key = 'room_' + Math.random().toString(36).slice(2, 6);
    let ref = 'demo-' + key;
    if (live) {
      const r = await api('create_room', { area: scopeArea || '', key, title: 'New Room', description: '' });
      if (!r?.ok) { flash('Create failed: ' + (r?.error || '?')); return; }
      ref = r.data.ref_id;
    }
    const node = {
      id: ref, type: 'room', position: { x: cx + 40, y: cy - 30 },
      data: { ref, key, title: 'New Room', description: '', area: scopeArea || '', tags: [] },
    };
    nodes = [...nodes, node];
    selectRoom(node);
    flash(live ? `Room ${ref} created` : 'Room added (demo)');
  }

  // --- inspector apply ---
  async function applyInspector() {
    if (!sel) return;
    saving = true;
    const title = iTitle.trim() || sel.key;
    if (live) {
      if (title !== sel.title) await api('set_title', { ref_id: sel.ref, title });
      if (iDesc !== sel.description) await api('set_description', { ref_id: sel.ref, description: iDesc });
    }
    nodes = nodes.map((n) => (n.id === sel.ref ? { ...n, data: { ...n.data, title, description: iDesc } } : n));
    sel = { ...sel, title, description: iDesc };
    dirty = false;
    saving = false;
    flash(live ? 'Room saved' : 'Room updated (demo)');
  }

  // --- save layout (persist node positions as _rx/_ry) ---
  async function saveLayout() {
    if (!live) { posDirty = false; flash('Layout saved (demo)'); return; }
    saving = true;
    for (const n of nodes.filter((n) => n.type === 'room')) {
      await api('set_attribute', { ref_id: n.id, key: '_rx', value: Math.round(n.position.x) });
      await api('set_attribute', { ref_id: n.id, key: '_ry', value: Math.round(n.position.y) });
    }
    posDirty = false;
    saving = false;
    flash('Layout saved');
  }
  function onNodeDragStop() { posDirty = true; }

  const selExits = $derived(sel ? edges.filter((e) => e.source === sel.ref) : []);
  function nameFor(ref) {
    const n = nodes.find((x) => x.id === ref);
    return n ? n.data.title || n.data.key : ref;
  }
  function gotoRoom(ref) {
    const n = nodes.find((x) => x.id === ref);
    if (n && n.type === 'room') selectRoom(n);
  }
</script>

<div class="rg">
  <!-- scope bar -->
  <div class="rg-scope">
    <label class="rg-field">
      <span>Area</span>
      <select bind:value={areaSel} onchange={onAreaChange} disabled={!!scopeNear}>
        <option value="">All rooms</option>
        {#each areas as a}
          <option value={a.area}>{a.area || '(unfiled)'} · {a.count}</option>
        {/each}
      </select>
    </label>

    {#if scopeNear}
      <span class="rg-focus">focus <b>{scopeNear}</b> · depth {scopeDepth}
        <button class="rg-x" onclick={clearFocus} aria-label="Clear focus"><XIcon size={12} /></button>
      </span>
    {/if}

    <span class="rg-spacer"></span>

    {#if truncated}
      <span class="rg-warn">showing a capped slice — narrow the filter</span>
    {/if}
    <span class="rg-pill" class:live>{live ? 'live game' : 'sample world'}</span>

    <Button size="sm" onclick={addRoom}><PlusIcon size={14} /> Room</Button>
    <Button size="sm" tone={posDirty ? 'accent' : undefined} onclick={saveLayout} disabled={saving}>
      <SaveIcon size={14} /> Save layout{posDirty ? ' •' : ''}
    </Button>
  </div>

  <!-- canvas -->
  <div class="rg-canvas">
    {#key reloadKey}
      <SvelteFlow
        bind:nodes
        bind:edges
        {nodeTypes}
        connectionMode={ConnectionMode.Loose}
        fitView
        fitViewOptions={{ padding: 0.25 }}
        minZoom={0.15}
        maxZoom={2.5}
        onnodeclick={handleNodeClick}
        onconnect={handleConnect}
        onnodedragstop={onNodeDragStop}
      >
        <Background gap={26} />
        <Controls showLock={false} />
        <MiniMap pannable zoomable nodeColor="var(--accent-amber, #c9956b)" />
      </SvelteFlow>
    {/key}

    {#if loading}
      <div class="rg-loading">loading world…</div>
    {/if}

    <!-- inspector -->
    {#if sel}
      <aside class="rg-insp">
        <header>
          <div class="ri-eyebrow">Room · <span class="ri-ref">{sel.ref}</span></div>
          <button class="rg-x" onclick={closeInspector} aria-label="Close"><XIcon size={15} /></button>
        </header>
        <div class="ri-body">
          <label class="ri-lbl" for="ri-title">Title</label>
          <input id="ri-title" class="ri-in ri-title" bind:value={iTitle} oninput={markDirty} />

          <label class="ri-lbl" for="ri-key">Key</label>
          <input id="ri-key" class="ri-in ri-mono" value={sel.key} readonly />

          <label class="ri-lbl" for="ri-desc">Description</label>
          <textarea id="ri-desc" class="ri-in ri-desc" bind:value={iDesc} oninput={markDirty}></textarea>

          {#if sel.tags?.length}
            <div class="ri-lbl">Tags</div>
            <div class="ri-tags">{#each sel.tags as t}<span class="ri-tag">{t}</span>{/each}</div>
          {/if}

          <div class="ri-sec">Exits</div>
          {#if selExits.length}
            {#each selExits as e}
              <div class="ri-exit">
                <span class="ri-dir">{e.data.dir}</span>
                <span class="ri-to">to <b>{nameFor(e.target)}</b></span>
                <button class="ri-go" onclick={() => gotoRoom(e.target)} aria-label="Go"><ArrowRightIcon size={13} /></button>
              </div>
            {/each}
          {:else}
            <div class="ri-empty">No exits lead out yet — drag from a side handle to draw one.</div>
          {/if}
        </div>
        <footer>
          <button class="ri-full" onclick={() => onedit(sel.ref)}>Full editor…</button>
          <span class="ri-dirty" class:on={dirty}>unsaved</span>
          <Button size="sm" onclick={applyInspector} disabled={!dirty || saving}>Apply</Button>
        </footer>
      </aside>
    {/if}

    <!-- direction picker -->
    {#if picker}
      <div class="rg-pickwrap" role="dialog" aria-label="Choose exit direction">
        <button class="rg-pickback" aria-label="Cancel" onclick={() => (picker = null)}></button>
        <div class="rg-pick">
          <div class="rg-pick-t">exit <b>{nameFor(picker.source)}</b> → <b>{nameFor(picker.target)}</b></div>
          <div class="rg-pick-grid">
            {#each DIR_GRID as d}
              {#if d}
                <button class:pre={d === picker.dir} onclick={() => commitExit(d)}>{d}</button>
              {:else}
                <span></span>
              {/if}
            {/each}
          </div>
          <div class="rg-pick-extra">
            {#each DIR_EXTRA as d}<button class:pre={d === picker.dir} onclick={() => commitExit(d)}>{d}</button>{/each}
          </div>
          <label class="rg-pick-rev"><input type="checkbox" bind:checked={picker.reverse} /> also add the return exit</label>
        </div>
      </div>
    {/if}

    {#if toast}<div class="rg-toast">{toast}</div>{/if}
  </div>
</div>

<style>
  .rg { display: flex; flex-direction: column; height: 100%; min-height: 0; }

  /* scope bar */
  .rg-scope {
    display: flex; align-items: center; gap: 12px;
    padding: 8px 14px;
    border-bottom: var(--border-width, 1px) solid var(--border-default, #2a2419);
    background: var(--bg-surface, #17140f);
  }
  .rg-field { display: flex; align-items: center; gap: 7px; font-size: 12px; color: var(--text-muted, #9a9186); }
  .rg-field select {
    font: inherit; font-size: 12.5px; color: var(--text-primary, #ece0c8);
    background: var(--bg-primary, #12100c);
    border: 1px solid var(--border-default, #332c22);
    border-radius: var(--radius-md, 7px);
    padding: 5px 8px;
  }
  .rg-field select:disabled { opacity: 0.5; }
  .rg-focus { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-muted, #9a9186); display: inline-flex; align-items: center; gap: 6px; }
  .rg-focus b { color: var(--accent-amber, #c9956b); }
  .rg-spacer { flex: 1; }
  .rg-warn { font-size: 11.5px; color: var(--accent-red, #d07a5a); }
  .rg-pill {
    font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta);
    padding: 3px 8px; border-radius: 999px;
    background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22);
    color: var(--text-muted, #9a9186);
  }
  .rg-pill.live { color: var(--accent-green, #8fb877); border-color: color-mix(in srgb, var(--accent-green, #8fb877) 40%, transparent); }
  .rg-x { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; padding: 2px; border-radius: 4px; line-height: 0; }
  .rg-x:hover { color: var(--accent-red, #d07a5a); background: var(--bg-primary, #12100c); }

  /* canvas */
  .rg-canvas { position: relative; flex: 1; min-height: 0; }
  .rg-canvas :global(.svelte-flow) { background: var(--bg-primary, #100e0b); }
  .rg-canvas :global(.svelte-flow__background) { color: var(--border-muted, #241f1a); }
  .rg-canvas :global(.svelte-flow__handle) {
    width: 9px; height: 9px; background: var(--accent-amber, #c9956b);
    border: 1.5px solid var(--bg-surface, #17140f); opacity: 0; transition: opacity 0.12s;
  }
  .rg-canvas :global(.svelte-flow__node:hover .svelte-flow__handle),
  .rg-canvas :global(.svelte-flow__node.selected .svelte-flow__handle) { opacity: 1; }
  .rg-canvas :global(.exit-edge .svelte-flow__edge-path) { stroke: var(--edge, #9c8863); stroke-width: 1.75; }
  .rg-canvas :global(.exit-special .svelte-flow__edge-path) { stroke-dasharray: 5 4; stroke: var(--accent-amber, #c9956b); }
  .rg-canvas :global(.svelte-flow__edge-label) {
    background: var(--bg-surface, #17140f);
    color: var(--text-muted, #b6a888);
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: var(--fs-meta); line-height: 1.4;
    padding: 1px 5px; border-radius: 4px;
    border: 1px solid var(--border-muted, #2a2419);
  }
  .rg-canvas :global(.svelte-flow__controls) { box-shadow: 0 4px 14px -8px rgba(0,0,0,.6); }
  .rg-canvas :global(.svelte-flow__controls-button) { background: var(--bg-surface, #17140f); border-bottom: 1px solid var(--border-default, #2a2419); color: var(--text-primary, #ece0c8); fill: currentColor; }
  .rg-canvas :global(.svelte-flow__controls-button:hover) { background: var(--bg-primary, #12100c); }
  .rg-canvas :global(.svelte-flow__minimap) { background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #2a2419); border-radius: 8px; }

  .rg-loading { position: absolute; inset: 0; display: grid; place-items: center; color: var(--text-muted, #9a9186); font-size: 13px; pointer-events: none; }

  /* inspector */
  .rg-insp {
    position: absolute; top: 12px; right: 12px; bottom: 12px; width: 310px; z-index: 6;
    display: flex; flex-direction: column;
    background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #2a2419);
    border-radius: 12px; box-shadow: 0 12px 40px -18px rgba(0,0,0,.7);
  }
  .rg-insp header { display: flex; align-items: center; justify-content: space-between; padding: 11px 13px; border-bottom: 1px solid var(--border-default, #2a2419); }
  .ri-eyebrow { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-label); letter-spacing: .12em; text-transform: uppercase; color: var(--text-muted, #9a9186); }
  .ri-ref { color: var(--accent-amber, #c9956b); }
  .ri-body { padding: 12px 13px; overflow-y: auto; flex: 1; }
  .ri-lbl { display: block; font-size: var(--fs-label); font-weight: 600; letter-spacing: .06em; text-transform: uppercase; color: var(--text-muted, #9a9186); margin: 12px 0 5px; }
  .ri-lbl:first-child { margin-top: 0; }
  .ri-in {
    width: 100%; font: inherit; font-size: 13px; color: var(--text-primary, #ece0c8);
    background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22);
    border-radius: 7px; padding: 7px 9px; box-sizing: border-box;
  }
  .ri-in:focus { outline: none; border-color: var(--accent-amber, #c9956b); }
  .ri-title { font-weight: 600; }
  .ri-mono { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; opacity: .8; }
  .ri-desc { min-height: 82px; line-height: 1.5; resize: vertical; }
  .ri-tags { display: flex; flex-wrap: wrap; gap: 4px; }
  .ri-tag { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); padding: 2px 5px; border-radius: 4px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-muted, #2a2419); color: var(--text-muted, #9a9186); }
  .ri-sec { font-size: var(--fs-label); font-weight: 600; letter-spacing: .06em; text-transform: uppercase; color: var(--text-muted, #9a9186); margin: 16px 0 8px; padding-bottom: 5px; border-bottom: 1px solid var(--border-muted, #2a2419); }
  .ri-exit { display: flex; align-items: center; gap: 8px; padding: 6px 8px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-muted, #2a2419); border-radius: 7px; margin-bottom: 5px; }
  .ri-dir { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-badge); text-transform: uppercase; color: var(--bg-primary, #12100c); background: var(--edge, #9c8863); border-radius: 4px; padding: 1px 6px; min-width: 30px; text-align: center; }
  .ri-to { flex: 1; font-size: 12px; color: var(--text-muted, #b6a888); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ri-to b { color: var(--text-primary, #ece0c8); }
  .ri-go { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; padding: 2px; line-height: 0; border-radius: 4px; }
  .ri-go:hover { color: var(--accent-amber, #c9956b); }
  .ri-empty { font-size: 12px; color: var(--text-muted, #9a9186); font-style: italic; line-height: 1.5; }
  .rg-insp footer { display: flex; align-items: center; gap: 8px; padding: 10px 13px; border-top: 1px solid var(--border-default, #2a2419); }
  .ri-full { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; font: inherit; font-size: 12px; padding: 4px 2px; }
  .ri-full:hover { color: var(--accent-amber, #c9956b); text-decoration: underline; }
  .ri-dirty { margin-right: auto; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--accent-amber, #c9956b); opacity: 0; }
  .ri-dirty.on { opacity: 1; }

  /* direction picker */
  .rg-pickwrap { position: absolute; inset: 0; z-index: 8; display: grid; place-items: center; }
  .rg-pickback { position: absolute; inset: 0; background: rgba(0,0,0,.35); border: none; cursor: pointer; }
  .rg-pick { position: relative; background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #2a2419); border-radius: 12px; padding: 14px; box-shadow: 0 16px 44px -16px rgba(0,0,0,.7); }
  .rg-pick-t { font-size: 12px; color: var(--text-muted, #9a9186); text-align: center; margin-bottom: 10px; }
  .rg-pick-t b { color: var(--text-primary, #ece0c8); }
  .rg-pick-grid { display: grid; grid-template-columns: repeat(3, 46px); grid-auto-rows: 36px; gap: 5px; }
  .rg-pick-extra { display: grid; grid-template-columns: repeat(4, 1fr); gap: 5px; margin-top: 5px; }
  .rg-pick button, .rg-pick-extra button {
    font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-primary, #ece0c8);
    background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 7px; cursor: pointer;
  }
  .rg-pick button:hover, .rg-pick-extra button:hover { border-color: var(--accent-amber, #c9956b); color: var(--accent-amber, #c9956b); }
  .rg-pick button.pre { border-color: var(--accent-amber, #c9956b); color: var(--accent-amber, #c9956b); box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent-amber, #c9956b) 25%, transparent); }
  .rg-pick-rev { display: flex; align-items: center; gap: 7px; margin-top: 11px; font-size: 11.5px; color: var(--text-muted, #b6a888); }
  .rg-pick-rev input { accent-color: var(--accent-amber, #c9956b); }

  .rg-toast {
    position: absolute; left: 50%; bottom: 18px; transform: translateX(-50%); z-index: 9;
    padding: 8px 15px; border-radius: 8px; font-size: 12.5px; font-weight: 500;
    background: var(--text-primary, #ece0c8); color: var(--bg-primary, #12100c);
    box-shadow: 0 10px 30px -10px rgba(0,0,0,.6);
  }
</style>
