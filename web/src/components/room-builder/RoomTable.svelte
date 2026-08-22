<script>
  import { Table, TableHeaderCell, SearchInput, Chip } from '@kenn-io/kit-ui';
  import { api } from '../../lib/api.js';
  import { route, setQuery } from '../../lib/router.svelte.js';
  import { sampleWorld, sampleAreas } from './sample.js';

  // The table view: the same scoped slice as the graph, as a sortable,
  // searchable list. Row → open that room in the graph (focused). Small data
  // (a slice is capped at 400 rows), so sort/filter is a plain derived — no
  // data-grid library needed.
  let { onopen = () => {} } = $props();

  let rows = $state([]);
  let areas = $state([]);
  let live = $state(false);
  let loading = $state(true);
  let truncated = $state(false);
  let q = $state('');
  let sortKey = $state('key');
  let sortAsc = $state(true);

  const scopeArea = $derived(route.query.area || '');

  async function load() {
    loading = true;
    try { const ar = await api('list_areas'); if (ar?.ok) areas = ar.data; } catch (e) { /* offline */ }

    const params = {};
    if (scopeArea) params.area = scopeArea;
    let slice = null;
    try { const res = await api('list_world_slice', params); if (res?.ok) { live = true; slice = res.data; } } catch (e) { /* offline */ }
    if (!slice) { live = false; if (!areas.length) areas = sampleAreas; slice = sampleWorld; }

    const norm = (o) => ({ ...o, ref: o.ref ?? o.ref_id });
    const roomsL = (slice.rooms || []).map(norm);
    const exitsL = (slice.exits || []).map(norm);
    const outCount = {};
    for (const e of exitsL) outCount[e.from] = (outCount[e.from] || 0) + 1;
    rows = roomsL.map((r) => ({ ...r, exits: outCount[r.ref] || 0, tags: r.tags || [] }));
    truncated = !!slice.truncated;
    loading = false;
  }

  let lastScope = null;
  $effect(() => {
    if (scopeArea !== lastScope) { lastScope = scopeArea; load(); }
  });

  const filtered = $derived.by(() => {
    const needle = q.trim().toLowerCase();
    const base = needle
      ? rows.filter((r) => `${r.key} ${r.title} ${(r.tags || []).join(' ')}`.toLowerCase().includes(needle))
      : rows;
    const dir = sortAsc ? 1 : -1;
    const val = (r) => (sortKey === 'exits' ? r.exits : String(r[sortKey] || '').toLowerCase());
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
</script>

<div class="rt">
  <div class="rt-bar">
    <label class="rt-field">
      <span>Area</span>
      <select value={scopeArea} onchange={onAreaChange}>
        <option value="">All rooms</option>
        {#each areas as a}
          <option value={a.area}>{a.area || '(unfiled)'} · {a.count}</option>
        {/each}
      </select>
    </label>
    <div class="rt-search"><SearchInput bind:value={q} placeholder="Filter by name, key, or tag…" /></div>
    <span class="rt-spacer"></span>
    {#if truncated}<span class="rt-warn">capped slice — narrow the filter</span>{/if}
    <span class="rt-count">{filtered.length} room{filtered.length === 1 ? '' : 's'}</span>
    <span class="rt-pill" class:live>{live ? 'live game' : 'sample world'}</span>
  </div>

  <div class="rt-scroll">
    <Table ariaLabel="Rooms">
      {#snippet header()}
        <TableHeaderCell label="Key" sortable sortDirection={sortDir('key')} onsort={() => sortBy('key')} />
        <TableHeaderCell label="Title" sortable sortDirection={sortDir('title')} onsort={() => sortBy('title')} />
        <TableHeaderCell label="Area" sortable sortDirection={sortDir('area')} onsort={() => sortBy('area')} />
        <TableHeaderCell label="Tags" />
        <TableHeaderCell label="Exits" numeric sortable sortDirection={sortDir('exits')} onsort={() => sortBy('exits')} />
        <TableHeaderCell label="Ref" numeric />
      {/snippet}

      {#each filtered as r (r.ref)}
        <tr class="rt-row" onclick={() => onopen(r.ref)} title="Open in graph">
          <td class="rt-key">{r.key}</td>
          <td class="rt-title">{r.title || r.key}</td>
          <td>{#if r.area}<span class="rt-area">{r.area}</span>{:else}<span class="rt-unfiled">unfiled</span>{/if}</td>
          <td>
            {#if r.tags.length}
              <div class="rt-tags">{#each r.tags as t}<Chip size="sm">{t}</Chip>{/each}</div>
            {:else}<span class="rt-dim">—</span>{/if}
          </td>
          <td class="rt-num">{r.exits}</td>
          <td class="rt-num rt-ref">{r.ref}</td>
        </tr>
      {/each}
    </Table>

    {#if !loading && filtered.length === 0}
      <div class="rt-empty">No rooms match “{q}”.</div>
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
  .rt-search { width: 280px; max-width: 40vw; }
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
  .rt-key { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-muted, #b6a888); white-space: nowrap; }
  .rt-title { font-weight: 600; color: var(--text-primary, #ece0c8); }
  .rt-area { font-size: 11px; padding: 1px 7px; border-radius: 999px; background: var(--bg-surface, #17140f); border: 1px solid var(--border-muted, #2a2419); color: var(--text-secondary, #b6a888); }
  .rt-unfiled { font-size: 11px; color: var(--text-muted, #8c8378); font-style: italic; }
  .rt-tags { display: flex; flex-wrap: wrap; gap: 4px; }
  .rt-num { text-align: right; font-variant-numeric: tabular-nums; }
  .rt-ref { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--accent-amber, #c9956b); }
  .rt-dim { color: var(--text-muted, #8c8378); }
  .rt-empty { padding: 28px; text-align: center; color: var(--text-muted, #9a9186); font-size: 13px; }
</style>
