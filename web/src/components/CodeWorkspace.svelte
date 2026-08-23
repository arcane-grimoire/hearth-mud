<script>
  import { Button, showFlash, Typeahead } from '@kenn-io/kit-ui';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import PlayIcon from '@lucide/svelte/icons/play';
  import SaveIcon from '@lucide/svelte/icons/save';
  import HistoryIcon from '@lucide/svelte/icons/history';
  import SearchIcon from '@lucide/svelte/icons/search';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import BookOpenIcon from '@lucide/svelte/icons/book-open';
  import CodeEditor from './code/CodeEditor.svelte';
  import HelpPanel from './code/HelpPanel.svelte';
  import { api } from '../lib/api.js';
  import { route } from '../lib/router.svelte.js';
  import { loadHooks, hookOptions, isValidHookName } from '../lib/hooks.js';
  import { hookTemplate } from '../lib/hook-templates.js';

  // Full-page Luau workspace at /builder/code: explorer (objects → hooks) |
  // editor | output panel. Deep-linkable via ?ref=#9&hook=on_enter.
  let { onexit = () => {} } = $props();

  let entries = $state([]); // [{ref_id, key, title, kind, area, hooks[]}]
  let loading = $state(true);
  let filter = $state('');
  let sel = $state(null); // {ref, hook, key, title}
  let source = $state('');
  let dirty = $state(false);
  let saving = $state(false);
  let running = $state(false);
  let output = $state(null); // {ok, text}
  let versions = $state([]);
  let showHist = $state(false);
  let newRef = $state('');
  let newHook = $state('');
  let hooksData = $state(null);
  let hookErr = $state('');
  const hookOpts = $derived(hookOptions(hooksData));

  // Scripting reference panel (right side). Local state — one boolean, nothing
  // else needs it. Default closed; remembered across visits. The slim rail
  // keeps it discoverable when closed.
  let helpOpen = $state((typeof localStorage !== 'undefined' && localStorage.getItem('cw-help')) === '1');
  $effect(() => { try { localStorage.setItem('cw-help', helpOpen ? '1' : '0'); } catch {} });

  const wantRef = $derived(route.query.ref || '');
  const wantHook = $derived(route.query.hook || '');

  async function loadExplorer() {
    loading = true;
    try {
      const res = await api('list_programs_all');
      entries = res?.ok ? res.data : [];
    } catch (e) { entries = []; }
    loading = false;
  }

  // Open a hook on any object by ref (an object with no programs isn't in the
  // explorer yet). Non-destructive: openHook loads existing code or seeds a
  // starter template, marking it dirty — nothing is written until Save.
  async function addHookToRef() {
    const r = newRef.trim();
    const h = newHook.trim();
    if (!r || !h) return;
    newRef = '';
    newHook = '';
    await openDeepLink(r, h);
  }

  onMountLike();
  function onMountLike() { loadExplorer(); loadHooks().then((d) => (hooksData = d)); }

  // Validate a picked/typed hook name against the engine's vocabulary before
  // it reaches set_program. Returning false keeps the Typeahead open with the
  // error row so the user can correct a typo without a round-trip. On a valid
  // pick, if a ref is already filled in, open the hook straight away (seeding
  // its starter) so the example appears the instant you choose it.
  function pickHook(v) {
    if (!isValidHookName(v, hooksData)) {
      hookErr = `“${v}” isn't a valid hook — use a known hook or an on_/cmd_/lib_ name`;
      return false;
    }
    hookErr = '';
    newHook = v;
    if (newRef.trim()) addHookToRef();
  }

  // Open the hook named in the URL once the explorer has loaded — even if the
  // object has no programs yet (so a deep link to a brand-new hook still opens,
  // seeded with a starter template).
  let opened = null;
  $effect(() => {
    const key = `${wantRef}|${wantHook}`;
    if (wantRef && wantHook && key !== opened && !loading) {
      opened = key;
      openDeepLink(wantRef, wantHook);
    }
  });
  async function openDeepLink(refId, hook) {
    let e = entries.find((x) => x.ref_id === refId);
    if (!e) {
      // Not in the explorer (no programs yet): look up its name so the header
      // reads nicely, then open an empty hook to seed a template into.
      try {
        const ex = await api('examine', { ref_id: refId });
        e = ex?.ok ? { ref_id: refId, key: ex.data.key, title: ex.data.title } : null;
      } catch (err) { /* ignore */ }
      e = e || { ref_id: refId, key: refId, title: refId };
    }
    openHook(e, hook);
  }

  const grouped = $derived.by(() => {
    const needle = filter.trim().toLowerCase();
    const map = new Map();
    for (const e of entries) {
      if (needle && !`${e.key} ${e.title} ${e.hooks.join(' ')}`.toLowerCase().includes(needle)) continue;
      const area = e.area || 'unfiled';
      if (!map.has(area)) map.set(area, []);
      map.get(area).push(e);
    }
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  });

  async function openHook(entry, hook) {
    if (dirty && !confirm('Discard unsaved changes?')) return;
    sel = { ref: entry.ref_id, hook, key: entry.key, title: entry.title };
    source = '';
    output = null;
    showHist = false;
    versions = [];
    let existing = null;
    try {
      const res = await api('list_programs', { ref_id: entry.ref_id });
      existing = res?.ok ? res.data.find((x) => x.hook === hook) : null;
    } catch (e) { existing = null; }
    // A program that defines no function is just a placeholder (e.g. the old
    // "-- on_enter hook" stub) — treat it as empty and seed a real example.
    // Strip Lua comments first so a comment mentioning "function" doesn't count.
    const codeOnly = (existing?.source || '')
      .replace(/--\[\[[\s\S]*?\]\]/g, '')
      .replace(/--[^\n]*/g, '');
    const hasCode = /\bfunction\b/.test(codeOnly);
    if (hasCode) {
      source = existing.source;
      dirty = false;
    } else {
      // Start from a working example. Mark it dirty so ⌘S / Save persists the
      // starter and the "unsaved" state shows.
      source = hookTemplate(hook);
      dirty = true;
    }
  }

  async function save(src) {
    if (!sel) return;
    saving = true;
    const body = src ?? source;
    const res = await api('set_program', { ref_id: sel.ref, hook: sel.hook, source: body });
    saving = false;
    if (res?.ok) {
      dirty = false;
      showFlash?.({ message: `Saved ${sel.hook}`, tone: 'success' });
      // A newly-created hook (or a new object) should show up in the explorer.
      if (!entries.some((e) => e.ref_id === sel.ref && e.hooks?.includes(sel.hook))) loadExplorer();
    }
    else { output = { ok: false, text: res?.error || 'Save failed' }; }
  }

  async function run() {
    if (!sel) return;
    running = true;
    output = { ok: true, text: 'Running…' };
    try {
      const res = await api('eval', { source });
      output = res?.ok ? { ok: true, text: res.data?.output ?? '(no output)' } : { ok: false, text: res?.error || 'Eval failed' };
    } catch (e) { output = { ok: false, text: String(e) }; }
    running = false;
  }

  async function toggleHistory() {
    showHist = !showHist;
    if (showHist && sel) {
      try {
        const res = await api('program_history', { ref_id: sel.ref, hook: sel.hook });
        versions = res?.ok ? res.data : [];
      } catch (e) { versions = []; }
    }
  }
  async function restore(version) {
    if (!sel) return;
    const res = await api('program_restore', { ref_id: sel.ref, hook: sel.hook, version });
    if (res?.ok) { await openHook({ ref_id: sel.ref, key: sel.key, title: sel.title }, sel.hook); showHist = false; showFlash?.({ message: `Restored v${version}`, tone: 'success' }); }
  }
