<script>
  import { Table, TableHeaderCell, SearchInput, Chip } from '@kenn-io/kit-ui';
  import WaypointsIcon from '@lucide/svelte/icons/waypoints';
  import { api } from '../../lib/api.js';
  import { route, setQuery } from '../../lib/router.svelte.js';
  import { sampleWorld, sampleAreas } from './sample.js';

  // The table view: a flat, sortable, searchable list of every object — not
  // just rooms. NPCs, items and players otherwise hide inside a room's
  // Contents; here a Kind filter surfaces them alongside rooms. Row → open the
  // modal editor; the trailing button (rooms only) jumps to that room focused
  // in the graph. Fed by `list_objects_full` (capped, so sort/filter is a
  // plain derived — no data-grid library needed).
  let { onopen = () => {}, onedit = () => {}, reloadSignal = 0 } = $props();

  const KINDS = [
    { value: 'all', label: 'All objects' },
    { value: 'room', label: 'Rooms' },
    { value: 'npc', label: 'NPCs' },
    { value: 'item', label: 'Items' },
    { value: 'player', label: 'Players' },
  ];

  let rows = $state([]);
  let areas = $state([]);
  let live = $state(false);
  let loading = $state(true);
  let truncated = $state(false);
  let q = $state('');
  let sortKey = $state('title');
  let sortAsc = $state(true);

  const scopeArea = $derived(route.query.area || '');
  const scopeKind = $derived(
    KINDS.some((k) => k.value === route.query.kind) ? route.query.kind : 'all',
  );

  async function load() {
    loading = true;
    try { const ar = await api('list_areas'); if (ar?.ok) areas = ar.data; } catch (e) { /* offline */ }

    const params = {};
    if (scopeArea) params.area = scopeArea;
    if (scopeKind !== 'all') params.kind = scopeKind;
    let data = null;
    try { const res = await api('list_objects_full', params); if (res?.ok) { live = true; data = res.data; } } catch (e) { /* offline */ }

    if (data) {
      rows = (data.objects || []).map((o) => ({
        ref: o.ref_id,
        key: o.key,
        kind: o.kind,
        title: o.title,
        area: o.area || '',
        location: o.location_ref || '',
        tags: o.tags || [],
      }));
      truncated = !!data.truncated;
    } else {
      // Offline: fall back to the bundled sample, which only carries rooms.
      live = false;
      if (!areas.length) areas = sampleAreas;
      rows = (sampleWorld.rooms || [])
        .filter((r) => scopeKind === 'all' || scopeKind === 'room')
        .filter((r) => !scopeArea || (r.area || '') === scopeArea)
        .map((r) => ({ ref: r.ref ?? r.ref_id, key: r.key, kind: 'room', title: r.title, area: r.area || '', location: '', tags: r.tags || [] }));
      truncated = false;
    }
    loading = false;
  }

  let lastScope = null;
  $effect(() => {
    const k = `${scopeArea}|${scopeKind}|${reloadSignal}`;
    if (k !== lastScope) { lastScope = k; load(); }
  });

  const filtered = $derived.by(() => {
    const needle = q.trim().toLowerCase();
    const base = needle
      ? rows.filter((r) => `${r.key} ${r.title || ''} ${r.kind} ${r.ref} ${(r.tags || []).join(' ')}`.toLowerCase().includes(needle))
      : rows;
    const dir = sortAsc ? 1 : -1;
    const val = (r) => String((sortKey === 'title' ? (r.title || r.key) : r[sortKey]) || '').toLowerCase();
    return [...base].sort((a, b) => {
      const av = val(a);
      const bv = val(b);
      return av < bv ? -dir : av > bv ? dir : 0;
    });
  });

  function sortBy(k) {
    if (sortKey === k) sortAsc = !sortAsc;
    else { sortKey = k; sortAsc = true; }
  }
  const sortDir = (k) => (sortKey === k ? (sortAsc ? 'asc' : 'desc') : null);
  function onAreaChange(e) { setQuery({ area: e.target.value || null, focus: null }); }
  function onKindChange(e) { setQuery({ kind: e.target.value === 'all' ? null : e.target.value }); }
</script>

