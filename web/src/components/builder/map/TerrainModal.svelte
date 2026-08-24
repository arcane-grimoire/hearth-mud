<script>
  import { Modal, Button, showFlash } from '@kenn-io/kit-ui';
  import AttrEditor from './AttrEditor.svelte';

  // Add/edit a terrain type. Ported from mapwright.html openTerrain + tSave.
  let { editingCh = null, palette = {}, schema = [], onsave = () => {}, ondelete = () => {}, onclose = () => {} } = $props();

  const seed = editingCh ? palette[editingCh] : { theme: '', title_prefix: '', passable: true, color: '#88aa66', attrs: {} };
  let ch = $state(editingCh || '');
  let color = $state(/^#[0-9a-f]{6}$/i.test(seed.color || '') ? seed.color : '#88aa66');
  let theme = $state(seed.theme || '');
  let prefix = $state(seed.title_prefix || '');
  let passable = $state(seed.passable !== false);
  let attrs = $state({ ...(seed.attrs || {}) });
  let attrEd;

  function save() {
    const key = ch.trim().slice(0, 1);
    if (!key) { showFlash('A terrain needs a single character', { tone: 'danger' }); return; }
    onsave(key, { theme: theme.trim(), title_prefix: prefix.trim(), passable, color, attrs }, editingCh);
  }
</script>

<Modal title={editingCh ? `Edit terrain ‘${editingCh}’` : 'New terrain type'} maxWidth="min(460px, calc(100vw - 32px))" {onclose}>
  <div class="body">
    <div class="row2">
      <div class="field mono"><label for="t-ch">Character</label><input id="t-ch" type="text" maxlength="1" spellcheck="false" placeholder="f" bind:value={ch} /></div>
      <div class="field"><label for="t-color">Swatch color</label><input id="t-color" class="color" type="color" bind:value={color} /></div>
    </div>
    <div class="row2">
      <div class="field"><label for="t-theme">Theme</label><input id="t-theme" type="text" spellcheck="false" placeholder="forest" bind:value={theme} /></div>
      <div class="field"><label for="t-prefix">Title prefix</label><input id="t-prefix" type="text" placeholder="Forest" bind:value={prefix} /></div>
    </div>
    <label class="check"><input type="checkbox" bind:checked={passable} /> Passable — players can walk here</label>
    <div class="sub">
      <h4>Attributes <span class="dim">read by your spawn / populate hooks</span></h4>
      <AttrEditor bind:this={attrEd} attrs={seed.attrs || {}} {schema} onchange={(o) => (attrs = o)} />
      <button class="addbtn" onclick={() => attrEd?.add()}>+ attribute</button>
    </div>
  </div>
  {#snippet footer()}
    {#if editingCh}<Button size="sm" tone="danger" surface="soft" label="Delete" onclick={() => ondelete(editingCh)} />{/if}
    <span class="spacer"></span>
    <Button size="sm" label="Cancel" onclick={onclose} />
    <Button size="sm" tone="accent" label="Save terrain" onclick={save} />
  {/snippet}
</Modal>

<style>
  .body { display: flex; flex-direction: column; gap: 12px; }
  .row2 { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
  .field { display: flex; flex-direction: column; gap: 5px; }
  .field label { font-size: 11px; text-transform: uppercase; letter-spacing: 0.07em; color: var(--text-muted); font-weight: 600; }
  .field input { background: var(--bg-inset); border: 1px solid var(--border-default); border-radius: 7px; padding: 8px 10px; font-size: 13px; color: var(--text-primary); outline: none; }
  .field input:focus { border-color: var(--accent-amber); }
  .field.mono input { font-family: var(--font-mono); }
  .field input.color { height: 38px; padding: 3px; cursor: pointer; }
  .check { display: flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--text-primary); }
  .check input { width: 15px; height: 15px; accent-color: var(--accent-amber); }
  .sub { border-top: 1px solid var(--border-default); padding-top: 13px; }
  .sub h4 { margin: 0 0 9px; font-family: var(--font-mono); font-size: var(--fs-label); letter-spacing: 0.12em; text-transform: uppercase; color: var(--text-muted); }
  .dim { text-transform: none; letter-spacing: 0; color: var(--text-muted); font-family: var(--font-sans); }
  .addbtn { margin-top: 8px; border: 1px dashed var(--border-default); background: transparent; color: var(--text-muted); border-radius: 7px; padding: 6px; font-size: 11.5px; font-weight: 500; width: 100%; cursor: pointer; }
  .addbtn:hover { color: var(--accent-amber); border-color: color-mix(in srgb, var(--accent-amber) 40%, transparent); }
  .spacer { flex: 1; }
</style>
