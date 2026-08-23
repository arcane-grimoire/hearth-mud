<script>
  import XIcon from '@lucide/svelte/icons/x';
  import SearchIcon from '@lucide/svelte/icons/search';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import CheckIcon from '@lucide/svelte/icons/check';
  import { loadHooks } from '../../lib/hooks.js';
  import { API_FUNCTIONS, API_GLOBALS, OBJECT_MEMBERS } from './hearth-api.js';
  import { hookTemplate } from '../../lib/hook-templates.js';

  // Scripting quick-reference docked to the right of the code editor
  // (/builder/code). Two sections: Hooks (the engine's live vocabulary via
  // `list_hooks`) and API (static function reference from hearth-api.js).
  // Rows expand inline for the signature + doc. Pure read-only data — nothing
  // here writes to the world.
  let { sel = null, onclose = () => {} } = $props();

  let q = $state('');
  let expanded = $state(null); // expanded row id ("api:emit", "hook:on_enter"…)

  // Context: the hook currently open in the editor gets a pinned card (doc +
  // signature + copy-template) at the top of the panel, and its row in the
  // Hooks list is highlighted and scrolled into view.
  let copied = $state(false);
  let copyTimer;
  const currentDoc = $derived(
    hooksData?.known?.find((h) => h.name === sel?.hook)?.describes || null,
  );
  async function copyTemplate() {
    if (!sel) return;
    try {
      await navigator.clipboard.writeText(hookTemplate(sel.hook));
      copied = true;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = false), 1500);
    } catch (e) { /* clipboard unavailable */ }
  }
  $effect(() => {
    const h = sel?.hook;
    if (!h) return;
    // Scroll after the highlight renders.
    requestAnimationFrame(() => {
      document.getElementById(`hp-hook-${h}`)?.scrollIntoView({ block: 'nearest' });
    });
  });

  // Engine-backed hook vocabulary (cached module-wide in lib/hooks.js).
  let hooksData = $state(null);
  loadHooks().then((d) => (hooksData = d));

  function toggle(id) {
    expanded = expanded === id ? null : id;
  }

  // --- Filtering -----------------------------------------------------------
  const needle = $derived(q.trim().toLowerCase());

  const hookGroups = $derived.by(() => {
    const known = hooksData?.known || [];
    const match = (h) =>
      !needle || `${h.name} ${h.describes || ''}`.toLowerCase().includes(needle);
    return [
      { label: 'Guards · can_', id: 'guards', items: known.filter((h) => h.name.startsWith('can_') && match(h)) },
      { label: 'Events · on_/cmd_', id: 'events', items: known.filter((h) => !h.name.startsWith('can_') && match(h)) },
    ].filter((g) => g.items.length);
  });

  const apiGroups = $derived.by(() => {
    const match = ([name, sig, doc]) =>
      !needle || `${name} ${sig} ${doc}`.toLowerCase().includes(needle);
    return [
      { label: 'Functions', id: 'fns', items: API_FUNCTIONS.filter(match), triple: true },
      { label: 'Hook arguments', id: 'globals', items: API_GLOBALS.filter(match), triple: true },
      { label: 'Object members', id: 'members', items: OBJECT_MEMBERS.filter(([m]) => !needle || m.includes(needle)), pair: true },
    ].filter((g) => g.items.length);
  });
</script>

