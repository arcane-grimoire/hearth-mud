<script>
  import { schemaFor } from '../../../lib/mapwright-toml.js';

  // Schema-aware key/value attribute editor. A key that matches a schema
  // definition renders a typed widget (enum→dropdown, bool→select, int/float→
  // number); anything else is free text (type inferred on export). Ported from
  // mapwright.html `attrEditor`. Emits the rebuilt {key:value} object on change.
  let { attrs = {}, schema = [], onchange = () => {} } = $props();

  // Local editable rows. Seeded once from the incoming attrs; thereafter the
  // component owns them (so renaming a key doesn't fight the parent's object).
  let rows = $state(Object.entries(attrs || {}).map(([key, val]) => ({ key, val: val ?? '' })));

  function emit() {
    const o = {};
    for (const r of rows) { const k = (r.key || '').trim(); if (k) o[k] = r.val ?? ''; }
    onchange(o);
  }
  function addRow() { rows = [...rows, { key: '', val: '' }]; }
  function removeRow(i) { rows = rows.filter((_, j) => j !== i); emit(); }

  export function add() { addRow(); }
</script>

<div class="ae">
  {#each rows as row, i (i)}
    {@const def = schemaFor(schema, (row.key || '').trim())}
    <div class="mini">
      <button class="del" title="Remove attribute" aria-label="Remove attribute" onclick={() => removeRow(i)}>×</button>
      <div class="grid">
        <div>
          <div class="lbl">key{#if def}<span class="tybadge">{def.type}</span>{/if}</div>
          <input class="in" list="mw-attr-keys" spellcheck="false" placeholder="spawn_table"
            bind:value={row.key} oninput={emit} />
        </div>
        <div>
          <div class="lbl">value</div>
          {#if def?.type === 'bool'}
            <select class="in" bind:value={row.val} onchange={emit}>
              <option value="true">true</option>
              <option value="false">false</option>
            </select>
          {:else if def?.type === 'enum'}
            <select class="in" bind:value={row.val} onchange={emit}>
              {#if row.val && !(def.values || []).includes(row.val)}<option value={row.val}>{row.val} (?)</option>{/if}
              {#each def.values || [] as v}<option value={v}>{v}</option>{/each}
              {#if !(def.values || []).length}<option value="">— define values —</option>{/if}
            </select>
          {:else if def?.type === 'int' || def?.type === 'float'}
            <input class="in" type="number" step={def.type === 'int' ? '1' : 'any'} bind:value={row.val} oninput={emit} />
          {:else}
            <input class="in" type="text" spellcheck="false" placeholder="value" bind:value={row.val} oninput={emit} />
          {/if}
        </div>
      </div>
    </div>
  {/each}
  <datalist id="mw-attr-keys">
    {#each schema || [] as d}<option value={d.key}></option>{/each}
  </datalist>
</div>

<style>
  .ae { display: flex; flex-direction: column; gap: 8px; }
  .mini { position: relative; border: 1px solid var(--border-default); background: var(--bg-inset); border-radius: 8px; padding: 9px; }
  .del { position: absolute; top: 6px; right: 6px; border: none; background: none; color: var(--text-muted); font-size: 15px; width: 20px; height: 20px; border-radius: 5px; line-height: 1; cursor: pointer; }
  .del:hover { color: var(--accent-red); background: color-mix(in srgb, var(--accent-red) 14%, transparent); }
  .grid { display: grid; grid-template-columns: 1fr 1.25fr; gap: 6px; align-items: end; }
  .lbl { font-size: var(--fs-label); color: var(--text-muted); font-family: var(--font-mono); letter-spacing: 0.04em; margin-bottom: 3px; }
  .tybadge { font-family: var(--font-mono); font-size: var(--fs-badge); text-transform: uppercase; letter-spacing: 0.05em; color: var(--accent-amber); background: color-mix(in srgb, var(--accent-amber) 14%, transparent); border: 1px solid color-mix(in srgb, var(--accent-amber) 40%, transparent); border-radius: 4px; padding: 0 4px; margin-left: 4px; vertical-align: 1px; }
  .in { width: 100%; background: var(--bg-surface); border: 1px solid var(--border-default); border-radius: 6px; padding: 5px 7px; font-size: 12px; color: var(--text-primary); font-family: var(--font-mono); outline: none; }
  .in:focus { border-color: var(--accent-amber); }
</style>
