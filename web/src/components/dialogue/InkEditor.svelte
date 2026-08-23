<script>
  import SaveIcon from '@lucide/svelte/icons/save';
  import PlugIcon from '@lucide/svelte/icons/plug';
  import PanelRightIcon from '@lucide/svelte/icons/panel-right';
  import MessagesSquareIcon from '@lucide/svelte/icons/messages-square';
  import { showFlash } from '@kenn-io/kit-ui';
  import { api } from '../../lib/api.js';
  import InkCodeEditor from './InkCodeEditor.svelte';
  import PlaytestPane from './PlaytestPane.svelte';

  // The full-surface dialogue editor — the Ink analogue of CodeOverlay. Opens
  // as its own top-level workspace tab: an Ink code editor on the left, a live
  // playtest on the right, and a one-click "wire up" that gives the NPC the
  // cmd_talk / on_dialog_choice hooks that play this script.
  let { refId = null, objName = '', onsaved = () => {} } = $props();

  let source = $state('');
  let original = $state('');
  let loading = $state(false);
  let saving = $state(false);
  let errors = $state(null);
  let showPlaytest = $state(true);
  let wiring = $state(false);
  const dirty = $derived(source !== original);

  // Note the leading `-> start`: Ink begins at the top of the file, so a
  // script that opens with a knot header needs a divert into it or nothing
  // plays. Getting this right in the sample saves a common first-timer snag.
  const SAMPLE =
    '-> start\n\n=== start ===\nHello, traveller. What brings you to the Last Stag?\n+ [Ask about the inn] -> inn\n+ [Ask about work] -> work\n+ [Leave] -> END\n\n=== inn ===\nThe Last Stag? Finest ale for miles. # emit:Aldric polishes a mug.\n-> start\n\n=== work ===\nThere\'s trouble in the old dungeon, if you\'ve the stomach for it.\n-> END';

  $effect(() => {
    if (refId) load(refId);
  });

  async function load(ref) {
    loading = true;
    errors = null;
    const res = await api('ink_load', { ref_id: ref });
    if (res.ok) {
      source = res.data?.source || '';
      original = source;
      errors = res.data?.errors || null;
    } else {
      source = original = '';
    }
    loading = false;
  }

  async function save() {
    saving = true;
    const res = await api('ink_save', { ref_id: refId, source });
    saving = false;
    if (res.ok) {
      original = source;
      errors = res.data?.valid ? null : res.data?.errors;
      showFlash(res.data?.valid ? 'Dialogue saved' : 'Saved — has compile errors', {
        tone: res.data?.valid ? 'success' : 'danger',
      });
      onsaved();
    } else {
      showFlash(res.error || 'Failed to save', { tone: 'danger' });
    }
  }

  // Give the NPC the two hooks that play this dialogue, following the project's
  // blessed convention (the `dialog` lib module drives rendering + the choice
  // loop; the script itself is the NPC's saved _ink_source). Never clobbers a
  // hook that already exists.
  const WIRE = {
    cmd_talk:
      'local dialog = require("dialog")\n\n-- Start this NPC\'s dialogue (its saved Ink source).\nfunction cmd_talk(this, actor, room, args)\n    dialog.start(actor, this)\nend\n',
    on_dialog_choice:
      'local dialog = require("dialog")\n\n-- Handle the player\'s reply during a conversation.\nfunction on_dialog_choice(this, actor, room, args)\n    dialog.on_choice(this, actor, room, args)\nend\n',
  };

  async function wireUp() {
    wiring = true;
    const existing = await api('list_programs', { ref_id: refId });
    const have = new Set((existing.ok ? existing.data?.hooks || existing.data || [] : []).map((h) => (typeof h === 'string' ? h : h.hook)));
    const todo = Object.keys(WIRE).filter((h) => !have.has(h));
    if (!todo.length) {
      wiring = false;
      showFlash('Already wired up (cmd_talk + on_dialog_choice exist)', { tone: 'default' });
      return;
    }
    let ok = 0;
    for (const hook of todo) {
      const r = await api('set_program', { ref_id: refId, hook, source: WIRE[hook] });
      if (r.ok) ok++;
    }
    wiring = false;
    showFlash(`Added ${ok} hook${ok === 1 ? '' : 's'} (${todo.join(', ')})`, { tone: ok ? 'success' : 'danger' });
    if (ok) onsaved();
  }
