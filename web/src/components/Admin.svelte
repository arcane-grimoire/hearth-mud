<script>
  import { Button, IconButton, TextInput, showFlash } from '@kenn-io/kit-ui';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import Search from '@lucide/svelte/icons/search';
  import X from '@lucide/svelte/icons/x';
  import { api } from '../lib/api.js';

  let { onclose = () => {} } = $props();

  let rooms = $state([]);
  let allObjects = $state([]);
  let loading = $state(true);
  let selectedRef = $state(null);
  let obj = $state(null);
  let objLoading = $state(false);
  let expandedRooms = $state(new Set());
  let search = $state('');
  let editingAttr = $state(null);
  let editValue = $state('');
  let newAttrKey = $state('');
  let newAttrValue = $state('');
  let newTag = $state('');

  $effect(() => { loadAll(); });

  async function loadAll() {
    loading = true;
    const [roomRes, objRes] = await Promise.all([
      api('list_rooms'),
      api('list_objects'),
    ]);
    if (roomRes.ok) rooms = roomRes.data.sort((a, b) => (a.key || '').localeCompare(b.key || ''));
    if (objRes.ok) allObjects = objRes.data;
    loading = false;
  }

  async function selectObject(refId) {
    selectedRef = refId;
    editingAttr = null;
    objLoading = true;
    const res = await api('examine', { ref_id: refId });
    if (res.ok) {
      obj = res.data;
    } else {
      showFlash(res.error || 'Failed to load', { tone: 'danger' });
      obj = null;
    }
    objLoading = false;
  }

  async function reload() {
    if (selectedRef) await selectObject(selectedRef);
  }

  function toggleRoom(refId) {
    const next = new Set(expandedRooms);
    if (next.has(refId)) next.delete(refId);
    else next.add(refId);
    expandedRooms = next;
    selectObject(refId);
  }

  let filteredTree = $derived.by(() => {
    const q = search.toLowerCase();
    return rooms.map(r => {
      const contents = allObjects.filter(o =>
        o.location_ref === r.ref_id && o.kind !== 'room'
      );
      const matchesRoom = !q ||
        (r.title || '').toLowerCase().includes(q) ||
        (r.key || '').toLowerCase().includes(q) ||
        r.ref_id.toLowerCase().includes(q);
      const matchingContents = contents.filter(o =>
        !q ||
        (o.title || o.key || '').toLowerCase().includes(q) ||
        o.ref_id.toLowerCase().includes(q)
      );
      if (!matchesRoom && matchingContents.length === 0) return null;
      return { ...r, contents: matchingContents, expanded: expandedRooms.has(r.ref_id) || (q && matchingContents.length > 0) };
    }).filter(Boolean);
  });

  let orphans = $derived.by(() => {
    const q = search.toLowerCase();
    return allObjects.filter(o => {
      if (o.kind === 'room') return false;
      if (rooms.find(r => r.ref_id === o.location_ref)) return false;
      if (q && !(o.title || o.key || '').toLowerCase().includes(q) && !o.ref_id.toLowerCase().includes(q)) return false;
      return true;
    });
  });

  let sortedAttrs = $derived(obj ? Object.keys(obj.attrs || {}).sort() : []);

  function startEdit(key) {
    editingAttr = key;
    const val = obj.attrs[key];
    editValue = typeof val === 'object' ? JSON.stringify(val, null, 2) : String(val);
  }

  function cancelEdit() {
    editingAttr = null;
    editValue = '';
  }

  async function saveAttr(key) {
    let value;
    try { value = JSON.parse(editValue); } catch { value = editValue; }
    const res = await api('set_attribute', { ref_id: obj.ref_id, key, value });
    if (res.ok) {
      showFlash(`Updated ${key}`, { tone: 'success' });
      editingAttr = null;
      await reload();
    } else {
      showFlash(res.error || 'Failed', { tone: 'danger' });
    }
  }

  async function deleteAttr(key) {
    const res = await api('set_attribute', { ref_id: obj.ref_id, key, value: null });
    if (res.ok) {
      showFlash(`Deleted ${key}`, { tone: 'success' });
      await reload();
    } else {
      showFlash(res.error || 'Failed', { tone: 'danger' });
    }
  }

  async function addAttr() {
    const key = newAttrKey.trim();
    if (!key) return;
    let value;
    try { value = JSON.parse(newAttrValue); } catch { value = newAttrValue; }
    const res = await api('set_attribute', { ref_id: obj.ref_id, key, value });
    if (res.ok) {
      showFlash(`Added ${key}`, { tone: 'success' });
      newAttrKey = '';
      newAttrValue = '';
      await reload();
    } else {
      showFlash(res.error || 'Failed', { tone: 'danger' });
    }
  }

  async function setTitle(e) {
    const res = await api('set_title', { ref_id: obj.ref_id, title: e.target.value });
    if (res.ok) showFlash('Title updated', { tone: 'success' });
    else showFlash(res.error || 'Failed', { tone: 'danger' });
  }

  async function setDescription(e) {
    const res = await api('set_description', { ref_id: obj.ref_id, description: e.target.value });
    if (res.ok) showFlash('Description updated', { tone: 'success' });
    else showFlash(res.error || 'Failed', { tone: 'danger' });
  }

  async function addTag() {
    const tag = newTag.trim();
    if (!tag) return;
    const res = await api('add_tag', { ref_id: obj.ref_id, tag });
    if (res.ok) {
      showFlash('Tag added', { tone: 'success' });
      newTag = '';
      await reload();
    } else {
      showFlash(res.error || 'Failed', { tone: 'danger' });
    }
  }

  async function removeTag(tag) {
    const res = await api('remove_tag', { ref_id: obj.ref_id, tag });
    if (res.ok) {
      showFlash('Tag removed', { tone: 'success' });
      await reload();
    } else {
      showFlash(res.error || 'Failed', { tone: 'danger' });
    }
  }

  async function deleteObject() {
    if (!confirm(`Delete ${obj.ref_id}?`)) return;
    const res = await api('delete_object', { ref_id: obj.ref_id });
    if (res.ok) {
      showFlash(`Deleted ${obj.ref_id}`, { tone: 'success' });
      selectedRef = null;
      obj = null;
      await loadAll();
    } else {
      showFlash(res.error || 'Failed', { tone: 'danger' });
    }
  }

  function kindClass(kind) {
    return ({ room: 'kind-room', npc: 'kind-npc', item: 'kind-item', player: 'kind-player' })[kind] || '';
  }

  function handleAttrKeydown(e, key) {
    if (e.key === 'Escape') cancelEdit();
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); saveAttr(key); }
  }
