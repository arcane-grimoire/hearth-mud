<script>
  import PlusIcon from '@lucide/svelte/icons/plus';
  import Settings2Icon from '@lucide/svelte/icons/settings-2';
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
  import { Tooltip } from '@kenn-io/kit-ui';
  import { dims, readable } from '../../../lib/mapwright-toml.js';

  // Left rail: tool selector (Paint/Erase/Inspect), terrain palette swatches,
  // attribute-schema entry, and the canvas size steppers. Ported from the
  // mapwright.html left rail.
  let {
    m,
    ontool = () => {},
    onbrush = () => {},
    onedit = () => {},
    onadd = () => {},
    onschema = () => {},
    ondim = () => {},
    onreset = () => {},
  } = $props();

  const TOOLS = [
    { key: 'paint', glyph: '🖌', label: 'Paint' },
    { key: 'erase', glyph: '⌫', label: 'Erase' },
    { key: 'inspect', glyph: '⌖', label: 'Inspect' },
  ];
  const size = $derived(dims(m.grid));
  const entries = $derived(Object.entries(m.palette));
</script>

<div class="rail">
  <section>
    <p class="label">Tool</p>
    <div class="tools">
      {#each TOOLS as t}
        <button class="tool" class:on={m.tool === t.key} onclick={() => ontool(t.key)}>
          <span class="g">{t.glyph}</span>{t.label}
        </button>
      {/each}
    </div>
  </section>

  <section>
    <p class="label">Terrain palette <Tooltip text="Add terrain type" align="end"><button class="add" aria-label="Add terrain type" onclick={onadd}><PlusIcon size={13} /></button></Tooltip></p>
    <div class="swatches">
      {#each entries as [ch, t] (ch)}
        <button class="swatch" class:on={m.tool === 'paint' && m.brush === ch}
          title="Click to paint · double-click to edit"
          onclick={() => onbrush(ch)} ondblclick={() => onedit(ch)} oncontextmenu={(e) => { e.preventDefault(); onedit(ch); }}>
          <span class="chip" style="background:{t.color || '#ccc'};color:{readable(t.color)}">{ch}</span>
          <span class="meta">
            <span class="nm">{t.theme || ch}</span>
            <span class="sub">'{ch}' · {t.title_prefix || t.theme || ''}</span>
          </span>
          {#if t.passable === false}<span class="imp">wall</span>{/if}
        </button>
      {/each}
    </div>
  </section>

  <section>
    <p class="label">Attribute types</p>
    <button class="wide" onclick={onschema}><Settings2Icon size={13} /> Manage attribute types</button>
  </section>

  <section>
    <p class="label">Canvas</p>
    <div class="dims">
      <div class="stepper"><span>cols</span><Tooltip text="Remove a column"><button aria-label="Remove a column" onclick={() => ondim('w', -1)}>−</button></Tooltip><span class="val">{size.w}</span><Tooltip text="Add a column"><button aria-label="Add a column" onclick={() => ondim('w', 1)}>+</button></Tooltip></div>
      <div class="stepper"><span>rows</span><Tooltip text="Remove a row"><button aria-label="Remove a row" onclick={() => ondim('h', -1)}>−</button></Tooltip><span class="val">{size.h}</span><Tooltip text="Add a row"><button aria-label="Add a row" onclick={() => ondim('h', 1)}>+</button></Tooltip></div>
    </div>
    <button class="reset" onclick={onreset}><RotateCcwIcon size={12} /> Reset to Iron Hills sample</button>
  </section>
</div>

<style>
  .rail { display: flex; flex-direction: column; gap: 20px; padding: 16px 14px; overflow-y: auto; height: 100%; background: var(--bg-surface); border-right: 1px solid var(--border-default); }
  section { display: block; }
  .label { font-family: var(--font-mono); font-size: 10.5px; letter-spacing: 0.14em; text-transform: uppercase; color: var(--text-muted); margin: 0 0 10px; display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .add { border: 1px solid var(--border-default); background: var(--bg-inset); color: var(--text-muted); width: 22px; height: 22px; border-radius: 5px; display: grid; place-items: center; cursor: pointer; }
  .add:hover { color: var(--accent-amber); border-color: color-mix(in srgb, var(--accent-amber) 40%, transparent); }

  .tools { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 6px; }
  .tool { border: 1px solid var(--border-default); background: var(--bg-inset); color: var(--text-muted); border-radius: 7px; padding: 8px 4px 6px; display: flex; flex-direction: column; align-items: center; gap: 3px; font-size: 10.5px; font-weight: 500; cursor: pointer; }
  .tool .g { font-size: 16px; line-height: 1; }
  .tool:hover { color: var(--text-primary); }
  .tool.on { background: color-mix(in srgb, var(--accent-amber) 14%, transparent); border-color: color-mix(in srgb, var(--accent-amber) 45%, transparent); color: var(--accent-amber); }

  .swatches { display: flex; flex-direction: column; gap: 5px; }
  .swatch { display: flex; align-items: center; gap: 10px; width: 100%; border: 1px solid var(--border-default); background: var(--bg-inset); color: var(--text-primary); border-radius: 8px; padding: 6px 8px; text-align: left; cursor: pointer; }
  .swatch:hover { border-color: var(--text-muted); }
  .swatch.on { border-color: var(--accent-amber); box-shadow: 0 0 0 1px var(--accent-amber) inset; }
  .chip { width: 26px; height: 26px; border-radius: 6px; flex: none; border: 1px solid rgba(0,0,0,.18); display: grid; place-items: center; font-family: var(--font-mono); font-weight: 600; font-size: 13px; }
  .meta { min-width: 0; flex: 1; }
  .nm { display: block; font-weight: 600; font-size: 12.5px; line-height: 1.25; text-transform: capitalize; }
  .sub { display: block; font-family: var(--font-mono); font-size: 10px; color: var(--text-muted); letter-spacing: 0.02em; }
  .imp { font-size: 9px; font-family: var(--font-mono); text-transform: uppercase; letter-spacing: 0.06em; color: var(--accent-amber); border: 1px solid color-mix(in srgb, var(--accent-amber) 55%, transparent); border-radius: 4px; padding: 1px 4px; flex: none; }

  .wide { width: 100%; justify-content: center; display: inline-flex; align-items: center; gap: 6px; border: 1px solid var(--border-default); background: var(--bg-inset); color: var(--text-primary); border-radius: 7px; padding: 7px 12px; font-size: 12.5px; font-weight: 500; cursor: pointer; }
  .wide:hover { border-color: var(--text-muted); }

  .dims { display: flex; align-items: center; gap: 14px; flex-wrap: wrap; }
  .stepper { display: flex; align-items: center; gap: 6px; }
  .stepper span:first-child { font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted); }
  .stepper .val { font-family: var(--font-mono); font-weight: 600; min-width: 20px; text-align: center; color: var(--text-primary); }
  .stepper button { border: 1px solid var(--border-default); background: var(--bg-inset); color: var(--text-primary); width: 24px; height: 24px; border-radius: 6px; font-size: 15px; line-height: 1; display: grid; place-items: center; cursor: pointer; }
  .stepper button:hover { background: var(--bg-surface-hover); }
  .reset { margin-top: 12px; display: inline-flex; align-items: center; gap: 6px; justify-content: center; width: 100%; border: 1px solid var(--border-default); background: transparent; color: var(--text-muted); border-radius: 7px; padding: 7px; font-size: 11.5px; cursor: pointer; }
  .reset:hover { color: var(--accent-amber); border-color: color-mix(in srgb, var(--accent-amber) 40%, transparent); }
</style>
