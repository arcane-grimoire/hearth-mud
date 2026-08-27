<script>
  import { Button, TextInput, showFlash, Tooltip } from '@kenn-io/kit-ui';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import { api } from '../../lib/api.js';
  import { REV, DIR_FULL, normDir } from '../room-builder/layout.js';

  // A room's structure: the exits leading out of it and the objects sitting
  // in it. Ported from the old RoomEditorModal so the unified builder edits a
  // room's connectivity and contents without dropping to the graph. `rooms`
  // seeds the exit-target datalist; `onedit` opens another object in its own
  // tab; `onchanged` asks the workspace to reload after a structural change.
  let { obj = null, rooms = [], onchanged = () => {}, onedit = () => {} } = $props();

  let exits = $state([]);
  let contents = $state([]);
  let loading = $state(true);
  let loadedFor = $state(null);

  let exitDir = $state('');
  let exitTarget = $state('');
  let exitReverse = $state(true);

  let newObjKind = $state('item');
  let newObjKey = $state('');

  // Which row (by ref_id) is armed for delete. Destructive actions here remove
  // real, persistent world content and there is no server-side undo, so every
  // delete is a two-step confirm rather than a single click.
  let confirmExit = $state(null);
  let confirmContent = $state(null);

  const AREA_OF = (a) => {
    const fk = a?._file_key;
    return typeof fk === 'string' && fk.includes('/') ? fk.split('/')[0] : '';
  };

  // Reload exits + contents whenever the panel points at a different room.
  $effect(() => {
    const ref = obj?.ref_id;
    if (ref && ref !== loadedFor) load(ref);
  });

  async function load(ref) {
    loading = true;
    loadedFor = ref;
    const [ex, co] = await Promise.all([
      api('list_exits', { room_ref: ref }).catch(() => ({ ok: false })),
      api('list_objects', { location: ref }).catch(() => ({ ok: false })),
    ]);
    exits = ex?.ok ? ex.data : [];
    contents = co?.ok ? co.data : [];
    confirmExit = null;
    confirmContent = null;
    loading = false;
  }
  function reload() { if (obj?.ref_id) load(obj.ref_id); }

  const roomName = (ref) => {
    const r = rooms.find((x) => x.ref_id === ref);
    return r ? `${r.title || r.key} ${ref}` : ref;
  };

  async function addExit() {
    const dir = exitDir.trim();
    const target = exitTarget.trim();
    if (!dir || !target) { showFlash('Direction and target are required', { tone: 'danger' }); return; }
    // Match the graph: create exits with the world's full direction word.
    const code = normDir(dir);
    const fullDir = DIR_FULL[code] || dir;
    const r = await api('create_exit', { source: obj.ref_id, direction: fullDir, target, aliases: null });
    if (!r?.ok) { showFlash(r?.error || 'Could not create the exit', { tone: 'danger' }); return; }
    // Optional reverse exit back from the target (only for known compass/updown dirs).
    const revCode = REV[code];
    if (exitReverse && revCode) {
      const revDir = DIR_FULL[revCode] || revCode;
      await api('create_exit', { source: target, direction: revDir, target: obj.ref_id, aliases: null });
    }
    exitDir = ''; exitTarget = '';
    onchanged();
    reload();
  }
  async function deleteExit(e) {
    const r = await api('delete_object', { ref_id: e.ref_id });
    if (r?.ok) { exits = exits.filter((x) => x.ref_id !== e.ref_id); confirmExit = null; onchanged(); }
    else showFlash(r?.error || 'Could not delete the exit', { tone: 'danger' });
  }

  async function addObject() {
    const key = newObjKey.trim();
    if (!key) { showFlash('A key is required', { tone: 'danger' }); return; }
    const r = await api('create_object', {
      area: AREA_OF(obj.attrs),
      key,
      kind: newObjKind,
      title: `New ${newObjKind}`,
      description: '',
      location: obj.ref_id,
    });
    if (r?.ok) { newObjKey = ''; onchanged(); reload(); }
    else showFlash(r?.error || `Could not create the ${newObjKind}`, { tone: 'danger' });
  }
  async function removeContent(o) {
    const r = await api('delete_object', { ref_id: o.ref_id });
    if (r?.ok) { contents = contents.filter((x) => x.ref_id !== o.ref_id); confirmContent = null; onchanged(); }
    else showFlash(r?.error || `Could not delete the ${o.kind}`, { tone: 'danger' });
  }
