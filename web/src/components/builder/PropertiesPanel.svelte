<script>
  import { Button, showFlash, Tooltip } from '@kenn-io/kit-ui';
  import { api } from '../../lib/api.js';
  import StructurePanel from './StructurePanel.svelte';

  // Object properties: identity, tags, aliases, attributes, plus the
  // kind-specific bits (an exit's direction/target, a movable object's
  // location) and delete. Lifted from Admin.svelte + the old RoomEditorModal so
  // the unified workspace has ONE properties surface instead of the several that
  // used to each re-implement this.
  let { obj = null, rooms = [], onchanged = () => {}, ondeleted = () => {}, onedit = () => {} } = $props();

  let editingAttr = $state(null);
  let editValue = $state('');
  let newAttrKey = $state('');
  let newAttrValue = $state('');
  let newTag = $state('');
  let newAlias = $state('');

  // Kind-specific editable state, reseeded whenever the panel points at a
  // different object (examine returns fresh data after each onchanged()).
  let exitDir = $state('');
  let exitTarget = $state('');
  let moveDest = $state('');
  let confirmDelete = $state(false);
  let syncedFor = $state(null);
  $effect(() => {
    if (obj && obj.ref_id !== syncedFor) {
      syncedFor = obj.ref_id;
      exitDir = obj.kind === 'exit' ? (obj.key || '') : '';
      exitTarget = obj.kind === 'exit' ? (obj.target_ref || '') : '';
      moveDest = '';
      confirmDelete = false;
    }
  });

  const isExit = $derived(obj?.kind === 'exit');
  const isRoom = $derived(obj?.kind === 'room');

  let sortedAttrs = $derived(obj ? Object.keys(obj.attrs || {}).sort() : []);
  // Engine-managed attrs (_file_key, _rx/_ry, …) are noise for most editing and
  // alarming to a newcomer, so they fold away under an Advanced disclosure while
  // the object's own attributes stay in the open.
  const userAttrs = $derived(sortedAttrs.filter((k) => !k.startsWith('_')));
  const sysAttrs = $derived(sortedAttrs.filter((k) => k.startsWith('_')));

  async function setTitle(e) {
    const res = await api('set_title', { ref_id: obj.ref_id, title: e.target.value });
    res.ok ? onchanged() : showFlash(res.error || 'Could not save the title', { tone: 'danger' });
  }
  async function setDescription(e) {
    const res = await api('set_description', { ref_id: obj.ref_id, description: e.target.value });
    res.ok ? onchanged() : showFlash(res.error || 'Could not save the description', { tone: 'danger' });
  }
  async function addTag() {
    const tag = newTag.trim();
    if (!tag) return;
    const res = await api('add_tag', { ref_id: obj.ref_id, tag });
    if (res.ok) { newTag = ''; onchanged(); } else showFlash(res.error || 'Could not add the tag', { tone: 'danger' });
  }
  async function removeTag(tag) {
    const res = await api('remove_tag', { ref_id: obj.ref_id, tag });
    res.ok ? onchanged() : showFlash(res.error || 'Could not remove the tag', { tone: 'danger' });
  }
  function startEdit(key) {
    editingAttr = key;
    const val = obj.attrs[key];
    editValue = typeof val === 'object' ? JSON.stringify(val, null, 2) : String(val);
  }
  async function saveAttr(key) {
    let value;
    try { value = JSON.parse(editValue); } catch { value = editValue; }
    const res = await api('set_attribute', { ref_id: obj.ref_id, key, value });
    if (res.ok) { editingAttr = null; onchanged(); } else showFlash(res.error || 'Could not save the attribute', { tone: 'danger' });
  }
  async function deleteAttr(key) {
    const res = await api('set_attribute', { ref_id: obj.ref_id, key, value: null });
    res.ok ? onchanged() : showFlash(res.error || 'Could not delete the attribute', { tone: 'danger' });
  }
  async function addAttr() {
    const key = newAttrKey.trim();
    if (!key) return;
    let value;
    try { value = JSON.parse(newAttrValue); } catch { value = newAttrValue; }
    const res = await api('set_attribute', { ref_id: obj.ref_id, key, value });
    if (res.ok) { newAttrKey = ''; newAttrValue = ''; onchanged(); } else showFlash(res.error || 'Could not add the attribute', { tone: 'danger' });
  }
  function attrKeydown(e, key) {
    if (e.key === 'Escape') editingAttr = null;
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); saveAttr(key); }
  }

  // Aliases — the whole set is replaced on every change (set_aliases).
  async function addAlias() {
    const a = newAlias.trim();
    const cur = obj.aliases || [];
    if (!a || cur.includes(a)) { newAlias = ''; return; }
    const res = await api('set_aliases', { ref_id: obj.ref_id, aliases: [...cur, a] });
    if (res.ok) { newAlias = ''; onchanged(); } else showFlash(res.error || 'Could not add the alias', { tone: 'danger' });
  }
  async function removeAlias(a) {
    const next = (obj.aliases || []).filter((x) => x !== a);
    const res = await api('set_aliases', { ref_id: obj.ref_id, aliases: next });
    res.ok ? onchanged() : showFlash(res.error || 'Could not remove the alias', { tone: 'danger' });
  }

  // Exit — retarget / rename direction in place.
  async function saveExit() {
    const res = await api('update_exit', {
      ref_id: obj.ref_id,
      direction: exitDir.trim() || null,
      target: exitTarget.trim() || null,
    });
    res.ok ? onchanged() : showFlash(res.error || 'Could not update the exit', { tone: 'danger' });
  }

  // Location — move a non-room object into another ref.
  async function doMove() {
    const dest = moveDest.trim();
    if (!dest) return;
    const res = await api('set_location', { ref_id: obj.ref_id, location: dest });
    if (res.ok) { moveDest = ''; onchanged(); } else showFlash(res.error || 'Could not move the object', { tone: 'danger' });
  }

  async function doDelete() {
    const res = await api('delete_object', { ref_id: obj.ref_id });
    if (res.ok) ondeleted(obj.ref_id);
    else showFlash(res.error || `Could not delete this ${obj.kind}`, { tone: 'danger' });
  }
