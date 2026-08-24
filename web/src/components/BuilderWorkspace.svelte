<script>
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import SearchIcon from '@lucide/svelte/icons/search';
  import LayersIcon from '@lucide/svelte/icons/layers';
  import TableIcon from '@lucide/svelte/icons/table';
  import MapIcon from '@lucide/svelte/icons/map';
  import Grid3x3Icon from '@lucide/svelte/icons/grid-3x3';
  import XIcon from '@lucide/svelte/icons/x';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import PanelLeftIcon from '@lucide/svelte/icons/panel-left';
  import FileCodeIcon from '@lucide/svelte/icons/file-code';
  import BoxIcon from '@lucide/svelte/icons/box';
  import MessagesSquareIcon from '@lucide/svelte/icons/messages-square';
  import { showFlash, Tooltip } from '@kenn-io/kit-ui';
  import { api } from '../lib/api.js';
  import { selection, selectRef, clearSelection } from '../lib/selection.svelte.js';
  import ObjectTable from './builder/ObjectTable.svelte';
  import BuilderTree from './builder/BuilderTree.svelte';
  import PropertiesPanel from './builder/PropertiesPanel.svelte';
  import HooksPanel from './builder/HooksPanel.svelte';
  import CodeOverlay from './builder/CodeOverlay.svelte';
  import InkEditor from './dialogue/InkEditor.svelte';
  import RoomGraph from './room-builder/RoomGraph.svelte';
  import ObjectFinder from './room-builder/ObjectFinder.svelte';
  import MapBuilder from './builder/map/MapBuilder.svelte';

  // The unified builder workspace: one shell, one selection, a tabbed editor.
  //   explorer tree │ tabbed editor (objects · hooks · overviews)
  // The left tree is the sole navigator. Everything you open — an object's
  // detail, a hook's code, the Table/Map overview — becomes a tab in the
  // center. No fixed side panel; the active tab decides what's on screen.
  let { onexit = () => {} } = $props();

  let objects = $state([]);
  let truncated = $state(false); // list_objects_full hit its cap — some objects are hidden
  let loading = $state(true);
  let kindFilter = $state('all');
  let search = $state('');

  let obj = $state(null);          // examined detail for the active tab's object
  let objLoading = $state(false);
  let subtab = $state('props');    // Properties | Hooks | Dialogue within an object tab

  // Which subtabs an object kind exposes. A room's exits + contents live inside
  // Properties (right under identity), not a separate tab. Only NPCs get
  // Dialogue. Everything gets Properties + Hooks.
  const subtabsFor = (kind) => {
    const t = ['props', 'hooks'];
    if (kind === 'npc') t.push('dialogue');
    return t;
  };
  const objSubtabs = $derived(subtabsFor(obj?.kind));
  // Keep the active subtab valid when the selected object changes kind.
  $effect(() => {
    if (obj && !objSubtabs.includes(subtab)) subtab = 'props';
  });
  const rooms = $derived(objects.filter((o) => o.kind === 'room'));

  // Open editor tabs. Each: { id, type:'object'|'code'|'table'|'map', ref?, hook? }
  let tabs = $state([]);
  let activeId = $state(null);
  const activeTab = $derived(tabs.find((t) => t.id === activeId) || null);

  let maps = $state([]); // named tile/terrain maps (Mapwright), each opens as a tab

  let finderOpen = $state(false);
  let sidebarOpen = $state(true);

  // New-object creator (in the sidebar).
  let newOpen = $state(false);
  let nKind = $state('room');
  let nKey = $state('');
  let nTitle = $state('');
  let creating = $state(false);

  async function createObject() {
    const key = nKey.trim();
    if (!key) { showFlash('A key is required', { tone: 'danger' }); return; }
    creating = true;
    let res;
    if (nKind === 'room') {
      res = await api('create_room', { area: '', key, title: nTitle.trim() || key });
    } else {
      // Default an item/NPC's location to the selected room, so it isn't born
      // orphaned when you're already standing in the room you want it in.
      const location = obj?.kind === 'room' ? obj.ref_id : undefined;
      res = await api('create_object', { area: '', key, kind: nKind, title: nTitle.trim() || key, location });
    }
    creating = false;
    if (res?.ok && res.data?.ref_id) {
      showFlash(`Created ${res.data.ref_id}`, { tone: 'success' });
      newOpen = false; nKey = ''; nTitle = '';
      await loadObjects();
      openObject(res.data.ref_id);
    } else {
      showFlash(res?.error || 'Create failed', { tone: 'danger' });
    }
  }

  const RAIL = [
    { key: 'all', label: 'All' },
    { key: 'room', label: 'Rooms' },
    { key: 'item', label: 'Items' },
    { key: 'npc', label: 'NPCs' },
    { key: 'player', label: 'Players' },
    { key: 'code', label: 'Code' },
  ];

  $effect(() => { loadObjects(); loadMaps(); });

  async function loadMaps() {
    const r = await api('list_maps');
    maps = r?.ok ? (r.data?.maps || []) : [];
  }

  async function loadObjects() {
    loading = true;
    // Objects AND their area come from list_objects_full, where area is derived
    // from each object's _file_key. Programs come separately, only to attach the
    // hooks list (Code filter + Hooks count). The old path took area from the
    // programs response — which lists only objects that HAVE a program — so
    // every program-less room (most rooms) fell into "Unfiled" despite having a
    // real _file_key area.
    const [objRes, progRes] = await Promise.all([
      api('list_objects_full', { limit: 3000 }),
      api('list_programs_all'),
    ]);
    const progById = new Map((progRes?.ok ? progRes.data : []).map((e) => [e.ref_id, e]));
    const list = objRes?.ok ? (objRes.data?.objects || []) : [];
    truncated = objRes?.ok ? !!objRes.data?.truncated : false;
    objects = list.map((o) => {
      const p = progById.get(o.ref_id);
      return { ...o, hooks: p?.hooks || [] }; // area already on o, from _file_key
    });
    loading = false;
  }

  function countFor(key) {
    if (key === 'all') return objects.length;
    if (key === 'code') return objects.filter((o) => o.hooks.length).length;
    return objects.filter((o) => o.kind === key).length;
  }

  const filtered = $derived.by(() => {
    const q = search.trim().toLowerCase();
    return objects.filter((o) => {
      if (kindFilter === 'code') { if (!o.hooks.length) return false; }
      else if (kindFilter !== 'all' && o.kind !== kindFilter) return false;
      if (q && !`${o.title || ''} ${o.key || ''} ${o.ref_id}`.toLowerCase().includes(q)) return false;
      return true;
    });
  });

  // The active tab's object drives the examine — one selection, the detail
  // follows it.
  $effect(() => {
    const ref = selection.ref;
    if (ref) examine(ref);
    else obj = null;
  });
  async function examine(ref) {
    objLoading = true;
    const res = await api('examine', { ref_id: ref });
    obj = res?.ok ? res.data : null;
    objLoading = false;
  }
  function refresh() { if (selection.ref) examine(selection.ref); }

  // ── Tabs ──────────────────────────────────────────────────────────────
  function tabIdOf(t) {
    if (t.type === 'code') return `code:${t.ref}:${t.hook}`;
    if (t.type === 'ink') return `ink:${t.ref}`;
    if (t.type === 'object') return `obj:${t.ref}`;
    if (t.type === 'maps') return `maps:${t.name || ''}`; // one tab per named map
    return t.type; // table | map are singletons
  }
  function openTab(t) {
    const id = tabIdOf(t);
    if (!tabs.some((x) => x.id === id)) tabs = [...tabs, { ...t, id }];
    activeId = id;
    if (t.ref) selectRef(t.ref);
  }
  function activate(id) {
    activeId = id;
    const t = tabs.find((x) => x.id === id);
    if (t?.ref) selectRef(t.ref); else clearSelection();
  }
  // Roving-tabindex keyboard model for the editor tab strip (WAI-ARIA tabs):
  // Enter/Space activates, arrows move between tabs and carry focus.
  function tabKeydown(e, id) {
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); activate(id); return; }
    const idx = tabs.findIndex((t) => t.id === id);
    if (idx < 0) return;
    let next = -1;
    if (e.key === 'ArrowRight') next = (idx + 1) % tabs.length;
    else if (e.key === 'ArrowLeft') next = (idx - 1 + tabs.length) % tabs.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = tabs.length - 1;
    else return;
    e.preventDefault();
    const nt = tabs[next];
    if (!nt) return;
    activate(nt.id);
    document.querySelector(`.tabx[data-tabid="${nt.id}"]`)?.focus();
  }
  function closeTab(id, e) {
    e?.stopPropagation();
    const i = tabs.findIndex((x) => x.id === id);
    tabs = tabs.filter((x) => x.id !== id);
    if (activeId === id) {
      const next = tabs[i] || tabs[i - 1] || null;
      activeId = next?.id || null;
      if (next?.ref) selectRef(next.ref); else clearSelection();
    }
  }
  const openObject = (ref) => openTab({ type: 'object', ref });
  const openHookTab = (ref, hook) => openTab({ type: 'code', ref, hook });
  const openInkTab = (ref) => openTab({ type: 'ink', ref });

  // A structural change (exit/content added or removed, hook removed) — re-examine
  // the open object AND refresh the explorer tree, since the object set changed.
  function structureChanged() { refresh(); loadObjects(); }

  // The object was deleted from its Properties tab — drop its tab and refresh.
  function onObjectDeleted(ref) {
    const id = tabIdOf({ type: 'object', ref });
    closeTab(id);
    loadObjects();
  }

  function nameOf(ref) {
    const o = objects.find((x) => x.ref_id === ref);
    return o?.title || o?.key || ref;
  }
  function tabLabel(t) {
    if (t.type === 'table') return 'Table';
    if (t.type === 'map') return 'Map';
    if (t.type === 'maps') return t.name || 'Map builder';
    if (t.type === 'code') return t.hook;
    if (t.type === 'ink') return `${nameOf(t.ref)} · dialogue`;
    return nameOf(t.ref);
  }

  function pickFinder(ref) { finderOpen = false; openObject(ref); }
  function onGlobalKey(e) {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); finderOpen = true; }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'b') { e.preventDefault(); sidebarOpen = !sidebarOpen; }
  }