</script>

<div class="sp">
  <section>
    <h3>Exits{#if !loading} <span class="cnt">{exits.length}</span>{/if}</h3>
    {#if loading}
      <div class="none">Loading…</div>
    {:else}
      <div class="rows">
        {#each exits as e (e.ref_id)}
          <div class="row-item" class:armed={confirmExit === e.ref_id}>
            <!-- Two destinations in one row, so each gets its own control: the
                 direction chip opens the exit object (aliases, locks, hooks),
                 the room name opens the room it leads to. Reading the row
                 left to right matches what each click does. -->
            <Tooltip text="Edit this exit"><button class="dir" onclick={() => onedit(e.ref_id)}>{e.direction}</button></Tooltip>
            <button class="link" onclick={() => onedit(e.target_ref)} title={`Open ${roomName(e.target_ref)}`}>→ {roomName(e.target_ref)}</button>
            {#if confirmExit === e.ref_id}
              <span class="confirm-q">Delete this exit?</span>
              <button class="confirm-go" onclick={() => deleteExit(e)}>Delete</button>
              <button class="confirm-no" onclick={() => (confirmExit = null)}>Cancel</button>
            {:else}
              <Tooltip text="Delete exit"><button class="del" aria-label="Delete exit" onclick={() => { confirmContent = null; confirmExit = e.ref_id; }}><TrashIcon size={13} /></button></Tooltip>
            {/if}
          </div>
        {:else}
          <div class="none">No exits yet.</div>
        {/each}
      </div>
      <form class="add" onsubmit={(e) => { e.preventDefault(); addExit(); }}>
        <TextInput bind:value={exitDir} placeholder="direction (e.g. north)" size="sm" />
        <input class="mi" list="struct-rooms" bind:value={exitTarget} placeholder="target room ref (e.g. #12)" />
        <Button size="sm" onclick={addExit}><PlusIcon size={13} /> Exit</Button>
      </form>
      <label class="rev"><input type="checkbox" bind:checked={exitReverse} /> also create the reverse exit back</label>
      <datalist id="struct-rooms">
        {#each rooms as r}<option value={r.ref_id}>{r.title || r.key}</option>{/each}
      </datalist>
    {/if}
  </section>

  <section>
    <h3>Contents{#if !loading} <span class="cnt">{contents.length}</span>{/if}</h3>
    {#if loading}
      <div class="none">Loading…</div>
    {:else}
      <div class="rows">
        {#each contents as o (o.ref_id)}
          <div class="row-item" class:armed={confirmContent === o.ref_id}>
            <span class="kind">{o.kind}</span>
            <button class="link" onclick={() => onedit(o.ref_id)} title="Edit this object">{o.title || o.key}</button>
            {#if o.kind === 'player'}
              <!-- A player in the room is live runtime state, not buildable
                   content. Never offer to delete them from here. -->
              <span class="here" title="A connected player — edit or delete players elsewhere">here now</span>
            {:else if confirmContent === o.ref_id}
              <span class="confirm-q">Delete {o.kind} permanently?</span>
              <button class="confirm-go" onclick={() => removeContent(o)}>Delete</button>
              <button class="confirm-no" onclick={() => (confirmContent = null)}>Cancel</button>
            {:else}
              <Tooltip text="Delete this {o.kind} permanently"><button class="del" aria-label="Delete {o.kind}" onclick={() => { confirmExit = null; confirmContent = o.ref_id; }}><TrashIcon size={13} /></button></Tooltip>
            {/if}
          </div>
        {:else}
          <div class="none">Empty. Add an NPC or item below.</div>
        {/each}
      </div>
      <form class="add" onsubmit={(e) => { e.preventDefault(); addObject(); }}>
        <select class="sel" bind:value={newObjKind}>
          <option value="item">item</option>
          <option value="npc">npc</option>
        </select>
        <TextInput bind:value={newObjKey} placeholder="key" size="sm" />
        <Button size="sm" onclick={addObject}><PlusIcon size={13} /> Add</Button>
      </form>
    {/if}
  </section>
</div>

<style>
  /* Embedded inside PropertiesPanel: display:contents dissolves this wrapper so
     the Exits/Contents sections become siblings of the other property cards,
     sharing .pp's gap. Each section is carded to match. */
  .sp { display: contents; }
  section { display: flex; flex-direction: column; gap: 8px; background: var(--bg-surface, #17140f); border: 1px solid var(--border-muted, #211d16); border-radius: 10px; padding: 12px 14px; }
  h3 { margin: 0 0 2px; font-size: var(--fs-label); font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-muted, #8c8378); }
  .cnt { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); opacity: 0.8; }
  .rows { display: flex; flex-direction: column; gap: 5px; }
  .row-item { display: flex; align-items: center; gap: 9px; padding: 6px 8px; background: var(--bg-inset, #12100c); border: 1px solid var(--border-muted, #211d16); border-radius: 7px; }
  .dir { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-badge); text-transform: uppercase; color: var(--bg-primary, #12100c); background: var(--edge, #9c8863); border: none; border-radius: 4px; padding: 1px 6px; cursor: pointer; }
  .dir:hover { background: var(--accent-amber, #c9956b); }
  .kind { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-badge); text-transform: uppercase; color: var(--bg-primary, #12100c); background: var(--edge, #9c8863); border-radius: 4px; padding: 1px 6px; }
  .link { flex: 1; text-align: left; background: none; border: none; cursor: pointer; font: inherit; font-size: 12.5px; color: var(--text-primary, #ece0c8); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .link:hover { color: var(--accent-amber, #c9956b); text-decoration: underline; }
  .del { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; padding: 3px; border-radius: 5px; line-height: 0; }
  .del:hover { color: var(--accent-red, #d07a5a); background: color-mix(in srgb, var(--accent-red, #d07a5a) 14%, transparent); }
  /* Armed (confirm) state: the row shifts to a danger tint so it's unmistakable
     which item is about to be deleted. */
  .row-item.armed { border-color: color-mix(in srgb, var(--accent-red, #d07a5a) 55%, transparent); background: color-mix(in srgb, var(--accent-red, #d07a5a) 10%, var(--bg-inset, #12100c)); }
  .confirm-q { font-size: 11.5px; color: var(--accent-red, #d07a5a); white-space: nowrap; }
  .confirm-go { flex: none; background: var(--accent-red, #d07a5a); border: none; color: #fff; cursor: pointer; font: inherit; font-size: 11.5px; font-weight: 600; padding: 3px 9px; border-radius: 6px; }
  .confirm-go:hover { filter: brightness(1.08); }
  .confirm-no { flex: none; background: none; border: 1px solid var(--border-default, #332c22); color: var(--text-secondary, #b6a888); cursor: pointer; font: inherit; font-size: 11.5px; padding: 3px 9px; border-radius: 6px; }
  .confirm-no:hover { border-color: var(--text-secondary, #b6a888); color: var(--text-primary, #ece0c8); }
  .here { flex: none; font-size: var(--fs-meta); color: var(--text-muted, #8c8378); background: var(--bg-surface, #17140f); border: 1px solid var(--border-muted, #211d16); border-radius: 999px; padding: 1px 8px; }
  .add { display: flex; gap: 6px; align-items: center; }
  .mi { flex: 1; min-width: 0; background: var(--bg-inset, #12100c); color: var(--text-primary, #ece0c8); border: 1px solid var(--border-default, #332c22); border-radius: 6px; padding: 6px 8px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; outline: none; }
  .mi:focus { border-color: var(--accent-amber, #c9956b); }
  .sel { font: inherit; font-size: 12px; color: var(--text-primary, #ece0c8); background: var(--bg-inset, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 6px; padding: 5px 6px; }
  .rev { display: flex; align-items: center; gap: 6px; font-size: 11px; color: var(--text-muted, #8c8378); cursor: pointer; }
  .none { color: var(--text-muted, #8c8378); font-style: italic; font-size: 11.5px; }
</style>
