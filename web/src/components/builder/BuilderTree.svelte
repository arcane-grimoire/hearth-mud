<script>
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import FolderIcon from '@lucide/svelte/icons/folder';
  import FileCodeIcon from '@lucide/svelte/icons/file-code';
  import Grid3x3Icon from '@lucide/svelte/icons/grid-3x3';
  import PackageIcon from '@lucide/svelte/icons/package';
  import LockIcon from '@lucide/svelte/icons/lock';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import { Tooltip } from '@kenn-io/kit-ui';

  // VS Code-style explorer: area (folder) → object → its hooks (files), plus a
  // "Maps" folder for the tile/terrain maps. Stays pinned on the left while you
  // edit. Clicking an object selects it; a hook opens the code editor; a map
  // opens the map builder — all as tabs.
  let {
    objects = [],
    selectedRef = null,
    activeHook = null,
    maps = [],
    activeMap = null,
    libraries = [],
    activeLib = null,
    onselect = () => {},
    onopenhook = () => {},
    onopenmap = () => {},
    onnewmap = () => {},
    onopenlib = () => {},
    onnewlib = () => {},
  } = $props();

  let mapsOpen = $state(true);
  let libsOpen = $state(true);

  // Objects grouped into area "folders", each sorted by name.
  const grouped = $derived.by(() => {
    const m = new Map();
    for (const o of objects) {
      const a = o.area || 'unfiled';
      if (!m.has(a)) m.set(a, []);
      m.get(a).push(o);
    }
    return [...m.entries()]
      .sort((a, b) => a[0].localeCompare(b[0]))
      .map(([area, objs]) => [area, objs.sort((x, y) => (x.title || x.key || '').localeCompare(y.title || y.key || ''))]);
  });

  // Expansion state. Areas open by default (collapsed set); objects closed by
  // default (expanded set) — most objects have no hooks worth unfolding.
  let collapsedAreas = $state(new Set());
  let expandedObjs = $state(new Set());

  function toggleArea(a) {
    const n = new Set(collapsedAreas);
    n.has(a) ? n.delete(a) : n.add(a);
    collapsedAreas = n;
  }
  function clickObject(o) {
    onselect(o.ref_id);
    if (o.hooks?.length) {
      const n = new Set(expandedObjs);
      n.has(o.ref_id) ? n.delete(o.ref_id) : n.add(o.ref_id);
      expandedObjs = n;
    }
  }
  function kindClass(k) {
    return ({ room: 'k-room', npc: 'k-npc', item: 'k-item', player: 'k-player', code: 'k-code', exit: 'k-exit' })[k] || '';
  }

  // Arrow-key navigation across the explorer. Rows are real <button>s (so Tab +
  // Enter already work); this adds Up/Down/Home/End to move focus between the
  // visible rows without reaching for the mouse or tabbing through each one.
  function treeKeydown(e) {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return;
    const items = [...e.currentTarget.querySelectorAll('button')].filter((b) => b.offsetParent !== null);
    if (!items.length) return;
    e.preventDefault();
    const idx = items.indexOf(document.activeElement);
    let next;
    if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = items.length - 1;
    else if (e.key === 'ArrowDown') next = idx < 0 ? 0 : Math.min(idx + 1, items.length - 1);
    else next = idx <= 0 ? 0 : idx - 1;
    items[next]?.focus();
  }
</script>