</script>

<div class="ws">
  <header class="top">
    <Tooltip text="Back to the game"><button class="back" onclick={onexit}><ArrowLeftIcon size={16} /> <span>Game</span></button></Tooltip>
    <Tooltip text="Toggle sidebar (⌘B)">
      <button class="icon-btn" aria-label="Toggle sidebar" onclick={() => (sidebarOpen = !sidebarOpen)}>
        <PanelLeftIcon size={15} />
      </button>
    </Tooltip>
    <div class="title"><LayersIcon size={16} /> <h1>Builder</h1></div>
    <span class="sp"></span>
    <Tooltip text="Find any object or map (⌘K)">
      <button class="find" onclick={() => (finderOpen = true)}>
        <SearchIcon size={14} /> <span>Find</span> <kbd>⌘K</kbd>
      </button>
    </Tooltip>
  </header>

  <div class="body" class:no-sidebar={!sidebarOpen}>
    {#if sidebarOpen}
    <!-- Left: explorer tree + view launchers -->
    <nav class="explorer">
      <div class="ex-top">
        <button class="new-btn" onclick={() => (newOpen = !newOpen)} title="Create a new object">
          <PlusIcon size={14} /> New object
        </button>
      </div>
      {#if newOpen}
        <div class="new-form">
          <div class="nf-kinds">
            {#each ['room', 'item', 'npc'] as k}
              <button class="nf-kind" class:on={nKind === k} onclick={() => (nKind = k)}>{k}</button>
            {/each}
          </div>
          <input class="nf-in" placeholder="key (e.g. crossroads)" bind:value={nKey}
            onkeydown={(e) => e.key === 'Enter' && createObject()} />
          <input class="nf-in" placeholder="title (optional)" bind:value={nTitle}
            onkeydown={(e) => e.key === 'Enter' && createObject()} />
          {#if nKind !== 'room'}
            <div class="nf-hint">{obj?.kind === 'room' ? `in ${obj.title || obj.ref_id}` : 'no room selected — will be unplaced'}</div>
          {/if}
          <div class="nf-actions">
            <button class="nf-go" disabled={creating || !nKey.trim()} onclick={createObject}>{creating ? '…' : 'Create'}</button>
            <button class="nf-x" onclick={() => (newOpen = false)}>Cancel</button>
          </div>
        </div>
      {/if}

      <div class="views">
        <button class="view-btn" onclick={() => openTab({ type: 'table' })} title="Object table"><TableIcon size={13} /> Table</button>
        <button class="view-btn" onclick={() => openTab({ type: 'map' })} title="Room map (connectivity)"><MapIcon size={13} /> Map</button>
      </div>

      <div class="chips">
        {#each RAIL as r}
          <button class="chip" class:active={kindFilter === r.key} onclick={() => (kindFilter = r.key)}>
            {r.label}<span class="cc">{countFor(r.key)}</span>
          </button>
        {/each}
      </div>
      <div class="ex-search">
        <SearchIcon size={12} />
        <input placeholder="Filter…" bind:value={search} />
      </div>
      <div class="ex-tree">
        {#if loading}
          <div class="none">Loading…</div>
        {:else}
          <BuilderTree
            objects={filtered}
            selectedRef={selection.ref}
            activeHook={activeTab?.type === 'code' ? activeTab.hook : null}
            maps={maps}
            activeMap={activeTab?.type === 'maps' ? activeTab.name : null}
            onselect={openObject}
            onopenhook={openHookTab}
            onopenmap={(m) => openTab({ type: 'maps', name: m })}
            onnewmap={() => openTab({ type: 'maps' })}
          />
          {#if truncated}
            <div class="ex-trunc" role="status">Showing the first {objects.length} objects — filter to find the rest.</div>
          {/if}
        {/if}
      </div>
    </nav>
    {/if}

    <!-- Center: tabbed editor -->
    <main class="center">
      {#if tabs.length}
        <div class="tabbar" role="tablist">
          {#each tabs as t (t.id)}
            <div class="tabx" class:active={t.id === activeId} role="tab"
              data-tabid={t.id}
              aria-selected={t.id === activeId}
              aria-controls="ws-panel"
              tabindex={t.id === activeId ? 0 : -1}
              onclick={() => activate(t.id)} onkeydown={(e) => tabKeydown(e, t.id)}>
              <span class="ti">
                {#if t.type === 'code'}<FileCodeIcon size={12} />{:else if t.type === 'ink'}<MessagesSquareIcon size={12} />{:else if t.type === 'object'}<BoxIcon size={12} />{:else if t.type === 'table'}<TableIcon size={12} />{:else if t.type === 'maps'}<Grid3x3Icon size={12} />{:else}<MapIcon size={12} />{/if}
              </span>
              <span class="tl">{tabLabel(t)}</span>
              {#if t.type === 'code' || t.type === 'object' || t.type === 'ink'}<span class="tref">{t.ref}</span>{/if}
              <Tooltip text="Close tab"><button class="tc-x" aria-label="Close tab" onclick={(e) => closeTab(t.id, e)}><XIcon size={12} /></button></Tooltip>
            </div>
          {/each}
        </div>
      {/if}

      <div class="view" id="ws-panel" role={activeTab ? 'tabpanel' : undefined} aria-label={activeTab ? tabLabel(activeTab) : undefined}>
        {#if !activeTab}
          <div class="welcome">
            <LayersIcon size={30} />
            <p class="w-lead">Build your world</p>
            <p class="w-hint">Everything in your MUD — rooms, items, NPCs, exits — is an object you edit here. Open one from the explorer, or start something new.</p>
            <div class="w-actions">
              <button class="w-go" onclick={() => { sidebarOpen = true; newOpen = true; }}>
                <PlusIcon size={15} /> New object
              </button>
              <button class="w-alt" onclick={() => openTab({ type: 'table' })}>
                <TableIcon size={14} /> Browse everything
              </button>
              <button class="w-alt" onclick={() => openTab({ type: 'map' })}>
                <MapIcon size={14} /> Room map
              </button>
            </div>
            <p class="w-legend">
              New here? An <b>object</b> is a room, item, or NPC · <b>hooks</b> are Luau scripts that react to events · <b>tags</b> and <b>attributes</b> describe an object.
            </p>
          </div>
        {:else if activeTab.type === 'code'}
          {#key activeTab.id}
            <CodeOverlay
              refId={activeTab.ref}
              hook={activeTab.hook}
              objName={nameOf(activeTab.ref)}
              onclose={() => closeTab(activeTab.id)}
              onsaved={refresh}
            />
          {/key}
        {:else if activeTab.type === 'ink'}
          {#key activeTab.id}
            <InkEditor
              refId={activeTab.ref}
              objName={nameOf(activeTab.ref)}
              onsaved={refresh}
            />
          {/key}
        {:else if activeTab.type === 'table'}
          <ObjectTable rows={filtered} selectedRef={selection.ref} onselect={openObject} />
        {:else if activeTab.type === 'map'}
          <RoomGraph onedit={openObject} />
        {:else if activeTab.type === 'maps'}
          <!-- Native map builder (Svelte port of Mapwright) — themed with kit-ui,
               deep-linked to a specific map by name. -->
          {#key activeTab.id}
            <MapBuilder name={activeTab.name || null} />
          {/key}
        {:else}
          <!-- object tab: the detail view, now in the center -->
          {#if objLoading || obj?.ref_id !== activeTab.ref}
            <div class="none">Loading…</div>
          {:else if obj}
            <div class="obj-tab">
              <div class="obj-head">
                <div class="oh-title">{obj.title || obj.key}</div>
                <div class="oh-meta"><span class="oh-ref">{obj.ref_id}</span> · {obj.kind}</div>
              </div>
              <div class="subtabs">
                <button class:on={subtab === 'props'} onclick={() => (subtab = 'props')}>Properties</button>
                <button class:on={subtab === 'hooks'} onclick={() => (subtab = 'hooks')}>Hooks{#if obj.programs?.length} <span class="sc">{obj.programs.length}</span>{/if}</button>
                {#if objSubtabs.includes('dialogue')}
                  <button class:on={subtab === 'dialogue'} onclick={() => (subtab = 'dialogue')}>Dialogue</button>
                {/if}
              </div>
              <div class="subbody">
                {#if subtab === 'props'}
                  <PropertiesPanel {obj} {rooms} onchanged={structureChanged} ondeleted={onObjectDeleted} onedit={openObject} />
                {:else if subtab === 'hooks'}
                  <HooksPanel {obj} activeHook={null} onopen={(h) => openHookTab(obj.ref_id, h)} onchanged={structureChanged} />
                {:else}
                  <div class="dlg-launch">
                    <MessagesSquareIcon size={26} />
                    <p class="dl-lead">Ink dialogue</p>
                    <p class="dl-hint">
                      {obj.attrs?._ink_source ? 'This NPC has a dialogue script.' : 'No dialogue yet.'}
                      Edit it with syntax highlighting and a live playtest in its own tab.
                    </p>
                    <button class="dl-open" onclick={() => openInkTab(obj.ref_id)}>
                      {obj.attrs?._ink_source ? 'Open dialogue editor' : 'Write dialogue'}
                    </button>
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        {/if}
      </div>
    </main>
  </div>
</div>

{#if finderOpen}
  <ObjectFinder
    onpick={pickFinder}
    onclose={() => (finderOpen = false)}
    extras={maps.map((m) => ({ id: m, label: m, kind: 'map' }))}
    onpickextra={(m) => { finderOpen = false; openTab({ type: 'maps', name: m }); }}
  />
{/if}

<svelte:window onkeydown={onGlobalKey} />

<style>
  .ws { position: fixed; inset: 0; z-index: 200; display: flex; flex-direction: column; background: var(--bg-primary, #0e0c0a); color: var(--text-primary, #ece0c8); }
  .top { display: flex; align-items: center; gap: 12px; padding: 9px 14px; border-bottom: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); }
  .back { display: inline-flex; align-items: center; gap: 6px; background: none; border: none; color: var(--text-primary, #ece0c8); cursor: pointer; font: inherit; font-size: 13px; padding: 5px 9px; border-radius: 8px; }
  .back:hover { background: var(--bg-primary, rgba(255,255,255,.06)); }
  .icon-btn { display: inline-flex; align-items: center; justify-content: center; background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; padding: 5px; border-radius: 6px; }
  .icon-btn:hover { background: var(--bg-primary, #12100c); color: var(--text-primary, #ece0c8); }
  .title { display: flex; align-items: center; gap: 8px; color: var(--accent-amber, #c9956b); }
  .title h1 { font-size: 15px; font-weight: 600; margin: 0; color: var(--text-primary, #ece0c8); }
  .sp { flex: 1; }
  .find { display: inline-flex; align-items: center; gap: 6px; font: inherit; font-size: 12.5px; color: var(--text-secondary, #b6a888); background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 8px; padding: 5px 10px; cursor: pointer; }
  .find:hover { border-color: var(--accent-amber, #c9956b); color: var(--text-primary, #ece0c8); }
  .find kbd { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); color: var(--text-muted, #8c8378); border: 1px solid var(--border-muted, #2a2419); border-radius: 4px; padding: 0 4px; }

  .body { flex: 1; display: grid; grid-template-columns: 250px 1fr; min-height: 0; }
  .body.no-sidebar { grid-template-columns: 1fr; }

  .explorer { display: flex; flex-direction: column; border-right: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); min-height: 0; }

  .ex-top { padding: 8px 8px 0; }
  .new-btn { display: flex; align-items: center; justify-content: center; gap: 6px; width: 100%; background: color-mix(in srgb, var(--accent-amber, #c9956b) 16%, transparent); border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); border-radius: 7px; padding: 6px 10px; cursor: pointer; font: inherit; font-size: 12.5px; font-weight: 600; }
  .new-btn:hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 24%, transparent); }
  .new-form { display: flex; flex-direction: column; gap: 6px; padding: 8px; margin: 8px; border: 1px solid var(--border-default, #332c22); border-radius: 8px; background: var(--bg-primary, #12100c); }
  .nf-kinds { display: flex; gap: 4px; }
  .nf-kind { flex: 1; text-transform: capitalize; background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #332c22); color: var(--text-muted, #8c8378); border-radius: 6px; padding: 4px; cursor: pointer; font: inherit; font-size: 11.5px; }
  .nf-kind.on { background: color-mix(in srgb, var(--accent-amber, #c9956b) 16%, transparent); border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }
  .nf-in { background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #332c22); border-radius: 6px; color: var(--text-primary, #ece0c8); padding: 5px 8px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; outline: none; }
  .nf-in:focus { border-color: var(--accent-amber, #c9956b); }
  .nf-hint { font-size: var(--fs-meta); color: var(--text-muted, #8c8378); }
  .nf-actions { display: flex; gap: 6px; }
  .nf-go { flex: 1; background: var(--accent-amber, #c9956b); border: none; color: var(--bg-primary, #12100c); font-weight: 600; border-radius: 6px; padding: 5px; cursor: pointer; font: inherit; font-size: 12px; }
  .nf-go:disabled { opacity: 0.5; cursor: default; }
  .nf-x { background: none; border: 1px solid var(--border-default, #332c22); color: var(--text-muted, #8c8378); border-radius: 6px; padding: 5px 10px; cursor: pointer; font: inherit; font-size: 12px; }

  .views { display: flex; gap: 4px; padding: 8px; }
  .view-btn { flex: 1; display: inline-flex; align-items: center; justify-content: center; gap: 5px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); color: var(--text-secondary, #b6a888); border-radius: 6px; padding: 5px 4px; cursor: pointer; font: inherit; font-size: 11.5px; }
  .view-btn:hover { border-color: var(--accent-amber, #c9956b); color: var(--text-primary, #ece0c8); }
  .chips { display: flex; flex-wrap: wrap; gap: 3px; padding: 8px 8px 6px; border-bottom: 1px solid var(--border-muted, #211d16); }
  .chip { display: inline-flex; align-items: center; gap: 4px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 12px; cursor: pointer; padding: 2px 8px; color: var(--text-muted, #8c8378); font: inherit; font-size: 11px; }
  .chip:hover { color: var(--text-primary, #ece0c8); }
  .chip.active { background: color-mix(in srgb, var(--accent-amber, #c9956b) 16%, transparent); border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }
  .cc { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); opacity: 0.8; }
  .ex-search { display: flex; align-items: center; gap: 7px; padding: 7px 11px; border-bottom: 1px solid var(--border-muted, #211d16); color: var(--text-muted, #8c8378); }
  .ex-search input { flex: 1; background: none; border: none; color: var(--text-primary, #ece0c8); font: inherit; font-size: 12px; outline: none; }
  .ex-tree { flex: 1; min-height: 0; overflow-y: auto; }
  .ex-trunc { margin: 6px 10px; padding: 6px 8px; font-size: var(--fs-meta); line-height: 1.4; color: var(--accent-amber, #c9956b); background: color-mix(in srgb, var(--accent-amber, #c9956b) 10%, transparent); border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 35%, transparent); border-radius: 6px; }

  .center { display: flex; flex-direction: column; min-width: 0; min-height: 0; }

  .tabbar { display: flex; align-items: stretch; gap: 0; overflow-x: auto; border-bottom: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); flex: none; }
  .tabx { display: inline-flex; align-items: center; gap: 6px; padding: 7px 8px 7px 11px; border-right: 1px solid var(--border-muted, #211d16); cursor: pointer; color: var(--text-muted, #9a9186); white-space: nowrap; max-width: 220px; background: none; border-top: 2px solid transparent; }
  .tabx:hover { background: var(--bg-primary, #12100c); color: var(--text-primary, #ece0c8); }
  .tabx.active { background: var(--bg-primary, #12100c); color: var(--text-primary, #ece0c8); border-top-color: var(--accent-amber, #c9956b); }
  .tabx .ti { display: inline-flex; color: var(--accent-blue, #6ea3d0); flex: none; }
  .tabx.active .ti { color: var(--accent-amber, #c9956b); }
  .tl { font-size: 12.5px; overflow: hidden; text-overflow: ellipsis; font-family: var(--font-mono, ui-monospace, monospace); }
  .tref { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); color: var(--text-muted, #8c8378); }
  .tc-x { display: inline-flex; align-items: center; justify-content: center; background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; padding: 1px; border-radius: 4px; line-height: 0; }
  .tc-x:hover { background: color-mix(in srgb, var(--accent-red, #c96a5a) 20%, transparent); color: var(--accent-red, #e06c75); }

  .view { flex: 1; min-height: 0; overflow: hidden; }

  .obj-tab { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .obj-head { padding: 12px 16px 10px; border-bottom: 1px solid var(--border-default, #2a2419); }
  .oh-title { font-size: 16px; font-weight: 700; color: var(--text-primary, #ece0c8); }
  .oh-meta { margin-top: 2px; font-size: 11px; color: var(--text-muted, #8c8378); }
  .oh-ref { font-family: var(--font-mono, ui-monospace, monospace); }
  .subtabs { display: flex; gap: 2px; padding: 6px 12px 0; border-bottom: 1px solid var(--border-default, #2a2419); }
  .subtabs button { background: none; border: none; border-bottom: 2px solid transparent; color: var(--text-muted, #8c8378); cursor: pointer; font: inherit; font-size: 12.5px; padding: 6px 10px; display: inline-flex; align-items: center; gap: 5px; }
  .subtabs button:hover { color: var(--text-primary, #ece0c8); }
  .subtabs button.on { color: var(--accent-amber, #c9956b); border-bottom-color: var(--accent-amber, #c9956b); }
  .sc { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); background: rgba(143,184,119,0.16); color: var(--accent-green, #8fb877); border-radius: 8px; padding: 0 5px; }
  .subbody { flex: 1; min-height: 0; overflow-y: auto; }

  .none { color: var(--text-muted, #8c8378); font-style: italic; padding: 16px; font-size: 12.5px; }

  .dlg-launch { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px; padding: 40px 24px; text-align: center; color: var(--text-muted, #8c8378); }
  .dlg-launch :global(svg) { color: color-mix(in srgb, var(--accent-amber, #c9956b) 60%, transparent); }
  .dl-lead { margin: 4px 0 0; font-size: 14px; font-weight: 600; color: var(--text-secondary, #b6a888); }
  .dl-hint { margin: 0; max-width: 380px; font-size: 12.5px; line-height: 1.5; }
  .dl-open { margin-top: 8px; background: color-mix(in srgb, var(--accent-amber, #c9956b) 16%, transparent); border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); border-radius: 8px; padding: 7px 16px; cursor: pointer; font: inherit; font-size: 12.5px; font-weight: 600; }
  .dl-open:hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 26%, transparent); }

  .welcome { height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px; padding: 24px; text-align: center; color: var(--text-muted, #8c8378); }
  .welcome > :global(svg) { color: color-mix(in srgb, var(--accent-amber, #c9956b) 55%, transparent); margin-bottom: 4px; }
  .w-lead { margin: 0; font-size: 17px; font-weight: 700; color: var(--text-primary, #ece0c8); }
  .w-hint { margin: 0; max-width: 440px; font-size: 12.5px; line-height: 1.55; }
  .w-actions { display: flex; flex-wrap: wrap; gap: 8px; justify-content: center; margin-top: 6px; }
  .w-go, .w-alt { display: inline-flex; align-items: center; gap: 7px; font: inherit; font-size: 12.5px; border-radius: 8px; padding: 8px 14px; cursor: pointer; }
  .w-go { background: var(--accent-amber, #c9956b); border: 1px solid var(--accent-amber, #c9956b); color: var(--bg-primary, #12100c); font-weight: 600; }
  .w-go:hover { filter: brightness(1.06); }
  .w-alt { background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #332c22); color: var(--text-secondary, #b6a888); }
  .w-alt:hover { border-color: var(--accent-amber, #c9956b); color: var(--text-primary, #ece0c8); }
  .w-legend { margin: 12px 0 0; max-width: 480px; font-size: 11.5px; line-height: 1.6; color: var(--text-muted, #8c8378); border-top: 1px solid var(--border-muted, #211d16); padding-top: 12px; }
  .w-legend b { font-family: var(--font-mono, ui-monospace, monospace); color: var(--accent-amber, #c9956b); font-weight: 600; }

  @media (max-width: 900px) {
    .body { grid-template-columns: 180px 1fr; }
  }
</style>
