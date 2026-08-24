<script>
  import { dims, readable } from '../../../lib/mapwright-toml.js';

  // The reactive map grid. Renders rows×cols tiles from m.grid + m.palette
  // (tile char, terrain color, readable ink, override dot, fixed-room anchor,
  // selection highlight, impassable hatch). Paint/erase on click & drag;
  // click selects in Inspect mode. Ported from mapwright.html renderGrid.
  let { m, onapply = () => {}, onselect = () => {} } = $props();

  const CELL = 42;
  const size = $derived(dims(m.grid));
  const cols = $derived(Array.from({ length: size.w }, (_, i) => i));
  const rowsIdx = $derived(Array.from({ length: size.h }, (_, i) => i));

  let painting = false;
  let gridEl;

  function cellAt(x, y) {
    const ch = (m.grid[y] || [])[x] ?? null;
    const t = ch != null ? m.palette[ch] : null;
    return { ch, t };
  }

  function onDown(e, x, y) {
    onselect(x, y);
    if (m.tool !== 'inspect') {
      painting = true;
      try { gridEl.setPointerCapture(e.pointerId); } catch (err) { /* ignore */ }
      onapply(x, y);
    }
  }
  function onOver(x, y) {
    if (painting) onapply(x, y);
  }
  function stop() { painting = false; }
</script>

<svelte:window onpointerup={stop} />

<div class="frame">
  <div class="gridwrap" style="--cell:{CELL}px">
    <div class="corner"></div>
    <div class="axis cols" style="grid-template-columns:repeat({size.w},var(--cell))">
      {#each cols as x}<div>{x}</div>{/each}
    </div>
    <div class="axis rows" style="grid-template-rows:repeat({size.h},var(--cell))">
      {#each rowsIdx as y}<div>{y}</div>{/each}
    </div>
    <div class="grid" bind:this={gridEl}
      style="grid-template-columns:repeat({size.w},var(--cell));grid-template-rows:repeat({size.h},var(--cell))">
      {#each rowsIdx as y}
        {#each cols as x}
          {@const c = cellAt(x, y)}
          {@const key = x + ',' + y}
          {@const ov = m.cells[key]}
          <div
            class="cell"
            class:empty={c.ch == null}
            class:impass={c.t && c.t.passable === false}
            class:sel={m.selected === key}
            style={c.ch != null ? `background:${c.t ? c.t.color : '#bbb'};color:${c.t ? readable(c.t.color) : 'rgba(0,0,0,.5)'}` : ''}
            role="button" tabindex="-1"
            onpointerdown={(e) => onDown(e, x, y)}
            onpointerover={() => onOver(x, y)}
          >
            {#if c.ch != null}{c.ch}{/if}
            {#if ov}<span class="ov"></span>{#if ov.fixed_room}<span class="fx">⚓</span>{/if}{/if}
          </div>
        {/each}
      {/each}
    </div>
  </div>
</div>

<style>
  .frame { background: var(--bg-inset); border: 1px solid var(--border-default); border-radius: 12px; box-shadow: 0 1px 2px rgba(0,0,0,.3), 0 10px 30px rgba(0,0,0,.4); padding: 14px; display: inline-grid; gap: 12px; }
  .gridwrap { display: inline-grid; grid-template-columns: auto auto; grid-template-rows: auto auto; gap: 4px; }
  /* .axis and .fx are on-canvas map annotations (coordinate ruler and per-tile
     effect glyph), sized to the grid rather than the UI type scale — kept off
     --fs-* on purpose so they track the cells, not the chrome. */
  .axis { display: grid; font-family: var(--font-mono); font-size: 10px; color: var(--text-muted); }
  .axis.cols { grid-auto-flow: column; }
  .axis.rows { grid-auto-flow: row; }
  .axis div { display: grid; place-items: center; }
  .axis.cols div { height: 14px; }
  .axis.rows div { width: 14px; }
  .grid { display: grid; background: var(--border-default); border: 1px solid var(--border-default); gap: 1px; touch-action: none; user-select: none; }
  .cell { position: relative; display: grid; place-items: center; font-family: var(--font-mono); font-weight: 600; font-size: 14px; color: rgba(0,0,0,.5); background: var(--bg-surface); cursor: pointer; }
  .cell.empty { background: repeating-linear-gradient(45deg, transparent, transparent 5px, rgba(200,210,196,.12) 5px, rgba(200,210,196,.12) 6px); color: transparent; }
  .cell.impass::after { content: ''; position: absolute; inset: 0; background: repeating-linear-gradient(45deg, rgba(0,0,0,.18) 0 2px, transparent 2px 6px); pointer-events: none; }
  .cell.sel { box-shadow: 0 0 0 3px var(--accent-amber); z-index: 2; }
  .ov { position: absolute; top: 3px; right: 3px; width: 7px; height: 7px; border-radius: 50%; background: var(--accent-amber); box-shadow: 0 0 0 1.5px var(--bg-inset); }
  .fx { position: absolute; bottom: 2px; left: 3px; font-size: 8px; color: rgba(0,0,0,.55); line-height: 1; }
</style>
