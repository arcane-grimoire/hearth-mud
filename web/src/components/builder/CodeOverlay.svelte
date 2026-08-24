<script>
  import { Button, showFlash, Tooltip } from '@kenn-io/kit-ui';
  import PlayIcon from '@lucide/svelte/icons/play';
  import SaveIcon from '@lucide/svelte/icons/save';
  import XIcon from '@lucide/svelte/icons/x';
  import BookOpenIcon from '@lucide/svelte/icons/book-open';
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import CodeEditor from '../code/CodeEditor.svelte';
  import HelpPanel from '../code/HelpPanel.svelte';
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

  // Collapsible scripting-reference panel docked on the right. Default closed;
  // remembered across visits. A slim rail keeps it discoverable when closed.
  let helpOpen = $state((typeof localStorage !== 'undefined' && localStorage.getItem('co-help')) === '1');
  $effect(() => { try { localStorage.setItem('co-help', helpOpen ? '1' : '0'); } catch {} });
  // What the panel highlights: the object + hook currently in the editor.
  const sel = $derived(refId && hook ? { hook, ref: refId, key: objName } : null);

  // Right-click "Look up in Help" from the editor: open the panel and seed its
  // search with the symbol. The nonce makes repeat lookups of the same word
  // re-trigger (the object identity changes even when the term repeats).
  let lookup = $state(null);
  let lookupN = 0;
  function lookupInHelp(word) {
    helpOpen = true;
    lookup = { term: word, n: ++lookupN };
  }

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
    <Tooltip text={helpOpen ? 'Hide scripting reference' : 'Show scripting reference'}>
      <button class="help-btn" class:on={helpOpen} aria-expanded={helpOpen}
        aria-controls={helpOpen ? 'co-help-panel' : undefined}
        onclick={() => (helpOpen = !helpOpen)}>
        <BookOpenIcon size={14} /> <span>Help</span>
      </button>
    </Tooltip>
    <Tooltip text="Close editor" align="end"><button class="x" aria-label="Close editor" onclick={onclose}><XIcon size={16} /></button></Tooltip>
  </header>

  <div class="mid">
    <div class="edit">
      {#if loading}
        <div class="none">Loading…</div>
      {:else}
        {#key refId + hook}
          <CodeEditor bind:value={source} onsave={save} onchange={() => {}} onlookup={lookupInHelp} />
        {/key}
      {/if}
    </div>

    {#if helpOpen}
      <div id="co-help-panel" class="help-col"><HelpPanel {sel} {lookup} open={helpOpen} onclose={() => (helpOpen = false)} /></div>
    {:else}
      <!-- Slim rail so the panel stays discoverable when closed. -->
      <button class="rail" aria-expanded="false" aria-controls="co-help-panel"
        onclick={() => (helpOpen = true)} title="Show scripting reference">
        <ChevronLeftIcon size={14} />
        <span class="rail-label">Help</span>
      </button>
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

  .help-btn { display: inline-flex; align-items: center; gap: 5px; font: inherit; font-size: 12px; color: var(--text-muted, #9a9186); background: none; border: 1px solid var(--border-default, #332c22); border-radius: var(--radius-md, 8px); padding: 4px 9px; cursor: pointer; }
  .help-btn:hover, .help-btn.on { border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }

  /* Editor + docked reference sit side by side; output stacks below both. */
  .mid { flex: 1; min-height: 0; display: flex; position: relative; }
  .edit { flex: 1; min-width: 0; min-height: 0; overflow: hidden; }
  .help-col { width: 320px; flex: none; min-height: 0; min-width: 0; }
  .rail { position: absolute; right: 0; top: 50%; transform: translateY(-50%); z-index: 5; display: flex; flex-direction: column; align-items: center; gap: 6px; width: 26px; padding: 12px 0; background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #2a2419); border-right: none; border-radius: 8px 0 0 8px; color: var(--text-muted, #9a9186); cursor: pointer; box-shadow: -6px 0 18px -12px rgba(0,0,0,.6); }
  .rail:hover { color: var(--accent-amber, #c9956b); width: 30px; }
  .rail-label { writing-mode: vertical-rl; text-orientation: mixed; font-size: 11px; letter-spacing: .06em; text-transform: uppercase; }

  @media (max-width: 720px) {
    /* Too narrow to dock — float the reference over the editor. */
    .help-col { position: absolute; inset: 0 0 0 auto; width: min(320px, 100%); z-index: 6; box-shadow: -12px 0 28px -16px rgba(0,0,0,.7); }
  }
  .none { color: var(--text-muted, #8c8378); font-style: italic; padding: 14px; }
  .out { border-top: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); max-height: 34vh; overflow: auto; }
  .oh { display: flex; align-items: center; justify-content: space-between; font-size: var(--fs-label); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #9a9186); padding: 6px 12px; border-bottom: 1px solid var(--border-muted, #211d16); }
  .out.err .oh { color: var(--accent-red, #d07a5a); }
  .oh button { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; }
  .out pre { margin: 0; padding: 10px 12px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-primary, #ece0c8); white-space: pre-wrap; }
  .out.err pre { color: var(--accent-red, #d98d78); }
</style>
