<script>
  import { SegmentedControl, Modal } from '@kenn-io/kit-ui';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import { route, setQuery } from '../lib/router.svelte.js';
  import RoomGraph from './room-builder/RoomGraph.svelte';
  import RoomTable from './room-builder/RoomTable.svelte';
  import RoomEditorModal from './room-builder/RoomEditorModal.svelte';

  // Full-page builder surface. Reached at /builder/rooms via the client
  // router, lazy-loaded so the play client never pulls this or the Svelte
  // Flow canvas into its bundle. Deep-linkable through the query string:
  // ?area=village&focus=%2312&depth=2&view=table.
  let { onexit = () => {} } = $props();

  const area = $derived(route.query.area || null);
  const focus = $derived(route.query.focus || null);
  const depth = $derived(route.query.depth || '2');
  const view = $derived(route.query.view === 'table' ? 'table' : 'graph');
  const viewOptions = [
    { value: 'graph', label: 'Graph' },
    { value: 'table', label: 'Table' },
  ];

  // Open a room from the table: focus it and drop back into the graph.
  function openInGraph(ref) { setQuery({ view: null, focus: ref, area: null }); }

  // Full modal editor (title + description + programs/hooks), reachable from a
  // table row or the graph inspector's "Full editor" button.
  let editingRef = $state(null);
  function openEditor(ref) { editingRef = ref; }
  function closeEditor() { editingRef = null; }

  // Bumped when the modal makes a structural change (tag/exit/delete) so the
  // active graph/table view reloads its slice.
  let dataVersion = $state(0);
</script>

<div class="room-builder">
  <header class="rb-top">
    <button class="rb-back" onclick={onexit}>
      <ArrowLeftIcon size={16} /> <span>Game</span>
    </button>
    <div class="rb-title">
      <svg class="rb-glyph" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
        <circle cx="5" cy="6" r="2.2" /><circle cx="19" cy="6" r="2.2" /><circle cx="12" cy="18" r="2.2" />
        <path d="M7 6h10M6 8l5 8M18 8l-5 8" stroke-linecap="round" />
      </svg>
      <h1>Room builder</h1>
      <span class="rb-scope">
        {#if area}area <b>{area}</b>{:else}<span class="muted">no area scoped</span>{/if}
        {#if focus}· focus <b>{focus}</b> · depth {depth}{/if}
      </span>
    </div>
    <div class="rb-spacer"></div>
    <SegmentedControl
      options={viewOptions}
      value={view}
      onchange={(v) => setQuery({ view: v === 'table' ? 'table' : null })}
      ariaLabel="View mode"
    />
  </header>

  <div class="rb-body">
    {#if view === 'table'}
      <RoomTable onopen={openInGraph} onedit={openEditor} reloadSignal={dataVersion} />
    {:else}
      <RoomGraph onedit={openEditor} reloadSignal={dataVersion} />
    {/if}
  </div>
</div>

{#if editingRef}
  <Modal title={`Edit · ${editingRef}`} maxWidth="min(580px, calc(100vw - 32px))" onclose={closeEditor}>
    <RoomEditorModal ref={editingRef} onclose={closeEditor} onchanged={() => (dataVersion += 1)} onedit={openEditor} />
  </Modal>
{/if}

<style>
  .room-builder {
    position: fixed; inset: 0; z-index: 200;
    display: flex; flex-direction: column;
    background: var(--bg-primary, #0e0c0a);
    color: var(--text-primary, #eee);
  }
  .rb-top {
    display: flex; align-items: center; gap: 18px;
    padding: 9px 14px;
    border-bottom: var(--border-width, 1px) solid var(--border-default, #2a2622);
    background: var(--bg-surface, #17140f);
  }
  .rb-back {
    display: inline-flex; align-items: center; gap: 6px;
    background: none; border: none; color: var(--text-primary, #eee);
    cursor: pointer; font: inherit; font-size: 13px;
    padding: 5px 9px; border-radius: var(--radius-md, 8px);
  }
  .rb-back:hover { background: var(--bg-primary, rgba(255, 255, 255, 0.06)); }
  .rb-back:focus-visible { outline: var(--focus-ring, 2px solid #c9956b); outline-offset: 1px; }
  .rb-title { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .rb-glyph { color: var(--accent-amber, #c9956b); flex: none; }
  .rb-title h1 { font-size: 15px; font-weight: 600; margin: 0; white-space: nowrap; }
  .rb-scope {
    font-size: 12px; color: var(--text-primary, #ccc);
    font-family: ui-monospace, "SF Mono", monospace;
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .rb-scope .muted { color: var(--text-muted, #8c8378); }
  .rb-scope b { color: var(--accent-amber, #c9956b); font-weight: 600; }
  .rb-spacer { flex: 1; }

  .rb-body { flex: 1; min-height: 0; overflow: hidden; }
</style>
