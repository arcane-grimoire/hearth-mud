<script>
  import { Modal, Button } from '@kenn-io/kit-ui';
  import { ATTR_TYPES } from '../../../lib/mapwright-toml.js';

  // Attribute-type schema editor. Ported from mapwright.html schema editor.
  // Edits a local copy of the rows and emits the cleaned schema on every change.
  let { schema = [], onchange = () => {}, onclose = () => {} } = $props();

  let rows = $state((schema || []).map((d) => ({ key: d.key, type: d.type, values: (d.values || []).join(', ') })));

  function emit() {
    onchange(rows.map((r) => ({
      key: (r.key || '').trim(),
      type: r.type,
      values: r.type === 'enum' ? (r.values || '').split(',').map((s) => s.trim()).filter(Boolean) : undefined,
    })).filter((d) => d.key));
  }
  function add() { rows = [...rows, { key: '', type: 'text', values: '' }]; }
  function remove(i) { rows = rows.filter((_, j) => j !== i); emit(); }
</script>

<Modal title="Attribute types" maxWidth="min(560px, calc(100vw - 32px))" {onclose}>
  <div class="body">
    <p class="intro">Declare the attributes your spawn / populate hooks read. A defined attribute gets a typed input wherever you use it — a dropdown for <b>enums</b>, a checkbox for <b>booleans</b>, a number field for <b>ints</b>. Anything undeclared stays free text. Exports as <code>[[terrain_attr]]</code> blocks in <code>terrain.toml</code>.</p>
    {#each rows as row, i (i)}
      <div class="mini">
        <button class="del" onclick={() => remove(i)}>×</button>
        <div class="grid">
          <div><div class="lbl">attribute key</div><input spellcheck="false" placeholder="danger" bind:value={row.key} oninput={emit} /></div>
          <div><div class="lbl">type</div>
            <select bind:value={row.type} onchange={emit}>
              {#each ATTR_TYPES as t}<option value={t}>{t}</option>{/each}
            </select>
          </div>
        </div>
        {#if row.type === 'enum'}
          <div><div class="lbl">enum values (comma-separated)</div><input spellcheck="false" placeholder="arid, temperate, alpine" bind:value={row.values} oninput={emit} /></div>
        {/if}
      </div>
    {/each}
    <button class="addbtn" onclick={add}>+ define attribute</button>
  </div>
  {#snippet footer()}
    <span class="spacer"></span>
    <Button size="sm" tone="accent" label="Done" onclick={onclose} />
  {/snippet}
</Modal>

<style>
  .body { display: flex; flex-direction: column; gap: 8px; }
  .intro { margin: 0 0 8px; color: var(--text-muted); font-size: 12.5px; line-height: 1.6; }
  .intro b { color: var(--text-secondary); }
  .intro code { font-family: var(--font-mono); font-size: 11.5px; color: var(--accent-amber); }
  .mini { position: relative; border: 1px solid var(--border-default); background: var(--bg-inset); border-radius: 8px; padding: 9px; display: flex; flex-direction: column; gap: 7px; }
  .del { position: absolute; top: 6px; right: 6px; border: none; background: none; color: var(--text-muted); font-size: 15px; width: 20px; height: 20px; border-radius: 5px; line-height: 1; cursor: pointer; }
  .del:hover { color: var(--accent-red); background: color-mix(in srgb, var(--accent-red) 14%, transparent); }
  .grid { display: grid; grid-template-columns: 1fr 96px; gap: 6px; align-items: end; }
  .lbl { font-size: 10px; color: var(--text-muted); font-family: var(--font-mono); letter-spacing: 0.04em; margin-bottom: 3px; }
  input, select { background: var(--bg-surface); border: 1px solid var(--border-default); border-radius: 6px; padding: 5px 7px; font-size: 12px; width: 100%; color: var(--text-primary); outline: none; }
  input:focus, select:focus { border-color: var(--accent-amber); }
  .addbtn { border: 1px dashed var(--border-default); background: transparent; color: var(--text-muted); border-radius: 7px; padding: 6px; font-size: 11.5px; font-weight: 500; width: 100%; cursor: pointer; }
  .addbtn:hover { color: var(--accent-amber); border-color: color-mix(in srgb, var(--accent-amber) 40%, transparent); }
  .spacer { flex: 1; }
</style>
