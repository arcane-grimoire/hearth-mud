<script>
  import { Button, showFlash } from '@kenn-io/kit-ui';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import PlayIcon from '@lucide/svelte/icons/play';
  import SaveIcon from '@lucide/svelte/icons/save';
  import HistoryIcon from '@lucide/svelte/icons/history';
  import SearchIcon from '@lucide/svelte/icons/search';
  import CodeEditor from './code/CodeEditor.svelte';
  import { api } from '../lib/api.js';
  import { route } from '../lib/router.svelte.js';

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

  onMountLike();
  function onMountLike() { loadExplorer(); }

  // Open the hook named in the URL once the explorer is loaded.
  let opened = null;
  $effect(() => {
    const key = `${wantRef}|${wantHook}`;
    if (wantRef && wantHook && key !== opened && entries.length) {
      opened = key;
      const e = entries.find((x) => x.ref_id === wantRef);
      if (e) openHook(e, wantHook);
    }
  });

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
    try {
      const res = await api('list_programs', { ref_id: entry.ref_id });
      const p = res?.ok ? res.data.find((x) => x.hook === hook) : null;
      source = p?.source || '';
    } catch (e) { source = ''; }
    dirty = false;
  }

  async function save(src) {
    if (!sel) return;
    saving = true;
    const body = src ?? source;
    const res = await api('set_program', { ref_id: sel.ref, hook: sel.hook, source: body });
    saving = false;
    if (res?.ok) { dirty = false; showFlash?.({ message: `Saved ${sel.hook}`, tone: 'success' }); }
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
    {#if sel}
      <Button size="sm" onclick={toggleHistory}><HistoryIcon size={14} /> History</Button>
      <Button size="sm" onclick={run} disabled={running}><PlayIcon size={14} /> Run</Button>
      <Button size="sm" tone="accent" onclick={() => save()} disabled={saving || !dirty}><SaveIcon size={14} /> Save</Button>
    {/if}
  </header>

  <div class="cw-main">
    <aside class="cw-explorer">
      <div class="cw-search">
        <SearchIcon size={13} />
        <input placeholder="Filter hooks…" bind:value={filter} />
      </div>
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

  .cw-main { display: grid; grid-template-columns: 244px 1fr; min-height: 0; }
  .cw-explorer { border-right: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); display: flex; flex-direction: column; min-height: 0; }
  .cw-search { display: flex; align-items: center; gap: 7px; padding: 9px 11px; border-bottom: 1px solid var(--border-muted, #2a2419); color: var(--text-muted, #9a9186); }
  .cw-search input { flex: 1; background: none; border: none; color: var(--text-primary, #ece0c8); font: inherit; font-size: 12.5px; outline: none; }
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