</script>

<div class="admin">
  <div class="admin-header">
    <IconButton ariaLabel="Back" size="sm" onclick={onclose}>
      <ArrowLeft size={14} />
    </IconButton>
    <span class="admin-title">Admin</span>
  </div>

  <div class="admin-body">
    <!-- Object list -->
    <div class="obj-list">
      <div class="search-row">
        <Search size={12} />
        <input
          class="search-input"
          type="text"
          placeholder="Filter objects..."
          bind:value={search}
        />
        {#if search}
          <button class="search-clear" onclick={() => search = ''}>
            <X size={12} />
          </button>
        {/if}
      </div>

      {#if loading}
        <div class="list-empty">Loading...</div>
      {:else}
        <div class="list-scroll">
          {#each filteredTree as room}
            <button
              class="obj-row"
              class:selected={selectedRef === room.ref_id}
              onclick={() => toggleRoom(room.ref_id)}
            >
              <span class="toggle" class:open={room.expanded}>&#9654;</span>
              <span class="kind-badge kind-room">room</span>
              <span class="obj-name">{room.title || room.key}</span>
              <span class="obj-ref">{room.ref_id}</span>
            </button>
            {#if room.expanded}
              {#each room.contents as child}
                <button
                  class="obj-row nested"
                  class:selected={selectedRef === child.ref_id}
                  onclick={() => selectObject(child.ref_id)}
                >
                  <span class="kind-badge {kindClass(child.kind)}">{child.kind}</span>
                  <span class="obj-name">{child.title || child.key}</span>
                  <span class="obj-ref">{child.ref_id}</span>
                </button>
              {/each}
            {/if}
          {/each}
          {#each orphans as child}
            <button
              class="obj-row"
              class:selected={selectedRef === child.ref_id}
              onclick={() => selectObject(child.ref_id)}
            >
              <span class="kind-badge {kindClass(child.kind)}">{child.kind}</span>
              <span class="obj-name">{child.title || child.key}</span>
              <span class="obj-ref">{child.ref_id}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Detail -->
    <div class="obj-detail">
      {#if objLoading}
        <div class="detail-empty">Loading...</div>
      {:else if !obj}
        <div class="detail-empty">Select an object</div>
      {:else}
        <div class="detail-scroll">
          <div class="detail-head">
            <div>
              <div class="detail-title">{obj.title || obj.key}</div>
              <div class="detail-meta">
                <span class="kind-badge {kindClass(obj.kind)}">{obj.kind}</span>
                <span class="meta-ref">{obj.ref_id}</span>
                <span class="meta-key">key: {obj.key}</span>
                {#if obj.location_ref}
                  <span class="meta-key">in: {obj.location_ref}</span>
                {/if}
              </div>
            </div>
            <Button size="sm" tone="danger" onclick={deleteObject} label="Delete" />
          </div>

          <!-- Identity -->
          <section class="section">
            <h3 class="section-label">Identity</h3>
            <div class="field">
              <span class="field-label">Title</span>
              <input type="text" class="field-input" value={obj.title || ''} onchange={setTitle} />
            </div>
            <div class="field">
              <span class="field-label">Description</span>
              <textarea class="field-textarea" rows="3" onchange={setDescription}>{obj.description || ''}</textarea>
            </div>
          </section>

          <!-- Tags -->
          <section class="section">
            <h3 class="section-label">Tags</h3>
            <div class="tag-list">
              {#each obj.tags || [] as tag}
                <span class="tag">
                  {tag}
                  <button class="tag-remove" onclick={() => removeTag(tag)}>&times;</button>
                </span>
              {/each}
            </div>
            <div class="inline-add">
              <input
                class="mono-input"
                type="text"
                placeholder="category:key"
                bind:value={newTag}
                onkeydown={(e) => e.key === 'Enter' && addTag()}
              />
              <Button size="sm" onclick={addTag} label="Add" />
            </div>
          </section>

          <!-- Attrs -->
          <section class="section">
            <h3 class="section-label">Attributes</h3>
            <div class="attrs-wrap">
              <table class="attrs-table">
                <thead>
                  <tr><th>Key</th><th>Value</th><th></th></tr>
                </thead>
                <tbody>
                  {#each sortedAttrs as key}
                    {@const val = obj.attrs[key]}
                    {@const display = typeof val === 'object' ? JSON.stringify(val, null, 2) : String(val)}
                    <tr>
                      <td class="attr-key">{key}</td>
                      {#if editingAttr === key}
                        <td class="attr-val editing">
                          <textarea
                            bind:value={editValue}
                            onkeydown={(e) => handleAttrKeydown(e, key)}
                          ></textarea>
                        </td>
                        <td class="attr-actions">
                          <button class="act-btn" onclick={() => saveAttr(key)}>Save</button>
                          <button class="act-btn" onclick={cancelEdit}>Cancel</button>
                        </td>
                      {:else}
                        <td class="attr-val" ondblclick={() => startEdit(key)} title="Double-click to edit">{display}</td>
                        <td class="attr-actions">
                          <button class="act-btn" onclick={() => startEdit(key)}>Edit</button>
                          <button class="act-btn del" onclick={() => deleteAttr(key)}>Del</button>
                        </td>
                      {/if}
                    </tr>
                  {/each}
                  {#if sortedAttrs.length === 0}
                    <tr><td colspan="3" class="no-attrs">No attributes</td></tr>
                  {/if}
                </tbody>
              </table>
            </div>
            <div class="inline-add">
              <input class="mono-input key-input" type="text" placeholder="key" bind:value={newAttrKey} />
              <input
                class="mono-input val-input"
                type="text"
                placeholder="value (JSON or string)"
                bind:value={newAttrValue}
                onkeydown={(e) => e.key === 'Enter' && addAttr()}
              />
              <Button size="sm" onclick={addAttr} label="Add" />
            </div>
          </section>

          <!-- Programs -->
          {#if (obj.programs || []).length > 0}
            <section class="section">
              <h3 class="section-label">Programs</h3>
              <div class="program-list">
                {#each obj.programs.sort() as p}
                  <span class="program-chip">{p}</span>
                {/each}
              </div>
            </section>
          {/if}

          <!-- Locks -->
          {#if Object.keys(obj.locks || {}).length > 0}
            <section class="section">
              <h3 class="section-label">Locks</h3>
              <div class="attrs-wrap">
                <table class="attrs-table">
                  <thead><tr><th>Lock</th><th>Expression</th></tr></thead>
                  <tbody>
                    {#each Object.keys(obj.locks).sort() as k}
                      <tr>
                        <td class="attr-key">{k}</td>
                        <td class="attr-val">{obj.locks[k]}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            </section>
          {/if}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .admin {
    width: 680px;
    min-width: 400px;
    max-width: 60vw;
    background: var(--bg-surface);
    border-left: 1px solid var(--border-default);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .admin-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border-default);
    flex-shrink: 0;
  }

  .admin-title {
    font-size: var(--font-size-sm);
    font-weight: 700;
    color: var(--text-primary);
    letter-spacing: 0.02em;
  }

  .admin-body {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  /* ── Object list ── */
  .obj-list {
    width: 240px;
    min-width: 180px;
    border-right: 1px solid var(--border-default);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .search-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border-default);
    color: var(--text-muted);
  }

  .search-input {
    flex: 1;
    background: none;
    border: none;
    color: var(--text-primary);
    font-family: inherit;
    font-size: var(--font-size-xs, 12px);
    outline: none;
  }
  .search-input::placeholder { color: var(--text-muted); }

  .search-clear {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 0;
    display: flex;
  }
  .search-clear:hover { color: var(--text-primary); }

  .list-scroll {
    flex: 1;
    overflow-y: auto;
    padding: 4px 0;
  }

  .list-empty {
    padding: 16px;
    color: var(--text-muted);
    font-size: var(--font-size-xs, 12px);
  }

  .obj-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    width: 100%;
    background: none;
    border: none;
    border-left: 3px solid transparent;
    color: var(--text-primary);
    font-family: inherit;
    font-size: var(--font-size-xs, 12px);
    cursor: pointer;
    text-align: left;
    transition: background 0.1s;
  }
  .obj-row:hover { background: var(--bg-hover, rgba(255,255,255,0.04)); }
  .obj-row.selected {
    background: var(--bg-active, rgba(255,255,255,0.06));
    border-left-color: var(--accent-amber, #c9956b);
  }
  .obj-row.nested { padding-left: 26px; }

  .toggle {
    font-size: 8px;
    color: var(--text-muted);
    transition: transform 0.15s;
    width: 10px;
    text-align: center;
    flex-shrink: 0;
  }
  .toggle.open { transform: rotate(90deg); }

  .kind-badge {
    font-family: var(--font-mono);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 1px 4px;
    border-radius: 3px;
    flex-shrink: 0;
  }
  .kind-room { background: rgba(86,182,194,0.15); color: var(--accent-teal, #56b6c2); }
  .kind-npc { background: rgba(201,149,107,0.15); color: var(--accent-amber, #c9956b); }
  .kind-item { background: rgba(97,175,239,0.15); color: var(--accent-blue, #61afef); }
  .kind-player { background: rgba(198,120,221,0.15); color: var(--accent-purple, #c678dd); }

  .obj-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .obj-ref {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--text-muted);
    flex-shrink: 0;
  }

  /* ── Detail panel ── */
  .obj-detail {
    flex: 1;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .detail-empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
    font-size: var(--font-size-sm, 13px);
  }

  .detail-scroll {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .detail-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .detail-title {
    font-size: var(--font-size-lg, 18px);
    font-weight: 700;
    color: var(--text-primary);
  }

  .detail-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 4px;
  }

  .meta-ref {
    font-family: var(--font-mono);
    font-size: var(--font-size-2xs, 10px);
    color: var(--text-muted);
  }

  .meta-key {
    font-size: var(--font-size-2xs, 10px);
    color: var(--text-muted);
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .section-label {
    font-size: var(--font-size-2xs, 10px);
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    margin: 0;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .field-label {
    font-size: var(--font-size-2xs, 10px);
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  .field-input, .field-textarea {
    background: var(--bg-inset, #0d0f14);
    color: var(--text-primary);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md, 4px);
    padding: 6px 8px;
    font-family: var(--font-mono);
    font-size: var(--font-size-xs, 12px);
    outline: none;
    width: 100%;
  }
  .field-textarea { resize: vertical; line-height: 1.5; }
  .field-input:focus, .field-textarea:focus { border-color: var(--accent-blue, #61afef); }

  /* ── Tags ── */
  .tag-list {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .tag {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-family: var(--font-mono);
    font-size: var(--font-size-2xs, 10px);
    padding: 2px 6px;
    border-radius: 3px;
    background: var(--bg-inset, #0d0f14);
    color: var(--text-secondary);
  }

  .tag-remove {
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: 13px;
    cursor: pointer;
    padding: 0 1px;
    line-height: 1;
  }
  .tag-remove:hover { color: var(--accent-red, #e06c75); }

  .inline-add {
    display: flex;
    gap: 4px;
    align-items: center;
  }

  .mono-input {
    background: var(--bg-inset, #0d0f14);
    color: var(--text-primary);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md, 4px);
    padding: 4px 6px;
    font-family: var(--font-mono);
    font-size: var(--font-size-2xs, 10px);
    outline: none;
  }
  .mono-input:focus { border-color: var(--accent-blue, #61afef); }
  .mono-input::placeholder { color: var(--text-muted); }
  .key-input { width: 100px; }
  .val-input { flex: 1; }

  /* ── Attrs table ── */
  .attrs-wrap { overflow-x: auto; }

  .attrs-table {
    width: 100%;
    border-collapse: collapse;
    font-variant-numeric: tabular-nums;
  }

  .attrs-table th {
    text-align: left;
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-muted);
    padding: 6px 8px;
    border-bottom: 1px solid var(--border-default);
  }

  .attrs-table td {
    padding: 4px 8px;
    border-bottom: 1px solid var(--border-default);
    vertical-align: top;
  }
  .attrs-table tr:last-child td { border-bottom: none; }

  .attr-key {
    font-family: var(--font-mono);
    font-size: var(--font-size-2xs, 10px);
    color: var(--accent-teal, #56b6c2);
    white-space: nowrap;
    width: 1%;
  }

  .attr-val {
    font-family: var(--font-mono);
    font-size: var(--font-size-2xs, 10px);
    color: var(--text-primary);
    max-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .attr-val.editing { padding: 2px; }
  .attr-val textarea {
    width: 100%;
    background: var(--bg-inset, #0d0f14);
    color: var(--text-primary);
    border: 1px solid var(--accent-blue, #61afef);
    border-radius: 3px;
    padding: 3px 5px;
    font-family: var(--font-mono);
    font-size: var(--font-size-2xs, 10px);
    outline: none;
    resize: vertical;
    min-height: 24px;
  }

  .attr-actions {
    width: 1%;
    white-space: nowrap;
  }

  .act-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    font-family: inherit;
    font-size: var(--font-size-2xs, 10px);
    cursor: pointer;
    padding: 2px 4px;
  }
  .act-btn:hover { color: var(--text-primary); }
  .act-btn.del:hover { color: var(--accent-red, #e06c75); }

  .no-attrs {
    color: var(--text-muted);
    text-align: center;
    font-size: var(--font-size-xs, 12px);
    padding: 12px !important;
  }

  /* ── Programs ── */
  .program-list {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .program-chip {
    font-family: var(--font-mono);
    font-size: var(--font-size-2xs, 10px);
    padding: 2px 6px;
    border-radius: 3px;
    background: rgba(86,182,194,0.15);
    color: var(--accent-teal, #56b6c2);
  }

  @media (max-width: 700px) {
    .admin { width: 100%; max-width: 100%; }
    .obj-list { width: 160px; min-width: 120px; }
  }
</style>
