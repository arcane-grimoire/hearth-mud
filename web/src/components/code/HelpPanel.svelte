<script>
  import XIcon from '@lucide/svelte/icons/x';
  import SearchIcon from '@lucide/svelte/icons/search';
  import CopyIcon from '@lucide/svelte/icons/copy';
  import CheckIcon from '@lucide/svelte/icons/check';
  import ListFilterIcon from '@lucide/svelte/icons/list-filter';
  import { loadHooks } from '../../lib/hooks.js';
  import { API_FUNCTIONS, API_GLOBALS, OBJECT_MEMBERS, API_CATEGORIES, API_EXAMPLES } from './hearth-api.js';
  import { hookTemplate } from '../../lib/hook-templates.js';

  // Scripting quick-reference docked to the right of the code editor. Two
  // sections: Hooks (the engine's live vocabulary via `list_hooks`, each row
  // expands to its starter template) and API (the function reference from
  // hearth-api.js, grouped into categories, each row expands to its signature,
  // doc, and — where useful — a worked example). Filtering expands every match
  // so the detail is visible without a second click. Read-only — nothing here
  // writes to the world.
  const fnByName = new Map(API_FUNCTIONS.map((r) => [r[0], r]));
  let { sel = null, open = true, lookup = null, onclose = () => {} } = $props();

  let q = $state('');
  // A right-click "Look up in Help" from the editor arrives as { term, n };
  // seed the search box and focus it. Keyed on the whole object so a repeat
  // lookup of the same word (new identity, bumped n) re-triggers.
  let lastLookupN = 0;
  $effect(() => {
    if (lookup && lookup.n !== lastLookupN) {
      lastLookupN = lookup.n;
      q = lookup.term;
      requestAnimationFrame(() => searchInput?.select());
    }
  });
  let expanded = $state(null); // expanded row id ("fns:emit", "members:attrs"…)
  let searchInput = $state(null);

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
  // Focus the search box when the panel is explicitly opened — not on mount,
  // so restoring a persisted-open panel never steals focus from the editor.
  let wasOpen = false;
  $effect(() => {
    if (open && !wasOpen) requestAnimationFrame(() => searchInput?.focus());
    wasOpen = open;
  });

  // Engine-backed hook vocabulary (cached module-wide in lib/hooks.js).
  let hooksData = $state(null);
  loadHooks().then((d) => (hooksData = d));

  function toggle(id) {
    expanded = expanded === id ? null : id;
  }

  // --- Filtering -----------------------------------------------------------
  const needle = $derived(q.trim().toLowerCase());
  // A row shows its detail when explicitly expanded, OR whenever a filter is
  // active — filtering already narrows to a handful of matches, so expanding
  // them all is what "search then read" wants (and it's how a right-click
  // lookup lands straight on the answer).
  const filtering = $derived(needle.length > 0);
  const shown = (id) => filtering || expanded === id;

  // Section filter: a small multi-select dropdown that narrows which groups
  // render. The ids match the group ids the hookGroups/apiGroups derivations
  // produce. Empty selection = show everything (the default). Selecting groups
  // ANDs with the text search.
  const GROUP_SECTIONS = [
    { label: 'Hooks', groups: [
      { id: 'events', label: 'Events · on_/cmd_' },
      { id: 'guards', label: 'Guards · can_' },
    ] },
    { label: 'API', groups: [
      { id: 'globals', label: 'Hook arguments' },
      { id: 'members', label: 'Object members' },
      ...API_CATEGORIES.map((c) => ({ id: c.id, label: c.label })),
    ] },
  ];
  let selectedGroups = $state([]); // [] = all
  let filterOpen = $state(false);
  const isGroupSel = (id) => selectedGroups.includes(id);
  const groupShown = (id) => selectedGroups.length === 0 || selectedGroups.includes(id);
  function toggleGroup(id) {
    selectedGroups = isGroupSel(id) ? selectedGroups.filter((x) => x !== id) : [...selectedGroups, id];
  }
  function clearGroups() { selectedGroups = []; }

  const hookGroups = $derived.by(() => {
    const known = hooksData?.known || [];
    const match = (h) =>
      !needle || `${h.name} ${h.describes || ''}`.toLowerCase().includes(needle);
    return [
      { label: 'Events · on_/cmd_', id: 'events', items: known.filter((h) => !h.name.startsWith('can_') && match(h)) },
      { label: 'Guards · can_', id: 'guards', items: known.filter((h) => h.name.startsWith('can_') && match(h)) },
    ].filter((g) => g.items.length && groupShown(g.id));
  });

  // Function reference, grouped by category (see API_CATEGORIES), plus the
  // hook-argument and object-member sections. Every group's items share one
  // shape — { name, sig, doc, example? } — so the template renders them
  // uniformly. Any function missing from a category lands in a trailing
  // "Other" bucket so a newly-added engine function can't silently vanish.
  const apiGroups = $derived.by(() => {
    const matchFn = (r) => !needle || `${r[0]} ${r[1]} ${r[2]}`.toLowerCase().includes(needle);
    const toItem = (r) => ({ name: r[0], sig: r[1], doc: r[2], example: API_EXAMPLES[r[0]] });
    const categorized = new Set(API_CATEGORIES.flatMap((c) => c.names));

    const fnGroups = API_CATEGORIES.map((c) => ({
      id: c.id, label: c.label,
      items: c.names.map((n) => fnByName.get(n)).filter(Boolean).filter(matchFn).map(toItem),
    }));
    const orphans = API_FUNCTIONS.filter((r) => !categorized.has(r[0]) && matchFn(r)).map(toItem);
    if (orphans.length) fnGroups.push({ id: 'other', label: 'Other', items: orphans });

    // Hook arguments first, so a search for "actor"/"room" lands on the object
    // itself (which expands to its properties) before the functions that
    // merely mention it. Object members follow as a directly-searchable list;
    // functions, the bulk of the reference, come last under their categories.
    return [
      { id: 'globals', label: 'Hook arguments',
        items: API_GLOBALS.filter(matchFn).map((r) => ({ name: r[0], sig: r[1], doc: r[2], isObject: r[3] === 'object' })) },
      { id: 'members', label: 'Object members · this.field / actor.field',
        items: OBJECT_MEMBERS
          .filter(([m, d]) => !needle || `${m} ${d}`.toLowerCase().includes(needle))
          .map(([m, d]) => ({ name: m, sig: m, doc: d })) },
      ...fnGroups,
    ].filter((g) => g.items.length && groupShown(g.id));
  });
