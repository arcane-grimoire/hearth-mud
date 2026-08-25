<script>
  import { Tooltip } from '@kenn-io/kit-ui';
  import { api } from '../../lib/api.js';

  // One declared attribute rendered as a typed widget, driven by its schema
  // descriptor. Matches PropertiesPanel's form tokens (.fi/.mi, --accent-*).
  //  - descriptor: { key, type, label, help, default, required, min, max, step,
  //                  values, ref_source, item_type, source }
  //  - value:   the effective value (own, else inherited, else default)
  //  - owned:   true when this object holds its own value for the key
  //  - origin:  short note for a non-owned field ("inherited from X" / "default")
  //  - onsave(value): write an own value (typed); onrevert(): drop the own value.
  let {
    descriptor,
    value = null,
    owned = false,
    origin = '',
    locked = false,
    onsave = () => {},
    onrevert = null,
  } = $props();

  const type = $derived(descriptor?.type || 'string');
  const label = $derived(descriptor?.label || descriptor?.key || '');
  const isList = $derived(type === 'list');

  // Ref candidates load lazily for ref fields (and list-of-ref).
  let refOptions = $state([]);
  $effect(() => {
    const src = type === 'ref' ? descriptor?.ref_source : (isList && descriptor?.item_type === 'ref' ? descriptor?.ref_source : null);
    if (!src) { refOptions = []; return; }
    let alive = true;
    api('list_ref_candidates', { ref_source: src }).then((res) => {
      if (alive) refOptions = res.ok && res.data?.candidates ? res.data.candidates : [];
    });
    return () => { alive = false; };
  });

  // Coerce a widget's raw value to the type the descriptor declares, so what we
  // persist is a real number/bool/etc. rather than a string.
  function coerce(t, raw) {
    if (t === 'int') { const n = parseInt(raw, 10); return Number.isNaN(n) ? null : n; }
    if (t === 'float') { const n = parseFloat(raw); return Number.isNaN(n) ? null : n; }
    if (t === 'bool') return !!raw;
    return raw;
  }

  function commit(raw) {
    onsave(coerce(type, raw));
  }

  // --- list handling: the whole array is rewritten on any row change ---
  const listVal = $derived(Array.isArray(value) ? value : []);
  function commitList(next) { onsave(next); }
  function setItem(i, raw) {
    const next = [...listVal];
    next[i] = coerce(descriptor?.item_type || 'string', raw);
    commitList(next);
  }
  function addItem() {
    const it = descriptor?.item_type || 'string';
    const blank = it === 'int' || it === 'float' ? 0 : it === 'bool' ? false : '';
    commitList([...listVal, blank]);
  }
  function removeItem(i) { commitList(listVal.filter((_, j) => j !== i)); }
</script>

