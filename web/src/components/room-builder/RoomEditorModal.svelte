<script>
  import { Button, TextInput, Chip, showFlash } from '@kenn-io/kit-ui';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import XIcon from '@lucide/svelte/icons/x';
  import { api } from '../../lib/api.js';

  // The object editor body (RoomBuilder wraps this in a kit-ui Modal). Works on
  // any object — rooms get Exits + Contents, other kinds (npc/item) get a
  // Location. Edits the live world directly. `onchanged` tells the parent to
  // reload after a structural change; `onedit` opens another object in place.
  let { ref, onclose = () => {}, onchanged = () => {}, onedit = () => {} } = $props();

  let loading = $state(true);
  let error = $state(null);
  let obj = $state(null);
  let title = $state('');
  let description = $state('');
  let titleDirty = $state(false);
  let descDirty = $state(false);
  let tags = $state([]);
  let attrs = $state([]); // [{key, value, dirty}]
  let exits = $state([]); // {ref_id, direction, target_ref}
  let contents = $state([]); // objects located in this room (rooms only)
  let programs = $state([]); // {hook, source, dirty, saving}
  let newTag = $state('');
  let newAttrKey = $state('');
  let newHook = $state('');
  let newObjKind = $state('item');
  let newObjKey = $state('');
  let aliases = $state([]);
  let newAlias = $state('');
  let exitDir = $state('');
  let exitTarget = $state('');
  let exitDirty = $state(false);
  let confirmDelete = $state(false);

  const isRoom = $derived(obj?.kind === 'room');
  const isExit = $derived(obj?.kind === 'exit');

  const AREA_OF = (a) => {
    const fk = a?._file_key;
    return typeof fk === 'string' && fk.includes('/') ? fk.split('/')[0] : null;
  };

  $effect(() => { if (ref) load(ref); });

  async function load(refId) {
    loading = true; error = null;
    const res = await api('examine', { ref_id: refId });
    if (!res?.ok) { error = res?.error || 'Failed to load'; loading = false; return; }
    obj = res.data;
    title = obj.title || '';
    description = obj.description || '';
    titleDirty = descDirty = false;
    tags = [...(obj.tags || [])];
    attrs = Object.entries(obj.attrs || {})
      .filter(([k]) => !['_rx', '_ry'].includes(k))
      .map(([key, value]) => ({ key, value: fmt(value), dirty: false }));
    aliases = [...(obj.aliases || [])];
    exitDir = obj.kind === 'exit' ? obj.key || '' : '';
    exitTarget = obj.kind === 'exit' ? obj.target_ref || '' : '';
    exitDirty = false;
    const room = obj.kind === 'room';
    const [ex, pr, co] = await Promise.all([
      room ? api('list_exits', { room_ref: refId }).catch(() => ({ ok: false })) : Promise.resolve({ ok: false }),
      api('list_programs', { ref_id: refId }).catch(() => ({ ok: false })),
      room ? api('list_objects', { location: refId }).catch(() => ({ ok: false })) : Promise.resolve({ ok: false }),
    ]);
    exits = ex?.ok ? ex.data : [];
    programs = pr?.ok ? pr.data.map((p) => ({ hook: p.hook, source: p.source || '', dirty: false, saving: false })) : [];
    contents = co?.ok ? co.data : [];
    loading = false;
  }

  const fmt = (v) => (typeof v === 'string' ? v : JSON.stringify(v));
  function parseVal(s) {
    const t = s.trim();
    if (t === 'true') return true;
    if (t === 'false') return false;
    if (t !== '' && !isNaN(Number(t))) return Number(t);
    return s;
  }

  async function saveTitle() {
    if (!titleDirty) return;
    const r = await api('set_title', { ref_id: ref, title: title.trim() || obj.key });
    if (r?.ok) { titleDirty = false; obj.title = title; onchanged(); showFlash?.({ message: 'Title saved', tone: 'success' }); }
    else showFlash?.({ message: r?.error || 'Save failed', tone: 'critical' });
  }
  async function saveDesc() {
    if (!descDirty) return;
    const r = await api('set_description', { ref_id: ref, description });
    if (r?.ok) { descDirty = false; obj.description = description; }
    else showFlash?.({ message: r?.error || 'Save failed', tone: 'critical' });
  }

  async function addTag() {
    const t = newTag.trim();
    if (!t) return;
    const r = await api('add_tag', { ref_id: ref, tag: t });
    if (r?.ok) { if (!tags.includes(t)) tags = [...tags, t]; newTag = ''; onchanged(); }
    else showFlash?.({ message: r?.error || 'Bad tag', tone: 'critical' });
  }
  async function removeTag(t) {
    const r = await api('remove_tag', { ref_id: ref, tag: t });
    if (r?.ok) { tags = tags.filter((x) => x !== t); onchanged(); }
  }

  async function saveAttr(a) {
    const r = await api('set_attribute', { ref_id: ref, key: a.key, value: parseVal(a.value) });
    if (r?.ok) { a.dirty = false; attrs = [...attrs]; }
    else showFlash?.({ message: r?.error || 'Save failed', tone: 'critical' });
  }
  async function addAttr() {
    const k = newAttrKey.trim();
    if (!k) return;
    const r = await api('set_attribute', { ref_id: ref, key: k, value: '' });
    if (r?.ok) { attrs = [...attrs, { key: k, value: '', dirty: false }]; newAttrKey = ''; }
    else showFlash?.({ message: r?.error || 'Failed', tone: 'critical' });
  }

  async function deleteExit(e) {
    const r = await api('delete_object', { ref_id: e.ref_id });
    if (r?.ok) { exits = exits.filter((x) => x.ref_id !== e.ref_id); onchanged(); }
    else showFlash?.({ message: r?.error || 'Failed', tone: 'critical' });
  }

  // Contents — objects located in this room (npcs, items).
  async function addObject() {
    const key = newObjKey.trim();
    if (!key) return;
    const r = await api('create_object', {
      area: AREA_OF(obj.attrs) || '',
      key,
      kind: newObjKind,
      title: `New ${newObjKind}`,
      description: '',
      location: ref,
    });
    if (r?.ok) {
      contents = [...contents, { ref_id: r.data.ref_id, key, kind: newObjKind, title: `New ${newObjKind}`, location_ref: ref }];
      newObjKey = '';
      onchanged();
    } else showFlash?.({ message: r?.error || 'Failed', tone: 'critical' });
  }
  async function removeContent(o) {
    const r = await api('delete_object', { ref_id: o.ref_id });
    if (r?.ok) { contents = contents.filter((x) => x.ref_id !== o.ref_id); onchanged(); }
    else showFlash?.({ message: r?.error || 'Failed', tone: 'critical' });
  }

  // Location — move a non-room object into another ref.
  let newLocation = $state('');
  async function moveTo() {
    const dest = newLocation.trim();
    if (!dest) return;
    const r = await api('set_location', { ref_id: ref, location: dest });
    if (r?.ok) { obj.location_ref = dest; newLocation = ''; onchanged(); showFlash?.({ message: `Moved to ${dest}`, tone: 'success' }); }
    else showFlash?.({ message: r?.error || 'Move failed', tone: 'critical' });
  }

  // Exit — change its direction / target in place.
  async function saveExit() {
    const r = await api('update_exit', { ref_id: ref, direction: exitDir.trim() || null, target: exitTarget.trim() || null });
    if (r?.ok) { exitDirty = false; obj.key = exitDir.trim(); obj.target_ref = exitTarget.trim(); onchanged(); showFlash?.({ message: 'Exit updated', tone: 'success' }); }
    else showFlash?.({ message: r?.error || 'Update failed', tone: 'critical' });
  }

  // Aliases — replace the whole set (works for any object).
  async function saveAliases(next) {
    const r = await api('set_aliases', { ref_id: ref, aliases: next });
    if (r?.ok) { aliases = next; onchanged(); }
    else showFlash?.({ message: r?.error || 'Failed', tone: 'critical' });
  }
  function addAlias() {
    const a = newAlias.trim();
    if (!a || aliases.includes(a)) { newAlias = ''; return; }
    saveAliases([...aliases, a]);
    newAlias = '';
  }
  const removeAlias = (a) => saveAliases(aliases.filter((x) => x !== a));

  async function addHook() {
    const h = newHook.trim();
    if (!h) return;
    const src = `-- ${h} hook\n`;
    const r = await api('set_program', { ref_id: ref, hook: h, source: src });
    if (r?.ok) { programs = [...programs, { hook: h, source: src, dirty: false, saving: false }]; newHook = ''; onchanged(); }
    else showFlash?.({ message: r?.error || 'Failed', tone: 'critical' });
  }
  async function saveHook(p) {
    p.saving = true; programs = [...programs];
    const r = await api('set_program', { ref_id: ref, hook: p.hook, source: p.source });
    p.saving = false; if (r?.ok) p.dirty = false;
    else showFlash?.({ message: r?.error || 'Save failed', tone: 'critical' });
    programs = [...programs];
  }
  async function removeHook(p) {
    const r = await api('remove_program', { ref_id: ref, hook: p.hook });
    if (r?.ok) { programs = programs.filter((x) => x.hook !== p.hook); onchanged(); }
  }

  async function doDelete() {
    const r = await api('delete_object', { ref_id: ref });
    if (r?.ok) { onchanged(); onclose(); }
    else showFlash?.({ message: r?.error || 'Delete failed', tone: 'critical' });
  }