</script>

<div class="cw">
  <header class="cw-top">
    <button class="cw-back" onclick={onexit}><ArrowLeftIcon size={16} /> <span>Game</span></button>
    <div class="cw-title">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" aria-hidden="true">
        <path d="M8 6l-5 6 5 6M16 6l5 6-5 6" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
      <h1>Code editor</h1>
      {#if sel}<span class="cw-cur">{sel.key} · <b>{sel.hook}</b>{#if dirty} <span class="cw-dot">●</span>{/if}</span>{/if}
    </div>
    <div class="cw-spacer"></div>
    <button class="cw-help" class:on={helpOpen} aria-expanded={helpOpen}
      aria-controls="cw-help-panel" onclick={() => (helpOpen = !helpOpen)}
      title="Scripting reference">
      <BookOpenIcon size={14} /> <span>Reference</span>
    </button>
    {#if sel}
      <Button size="sm" onclick={toggleHistory}><HistoryIcon size={14} /> History</Button>
      <Button size="sm" onclick={run} disabled={running}><PlayIcon size={14} /> Run</Button>
      <Button size="sm" tone="accent" onclick={() => save()} disabled={saving || !dirty}><SaveIcon size={14} /> Save</Button>
    {/if}
  </header>

  <div class="cw-main" class:help-open={helpOpen}>
    <aside class="cw-explorer">
      <div class="cw-search">
        <SearchIcon size={13} />
        <input placeholder="Filter hooks…" bind:value={filter} />
      </div>
      <form class="cw-newhook" onsubmit={(e) => { e.preventDefault(); addHookToRef(); }}>
        <input class="cw-nh-ref" bind:value={newRef} placeholder="#ref" spellcheck="false" />
        <div class="cw-nh-hook">
          <Typeahead
            options={hookOpts}
            value={newHook}
            fallbackLabel="new hook"
            placeholder="on_enter, cmd_talk…"
            emptyLabel="No matching hook"
            allowCustom
            customLabel={'Use "{query}"'}
            onselect={pickHook}
          />
        </div>
        <button type="submit" title="Open hook (scaffolds a starter if it's new)"><PlusIcon size={13} /></button>
      </form>
      {#if hookErr}<div class="cw-nh-err">{hookErr}</div>{/if}
      <div class="cw-tree">
        {#if loading}
          <div class="cw-dim">Loading…</div>
        {:else if !grouped.length}
          <div class="cw-dim">No programs yet. Add a hook from a room’s editor.</div>
        {:else}
          {#each grouped as [area, objs]}
            <div class="cw-area">{area}</div>
            {#each objs as e}
              <div class="cw-obj">{e.title || e.key}</div>
              {#each e.hooks as h}
                <button class="cw-hook" class:active={sel && sel.ref === e.ref_id && sel.hook === h}
                  onclick={() => openHook(e, h)}>{h}</button>
              {/each}
            {/each}
          {/each}
        {/if}
      </div>
    </aside>

    <section class="cw-editor">
      {#if sel}
        {#key sel.ref + sel.hook}
          <CodeEditor bind:value={source} onsave={save} onchange={() => (dirty = true)} />
        {/key}
        {#if showHist}
          <div class="cw-hist">
            <div class="cw-hist-h">Versions</div>
            {#if versions.length}
              {#each versions as v, i}
                <div class="cw-ver">
                  <span class="cw-vn">v{v.version ?? i + 1}</span>
                  <span class="cw-vm">{v.author || v.recorded_at || ''}</span>
                  <button onclick={() => restore(v.version ?? i + 1)}>Restore</button>
                </div>
              {/each}
            {:else}
              <div class="cw-dim">No history.</div>
            {/if}
          </div>
        {/if}
      {:else}
        <div class="cw-empty">
          <p>Pick a hook from the left to edit its Luau.</p>
          <p class="cw-hint">⌘S saves · Run evals as your character · errors lint live.</p>
        </div>
      {/if}
    </section>

    {#if helpOpen}
      <div id="cw-help-panel" class="cw-help-col"><HelpPanel onclose={() => (helpOpen = false)} /></div>
    {:else}
      <!-- Slim rail so the panel stays discoverable when closed -->
      <button id="cw-help-rail" class="cw-rail" aria-expanded="false"
        aria-controls="cw-help-panel" onclick={() => (helpOpen = true)}
        title="Scripting reference"><BookOpenIcon size={15} /></button>
    {/if}
  </div>

  {#if output}
    <div class="cw-panel" class:err={!output.ok}>
      <div class="cw-panel-h">{output.ok ? 'output' : 'error'}<button class="cw-x" onclick={() => (output = null)}>✕</button></div>
      <pre>{output.text}</pre>
    </div>
  {/if}
</div>

<style>
  .cw { position: fixed; inset: 0; z-index: 200; display: grid; grid-template-rows: auto 1fr auto; background: var(--bg-primary, #0e0c0a); color: var(--text-primary, #ece0c8); }
  .cw-top { display: flex; align-items: center; gap: 14px; padding: 9px 14px; border-bottom: var(--border-width, 1px) solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); }
  .cw-back { display: inline-flex; align-items: center; gap: 6px; background: none; border: none; color: var(--text-primary, #ece0c8); cursor: pointer; font: inherit; font-size: 13px; padding: 5px 9px; border-radius: var(--radius-md, 8px); }
  .cw-back:hover { background: var(--bg-primary, rgba(255,255,255,.06)); }
  .cw-title { display: flex; align-items: center; gap: 9px; min-width: 0; }
  .cw-title svg { color: var(--accent-amber, #c9956b); }
  .cw-title h1 { font-size: 15px; font-weight: 600; margin: 0; white-space: nowrap; }
  .cw-cur { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-muted, #9a9186); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .cw-cur b { color: var(--accent-amber, #c9956b); }
  .cw-dot { color: var(--accent-amber, #c9956b); }
  .cw-spacer { flex: 1; }

  .cw-main { display: grid; grid-template-columns: 244px 1fr; min-height: 0; position: relative; }
  .cw-main.help-open { grid-template-columns: 244px 1fr 320px; }
  .cw-help { display: inline-flex; align-items: center; gap: 6px; font: inherit; font-size: 12px; color: var(--text-muted, #9a9186); background: none; border: 1px solid var(--border-default, #332c22); border-radius: var(--radius-md, 8px); padding: 4px 10px; cursor: pointer; }
  .cw-help:hover, .cw-help.on { border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }
  .cw-help-col { min-height: 0; min-width: 0; }
  .cw-rail { position: absolute; right: 0; top: 50%; transform: translateY(-50%); z-index: 210; display: flex; flex-direction: column; align-items: center; justify-content: center; width: 26px; height: 56px; background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #2a2419); border-right: none; border-radius: 8px 0 0 8px; color: var(--text-muted, #9a9186); cursor: pointer; box-shadow: -6px 0 18px -12px rgba(0,0,0,.6); }
  .cw-rail:hover { color: var(--accent-amber, #c9956b); width: 30px; }

  .cw-explorer { border-right: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); display: flex; flex-direction: column; min-height: 0; }
  .cw-search { display: flex; align-items: center; gap: 7px; padding: 9px 11px; border-bottom: 1px solid var(--border-muted, #2a2419); color: var(--text-muted, #9a9186); }
  .cw-search input { flex: 1; background: none; border: none; color: var(--text-primary, #ece0c8); font: inherit; font-size: 12.5px; outline: none; }
  .cw-newhook { display: flex; align-items: center; gap: 5px; padding: 7px 9px; border-bottom: 1px solid var(--border-muted, #2a2419); }
  .cw-nh-ref { background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 6px; color: var(--text-primary, #ece0c8); font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; padding: 4px 6px; outline: none; width: 48px; flex: none; }
  .cw-nh-ref:focus { border-color: var(--accent-amber, #c9956b); }
  .cw-nh-hook { flex: 1; min-width: 0; }
  /* The explorer is narrow; let the hook picker's popover spill wider so the
     descriptions stay readable (it floats over the editor pane, which is fine). */
  .cw-nh-hook :global(.kit-typeahead__panel) { min-width: 300px; }
  .cw-nh-err { color: var(--accent-red, #d07a5a); font-size: 10.5px; padding: 4px 9px 0; }
  .cw-newhook > button { background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 6px; color: var(--text-muted, #9a9186); cursor: pointer; padding: 4px; line-height: 0; }
  .cw-newhook > button:hover { border-color: var(--accent-amber, #c9956b); color: var(--accent-amber, #c9956b); }
  .cw-tree { flex: 1; overflow-y: auto; padding: 6px 0 20px; }
  .cw-area { font-family: var(--font-mono, ui-monospace, monospace); font-size: 10px; letter-spacing: .08em; text-transform: uppercase; color: var(--accent-amber, #c9956b); padding: 10px 12px 3px; }
  .cw-obj { font-size: 12px; color: var(--text-secondary, #b6a888); padding: 3px 12px 1px; font-weight: 600; }
  .cw-hook { display: block; width: 100%; text-align: left; background: none; border: none; cursor: pointer; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-muted, #9a9186); padding: 3px 12px 3px 24px; }
  .cw-hook:hover { color: var(--text-primary, #ece0c8); background: var(--bg-primary, #12100c); }
  .cw-hook.active { color: var(--accent-amber, #c9956b); background: color-mix(in srgb, var(--accent-amber, #c9956b) 12%, transparent); }
  .cw-dim { color: var(--text-muted, #8c8378); font-size: 12px; font-style: italic; padding: 12px; }

  .cw-editor { position: relative; min-height: 0; overflow: hidden; }
  .cw-empty { height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 6px; color: var(--text-muted, #9a9186); }
  .cw-hint { font-size: 12px; color: var(--text-muted, #8c8378); font-family: var(--font-mono, ui-monospace, monospace); }

  .cw-hist { position: absolute; top: 10px; right: 10px; width: 240px; max-height: 60%; overflow-y: auto; background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #2a2419); border-radius: 10px; box-shadow: 0 12px 34px -16px rgba(0,0,0,.7); padding: 8px; }
  .cw-hist-h { font-size: 10px; text-transform: uppercase; letter-spacing: .07em; color: var(--text-muted, #9a9186); padding: 4px 6px 8px; }
  .cw-ver { display: flex; align-items: center; gap: 8px; padding: 5px 6px; border-radius: 6px; }
  .cw-ver:hover { background: var(--bg-primary, #12100c); }
  .cw-vn { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--accent-amber, #c9956b); }
  .cw-vm { flex: 1; font-size: 11px; color: var(--text-muted, #8c8378); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cw-ver button { background: none; border: 1px solid var(--border-default, #332c22); color: var(--text-secondary, #b6a888); border-radius: 5px; font-size: 11px; padding: 2px 7px; cursor: pointer; }
  .cw-ver button:hover { border-color: var(--accent-amber, #c9956b); color: var(--accent-amber, #c9956b); }

  .cw-panel { border-top: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); max-height: 34vh; overflow: auto; }
  .cw-panel-h { display: flex; align-items: center; justify-content: space-between; font-size: 10px; text-transform: uppercase; letter-spacing: .07em; color: var(--text-muted, #9a9186); padding: 6px 12px; border-bottom: 1px solid var(--border-muted, #2a2419); }
  .cw-panel.err .cw-panel-h { color: var(--accent-red, #d07a5a); }
  .cw-panel pre { margin: 0; padding: 10px 12px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-primary, #ece0c8); white-space: pre-wrap; }
  .cw-panel.err pre { color: var(--accent-red, #d98d78); }
  .cw-x { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; font-size: 12px; }
</style>