<aside class="hp" aria-label="Scripting reference">
  <header class="hp-top">
    <div class="hp-search">
      <SearchIcon size={13} />
      <!-- svelte-ignore a11y_autofocus -->
      <input placeholder="Search the reference…" bind:value={q} autofocus />
    </div>
    <button class="hp-x" onclick={onclose} aria-label="Close reference panel" title="Close">
      <XIcon size={14} />
    </button>
  </header>

  <div class="hp-body">
    {#if sel}
      <div class="hp-cur">
        <div class="hp-h">Current · {sel.key || sel.ref}</div>
        <span class="hp-name hp-cur-name">{sel.hook}</span>
        {#if currentDoc}<p class="hp-cur-doc">{currentDoc}</p>{/if}
        <code class="hp-sig">function {sel.hook}(this, actor, room, args)</code>
        <button class="hp-copy" onclick={copyTemplate} title="Copy the starter example for this hook">
          {#if copied}<CheckIcon size={12} /> Copied{:else}<CopyIcon size={12} /> Copy starter{/if}
        </button>
      </div>
    {/if}

    {#if hookGroups.length}
      {#each hookGroups as g}
        <div class="hp-h">{g.label}</div>
        {#each g.items as h (h.name)}
          {@const id = `hook:${h.name}`}
          <button id={`hp-hook-${h.name}`} class="hp-row" class:cur={sel && sel.hook === h.name}
            class:open={expanded === id} aria-expanded={expanded === id}
            onclick={() => toggle(id)}>
            <span class="hp-name">{h.name}</span>
            <span class="hp-doc">{h.describes}</span>
          </button>
          {#if expanded === id}
            <div class="hp-detail">
              <code>function {h.name}(this, actor, room, args)</code>
            </div>
          {/if}
        {/each}
      {/each}
    {:else if !hooksData}
      <div class="hp-dim">Loading hooks…</div>
    {/if}

    {#each apiGroups as g}
      <div class="hp-h">{g.label}</div>
      {#each g.items as row (row[0])}
        {@const name = row[0]}
        {@const id = `${g.id}:${name}`}
        <button class="hp-row" class:open={expanded === id} aria-expanded={expanded === id}
          onclick={() => toggle(id)}>
          <span class="hp-name">{name}</span>
          <span class="hp-doc">{row[2] || row[1]}</span>
        </button>
        {#if expanded === id}
          <div class="hp-detail">
            <code>{row[1] || name}</code>
            {#if row[2] && g.triple && row[1] !== row[2]}<p>{row[2]}</p>{/if}
          </div>
        {/if}
      {/each}
    {/each}

    {#if !hookGroups.length && !apiGroups.length && hooksData}
      <div class="hp-dim">Nothing matches “{q}”.</div>
    {/if}
  </div>
</aside>

<style>
  .hp {
    display: flex; flex-direction: column; min-height: 0;
    border-left: 1px solid var(--border-default, #2a2419);
    background: var(--bg-surface, #17140f);
  }
  .hp-top { display: flex; align-items: center; gap: 6px; padding: 8px 8px 8px 11px; border-bottom: 1px solid var(--border-muted, #2a2419); }
  .hp-search { flex: 1; display: flex; align-items: center; gap: 7px; color: var(--text-muted, #9a9186); min-width: 0; }
  .hp-search input { width: 100%; background: none; border: none; color: var(--text-primary, #ece0c8); font: inherit; font-size: 12.5px; outline: none; }
  .hp-x { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; padding: 3px; line-height: 0; border-radius: 5px; flex: none; }
  .hp-x:hover { color: var(--text-primary, #ece0c8); background: var(--bg-primary, #12100c); }

  .hp-body { flex: 1; overflow-y: auto; padding: 4px 0 20px; }
  .hp-h { font-family: var(--font-mono, ui-monospace, monospace); font-size: 10px; letter-spacing: .08em; text-transform: uppercase; color: var(--accent-amber, #c9956b); padding: 12px 12px 3px; }
  .hp-row { display: block; width: 100%; text-align: left; background: none; border: none; cursor: pointer; padding: 4px 12px; }
  .hp-row:hover { background: var(--bg-primary, #12100c); }
  .hp-row.open { background: color-mix(in srgb, var(--accent-amber, #c9956b) 10%, transparent); }
  .hp-name { display: block; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-secondary, #b6a888); }
  .hp-row.open .hp-name, .hp-row:hover .hp-name { color: var(--accent-amber, #c9956b); }
  .hp-doc { display: block; font-size: 11px; color: var(--text-muted, #8c8378); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .hp-detail { margin: 0 12px 6px; padding: 7px 9px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-muted, #2a2419); border-radius: 7px; }
  .hp-detail code { display: block; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--accent-green, #8fb877); white-space: pre-wrap; word-break: break-word; }
  .hp-detail p { margin: 5px 0 0; font-size: 11.5px; line-height: 1.45; color: var(--text-secondary, #b6a888); }
  .hp-dim { color: var(--text-muted, #8c8378); font-size: 12px; font-style: italic; padding: 12px; }

  /* Current-hook card (context wiring) */
  .hp-cur { margin: 6px 8px 2px; padding: 9px 10px; background: var(--bg-primary, #12100c); border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 35%, transparent); border-radius: 9px; }
  .hp-cur .hp-h { padding: 0 0 4px; color: var(--text-muted, #8c8378); }
  .hp-cur-name { font-size: 13px; color: var(--accent-amber, #c9956b); }
  .hp-cur-doc { margin: 3px 0 6px; font-size: 11.5px; line-height: 1.4; color: var(--text-secondary, #b6a888); }
  .hp-sig { display: block; font-family: var(--font-mono, ui-monospace, monospace); font-size: 10.5px; color: var(--accent-green, #8fb877); white-space: pre-wrap; word-break: break-word; margin-bottom: 7px; }
  .hp-copy { display: inline-flex; align-items: center; gap: 5px; font: inherit; font-size: 11px; color: var(--accent-amber, #c9956b); background: none; border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 40%, transparent); border-radius: 6px; padding: 3px 9px; cursor: pointer; }
  .hp-copy:hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 12%, transparent); }

  /* The row matching the hook open in the editor */
  .hp-row.cur { box-shadow: inset 3px 0 0 var(--accent-amber, #c9956b); background: color-mix(in srgb, var(--accent-amber, #c9956b) 8%, transparent); }
  .hp-row.cur .hp-name { color: var(--accent-amber, #c9956b); }
</style>
