<script>
  import { Button, showFlash, Tooltip } from '@kenn-io/kit-ui';
  import { api } from '../../lib/api.js';

  // Object properties: identity, tags, attributes. Lifted from Admin.svelte so
  // the unified workspace has ONE properties surface instead of the three that
  // exist today (Editor, Admin, RoomEditorModal each re-implemented this).
  let { obj = null, onchanged = () => {} } = $props();

  let editingAttr = $state(null);
  let editValue = $state('');
  let newAttrKey = $state('');
  let newAttrValue = $state('');
  let newTag = $state('');

  let sortedAttrs = $derived(obj ? Object.keys(obj.attrs || {}).sort() : []);

  async function setTitle(e) {
    const res = await api('set_title', { ref_id: obj.ref_id, title: e.target.value });
    res.ok ? onchanged() : showFlash(res.error || 'Failed', { tone: 'danger' });
  }
  async function setDescription(e) {
    const res = await api('set_description', { ref_id: obj.ref_id, description: e.target.value });
    res.ok ? onchanged() : showFlash(res.error || 'Failed', { tone: 'danger' });
  }
  async function addTag() {
    const tag = newTag.trim();
    if (!tag) return;
    const res = await api('add_tag', { ref_id: obj.ref_id, tag });
    if (res.ok) { newTag = ''; onchanged(); } else showFlash(res.error || 'Failed', { tone: 'danger' });
  }
  async function removeTag(tag) {
    const res = await api('remove_tag', { ref_id: obj.ref_id, tag });
    res.ok ? onchanged() : showFlash(res.error || 'Failed', { tone: 'danger' });
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
    if (res.ok) { editingAttr = null; onchanged(); } else showFlash(res.error || 'Failed', { tone: 'danger' });
  }
  async function deleteAttr(key) {
    const res = await api('set_attribute', { ref_id: obj.ref_id, key, value: null });
    res.ok ? onchanged() : showFlash(res.error || 'Failed', { tone: 'danger' });
  }
  async function addAttr() {
    const key = newAttrKey.trim();
    if (!key) return;
    let value;
    try { value = JSON.parse(newAttrValue); } catch { value = newAttrValue; }
    const res = await api('set_attribute', { ref_id: obj.ref_id, key, value });
    if (res.ok) { newAttrKey = ''; newAttrValue = ''; onchanged(); } else showFlash(res.error || 'Failed', { tone: 'danger' });
  }
  function attrKeydown(e, key) {
    if (e.key === 'Escape') editingAttr = null;
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); saveAttr(key); }
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

    <section>
      <h3>Tags</h3>
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
      <h3>Attributes</h3>
      <table class="attrs">
        <tbody>
          {#each sortedAttrs as key}
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
          {#if !sortedAttrs.length}<tr><td colspan="3" class="none pad">No attributes</td></tr>{/if}
        </tbody>
      </table>
      <div class="row">
        <input class="mi kw" placeholder="key" bind:value={newAttrKey} />
        <input class="mi" placeholder="value (JSON or string)" bind:value={newAttrValue} onkeydown={(e) => e.key === 'Enter' && addAttr()} />
        <Button size="sm" onclick={addAttr} label="Add" />
      </div>
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
  </div>
{/if}

<style>
  .pp { display: flex; flex-direction: column; gap: 18px; padding: 14px; }
  section { display: flex; flex-direction: column; gap: 8px; }
  h3 { margin: 0; font-size: 10px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-muted, #8c8378); }
  .fl { display: flex; flex-direction: column; gap: 4px; font-size: 10px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #8c8378); }
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
  .tag { display: inline-flex; align-items: center; gap: 4px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 10px; padding: 2px 6px; border-radius: 3px; background: var(--bg-inset, #12100c); color: var(--text-secondary, #b6a888); }
  .tag button { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; font-size: 13px; line-height: 1; padding: 0; }
  .tag button:hover { color: var(--accent-red, #e06c75); }
  .none { color: var(--text-muted, #8c8378); font-style: italic; font-size: 11px; }
  .pad { padding: 10px !important; text-align: center; }
  .attrs { width: 100%; border-collapse: collapse; }
  .attrs td { padding: 4px 6px; border-bottom: 1px solid var(--border-muted, #211d16); vertical-align: top; font-family: var(--font-mono, ui-monospace, monospace); font-size: 10.5px; }
  .ak { color: var(--accent-teal, #56b6c2); white-space: nowrap; width: 1%; }
  .av { color: var(--text-primary, #ece0c8); word-break: break-word; white-space: pre-wrap; }
  .av textarea { width: 100%; background: var(--bg-inset, #12100c); color: var(--text-primary, #ece0c8); border: 1px solid var(--accent-amber, #c9956b); border-radius: 3px; padding: 3px 5px; font-family: inherit; font-size: 10.5px; outline: none; resize: vertical; min-height: 24px; }
  .aa { white-space: nowrap; width: 1%; }
  .aa button { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; font-size: 10.5px; padding: 2px 4px; }
  .aa button:hover { color: var(--text-primary, #ece0c8); }
  .aa button.del:hover { color: var(--accent-red, #e06c75); }
</style>