</script>

<!-- Escape closes the section dropdown first if it's open, else the panel. -->
<svelte:window onkeydown={(e) => { if (e.key !== 'Escape') return; if (filterOpen) filterOpen = false; else onclose(); }} />

<aside class="hp" aria-label="Scripting reference">
  <header class="hp-top">
    <div class="hp-search">
      <SearchIcon size={13} />
      <!-- Focus follows explicit opens only (see effect below) — never mount,
           so a persisted-open panel doesn't yank focus from the editor. -->
      <input placeholder="Search the reference…" bind:value={q} bind:this={searchInput} />
    </div>
    <button class="hp-filter" class:on={selectedGroups.length > 0} onclick={() => (filterOpen = !filterOpen)}
      aria-expanded={filterOpen} aria-haspopup="true" title="Filter sections">
      <ListFilterIcon size={14} />
      {#if selectedGroups.length > 0}<span class="hp-fbadge">{selectedGroups.length}</span>{/if}
    </button>
    <button class="hp-x" onclick={onclose} aria-label="Close reference panel" title="Close">
      <XIcon size={14} />
    </button>
  </header>

  {#if filterOpen}
    <!-- Click-away backdrop, then the checkbox popover above it. -->
    <button class="hp-pop-back" aria-label="Close section filter" onclick={() => (filterOpen = false)}></button>
    <div class="hp-pop" role="group" aria-label="Filter sections">
      <div class="hp-pop-top">
        <span>Show sections</span>
        {#if selectedGroups.length > 0}
          <button class="hp-pop-clear" onclick={clearGroups}>Clear</button>
        {/if}
      </div>
      <div class="hp-pop-body">
        {#each GROUP_SECTIONS as sec}
          <div class="hp-pop-h">{sec.label}</div>
          {#each sec.groups as g}
            <label class="hp-pop-row">
              <input type="checkbox" checked={isGroupSel(g.id)} onchange={() => toggleGroup(g.id)} />
              <span>{g.label}</span>
            </label>
          {/each}
        {/each}
      </div>
    </div>
  {/if}

  <div class="hp-body">
    {#if sel}
      <div class="hp-cur">
        <div class="hp-h">Current · {sel.key || sel.ref}</div>
        <span class="hp-name hp-cur-name">{sel.hook}</span>
        {#if currentDoc}<p class="hp-cur-doc">{currentDoc}</p>{/if}
        <!-- No rendered signature: hook arg-lists differ per hook
             (on_tick(this), can_get(this, actor, item), …) and the engine
             doesn't expose them yet. A made-up uniform signature would read
             as authoritative and be wrong for most hooks. -->
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
          <!-- Hooks show their one-line description inline always, so the
               starter template stays click-to-reveal even while filtering
               (unlike API rows, where the signature/doc IS the payload). -->
          <button id={`hp-hook-${h.name}`} class="hp-row" class:cur={sel && sel.hook === h.name}
            class:open={expanded === id} aria-expanded={expanded === id} onclick={() => toggle(id)}>
            <span class="hp-name">{h.name}</span>
            <span class="hp-doc">{h.describes}</span>
          </button>
          {#if expanded === id}
            <div class="hp-detail">
              <div class="hp-detail-h">Starter template</div>
              <code>{hookTemplate(h.name)}</code>
            </div>
          {/if}
        {/each}
      {/each}
    {:else if !hooksData}
      <div class="hp-dim">Loading hooks…</div>
    {/if}

    {#each apiGroups as g}
      <div class="hp-h">{g.label}</div>
      {#each g.items as row (row.name)}
        {@const id = `${g.id}:${row.name}`}
        <button class="hp-row" class:open={shown(id)} aria-expanded={shown(id)}
          onclick={() => toggle(id)}>
          <span class="hp-name">{row.name}</span>
          <span class="hp-doc">{row.doc || row.sig}</span>
        </button>
        {#if shown(id)}
          <div class="hp-detail">
            {#if row.sig && row.sig !== row.name}<code>{row.sig}</code>{/if}
            {#if row.doc && row.doc !== row.sig}<p>{row.doc}</p>{/if}
            {#if row.example}
              <div class="hp-detail-h">Example</div>
              <pre class="hp-ex">{row.example}</pre>
            {/if}
            {#if row.isObject}
              <div class="hp-detail-h">Properties · {row.name}.field</div>
              <ul class="hp-members">
                {#each OBJECT_MEMBERS as [m, d]}
                  <li><code class="hp-mem">{row.name}.{m}</code> <span>{d}</span></li>
                {/each}
              </ul>
            {/if}
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
    display: flex; flex-direction: column; min-height: 0; position: relative;
    border-left: 1px solid var(--border-default, #2a2419);
    background: var(--bg-surface, #17140f);
  }
  .hp-top { display: flex; align-items: center; gap: 6px; padding: 8px 8px 8px 11px; border-bottom: 1px solid var(--border-muted, #2a2419); }
  .hp-search { flex: 1; display: flex; align-items: center; gap: 7px; color: var(--text-muted, #9a9186); min-width: 0; }
  .hp-search input { width: 100%; background: none; border: none; color: var(--text-primary, #ece0c8); font: inherit; font-size: 12.5px; outline: none; }
  .hp-x, .hp-filter { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; padding: 3px; line-height: 0; border-radius: 5px; flex: none; }
  .hp-x:hover, .hp-filter:hover { color: var(--text-primary, #ece0c8); background: var(--bg-primary, #12100c); }
  .hp-filter { display: inline-flex; align-items: center; gap: 3px; }
  .hp-filter.on { color: var(--accent-amber, #c9956b); }
  .hp-fbadge { font-family: var(--font-mono, ui-monospace, monospace); font-size: 9.5px; line-height: 1; min-width: 13px; height: 13px; padding: 0 3px; display: inline-flex; align-items: center; justify-content: center; border-radius: 999px; background: var(--accent-amber, #c9956b); color: var(--bg-primary, #12100c); }

  /* Section-filter dropdown */
  .hp-pop-back { position: absolute; inset: 0; z-index: 20; background: none; border: none; padding: 0; cursor: default; }
  .hp-pop { position: absolute; top: 44px; right: 8px; z-index: 21; width: 210px; max-height: min(60%, 340px); display: flex; flex-direction: column; background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #332c22); border-radius: 9px; box-shadow: 0 12px 30px -12px rgba(0,0,0,.7); overflow: hidden; }
  .hp-pop-top { display: flex; align-items: center; justify-content: space-between; padding: 8px 10px; border-bottom: 1px solid var(--border-muted, #2a2419); font-size: var(--fs-label, 10px); letter-spacing: .06em; text-transform: uppercase; color: var(--text-muted, #8c8378); }
  .hp-pop-clear { background: none; border: none; color: var(--accent-amber, #c9956b); font: inherit; font-size: 10.5px; text-transform: none; letter-spacing: 0; cursor: pointer; padding: 0; }
  .hp-pop-clear:hover { text-decoration: underline; }
  .hp-pop-body { overflow-y: auto; padding: 4px 0 6px; }
  .hp-pop-h { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-label, 10px); letter-spacing: .08em; text-transform: uppercase; color: var(--accent-amber, #c9956b); padding: 8px 10px 2px; }
  .hp-pop-row { display: flex; align-items: center; gap: 8px; padding: 4px 10px; font-size: 12px; color: var(--text-secondary, #b6a888); cursor: pointer; }
  .hp-pop-row:hover { background: var(--bg-primary, #12100c); }
  .hp-pop-row input { accent-color: var(--accent-amber, #c9956b); cursor: pointer; margin: 0; flex: none; }

  .hp-body { flex: 1; overflow-y: auto; padding: 4px 0 20px; }
  .hp-h { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-label); letter-spacing: .08em; text-transform: uppercase; color: var(--accent-amber, #c9956b); padding: 12px 12px 3px; }
  .hp-row { display: block; width: 100%; text-align: left; background: none; border: none; cursor: pointer; padding: 4px 12px; }
  .hp-row:hover { background: var(--bg-primary, #12100c); }
  .hp-row.open { background: color-mix(in srgb, var(--accent-amber, #c9956b) 10%, transparent); }
  .hp-name { display: block; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-secondary, #b6a888); }
  .hp-row.open .hp-name, .hp-row:hover .hp-name { color: var(--accent-amber, #c9956b); }
  .hp-doc { display: block; font-size: 11px; color: var(--text-muted, #8c8378); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .hp-detail { margin: 0 12px 6px; padding: 7px 9px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-muted, #2a2419); border-radius: 7px; }
  .hp-detail code { display: block; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--accent-green, #8fb877); white-space: pre-wrap; word-break: break-word; }
  .hp-detail p { margin: 5px 0 0; font-size: 11.5px; line-height: 1.45; color: var(--text-secondary, #b6a888); }
  .hp-detail-h { margin: 8px 0 3px; font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta, 10px); letter-spacing: .07em; text-transform: uppercase; color: var(--text-muted, #8c8378); }
  .hp-detail-h:first-child { margin-top: 0; }
  .hp-ex { margin: 0; padding: 7px 8px; background: color-mix(in srgb, var(--accent-amber, #c9956b) 6%, var(--bg-surface, #17140f)); border: 1px solid var(--border-muted, #2a2419); border-radius: 6px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; line-height: 1.5; color: var(--text-primary, #ece0c8); white-space: pre-wrap; word-break: break-word; overflow-x: auto; }
  .hp-members { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 3px; }
  .hp-members li { display: flex; flex-direction: column; gap: 1px; }
  .hp-mem { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--accent-green, #8fb877); }
  .hp-members li span { font-size: 11px; line-height: 1.4; color: var(--text-secondary, #b6a888); }
  .hp-dim { color: var(--text-muted, #8c8378); font-size: 12px; font-style: italic; padding: 12px; }

  /* Current-hook card (context wiring) */
  .hp-cur { margin: 6px 8px 2px; padding: 9px 10px; background: var(--bg-primary, #12100c); border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 35%, transparent); border-radius: 9px; }
  .hp-cur .hp-h { padding: 0 0 4px; color: var(--text-muted, #8c8378); }
  .hp-cur-name { font-size: 13px; color: var(--accent-amber, #c9956b); }
  .hp-cur-doc { margin: 3px 0 6px; font-size: 11.5px; line-height: 1.4; color: var(--text-secondary, #b6a888); }
  .hp-sig { display: block; font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); color: var(--accent-green, #8fb877); white-space: pre-wrap; word-break: break-word; margin-bottom: 7px; }
  .hp-copy { display: inline-flex; align-items: center; gap: 5px; font: inherit; font-size: 11px; color: var(--accent-amber, #c9956b); background: none; border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 40%, transparent); border-radius: 6px; padding: 3px 9px; cursor: pointer; }
  .hp-copy:hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 12%, transparent); }

  /* Hook rows are informational (doc-only — the engine doesn't expose
     per-hook arg lists), so they're divs, not clickable buttons. */
  .hp-row:not(button):hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 6%, transparent); }

  /* The row matching the hook open in the editor */
  .hp-row.cur { box-shadow: inset 3px 0 0 var(--accent-amber, #c9956b); background: color-mix(in srgb, var(--accent-amber, #c9956b) 8%, transparent); }
  .hp-row.cur .hp-name { color: var(--accent-amber, #c9956b); }
</style>
