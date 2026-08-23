<script>
  import { readable } from '../../../lib/mapwright-toml.js';
  import AttrEditor from './AttrEditor.svelte';

  // Per-tile room inspector. Edits m.cells[selected] in place; empty override
  // objects are pruned. Ported from mapwright.html renderInspector. `ver` is
  // bumped on structural changes (add/remove) to force a re-render, mirroring
  // the original commit(rerender) pattern; plain field edits just persist.
  let { m, onchange = () => {} } = $props();
  let ver = $state(0);

  const sel = $derived(m.selected);
  const coord = $derived(sel ? sel.split(',').map(Number) : null);
  const ch = $derived(coord ? ((m.grid[coord[1]] || [])[coord[0]] ?? null) : null);
  const terr = $derived(ch != null ? m.palette[ch] : null);

  function cur() {
    const c = m.cells[sel] || (m.cells[sel] = {});
    return c;
  }
  function clean() {
    const c = m.cells[sel];
    if (!c) return;
    if (!c.title && !c.description && !c.fixed_room && c.passable === undefined
      && !(c.encounters && c.encounters.length) && !(c.objects && c.objects.length)
      && !(c.attrs && Object.keys(c.attrs).length)) delete m.cells[sel];
  }
  function commit() { clean(); onchange(); }
  function structural() { clean(); onchange(); ver++; }

  const cell = () => m.cells[sel] || {};

  function setField(k, val) { cur()[k] = val || undefined; commit(); }
  function setPassable(checked) { if (checked) cur().passable = false; else if (m.cells[sel]) delete m.cells[sel].passable; commit(); }

  function addEnc() { const c = cur(); c.encounters = c.encounters || []; c.encounters.push({ monster: 'goblin', count: [1, 2] }); structural(); }
  function rmEnc(i) { const c = cur(); c.encounters.splice(i, 1); if (!c.encounters.length) c.encounters = undefined; structural(); }
  function encField(en, k, val) {
    if (k === 'monster') en.monster = val;
    else { en.count = en.count || [1, 1]; en.count[k === 'min' ? 0 : 1] = Math.max(1, +val || 1); }
    commit();
  }
  function addObj() { const c = cur(); c.objects = c.objects || []; c.objects.push({ key: '', kind: 'npc', title: '', description: '' }); structural(); }
  function rmObj(i) { const c = cur(); c.objects.splice(i, 1); if (!c.objects.length) c.objects = undefined; structural(); }
  function objField(ob, k, val) { ob[k] = val; commit(); }

  function onAttrs(o) {
    if (Object.keys(o).length) cur().attrs = o;
    else if (m.cells[sel]) delete m.cells[sel].attrs;
    commit();
  }
  function clearCell() { delete m.cells[sel]; onchange(); ver++; }
</script>