</script>

<div class="rem-body">
  {#if loading}
    <div class="rem-msg">Loading…</div>
  {:else if error}
    <div class="rem-msg rem-err">{error}</div>
  {:else}
    <div class="rem-meta">
      <span class="rem-key">{obj.key}</span>
      <span class="rem-kind">{obj.kind}</span>
      {#if AREA_OF(obj.attrs)}<span class="rem-area">{AREA_OF(obj.attrs)}</span>{/if}
    </div>

    <label class="rem-lbl" for="rem-title">Title</label>
    <input id="rem-title" class="rem-in rem-title" bind:value={title}
      oninput={() => (titleDirty = true)} onblur={saveTitle} />

    <label class="rem-lbl" for="rem-desc">Description</label>
    <textarea id="rem-desc" class="rem-in rem-desc" bind:value={description}
      oninput={() => (descDirty = true)} onblur={saveDesc}></textarea>

    <div class="rem-sec">Tags</div>
    <div class="rem-tags">
      {#each tags as t}
        <span class="rem-tag">{t}<button aria-label="Remove tag" onclick={() => removeTag(t)}><XIcon size={11} /></button></span>
      {:else}
        <span class="rem-dim">No tags.</span>
      {/each}
    </div>
    <form class="rem-add" onsubmit={(e) => { e.preventDefault(); addTag(); }}>
      <TextInput bind:value={newTag} placeholder="category:key" size="sm" />
      <Button size="sm" onclick={addTag}><PlusIcon size={13} /> Tag</Button>
    </form>

    {#if isRoom}
      <div class="rem-sec">Exits ({exits.length})</div>
      {#each exits as e}
        <div class="rem-exit">
          <span class="rem-dir">{e.direction}</span>
          <button class="rem-cname" onclick={() => onedit(e.ref_id)}>→ {e.target_ref}</button>
          <button class="rem-del" aria-label="Delete exit" onclick={() => deleteExit(e)}><TrashIcon size={13} /></button>
        </div>
      {:else}
        <div class="rem-dim">No exits. Draw one from the graph.</div>
      {/each}

      <div class="rem-sec">Contents ({contents.length})</div>
      {#each contents as o}
        <div class="rem-exit">
          <span class="rem-ckind">{o.kind}</span>
          <button class="rem-cname" onclick={() => onedit(o.ref_id)}>{o.title || o.key}</button>
          <button class="rem-del" aria-label="Remove object" onclick={() => removeContent(o)}><TrashIcon size={13} /></button>
        </div>
      {:else}
        <div class="rem-dim">Empty. Add an NPC or item below.</div>
      {/each}
      <form class="rem-add" onsubmit={(e) => { e.preventDefault(); addObject(); }}>
        <select class="rem-kindsel" bind:value={newObjKind}>
          <option value="item">item</option>
          <option value="npc">npc</option>
        </select>
        <TextInput bind:value={newObjKey} placeholder="key" size="sm" />
        <Button size="sm" onclick={addObject}><PlusIcon size={13} /> Add</Button>
      </form>
    {:else if isExit}
      <div class="rem-sec">Exit</div>
      <label class="rem-lbl" for="rem-dir">Direction</label>
      <input id="rem-dir" class="rem-in ri-mono" bind:value={exitDir} oninput={() => (exitDirty = true)} />
      <label class="rem-lbl" for="rem-tgt">Target room</label>
      <input id="rem-tgt" class="rem-in ri-mono" bind:value={exitTarget} oninput={() => (exitDirty = true)} placeholder="#12" />
      <div class="rem-add">
        <span class="rem-dim">from {obj.location_ref || '(unplaced)'}</span>
        <span style="flex:1"></span>
        <Button size="sm" disabled={!exitDirty} onclick={saveExit}>Save exit</Button>
      </div>
    {:else}
      <div class="rem-sec">Location</div>
      <div class="rem-exit"><span class="rem-to">in {obj.location_ref || '(nowhere)'}</span></div>
      <form class="rem-add" onsubmit={(e) => { e.preventDefault(); moveTo(); }}>
        <TextInput bind:value={newLocation} placeholder="move to a room ref, e.g. #12" size="sm" />
        <Button size="sm" onclick={moveTo}>Move</Button>
      </form>
    {/if}

    <div class="rem-sec">Aliases</div>
    <div class="rem-tags">
      {#each aliases as a}
        <span class="rem-tag">{a}<button aria-label="Remove alias" onclick={() => removeAlias(a)}><XIcon size={11} /></button></span>
      {:else}
        <span class="rem-dim">No aliases.</span>
      {/each}
    </div>
    <form class="rem-add" onsubmit={(e) => { e.preventDefault(); addAlias(); }}>
      <TextInput bind:value={newAlias} placeholder={isExit ? 'alt direction (e.g. n)' : 'name people can use'} size="sm" />
      <Button size="sm" onclick={addAlias}><PlusIcon size={13} /> Alias</Button>
    </form>

    <div class="rem-sec">Attributes</div>
    {#each attrs as a}
      <div class="rem-attr">
        <span class="rem-akey" class:internal={a.key.startsWith('_')}>{a.key}</span>
        <input class="rem-in rem-aval" bind:value={a.value} oninput={() => (a.dirty = true)} onblur={() => a.dirty && saveAttr(a)} />
      </div>
    {:else}
      <div class="rem-dim">No attributes.</div>
    {/each}
    <form class="rem-add" onsubmit={(e) => { e.preventDefault(); addAttr(); }}>
      <TextInput bind:value={newAttrKey} placeholder="attribute key" size="sm" />
      <Button size="sm" onclick={addAttr}><PlusIcon size={13} /> Attr</Button>
    </form>

    <div class="rem-sec">Programs</div>
    {#each programs as p}
      <div class="rem-prog">
        <div class="rem-prog-top">
          <span class="rem-hook">{p.hook}</span>
          <span class="rem-spacer"></span>
          {#if p.dirty}<Button size="sm" disabled={p.saving} onclick={() => saveHook(p)}>Save</Button>{/if}
          <button class="rem-del" aria-label="Remove hook" onclick={() => removeHook(p)}><TrashIcon size={13} /></button>
        </div>
        <textarea class="rem-in rem-src" bind:value={p.source} oninput={() => { p.dirty = true; programs = [...programs]; }}></textarea>
      </div>
    {:else}
      <div class="rem-dim">No programs.</div>
    {/each}
    <form class="rem-add" onsubmit={(e) => { e.preventDefault(); addHook(); }}>
      <TextInput bind:value={newHook} placeholder="hook name (e.g. on_enter)" size="sm" />
      <Button size="sm" onclick={addHook}><PlusIcon size={13} /> Hook</Button>
    </form>

    <div class="rem-danger">
      {#if confirmDelete}
        <span class="rem-confirm">Delete {obj.key} permanently?</span>
        <Button size="sm" tone="critical" onclick={doDelete}>Delete</Button>
        <Button size="sm" onclick={() => (confirmDelete = false)}>Cancel</Button>
      {:else}
        <button class="rem-dellink" onclick={() => (confirmDelete = true)}><TrashIcon size={13} /> Delete {obj.kind}</button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .rem-body { display: flex; flex-direction: column; max-height: min(72vh, 720px); overflow-y: auto; padding-right: 2px; }
  .rem-msg { padding: 24px; text-align: center; color: var(--text-muted, #9a9186); }
  .rem-err { color: var(--accent-red, #d07a5a); }
  .rem-meta { display: flex; align-items: center; gap: 8px; margin-bottom: 12px; }
  .rem-key { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-secondary, #b6a888); }
  .rem-kind, .rem-area { font-size: 10px; text-transform: uppercase; letter-spacing: .05em; padding: 1px 7px; border-radius: 999px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-muted, #2a2419); color: var(--text-muted, #9a9186); }
  .rem-area { color: var(--accent-amber, #c9956b); }

  .rem-lbl { font-size: 10px; font-weight: 600; letter-spacing: .06em; text-transform: uppercase; color: var(--text-muted, #9a9186); margin: 12px 0 5px; }
  .rem-in { width: 100%; box-sizing: border-box; font: inherit; font-size: 13px; color: var(--text-primary, #ece0c8); background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 7px; padding: 7px 9px; }
  .rem-in:focus { outline: none; border-color: var(--accent-amber, #c9956b); }
  .rem-title { font-weight: 600; }
  .rem-desc { min-height: 72px; line-height: 1.5; resize: vertical; }

  .rem-sec { font-size: 10px; font-weight: 600; letter-spacing: .06em; text-transform: uppercase; color: var(--text-muted, #9a9186); margin: 18px 0 8px; padding-bottom: 5px; border-bottom: 1px solid var(--border-muted, #2a2419); }
  .rem-dim { font-size: 12px; color: var(--text-muted, #8c8378); font-style: italic; }

  .rem-tags { display: flex; flex-wrap: wrap; gap: 5px; margin-bottom: 8px; }
  .rem-tag { display: inline-flex; align-items: center; gap: 4px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 10.5px; padding: 2px 4px 2px 7px; border-radius: 5px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-muted, #2a2419); color: var(--text-secondary, #b6a888); }
  .rem-tag button { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; padding: 0; line-height: 0; display: grid; place-items: center; }
  .rem-tag button:hover { color: var(--accent-red, #d07a5a); }

  .rem-add { display: flex; gap: 6px; align-items: center; margin-bottom: 4px; }
  .rem-add :global(.kit-text-input), .rem-add :global(input) { flex: 1; }

  .rem-exit { display: flex; align-items: center; gap: 9px; padding: 6px 8px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-muted, #2a2419); border-radius: 7px; margin-bottom: 5px; }
  .rem-dir { font-family: var(--font-mono, ui-monospace, monospace); font-size: 10px; text-transform: uppercase; color: var(--bg-primary, #12100c); background: var(--edge, #9c8863); border-radius: 4px; padding: 1px 6px; }
  .rem-to { flex: 1; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-secondary, #b6a888); }

  .rem-ckind { font-family: var(--font-mono, ui-monospace, monospace); font-size: 9px; text-transform: uppercase; color: var(--bg-primary, #12100c); background: var(--edge, #9c8863); border-radius: 4px; padding: 1px 6px; }
  .rem-cname { flex: 1; text-align: left; background: none; border: none; cursor: pointer; font: inherit; font-size: 12.5px; color: var(--text-primary, #ece0c8); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .rem-cname:hover { color: var(--accent-amber, #c9956b); text-decoration: underline; }
  .rem-kindsel { font: inherit; font-size: 12px; color: var(--text-primary, #ece0c8); background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 6px; padding: 5px 6px; }
  .rem-attr { display: flex; align-items: center; gap: 8px; margin-bottom: 5px; }
  .rem-akey { flex: 0 0 38%; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11.5px; color: var(--text-secondary, #b6a888); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .rem-akey.internal { color: var(--text-muted, #8c8378); }
  .rem-aval { flex: 1; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; padding: 5px 8px; }

  .rem-prog { border: 1px solid var(--border-muted, #2a2419); border-radius: 8px; padding: 8px; margin-bottom: 7px; background: var(--bg-primary, #12100c); }
  .rem-prog-top { display: flex; align-items: center; gap: 8px; margin-bottom: 6px; }
  .rem-hook { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--accent-amber, #c9956b); }
  .rem-spacer { flex: 1; }
  .rem-src { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11.5px; min-height: 60px; line-height: 1.45; resize: vertical; }

  .rem-del { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; padding: 3px; border-radius: 5px; line-height: 0; }
  .rem-del:hover { color: var(--accent-red, #d07a5a); background: color-mix(in srgb, var(--accent-red, #d07a5a) 14%, transparent); }

  .rem-danger { display: flex; align-items: center; gap: 8px; margin-top: 20px; padding-top: 14px; border-top: 1px solid var(--border-muted, #2a2419); }
  .rem-confirm { font-size: 12.5px; color: var(--accent-red, #d07a5a); margin-right: auto; }
  .rem-dellink { display: inline-flex; align-items: center; gap: 6px; background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; font: inherit; font-size: 12.5px; padding: 4px 2px; }
  .rem-dellink:hover { color: var(--accent-red, #d07a5a); }
</style>
