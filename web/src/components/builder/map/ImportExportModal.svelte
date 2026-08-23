<script>
  import { Modal, Button, showFlash } from '@kenn-io/kit-ui';
  import { serializeMap, serializePalette, parseTOML, importMap, importPalette } from '../../../lib/mapwright-toml.js';

  // Import/export TOML for the map file or the shared terrain.toml. Ported from
  // mapwright.html modal. On import, parses and hands the parsed root up.
  let { mode = 'export', mapState, server = false, onimport = () => {}, onsavedoc = () => {}, onclose = () => {} } = $props();

  let doc = $state('map'); // 'map' | 'palette'
  let text = $state('');
  let msg = $state('');
  let msgErr = $state(false);

  function refresh() {
    if (mode === 'export') text = doc === 'map' ? serializeMap(mapState) : serializePalette(mapState);
  }
  function setDoc(d) { doc = d; msg = ''; msgErr = false; refresh(); }
  refresh();

  async function copy() {
    try { await navigator.clipboard.writeText(text); msg = 'Copied to clipboard'; msgErr = false; }
    catch (e) { msg = 'Select-all ready — press ⌘/Ctrl-C'; msgErr = false; }
  }
  function load() {
    try {
      const root = parseTOML(text);
      if (doc === 'map') importMap(root); else importPalette(root); // validate shape (throws)
      onimport(doc, root);
      showFlash((doc === 'map' ? 'Map' : 'Palette') + ' loaded', { tone: 'success' });
      onclose();
    } catch (err) { msg = err.message || 'Could not parse TOML'; msgErr = true; }
  }
</script>

<Modal title={mode === 'export' ? 'Export TOML' : 'Import TOML'} maxWidth="min(680px, calc(100vw - 32px))" {onclose}>
  <div class="body">
    <div class="tabs">
      <button class:on={doc === 'map'} onclick={() => setDoc('map')}>Map file</button>
      <button class:on={doc === 'palette'} onclick={() => setDoc('palette')}>terrain.toml</button>
    </div>
    <textarea bind:value={text} readonly={mode === 'export'} spellcheck="false"
      placeholder={mode === 'import' ? 'Paste a map file or terrain.toml here, pick which it is above, then Load.' : ''}></textarea>
  </div>
  {#snippet footer()}
    <span class="msg" class:err={msgErr}>{msg}</span>
    {#if server}<Button size="sm" tone="accent" surface="soft" label="Save to game" onclick={() => onsavedoc(doc)} />{/if}
    {#if mode === 'export'}<Button size="sm" label="Copy" onclick={copy} />{/if}
    {#if mode === 'import'}<Button size="sm" tone="accent" label="Load into builder" onclick={load} />{/if}
    <Button size="sm" label="Close" onclick={onclose} />
  {/snippet}
</Modal>

<style>
  .body { display: flex; flex-direction: column; gap: 12px; }
  .tabs { display: flex; gap: 4px; }
  .tabs button { border: 1px solid transparent; background: transparent; color: var(--text-muted); border-radius: 7px; padding: 5px 11px; font-size: 12.5px; font-weight: 500; cursor: pointer; }
  .tabs button.on { background: var(--bg-inset); color: var(--text-primary); border-color: var(--border-default); }
  textarea { width: 100%; min-height: 320px; background: var(--bg-inset); color: var(--text-primary); border: 1px solid var(--border-default); border-radius: 9px; padding: 12px; font-family: var(--font-mono); font-size: 12px; line-height: 1.6; resize: vertical; white-space: pre; tab-size: 2; outline: none; }
  textarea:focus { border-color: var(--accent-amber); }
  .msg { font-size: 12px; color: var(--text-muted); flex: 1; }
  .msg.err { color: var(--accent-red); }
</style>