<div class="rt">
  <div class="rt-bar">
    <label class="rt-field">
      <span>Kind</span>
      <select value={scopeKind} onchange={onKindChange}>
        {#each KINDS as k}<option value={k.value}>{k.label}</option>{/each}
      </select>
    </label>
    <label class="rt-field">
      <span>Area</span>
      <select value={scopeArea} onchange={onAreaChange}>
        <option value="">All areas</option>
        {#each areas as a}
          <option value={a.area}>{a.area || '(unfiled)'} · {a.count}</option>
        {/each}
      </select>
    </label>
    <div class="rt-search"><SearchInput bind:value={q} placeholder="Filter by name, key, kind, ref, or tag…" /></div>
    <span class="rt-spacer"></span>
    {#if truncated}<span class="rt-warn">capped — narrow the filter</span>{/if}
    <span class="rt-count">{filtered.length} object{filtered.length === 1 ? '' : 's'}</span>
    <span class="rt-pill" class:live>{live ? 'live game' : 'sample world'}</span>
  </div>

  <div class="rt-scroll">
    <Table ariaLabel="Objects">
      {#snippet header()}
        <TableHeaderCell label="Kind" sortable sortDirection={sortDir('kind')} onsort={() => sortBy('kind')} />
        <TableHeaderCell label="Title" sortable sortDirection={sortDir('title')} onsort={() => sortBy('title')} />
        <TableHeaderCell label="Key" sortable sortDirection={sortDir('key')} onsort={() => sortBy('key')} />
        <TableHeaderCell label="Area" sortable sortDirection={sortDir('area')} onsort={() => sortBy('area')} />
        <TableHeaderCell label="Location" sortable sortDirection={sortDir('location')} onsort={() => sortBy('location')} />
        <TableHeaderCell label="Tags" />
        <TableHeaderCell label="Ref" numeric />
        <TableHeaderCell label="" />
      {/snippet}

      {#each filtered as r (r.ref)}
        <tr class="rt-row" onclick={() => onedit(r.ref)} title="Edit {r.kind}">
          <td><span class="rt-kind kind-{r.kind}">{r.kind}</span></td>
          <td class="rt-title">{r.title || r.key}</td>
          <td class="rt-key">{r.key}</td>
          <td>{#if r.area}<span class="rt-area">{r.area}</span>{:else}<span class="rt-unfiled">unfiled</span>{/if}</td>
          <td>{#if r.location}<span class="rt-loc">{r.location}</span>{:else}<span class="rt-dim">—</span>{/if}</td>
          <td>
            {#if r.tags.length}
              <div class="rt-tags">{#each r.tags as t}<Chip size="sm">{t}</Chip>{/each}</div>
            {:else}<span class="rt-dim">—</span>{/if}
          </td>
          <td class="rt-num rt-ref">{r.ref}</td>
          <td class="rt-actions">
            {#if r.kind === 'room'}
              <button class="rt-jump" title="Open in graph" onclick={(e) => { e.stopPropagation(); onopen(r.ref); }}>
                <WaypointsIcon size={14} />
              </button>
            {/if}
          </td>
        </tr>
      {/each}
    </Table>

    {#if !loading && filtered.length === 0}
      <div class="rt-empty">No objects match{q ? ` “${q}”` : ' this filter'}.</div>
    {/if}
  </div>
</div>

<style>
  .rt { display: flex; flex-direction: column; height: 100%; min-height: 0; background: var(--bg-primary, #100e0b); }
  .rt-bar {
    display: flex; align-items: center; gap: 12px;
    padding: 8px 14px;
    border-bottom: var(--border-width, 1px) solid var(--border-default, #2a2419);
    background: var(--bg-surface, #17140f);
  }
  .rt-field { display: flex; align-items: center; gap: 7px; font-size: 12px; color: var(--text-muted, #9a9186); }
  .rt-field select {
    font: inherit; font-size: 12.5px; color: var(--text-primary, #ece0c8);
    background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22);
    border-radius: var(--radius-md, 7px); padding: 5px 8px;
  }
  .rt-search { width: 260px; max-width: 34vw; }
  .rt-spacer { flex: 1; }
  .rt-warn { font-size: 11.5px; color: var(--accent-red, #d07a5a); }
  .rt-count { font-size: 12px; color: var(--text-muted, #9a9186); font-variant-numeric: tabular-nums; }
  .rt-pill {
    font-family: var(--font-mono, ui-monospace, monospace); font-size: 10.5px;
    padding: 3px 8px; border-radius: 999px;
    background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); color: var(--text-muted, #9a9186);
  }
  .rt-pill.live { color: var(--accent-green, #8fb877); border-color: color-mix(in srgb, var(--accent-green, #8fb877) 40%, transparent); }

  .rt-scroll { flex: 1; min-height: 0; overflow: auto; }
  .rt-row { cursor: pointer; }
  .rt-row:hover :global(td) { background: color-mix(in srgb, var(--accent-amber, #c9956b) 12%, transparent); }
  .rt-kind {
    font-family: var(--font-mono, ui-monospace, monospace); font-size: 9px; text-transform: uppercase;
    letter-spacing: .04em; padding: 2px 6px; border-radius: 4px;
    background: var(--bg-primary, #12100c); border: 1px solid var(--border-muted, #2a2419); color: var(--text-muted, #9a9186);
  }
  .kind-room { color: var(--accent-amber, #c9956b); border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 34%, transparent); }
  .kind-npc { color: var(--accent-green, #8fb877); border-color: color-mix(in srgb, var(--accent-green, #8fb877) 34%, transparent); }
  .kind-item { color: var(--accent-blue, #6ea3d0); border-color: color-mix(in srgb, var(--accent-blue, #6ea3d0) 34%, transparent); }
  .kind-player { color: #d3a2d8; border-color: color-mix(in srgb, #d3a2d8 34%, transparent); }
  .rt-key { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-muted, #b6a888); white-space: nowrap; }
  .rt-title { font-weight: 600; color: var(--text-primary, #ece0c8); }
  .rt-area { font-size: 11px; padding: 1px 7px; border-radius: 999px; background: var(--bg-surface, #17140f); border: 1px solid var(--border-muted, #2a2419); color: var(--text-secondary, #b6a888); }
  .rt-unfiled { font-size: 11px; color: var(--text-muted, #8c8378); font-style: italic; }
  .rt-loc { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--text-muted, #b6a888); }
  .rt-tags { display: flex; flex-wrap: wrap; gap: 4px; }
  .rt-num { text-align: right; font-variant-numeric: tabular-nums; }
  .rt-ref { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--accent-amber, #c9956b); }
  .rt-dim { color: var(--text-muted, #8c8378); }
  .rt-actions { text-align: right; width: 40px; }
  .rt-jump {
    background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer;
    padding: 4px; border-radius: 6px; line-height: 0;
  }
  .rt-jump:hover { color: var(--accent-amber, #c9956b); background: color-mix(in srgb, var(--accent-amber, #c9956b) 14%, transparent); }
  .rt-empty { padding: 28px; text-align: center; color: var(--text-muted, #9a9186); font-size: 13px; }
</style>