<div class="tree" role="group" aria-label="Objects and maps" onkeydown={treeKeydown}>
  <!-- Maps folder — the tile/terrain maps live alongside object areas. -->
  <div class="area maps-folder">
    <button class="area-btn" onclick={() => (mapsOpen = !mapsOpen)}>
      <span class="chev" class:open={mapsOpen}><ChevronRight size={12} /></span>
      <FolderIcon size={13} />
      <span class="area-name">Maps</span>
      <span class="area-count">{maps.length}</span>
    </button>
    <Tooltip text="Map builder — new / all maps" align="end"><button class="area-add" aria-label="Map builder" onclick={onnewmap}><PlusIcon size={12} /></button></Tooltip>
  </div>
  {#if mapsOpen}
    {#each maps as m}
      <button class="map-leaf" class:active={m === activeMap} onclick={() => onopenmap(m)}>
        <Grid3x3Icon size={12} />
        <span class="hook-name">{m}</span>
      </button>
    {/each}
    {#if maps.length === 0}<div class="none sub">No maps yet</div>{/if}
  {/if}

  <!-- Libraries folder — require()able lib modules, authored in-world (no file
       access), alongside object areas and maps. -->
  <div class="area maps-folder">
    <button class="area-btn" onclick={() => (libsOpen = !libsOpen)}>
      <span class="chev" class:open={libsOpen}><ChevronRight size={12} /></span>
      <FolderIcon size={13} />
      <span class="area-name">Libraries</span>
      <span class="area-count">{libraries.length}</span>
    </button>
    <Tooltip text="New library" align="end"><button class="area-add" aria-label="New library" onclick={onnewlib}><PlusIcon size={12} /></button></Tooltip>
  </div>
  {#if libsOpen}
    {#each libraries as lib (lib.ref_id + ':' + lib.name)}
      <button class="map-leaf" class:active={lib.name === activeLib} onclick={() => onopenlib(lib.ref_id, lib.name)}>
        <PackageIcon size={12} />
        <span class="hook-name">{lib.name}</span>
        {#if lib.locked}<Tooltip text="Locked (file-authoritative)"><LockIcon size={10} /></Tooltip>{/if}
      </button>
    {/each}
    {#if libraries.length === 0}<div class="none sub">No libraries yet</div>{/if}
  {/if}

  {#each grouped as [area, objs]}
    <button class="area" onclick={() => toggleArea(area)}>
      <span class="chev" class:open={!collapsedAreas.has(area)}><ChevronRight size={12} /></span>
      <FolderIcon size={13} />
      <span class="area-name">{area}</span>
      <span class="area-count">{objs.length}</span>
    </button>
    {#if !collapsedAreas.has(area)}
      {#each objs as o (o.ref_id)}
        <button class="obj" class:sel={o.ref_id === selectedRef} onclick={() => clickObject(o)}>
          {#if o.hooks?.length}
            <span class="chev sm" class:open={expandedObjs.has(o.ref_id)}><ChevronRight size={11} /></span>
          {:else}
            <span class="chev sm spacer"></span>
          {/if}
          <span class="kb {kindClass(o.kind)}">{o.kind.slice(0, 3)}</span>
          <span class="obj-name">{o.title || o.key}</span>
          {#if o.hooks?.length}<span class="obj-count">{o.hooks.length}</span>{/if}
        </button>
        {#if expandedObjs.has(o.ref_id)}
          {#each [...o.hooks].sort() as h}
            <button
              class="hook"
              class:active={o.ref_id === selectedRef && h === activeHook}
              onclick={() => onopenhook(o.ref_id, h)}
            >
              <FileCodeIcon size={12} />
              <span class="hook-name">{h}</span>
            </button>
          {/each}
        {/if}
      {/each}
    {/if}
  {/each}
  {#if grouped.length === 0}
    <div class="none">No objects match.</div>
  {/if}
</div>

<style>
  .tree { height: 100%; overflow-y: auto; padding: 4px 0 20px; font-size: 12.5px; }
  button { display: flex; align-items: center; gap: 5px; width: 100%; text-align: left; background: none; border: none; cursor: pointer; font: inherit; }
  .chev { display: inline-flex; color: var(--text-muted, #8c8378); transition: transform 0.12s; flex: none; }
  .chev.open { transform: rotate(90deg); }
  .chev.sm { width: 12px; }
  .chev.spacer { width: 12px; }

  .area { padding: 5px 10px; color: var(--text-secondary, #b6a888); font-weight: 600; }
  .area:hover { background: var(--bg-primary, #12100c); }
  .area :global(svg) { color: var(--accent-amber, #c9956b); }

  /* Maps folder: a folder row with an inline "+" (map builder). */
  .maps-folder { display: flex; align-items: center; padding: 0; }
  .maps-folder:hover { background: var(--bg-primary, #12100c); }
  .area-btn { flex: 1; padding: 5px 10px; color: var(--text-secondary, #b6a888); font-weight: 600; }
  .area-btn :global(svg) { color: var(--accent-amber, #c9956b); }
  .area-add { width: auto; flex: none; padding: 4px 9px; color: var(--text-muted, #8c8378); }
  .area-add:hover { color: var(--accent-amber, #c9956b); }
  .map-leaf { padding: 4px 10px 4px 30px; color: var(--text-muted, #9a9186); font-size: 12px; }
  .map-leaf:hover { background: var(--bg-primary, #12100c); color: var(--text-primary, #ece0c8); }
  .map-leaf.active { background: color-mix(in srgb, var(--accent-amber, #c9956b) 16%, transparent); color: var(--accent-amber, #c9956b); }
  .map-leaf :global(svg) { color: var(--accent-green, #8fb877); flex: none; }
  .map-leaf.active :global(svg) { color: var(--accent-amber, #c9956b); }
  .none.sub { padding-left: 30px; font-size: 11px; }
  .area-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; text-transform: capitalize; }
  .area-count { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); color: var(--text-muted, #8c8378); }

  .obj { padding: 4px 10px 4px 16px; color: var(--text-primary, #ece0c8); }
  .obj:hover { background: var(--bg-primary, #12100c); }
  .obj.sel { background: color-mix(in srgb, var(--accent-amber, #c9956b) 14%, transparent); }
  .obj.sel .obj-name { color: var(--accent-amber, #c9956b); }
  .obj-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .obj-count { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); color: var(--accent-green, #8fb877); background: rgba(143,184,119,0.14); border-radius: 7px; padding: 0 5px; }
  .kb { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-badge); font-weight: 700; text-transform: uppercase; padding: 1px 4px; border-radius: 3px; flex: none; }
  .k-room { background: rgba(86,182,194,0.15); color: var(--accent-teal, #56b6c2); }
  .k-npc { background: rgba(201,149,107,0.15); color: var(--accent-amber, #c9956b); }
  .k-item { background: rgba(97,175,239,0.15); color: var(--accent-blue, #61afef); }
  .k-player { background: rgba(198,120,221,0.15); color: var(--accent-purple, #c678dd); }
  .k-code { background: rgba(143,184,119,0.15); color: var(--accent-green, #8fb877); }
  .k-exit { background: rgba(255,255,255,0.08); color: var(--text-muted, #9a9186); }

  .hook { padding: 3px 10px 3px 40px; color: var(--text-muted, #9a9186); font-family: var(--font-mono, ui-monospace, monospace); font-size: 11.5px; }
  .hook:hover { background: var(--bg-primary, #12100c); color: var(--text-primary, #ece0c8); }
  .hook :global(svg) { color: var(--accent-blue, #6ea3d0); flex: none; }
  .hook.active { background: color-mix(in srgb, var(--accent-amber, #c9956b) 16%, transparent); color: var(--accent-amber, #c9956b); }
  .hook.active :global(svg) { color: var(--accent-amber, #c9956b); }
  .hook-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .none { color: var(--text-muted, #8c8378); font-style: italic; padding: 12px; }
</style>