</script>

{#if obj}
  <div class="pp">
    <section>
      <h3>Identity</h3>
      <label class="fl">Title
        <input type="text" class="fi" value={obj.title || ''} onchange={setTitle} />
      </label>
      <label class="fl">Description
        <textarea class="ft" rows="4" onchange={setDescription}>{obj.description || ''}</textarea>
      </label>
    </section>

    {#if isRoom}
      <!-- A room's connectivity + contents are its most important content, so
           they sit right under identity rather than behind attributes. -->
      <StructurePanel {obj} {rooms} {onchanged} {onedit} />
    {/if}

    {#if isExit}
      <section>
        <h3>Exit</h3>
        <label class="fl">Direction
          <input type="text" class="fi" bind:value={exitDir} placeholder="north" />
        </label>
        <label class="fl">Target room
          <input type="text" class="fi" bind:value={exitTarget} placeholder="#12" />
        </label>
        <div class="row">
          <span class="none">from {obj.location_ref || '(unplaced)'}</span>
          <span style="flex:1"></span>
          <Button size="sm" onclick={saveExit} label="Save exit" />
        </div>
      </section>
    {:else if !isRoom}
      <section>
        <h3>Location</h3>
        <div class="none">in {obj.location_ref || '(nowhere)'}</div>
        <div class="row">
          <input class="mi" placeholder="move to a room ref, e.g. #12" bind:value={moveDest} onkeydown={(e) => e.key === 'Enter' && doMove()} />
          <Button size="sm" onclick={doMove} label="Move" />
        </div>
      </section>
    {/if}

    <section>
      <h3>Tags</h3>
      <p class="hint">A <code>category:key</code> label — used by locks, queries, and behavior (e.g. <code>system:hidden</code>).</p>
      <div class="tags">
        {#each obj.tags || [] as tag}
          <span class="tag">{tag}<Tooltip text="Remove tag"><button aria-label="Remove tag" onclick={() => removeTag(tag)}>×</button></Tooltip></span>
        {/each}
        {#if !(obj.tags || []).length}<span class="none">no tags</span>{/if}
      </div>
      <div class="row">
        <input class="mi" placeholder="category:key" bind:value={newTag} onkeydown={(e) => e.key === 'Enter' && addTag()} />
        <Button size="sm" onclick={addTag} label="Add" />
      </div>
    </section>

    <section>
      <h3>Aliases</h3>
      <div class="tags">
        {#each obj.aliases || [] as a}
          <span class="tag">{a}<Tooltip text="Remove alias"><button aria-label="Remove alias" onclick={() => removeAlias(a)}>×</button></Tooltip></span>
        {/each}
        {#if !(obj.aliases || []).length}<span class="none">no aliases</span>{/if}
      </div>
      <div class="row">
        <input class="mi" placeholder={isExit ? 'alt direction (e.g. n)' : 'name people can use'} bind:value={newAlias} onkeydown={(e) => e.key === 'Enter' && addAlias()} />
        <Button size="sm" onclick={addAlias} label="Add" />
      </div>
    </section>

    <section>
      <h3>Attributes</h3>
      <p class="hint">Freeform data stored on this object — read and written by softcode.</p>
      <table class="attrs">
        <tbody>
          {#each userAttrs as key}
            {@const val = obj.attrs[key]}
            {@const disp = typeof val === 'object' ? JSON.stringify(val) : String(val)}
            <tr>
              <td class="ak">{key}</td>
              {#if editingAttr === key}
                <td class="av"><textarea bind:value={editValue} onkeydown={(e) => attrKeydown(e, key)}></textarea></td>
                <td class="aa"><button onclick={() => saveAttr(key)}>Save</button></td>
              {:else}
                <td class="av" ondblclick={() => startEdit(key)} title="Double-click to edit">{disp}</td>
                <td class="aa">
                  <button onclick={() => startEdit(key)}>Edit</button>
                  <Tooltip text="Delete attribute"><button class="del" onclick={() => deleteAttr(key)}>Del</button></Tooltip>
                </td>
              {/if}
            </tr>
          {/each}
          {#if !userAttrs.length}<tr><td colspan="3" class="none pad">No attributes</td></tr>{/if}
        </tbody>
      </table>
      <div class="row">
        <input class="mi kw" placeholder="key" bind:value={newAttrKey} />
        <input class="mi" placeholder="value (JSON or string)" bind:value={newAttrValue} onkeydown={(e) => e.key === 'Enter' && addAttr()} />
        <Button size="sm" onclick={addAttr} label="Add" />
      </div>

      {#if sysAttrs.length}
        <details class="sys">
          <summary>System attributes <span class="sys-n">{sysAttrs.length}</span></summary>
          <p class="hint">Engine-managed — usually leave these alone. <code>_file_key</code> sets an object's area; <code>_rx</code>/<code>_ry</code> hold its map position.</p>
          <table class="attrs">
            <tbody>
              {#each sysAttrs as key}
                {@const val = obj.attrs[key]}
                {@const disp = typeof val === 'object' ? JSON.stringify(val) : String(val)}
                <tr>
                  <td class="ak internal">{key}</td>
                  {#if editingAttr === key}
                    <td class="av"><textarea bind:value={editValue} onkeydown={(e) => attrKeydown(e, key)}></textarea></td>
                    <td class="aa"><button onclick={() => saveAttr(key)}>Save</button></td>
                  {:else}
                    <td class="av" ondblclick={() => startEdit(key)} title="Double-click to edit">{disp}</td>
                    <td class="aa">
                      <button onclick={() => startEdit(key)}>Edit</button>
                      <Tooltip text="Delete attribute"><button class="del" onclick={() => deleteAttr(key)}>Del</button></Tooltip>
                    </td>
                  {/if}
                </tr>
              {/each}
            </tbody>
          </table>
        </details>
      {/if}
    </section>

    {#if Object.keys(obj.locks || {}).length}
      <section>
        <h3>Locks</h3>
        <table class="attrs">
          <tbody>
            {#each Object.keys(obj.locks).sort() as k}
              <tr><td class="ak">{k}</td><td class="av">{obj.locks[k]}</td></tr>
            {/each}
          </tbody>
        </table>
      </section>
    {/if}

    <section class="danger">
      {#if confirmDelete}
        <span class="confirm">Delete {obj.key} permanently?</span>
        <Button size="sm" tone="critical" onclick={doDelete} label="Delete" />
        <Button size="sm" onclick={() => (confirmDelete = false)} label="Cancel" />
      {:else}
        <button class="del-link" onclick={() => (confirmDelete = true)}>Delete {obj.kind}</button>
      {/if}
    </section>
  </div>
{/if}

<style>
  .pp { display: flex; flex-direction: column; gap: 12px; padding: 12px; }
  /* Each group is a raised card so the long form reads as distinct blocks
     rather than one undivided scroll. */
  section { display: flex; flex-direction: column; gap: 8px; background: var(--bg-surface, #17140f); border: 1px solid var(--border-muted, #211d16); border-radius: 10px; padding: 12px 14px; }
  section.danger { background: none; border: none; border-radius: 0; padding: 6px 14px 0; }
  h3 { margin: 0 0 2px; font-size: var(--fs-label); font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-muted, #8c8378); }
  .fl { display: flex; flex-direction: column; gap: 4px; font-size: var(--fs-label); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #8c8378); }
  .fi, .ft, .mi {
    background: var(--bg-inset, #12100c); color: var(--text-primary, #ece0c8);
    border: 1px solid var(--border-default, #332c22); border-radius: 6px; padding: 6px 8px;
    font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; outline: none; width: 100%;
  }
  .ft { resize: vertical; line-height: 1.5; }
  .fi:focus, .ft:focus, .mi:focus { border-color: var(--accent-amber, #c9956b); }
  .row { display: flex; gap: 6px; align-items: center; }
  .kw { width: 110px; flex: none; }
  .tags { display: flex; flex-wrap: wrap; gap: 4px; }
  .tag { display: inline-flex; align-items: center; gap: 4px; font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); padding: 2px 6px; border-radius: 3px; background: var(--bg-inset, #12100c); color: var(--text-secondary, #b6a888); }
  .tag button { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; font-size: 13px; line-height: 1; padding: 0; }
  .tag button:hover { color: var(--accent-red, #e06c75); }
  .none { color: var(--text-muted, #8c8378); font-style: italic; font-size: 11px; }
  .pad { padding: 10px !important; text-align: center; }
  .attrs { width: 100%; border-collapse: collapse; }
  .attrs td { padding: 4px 6px; border-bottom: 1px solid var(--border-muted, #211d16); vertical-align: top; font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); }
  .ak { color: var(--accent-teal, #56b6c2); white-space: nowrap; width: 1%; }
  .av { color: var(--text-primary, #ece0c8); word-break: break-word; white-space: pre-wrap; }
  .av textarea { width: 100%; background: var(--bg-inset, #12100c); color: var(--text-primary, #ece0c8); border: 1px solid var(--accent-amber, #c9956b); border-radius: 3px; padding: 3px 5px; font-family: inherit; font-size: var(--fs-meta); outline: none; resize: vertical; min-height: 24px; }
  .aa { white-space: nowrap; width: 1%; }
  .aa button { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; font-size: var(--fs-meta); padding: 2px 4px; }
  .aa button:hover { color: var(--text-primary, #ece0c8); }
  .aa button.del:hover { color: var(--accent-red, #e06c75); }
  .ak.internal { color: var(--text-muted, #8c8378); }
  /* Point-of-use concept hints for the newcomer half of the audience. Muted and
     small so they read as help, not chrome; power users glide past them. */
  .hint { margin: -2px 0 2px; font-size: var(--fs-meta); line-height: 1.5; color: var(--text-muted, #8c8378); }
  .hint code { font-family: var(--font-mono, ui-monospace, monospace); color: var(--text-secondary, #b6a888); }
  details.sys { margin-top: 10px; border-top: 1px solid var(--border-muted, #211d16); padding-top: 8px; }
  details.sys summary { cursor: pointer; font-size: var(--fs-label); font-weight: 700; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #8c8378); list-style: none; display: flex; align-items: center; gap: 6px; }
  details.sys summary::-webkit-details-marker { display: none; }
  details.sys summary::before { content: '▸'; color: var(--accent-amber, #c9956b); font-size: 10px; }
  details.sys[open] summary::before { content: '▾'; }
  details.sys summary:hover { color: var(--text-secondary, #b6a888); }
  .sys-n { font-family: var(--font-mono, ui-monospace, monospace); font-weight: 400; color: var(--text-muted, #8c8378); }
  .danger { flex-direction: row; align-items: center; gap: 8px; }
  .confirm { font-size: 12.5px; color: var(--accent-red, #e06c75); margin-right: auto; }
  .del-link { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; font: inherit; font-size: 12.5px; padding: 4px 2px; }
  .del-link:hover { color: var(--accent-red, #e06c75); }
</style>
