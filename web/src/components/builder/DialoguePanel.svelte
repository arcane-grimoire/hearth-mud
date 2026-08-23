<script>
  import { Button, showFlash } from '@kenn-io/kit-ui';
  import { api } from '../../lib/api.js';

  // Ink dialogue editor. The engine already had ink_save/ink_load/ink_compile
  // REST actions — but no web component ever called them, so dialogue was
  // telnet-only by accident. Giving it a tab next to Properties and Hooks is
  // most of what makes the workspace feel like one tool.
  let { refId = null } = $props();

  let source = $state('');
  let original = $state('');
  let loading = $state(false);
  let saving = $state(false);
  let errors = $state(null);
  const dirty = $derived(source !== original);

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
      showFlash(res.data?.valid ? 'Dialogue saved' : 'Saved with errors', { tone: res.data?.valid ? 'success' : 'danger' });
    } else {
      showFlash(res.error || 'Failed to save', { tone: 'danger' });
    }
  }
</script>

<div class="dp">
  <div class="bar">
    <span class="lbl">Ink dialogue</span>
    {#if dirty}<span class="dot">● unsaved</span>{/if}
    <span class="sp"></span>
    <Button size="sm" tone="accent" onclick={save} disabled={saving || !dirty} label={saving ? '…' : 'Save'} />
  </div>
  {#if loading}
    <div class="none">Loading…</div>
  {:else}
    <textarea
      class="ink"
      bind:value={source}
      spellcheck="false"
      placeholder={'=== start ===\nHello, traveller.\n+ [Ask about the inn] -> inn\n+ [Leave] -> END\n\n=== inn ===\nThe Last Stag? Finest ale for miles.\n-> END'}
    ></textarea>
    {#if errors}
      <div class="err"><b>Compile errors</b><pre>{typeof errors === 'string' ? errors : JSON.stringify(errors, null, 2)}</pre></div>
    {/if}
  {/if}
</div>

<style>
  .dp { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .bar { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--border-default, #2a2419); }
  .lbl { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #8c8378); }
  .dot { font-size: 11px; color: var(--accent-amber, #c9956b); }
  .sp { flex: 1; }
  .ink {
    flex: 1; min-height: 220px; resize: none;
    background: var(--bg-primary, #12100c); color: var(--text-primary, #ece0c8);
    border: none; padding: 12px; outline: none;
    font-family: var(--font-mono, ui-monospace, monospace); font-size: 12.5px; line-height: 1.55;
  }
  .err { border-top: 1px solid var(--border-default, #2a2419); background: color-mix(in srgb, var(--accent-red, #c96a5a) 8%, transparent); padding: 8px 12px; }
  .err b { font-size: 10px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--accent-red, #d07a5a); }
  .err pre { margin: 4px 0 0; font-size: 11px; color: var(--accent-red, #d98d78); white-space: pre-wrap; }
  .none { color: var(--text-muted, #8c8378); font-style: italic; padding: 14px; }
</style>