</script>

<div class="ink-editor">
  <header class="bar">
    <span class="ic"><MessagesSquareIcon size={14} /></span>
    <span class="name">{objName || refId}</span>
    <span class="sub">dialogue</span>
    {#if dirty}<span class="dot">● unsaved</span>{/if}
    <span class="sp"></span>
    <button class="btn" onclick={wireUp} disabled={wiring} title="Give this NPC cmd_talk + on_dialog_choice hooks">
      <PlugIcon size={13} /> {wiring ? '…' : 'Wire up NPC'}
    </button>
    <button class="btn" class:on={showPlaytest} onclick={() => (showPlaytest = !showPlaytest)} title="Toggle playtest pane">
      <PanelRightIcon size={13} /> Playtest
    </button>
    <button class="btn save" onclick={save} disabled={saving || !dirty}>
      <SaveIcon size={13} /> {saving ? '…' : 'Save'}
    </button>
  </header>

  {#if loading}
    <div class="none">Loading…</div>
  {:else}
    <div class="split" class:solo={!showPlaytest}>
      <div class="edit-col">
        {#key refId}
          <InkCodeEditor bind:value={source} onsave={save} />
        {/key}
        {#if errors}
          <div class="err"><b>Compile errors</b><pre>{typeof errors === 'string' ? errors : JSON.stringify(errors, null, 2)}</pre></div>
        {/if}
        {#if !source.trim()}
          <button class="seed" onclick={() => (source = SAMPLE)}>Insert a sample conversation</button>
        {/if}
      </div>
      {#if showPlaytest}
        <div class="play-col">
          {#key refId}
            <PlaytestPane {refId} {source} />
          {/key}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .ink-editor { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .bar { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); flex: none; }
  .ic { display: inline-flex; color: var(--accent-amber, #c9956b); }
  .name { font-size: 13px; font-weight: 600; color: var(--text-primary, #ece0c8); }
  .sub { font-size: 10px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #8c8378); }
  .dot { font-size: 11px; color: var(--accent-amber, #c9956b); }
  .sp { flex: 1; }
  .btn { display: inline-flex; align-items: center; gap: 5px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); color: var(--text-secondary, #b6a888); border-radius: 6px; padding: 5px 10px; cursor: pointer; font: inherit; font-size: 12px; }
  .btn:hover:not(:disabled) { border-color: var(--accent-amber, #c9956b); color: var(--text-primary, #ece0c8); }
  .btn:disabled { opacity: 0.5; cursor: default; }
  .btn.on { color: var(--accent-amber, #c9956b); border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); }
  .btn.save { background: color-mix(in srgb, var(--accent-amber, #c9956b) 16%, transparent); border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }
  .btn.save:hover:not(:disabled) { background: color-mix(in srgb, var(--accent-amber, #c9956b) 26%, transparent); }

  .split { flex: 1; min-height: 0; display: grid; grid-template-columns: 1fr 380px; }
  .split.solo { grid-template-columns: 1fr; }
  .edit-col { display: flex; flex-direction: column; min-width: 0; min-height: 0; position: relative; }
  .play-col { min-width: 0; min-height: 0; border-left: 1px solid var(--border-default, #2a2419); }

  .err { border-top: 1px solid var(--border-default, #2a2419); background: color-mix(in srgb, var(--accent-red, #c96a5a) 8%, transparent); padding: 8px 12px; max-height: 140px; overflow-y: auto; flex: none; }
  .err b { font-size: 10px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--accent-red, #d07a5a); }
  .err pre { margin: 4px 0 0; font-size: 11px; color: var(--accent-red, #d98d78); white-space: pre-wrap; }

  .seed { position: absolute; bottom: 14px; left: 50%; transform: translateX(-50%); background: var(--bg-surface, #17140f); border: 1px dashed var(--border-default, #332c22); color: var(--text-muted, #8c8378); border-radius: 7px; padding: 6px 12px; cursor: pointer; font: inherit; font-size: 12px; }
  .seed:hover { border-color: var(--accent-amber, #c9956b); color: var(--text-primary, #ece0c8); }

  .none { color: var(--text-muted, #8c8378); font-style: italic; padding: 16px; }
</style>