<div class="insp">
  <p class="label">Room inspector</p>

  {#if !sel}
    <p class="empty">No room selected.<br><br>Switch to <b>Inspect</b> and click a tile to give it a hand-written <b>title</b>, <b>description</b>, a linked <b>fixed room</b>, monster <b>encounters</b>, or placed <b>objects</b>. Tiles without overrides use their terrain theme's generated text.</p>
  {:else}
    {#key sel + ':' + ver}
      <div class="head">
        <span class="coord">{coord[0]},{coord[1]}</span>
        <span class="terr">
          {#if ch != null}<i style="background:{terr ? terr.color : '#ccc'}"></i>{terr ? terr.theme : ch}{:else}empty tile{/if}
        </span>
      </div>

      {#if ch == null}
        <p class="empty" style="margin-top:8px">Paint a terrain onto this tile first.</p>
      {:else}
        {@const c = cell()}
        <div class="field">
          <label for="mw-title">Title</label>
          <input id="mw-title" type="text" value={c.title || ''} placeholder={(terr?.title_prefix || '') + ' (' + coord[0] + ',' + coord[1] + ')'} oninput={(e) => setField('title', e.target.value)} />
        </div>
        <div class="field">
          <label for="mw-desc">Description</label>
          <textarea id="mw-desc" placeholder={"Leave blank to use the " + (terr?.theme || '') + " theme's generated text."} oninput={(e) => setField('description', e.target.value)}>{c.description || ''}</textarea>
        </div>
        <div class="field mono">
          <label for="mw-fixed">Fixed room link <span class="dim">— area/key</span></label>
          <input id="mw-fixed" type="text" value={c.fixed_room || ''} placeholder="town/crossroads" oninput={(e) => setField('fixed_room', e.target.value)} />
        </div>
        <label class="check"><input type="checkbox" checked={c.passable === false} onchange={(e) => setPassable(e.target.checked)} /> Force impassable (override terrain)</label>

        <div class="sub">
          <h4>Encounters <span class="dim">{(c.encounters || []).length || ''}</span></h4>
          {#each c.encounters || [] as en, i}
            <div class="mini">
              <button class="del" onclick={() => rmEnc(i)}>×</button>
              <div class="enc">
                <div><div class="lbl">monster</div><input value={en.monster || ''} oninput={(e) => encField(en, 'monster', e.target.value)} /></div>
                <div><div class="lbl">min</div><input type="number" min="1" value={en.count ? en.count[0] : 1} oninput={(e) => encField(en, 'min', e.target.value)} /></div>
                <div><div class="lbl">max</div><input type="number" min="1" value={en.count ? en.count[1] : 1} oninput={(e) => encField(en, 'max', e.target.value)} /></div>
              </div>
            </div>
          {/each}
          <button class="addbtn" onclick={addEnc}>+ encounter</button>
        </div>

        <div class="sub">
          <h4>Objects <span class="dim">{(c.objects || []).length || ''}</span></h4>
          {#each c.objects || [] as ob, i}
            <div class="mini">
              <button class="del" onclick={() => rmObj(i)}>×</button>
              <div class="obj2">
                <div><div class="lbl">key</div><input value={ob.key || ''} oninput={(e) => objField(ob, 'key', e.target.value)} /></div>
                <div><div class="lbl">kind</div>
                  <select value={ob.kind || 'npc'} onchange={(e) => objField(ob, 'kind', e.target.value)}>
                    <option value="npc">npc</option><option value="item">item</option>
                  </select>
                </div>
              </div>
              <div><div class="lbl">title</div><input value={ob.title || ''} oninput={(e) => objField(ob, 'title', e.target.value)} /></div>
              <div><div class="lbl">description</div><input value={ob.description || ''} oninput={(e) => objField(ob, 'description', e.target.value)} /></div>
            </div>
          {/each}
          <button class="addbtn" onclick={addObj}>+ object</button>
        </div>

        <div class="sub">
          <h4>Attributes <span class="dim">{(c.attrs ? Object.keys(c.attrs).length : 0) || ''}</span></h4>
          <AttrEditor attrs={c.attrs || {}} schema={m.schema} onchange={onAttrs} />
        </div>

        <button class="clear" onclick={clearCell}>Clear all overrides on this tile</button>
      {/if}
    {/key}
  {/if}
</div>

<style>
  .insp { display: flex; flex-direction: column; padding: 16px 14px; overflow-y: auto; height: 100%; background: var(--bg-surface); border-left: 1px solid var(--border-default); }
  .label { font-family: var(--font-mono); font-size: 10.5px; letter-spacing: 0.14em; text-transform: uppercase; color: var(--text-muted); margin: 0 0 12px; }
  .empty { color: var(--text-muted); font-size: 13px; line-height: 1.6; }
  .empty b { color: var(--text-primary); }
  .head { display: flex; align-items: center; gap: 10px; margin-bottom: 8px; }
  .coord { font-family: var(--font-mono); font-weight: 600; font-size: 15px; color: var(--text-primary); }
  .terr { font-family: var(--font-mono); font-size: 11px; color: var(--text-muted); display: inline-flex; align-items: center; gap: 6px; }
  .terr i { width: 12px; height: 12px; border-radius: 3px; display: inline-block; border: 1px solid rgba(0,0,0,.2); }
  .field { display: flex; flex-direction: column; gap: 5px; margin-bottom: 13px; }
  .field label { font-size: 11px; text-transform: uppercase; letter-spacing: 0.07em; color: var(--text-muted); font-weight: 600; }
  .dim { text-transform: none; letter-spacing: 0; color: var(--text-muted); font-weight: 400; }
  .field input[type=text], .field textarea { background: var(--bg-inset); border: 1px solid var(--border-default); border-radius: 7px; padding: 8px 10px; font-size: 13px; width: 100%; color: var(--text-primary); outline: none; }
  .field.mono input { font-family: var(--font-mono); font-size: 12.5px; }
  .field textarea { resize: vertical; min-height: 66px; line-height: 1.5; }
  .field input:focus, .field textarea:focus { border-color: var(--accent-amber); }
  .check { display: flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--text-primary); }
  .check input { width: 15px; height: 15px; accent-color: var(--accent-amber); }
  .sub { border-top: 1px solid var(--border-default); padding-top: 13px; margin-top: 13px; }
  .sub h4 { margin: 0 0 9px; font-family: var(--font-mono); font-size: 10.5px; letter-spacing: 0.12em; text-transform: uppercase; color: var(--text-muted); display: flex; justify-content: space-between; }
  .mini { position: relative; border: 1px solid var(--border-default); background: var(--bg-inset); border-radius: 8px; padding: 9px; margin-bottom: 8px; display: flex; flex-direction: column; gap: 7px; }
  .del { position: absolute; top: 6px; right: 6px; border: none; background: none; color: var(--text-muted); font-size: 15px; width: 20px; height: 20px; border-radius: 5px; line-height: 1; cursor: pointer; }
  .del:hover { color: var(--accent-red); background: color-mix(in srgb, var(--accent-red) 14%, transparent); }
  .enc { display: grid; grid-template-columns: 1fr 46px 46px; gap: 6px; align-items: end; }
  .obj2 { display: grid; grid-template-columns: 1fr 74px; gap: 6px; align-items: end; }
  .lbl { font-size: 10px; color: var(--text-muted); font-family: var(--font-mono); letter-spacing: 0.04em; margin-bottom: 3px; }
  .mini input, .mini select { background: var(--bg-surface); border: 1px solid var(--border-default); border-radius: 6px; padding: 5px 7px; font-size: 12px; width: 100%; color: var(--text-primary); outline: none; }
  .mini input:focus, .mini select:focus { border-color: var(--accent-amber); }
  .addbtn { border: 1px dashed var(--border-default); background: transparent; color: var(--text-muted); border-radius: 7px; padding: 6px; font-size: 11.5px; font-weight: 500; width: 100%; cursor: pointer; }
  .addbtn:hover { color: var(--accent-amber); border-color: color-mix(in srgb, var(--accent-amber) 40%, transparent); }
  .clear { margin-top: 6px; border: 1px solid var(--border-default); background: transparent; color: var(--text-muted); border-radius: 7px; padding: 7px; font-size: 11.5px; width: 100%; cursor: pointer; }
  .clear:hover { color: var(--accent-red); border-color: color-mix(in srgb, var(--accent-red) 40%, transparent); }
</style>
