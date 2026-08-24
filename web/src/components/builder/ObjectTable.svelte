<script>
  // The builder "home": a flat, sortable table over every GameObject. The map
  // is a *view* of the same rows (rooms only) — this is the ground truth,
  // because items, NPCs, code and players have no spatial position.
  let { rows = [], selectedRef = null, onselect = () => {} } = $props();

  let sortKey = $state('title');
  let sortDir = $state(1);
  function sortBy(k) {
    if (sortKey === k) sortDir = -sortDir;
    else { sortKey = k; sortDir = 1; }
  }

  const sorted = $derived.by(() => {
    const r = [...rows];
    r.sort((a, b) => {
      let av, bv;
      if (sortKey === 'hooks') { av = a.hooks?.length || 0; bv = b.hooks?.length || 0; }
      else { av = (a[sortKey] ?? '').toString().toLowerCase(); bv = (b[sortKey] ?? '').toString().toLowerCase(); }
      if (av < bv) return -sortDir;
      if (av > bv) return sortDir;
      return 0;
    });
    return r;
  });

  function kindClass(k) {
    return ({ room: 'k-room', npc: 'k-npc', item: 'k-item', player: 'k-player', code: 'k-code', exit: 'k-exit' })[k] || '';
  }
  const arrow = (k) => (sortKey === k ? (sortDir === 1 ? ' ▲' : ' ▼') : '');
</script>

<div class="ot">
  <table>
    <thead>
      <tr>
        <th class="c-kind" onclick={() => sortBy('kind')}>Kind{arrow('kind')}</th>
        <th class="c-title" onclick={() => sortBy('title')}>Title{arrow('title')}</th>
        <th class="c-area" onclick={() => sortBy('area')}>Area{arrow('area')}</th>
        <th class="c-hooks" onclick={() => sortBy('hooks')}>Code{arrow('hooks')}</th>
        <th class="c-ref" onclick={() => sortBy('ref_id')}>Ref{arrow('ref_id')}</th>
      </tr>
    </thead>
    <tbody>
      {#each sorted as row (row.ref_id)}
        <tr class:sel={row.ref_id === selectedRef} onclick={() => onselect(row.ref_id)}>
          <td class="c-kind"><span class="kb {kindClass(row.kind)}">{row.kind}</span></td>
          <td class="c-title">{row.title || row.key || '—'}</td>
          <td class="c-area">{row.area || ''}</td>
          <td class="c-hooks">{#if row.hooks?.length}<span class="pill">{row.hooks.length}</span>{/if}</td>
          <td class="c-ref">{row.ref_id}</td>
        </tr>
      {/each}
      {#if sorted.length === 0}
        <tr><td colspan="5" class="empty">No objects match.</td></tr>
      {/if}
    </tbody>
  </table>
</div>

<style>
  .ot { height: 100%; overflow: auto; }
  table { width: 100%; border-collapse: collapse; font-size: 12.5px; }
  thead th {
    position: sticky; top: 0; z-index: 1;
    text-align: left; font-size: var(--fs-label); font-weight: 600;
    text-transform: uppercase; letter-spacing: 0.06em;
    color: var(--text-muted, #8c8378);
    background: var(--bg-surface, #17140f);
    padding: 8px 10px; border-bottom: 1px solid var(--border-default, #2a2419);
    cursor: pointer; white-space: nowrap; user-select: none;
  }
  thead th:hover { color: var(--text-primary, #ece0c8); }
  tbody td {
    padding: 6px 10px; border-bottom: 1px solid var(--border-muted, #211d16);
    color: var(--text-primary, #ece0c8); vertical-align: middle;
  }
  tbody tr { cursor: pointer; }
  tbody tr:hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 6%, transparent); }
  tbody tr.sel { background: color-mix(in srgb, var(--accent-amber, #c9956b) 14%, transparent); }
  tbody tr.sel td { box-shadow: inset 3px 0 0 var(--accent-amber, #c9956b); }
  .c-title { width: 100%; }
  .c-ref, .c-area { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--text-muted, #9a9186); white-space: nowrap; }
  .c-kind, .c-hooks { white-space: nowrap; }
  .kb {
    font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-badge); font-weight: 600;
    text-transform: uppercase; letter-spacing: 0.04em; padding: 1px 5px; border-radius: 3px;
  }
  .k-room { background: rgba(86,182,194,0.15); color: var(--accent-teal, #56b6c2); }
  .k-npc { background: rgba(201,149,107,0.15); color: var(--accent-amber, #c9956b); }
  .k-item { background: rgba(97,175,239,0.15); color: var(--accent-blue, #61afef); }
  .k-player { background: rgba(198,120,221,0.15); color: var(--accent-purple, #c678dd); }
  .k-code { background: rgba(143,184,119,0.15); color: var(--accent-green, #8fb877); }
  .k-exit { background: rgba(255,255,255,0.08); color: var(--text-muted, #9a9186); }
  .pill {
    display: inline-block; min-width: 16px; text-align: center;
    font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta);
    padding: 0 5px; border-radius: 8px;
    background: rgba(143,184,119,0.16); color: var(--accent-green, #8fb877);
  }
  .empty { text-align: center; color: var(--text-muted, #8c8378); font-style: italic; padding: 24px !important; }
</style>