<div class="af" class:inh={!owned}>
  <div class="af-head">
    <span class="af-label">{label}{#if descriptor?.required}<span class="req" title="Required">*</span>{/if}</span>
    <span class="af-type">{type}{#if isList && descriptor?.item_type}&lt;{descriptor.item_type}&gt;{/if}</span>
    {#if owned && onrevert}
      <Tooltip text="Revert to the inherited / default value">
        <button class="revert" onclick={onrevert} disabled={locked} aria-label="Revert">↺</button>
      </Tooltip>
    {/if}
  </div>

  {#if type === 'bool'}
    <label class="af-check">
      <input type="checkbox" checked={!!value} onchange={(e) => commit(e.target.checked)} disabled={locked} />
      <span>{value ? 'true' : 'false'}</span>
    </label>
  {:else if type === 'int' || type === 'float'}
    <input class="fi" type="number" value={value ?? ''}
      min={descriptor?.min ?? undefined} max={descriptor?.max ?? undefined}
      step={descriptor?.step ?? (type === 'int' ? 1 : 'any')}
      onchange={(e) => commit(e.target.value)} disabled={locked} />
  {:else if type === 'enum'}
    <select class="fi" value={value ?? ''} onchange={(e) => commit(e.target.value)} disabled={locked}>
      <option value="" disabled>— choose —</option>
      {#each descriptor?.values || [] as v}<option value={v}>{v}</option>{/each}
    </select>
  {:else if type === 'ref'}
    <select class="fi" value={value ?? ''} onchange={(e) => commit(e.target.value)} disabled={locked}>
      <option value="">— none —</option>
      {#each refOptions as c}<option value={c.ref_id}>{c.label} ({c.ref_id})</option>{/each}
    </select>
  {:else if type === 'color'}
    <div class="af-color">
      <input type="color" value={/^#[0-9a-fA-F]{6}$/.test(value || '') ? value : '#000000'}
        onchange={(e) => commit(e.target.value)} disabled={locked} />
      <input class="fi" type="text" value={value ?? ''} placeholder="#rrggbb"
        onchange={(e) => commit(e.target.value)} disabled={locked} />
    </div>
  {:else if type === 'text'}
    <textarea class="ft" rows="3" value={value ?? ''} onchange={(e) => commit(e.target.value)} disabled={locked}></textarea>
  {:else if isList}
    <div class="af-list">
      {#each listVal as item, i}
        <div class="af-row">
          {#if descriptor?.item_type === 'bool'}
            <input type="checkbox" checked={!!item} onchange={(e) => setItem(i, e.target.checked)} disabled={locked} />
          {:else if descriptor?.item_type === 'int' || descriptor?.item_type === 'float'}
            <input class="fi" type="number" value={item ?? ''} step={descriptor?.item_type === 'int' ? 1 : 'any'} onchange={(e) => setItem(i, e.target.value)} disabled={locked} />
          {:else if descriptor?.item_type === 'ref'}
            <select class="fi" value={item ?? ''} onchange={(e) => setItem(i, e.target.value)} disabled={locked}>
              <option value="">— none —</option>
              {#each refOptions as c}<option value={c.ref_id}>{c.label} ({c.ref_id})</option>{/each}
            </select>
          {:else}
            <input class="fi" type="text" value={item ?? ''} onchange={(e) => setItem(i, e.target.value)} disabled={locked} />
          {/if}
          <button class="li-del" onclick={() => removeItem(i)} disabled={locked} aria-label="Remove item">×</button>
        </div>
      {/each}
      {#if !listVal.length}<span class="none">empty list</span>{/if}
      <button class="li-add" onclick={addItem} disabled={locked}>+ add</button>
    </div>
  {:else}
    <input class="fi" type="text" value={value ?? ''} pattern={descriptor?.pattern ?? undefined}
      onchange={(e) => commit(e.target.value)} disabled={locked} />
  {/if}

  {#if descriptor?.help}<div class="af-help">{descriptor.help}</div>{/if}
  {#if !owned && origin}<div class="af-origin">{origin}</div>{/if}
</div>

<style>
  .af { display: flex; flex-direction: column; gap: 4px; }
  .af-head { display: flex; align-items: baseline; gap: 6px; }
  .af-label { font-size: var(--fs-label); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #8c8378); font-weight: 700; }
  .req { color: var(--accent-red, #e06c75); margin-left: 1px; }
  .af-type { font-family: var(--font-mono, ui-monospace, monospace); font-size: 10px; color: var(--accent-teal, #56b6c2); opacity: 0.8; }
  .revert { margin-left: auto; background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; font-size: 13px; line-height: 1; padding: 0 2px; }
  .revert:hover { color: var(--accent-amber, #c9956b); }
  .fi, .ft {
    background: var(--bg-inset, #12100c); color: var(--text-primary, #ece0c8);
    border: 1px solid var(--border-default, #332c22); border-radius: 6px; padding: 6px 8px;
    font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; outline: none; width: 100%;
  }
  .ft { resize: vertical; line-height: 1.5; }
  .fi:focus, .ft:focus { border-color: var(--accent-amber, #c9956b); }
  .fi:disabled, .ft:disabled { opacity: 0.55; cursor: not-allowed; }
  select.fi { appearance: none; cursor: pointer; }
  .af-check { display: inline-flex; align-items: center; gap: 6px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-primary, #ece0c8); cursor: pointer; }
  .af-check input { accent-color: var(--accent-amber, #c9956b); }
  .af-color { display: flex; gap: 6px; align-items: center; }
  .af-color input[type="color"] { width: 34px; height: 30px; padding: 0; border: 1px solid var(--border-default, #332c22); border-radius: 6px; background: var(--bg-inset, #12100c); cursor: pointer; }
  /* Inherited/default fields read muted, matching the panel's inherited rows. */
  .af.inh .fi, .af.inh .ft { color: color-mix(in srgb, var(--text-primary, #ece0c8) 72%, transparent); border-style: dashed; }
  .af-help { font-size: var(--fs-meta); line-height: 1.4; color: var(--text-muted, #8c8378); }
  .af-origin { font-size: var(--fs-meta); font-style: italic; color: color-mix(in srgb, var(--accent-amber, #c9956b) 70%, var(--text-muted, #8c8378)); }
  .af-list { display: flex; flex-direction: column; gap: 4px; }
  .af-row { display: flex; gap: 4px; align-items: center; }
  .li-del { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; font-size: 14px; line-height: 1; padding: 0 4px; }
  .li-del:hover { color: var(--accent-red, #e06c75); }
  .li-add { align-self: flex-start; background: none; border: 1px dashed var(--border-default, #332c22); color: var(--text-muted, #8c8378); border-radius: 6px; padding: 3px 8px; font-size: var(--fs-meta); cursor: pointer; }
  .li-add:hover:not(:disabled) { color: var(--accent-amber, #c9956b); border-color: var(--accent-amber, #c9956b); }
  .li-add:disabled, .li-del:disabled, .revert:disabled { opacity: 0.4; cursor: not-allowed; }
  .none { color: var(--text-muted, #8c8378); font-style: italic; font-size: 11px; }
</style>
