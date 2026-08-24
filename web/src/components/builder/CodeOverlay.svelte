<script>
  import { Button, showFlash, Tooltip } from '@kenn-io/kit-ui';
  import PlayIcon from '@lucide/svelte/icons/play';
  import SaveIcon from '@lucide/svelte/icons/save';
  import XIcon from '@lucide/svelte/icons/x';
  import CodeEditor from '../code/CodeEditor.svelte';
  import { api } from '../../lib/api.js';
  import { hookTemplate } from '../../lib/hook-templates.js';

  // The one code surface: full CodeMirror (lint, autocomplete, ⌘S) scoped to a
  // single object+hook, opened from the Hooks panel or a map node. Save writes
  // the program; Run evals the buffer as your character.
  let { refId = null, hook = null, objName = '', onclose = () => {}, onsaved = () => {} } = $props();

  let source = $state('');
  let original = $state('');
  let loading = $state(true);
  let saving = $state(false);
  let running = $state(false);
  let output = $state(null);
  const dirty = $derived(source !== original);

  $effect(() => {
    if (refId && hook) load(refId, hook);
  });

  async function load(ref, h) {
    loading = true;
    output = null;
    const res = await api('list_programs', { ref_id: ref });
    const existing = res?.ok ? res.data.find((x) => x.hook === h) : null;
    // A stub with no function definition → seed a real starter (mark dirty).
    const codeOnly = (existing?.source || '').replace(/--\[\[[\s\S]*?\]\]/g, '').replace(/--[^\n]*/g, '');
    if (/\bfunction\b/.test(codeOnly)) {
      source = original = existing.source;
    } else {
      source = hookTemplate(h);
      original = ''; // so it reads as dirty and Save persists the starter
    }
    loading = false;
  }

  async function save(src) {
    saving = true;
    const body = src ?? source;
    const res = await api('set_program', { ref_id: refId, hook, source: body });
    saving = false;
    if (res?.ok) {
      original = body;
      source = body;
      showFlash(`Saved ${hook}`, { tone: 'success' });
      onsaved();
    } else {
      output = { ok: false, text: res?.error || 'Save failed' };
    }
  }

  async function run() {
    running = true;
    output = { ok: true, text: 'Running…' };
    const res = await api('eval', { source });
    output = res?.ok ? { ok: true, text: res.data?.output ?? '(no output)' } : { ok: false, text: res?.error || 'Eval failed' };
    running = false;
  }
</script>

<div class="co">
  <header>
    <div class="cur">
      <span class="obj">{objName || refId}</span>
      <span class="sep">·</span>
      <b>{hook}</b>
      {#if dirty}<span class="dot">●</span>{/if}
    </div>
    <span class="sp"></span>
    <Tooltip text="Run — evaluate the buffer as your character"><Button size="sm" onclick={run} disabled={running}><PlayIcon size={13} /> Run</Button></Tooltip>
    <Tooltip text="Save program (⌘S)"><Button size="sm" tone="accent" onclick={() => save()} disabled={saving || !dirty}><SaveIcon size={13} /> Save</Button></Tooltip>
    <Tooltip text="Close editor" align="end"><button class="x" aria-label="Close editor" onclick={onclose}><XIcon size={16} /></button></Tooltip>
  </header>

  <div class="edit">
    {#if loading}
      <div class="none">Loading…</div>
    {:else}
      {#key refId + hook}
        <CodeEditor bind:value={source} onsave={save} onchange={() => {}} />
      {/key}
    {/if}
  </div>

  {#if output}
    <div class="out" class:err={!output.ok}>
      <div class="oh">{output.ok ? 'output' : 'error'}<Tooltip text="Dismiss" align="end"><button aria-label="Dismiss output" onclick={() => (output = null)}>✕</button></Tooltip></div>
      <pre>{output.text}</pre>
    </div>
  {/if}
</div>

<style>
  .co { display: flex; flex-direction: column; height: 100%; min-height: 0; background: var(--bg-primary, #12100c); }
  header { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); }
  .cur { display: flex; align-items: center; gap: 6px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12.5px; color: var(--text-muted, #9a9186); min-width: 0; }
  .cur .obj { color: var(--text-secondary, #b6a888); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cur b { color: var(--accent-amber, #c9956b); }
  .sep { color: var(--text-muted, #8c8378); }
  .dot { color: var(--accent-amber, #c9956b); }
  .sp { flex: 1; }
  .x { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; padding: 2px; line-height: 0; }
  .x:hover { color: var(--text-primary, #ece0c8); }
  .edit { flex: 1; min-height: 0; overflow: hidden; }
  .none { color: var(--text-muted, #8c8378); font-style: italic; padding: 14px; }
  .out { border-top: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); max-height: 34vh; overflow: auto; }
  .oh { display: flex; align-items: center; justify-content: space-between; font-size: var(--fs-label); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #9a9186); padding: 6px 12px; border-bottom: 1px solid var(--border-muted, #211d16); }
  .out.err .oh { color: var(--accent-red, #d07a5a); }
  .oh button { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; }
  .out pre { margin: 0; padding: 10px 12px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-primary, #ece0c8); white-space: pre-wrap; }
  .out.err pre { color: var(--accent-red, #d98d78); }
</style>
