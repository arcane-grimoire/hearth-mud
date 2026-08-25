<script>
  import { Button, showFlash, Tooltip } from '@kenn-io/kit-ui';
  import { api } from '../../lib/api.js';
  import { bbcodeToHtml } from '../../lib/bbcode.js';
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
  let archDraft = $state('');   // ref to delegate to (set/change archetype)

  // Kind-specific editable state, reseeded whenever the panel points at a
  // different object (examine returns fresh data after each onchanged()).
  let exitDir = $state('');
  let exitTarget = $state('');
  let moveDest = $state('');
  let confirmDelete = $state(false);
  let descDraft = $state('');   // live description text, drives the player's-view preview
  let syncedFor = $state(null);
  $effect(() => {
    if (obj && obj.ref_id !== syncedFor) {
      syncedFor = obj.ref_id;
      exitDir = obj.kind === 'exit' ? (obj.key || '') : '';
      exitTarget = obj.kind === 'exit' ? (obj.target_ref || '') : '';
      moveDest = '';
      confirmDelete = false;
      descDraft = obj.description || '';
      archDraft = '';
    }
  });

  const isExit = $derived(obj?.kind === 'exit');
  const isRoom = $derived(obj?.kind === 'room');
  // A `system:locked` object is file-authoritative: its definition is read-only
  // to in-game authoring (edit the source file and @reload-world). The server
  // refuses every authoring edit; the panel mirrors that by disabling the
  // controls and showing a banner, so it reads as "locked", not "broken".
  const locked = $derived(obj?.locked === true);

  let sortedAttrs = $derived(obj ? Object.keys(obj.attrs || {}).sort() : []);
  // Engine-managed attrs (_file_key, _rx/_ry, …) are noise for most editing and
  // alarming to a newcomer, so they fold away under an Advanced disclosure while
  // the object's own attributes stay in the open.
  const userAttrs = $derived(sortedAttrs.filter((k) => !k.startsWith('_')));
  const sysAttrs = $derived(sortedAttrs.filter((k) => k.startsWith('_')));

  // Delegation: resolved_attrs carries per-attr provenance {value, source,
  // overrides}. `source === 'own'` means this object holds the value; any other
  // source is the ancestor ref it's inherited from. `overrides` flags an own
  // value that shadows an inherited one (so we can offer "revert to inherited").
  const resolved = $derived(obj?.resolved_attrs || {});
  const inheritedAttrs = $derived(
    Object.keys(resolved)
      .filter((k) => resolved[k]?.source !== 'own' && !k.startsWith('_'))
      .sort(),
  );
  function overridesInherited(key) { return resolved[key]?.overrides === true; }

  // Title/description/tags can also be inherited. `title` is the object's OWN
  // title (null when unset); `resolved_title` is the effective value up the
  // chain. When own is unset but a resolved value exists, show it muted so the
  // builder reflects what a player actually sees — editing still writes an own
  // override via the same set_title/set_description.
  const ownTitle = $derived(obj?.title ?? '');
  const inheritedTitle = $derived(!ownTitle && obj?.resolved_title ? obj.resolved_title : '');
  const ownDesc = $derived(obj?.description ?? '');
  const inheritedDesc = $derived(!ownDesc.trim() && obj?.resolved_description ? obj.resolved_description : '');
  // Tags split into own (editable/removable) vs inherited (from the archetype,
  // shown muted, not removable here).
  const resolvedTags = $derived(obj?.resolved_tags || []);
  const ownTags = $derived(resolvedTags.filter((t) => t.source === 'own').map((t) => t.tag));
  const ownTagList = $derived(resolvedTags.length ? ownTags : (obj?.tags || []));
  const inheritedTags = $derived(resolvedTags.filter((t) => t.source !== 'own'));

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
    if (locked) return;
    editingAttr = key;
    const val = obj.attrs[key];
    editValue = typeof val === 'object' ? JSON.stringify(val, null, 2) : String(val);
  }
  async function saveAttr(key) {
    if (locked) return;
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
  // Copy an inherited value down as an own attribute so it can be edited here;
  // the reverse ("Revert") is just deleteAttr, which drops the own value and
  // lets the inherited one show through again.
  async function overrideAttr(key) {
    const value = resolved[key]?.value ?? null;
    const res = await api('set_attribute', { ref_id: obj.ref_id, key, value });
    res.ok ? onchanged() : showFlash(res.error || 'Could not override the attribute', { tone: 'danger' });
  }

  // Archetype delegation — point this object at another (inherit its
  // title/attrs/tags/hooks) or flatten the chain and stop delegating.
  async function setArchetype() {
    const ref = archDraft.trim();
    if (!ref) return;
    const res = await api('set_archetype', { ref_id: obj.ref_id, archetype_ref: ref });
    if (res.ok) { archDraft = ''; onchanged(); }
    else showFlash(res.error || 'Could not set the archetype', { tone: 'danger' });
  }
  async function detachObject() {
    const res = await api('detach_object', { ref_id: obj.ref_id });
    res.ok ? onchanged() : showFlash(res.error || 'Could not detach the object', { tone: 'danger' });
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
  <div class="pp" class:is-locked={locked}>
    {#if locked}
      <div class="lock-banner" role="note">
        <span class="lock-ico" aria-hidden="true">🔒</span>
        <div>
          <strong>Locked — file-authoritative</strong>
          <span class="lock-sub">This object is defined in a source file (<code>system:locked</code>). Edit the file and run <code>@reload-world</code>; changes made here would not be saved.</span>
        </div>
      </div>
    {/if}
    <section>
      <h3>Identity</h3>
      <label class="fl">Title
        <input type="text" class="fi" class:inh-field={inheritedTitle} value={obj.title || ''} placeholder={inheritedTitle} onchange={setTitle} disabled={locked} />
      </label>
      {#if inheritedTitle}<div class="inh-note">inherited from {obj.archetype?.title || obj.archetype_ref} — type to override</div>{/if}
      <label class="fl">Description
        <textarea class="ft" class:inh-field={inheritedDesc} rows="4" bind:value={descDraft} placeholder={inheritedDesc} onchange={setDescription} disabled={locked}></textarea>
      </label>
      {#if inheritedDesc}<div class="inh-note">inherited from {obj.archetype?.title || obj.archetype_ref} — type to override</div>{/if}

      {#if !isExit}
        <!-- The description is prose a player reads; show it as they will, live,
             rendered through the same BBCode the game uses. This is the one
             place the "text world" is the point, not a form field. -->
        <div class="pview">
          <div class="pv-cap">Player's view</div>
          <div class="pv-screen">
            <div class="pv-name">{obj.title || obj.resolved_title || obj.key}</div>
            {#if descDraft.trim()}
              <div class="pv-desc">{@html bbcodeToHtml(descDraft)}</div>
            {:else if inheritedDesc}
              <div class="pv-desc">{@html bbcodeToHtml(inheritedDesc)}</div>
            {:else}
              <div class="pv-none">No description yet — a player would see only the name.</div>
            {/if}
          </div>
        </div>
      {/if}
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
          <input type="text" class="fi" bind:value={exitDir} placeholder="north" disabled={locked} />
        </label>
        <label class="fl">Target room
          <input type="text" class="fi" bind:value={exitTarget} placeholder="#12" disabled={locked} />
        </label>
        <div class="row">
          <span class="none">from {obj.location_ref || '(unplaced)'}</span>
          <span style="flex:1"></span>
          <Button size="sm" onclick={saveExit} label="Save exit" disabled={locked} />
        </div>
      </section>
    {:else if !isRoom}
      <section>
        <h3>Location</h3>
        <div class="none">in {obj.location_ref || '(nowhere)'}</div>
        <div class="row">
          <input class="mi" placeholder="move to a room ref, e.g. #12" bind:value={moveDest} onkeydown={(e) => e.key === 'Enter' && doMove()} disabled={locked} />
          <Button size="sm" onclick={doMove} label="Move" disabled={locked} />
        </div>
      </section>
    {/if}

    {#if !isExit}
      <section>
        <h3>Archetype</h3>
        <p class="hint">Delegate to another object to inherit its title, attributes, tags, and hooks. This object overrides only what it sets itself.</p>
        {#if obj.archetype}
          <div class="arch-cur">
            <span class="arch-ico">▸</span>
            <span class="arch-name">{obj.archetype.title || obj.archetype.ref_id}</span>
            <span class="arch-ref">{obj.archetype.ref_id}</span>
            <span style="flex:1"></span>
            <Tooltip text="Copy inherited values down onto this object and stop delegating">
              <Button size="sm" onclick={detachObject} label="Detach" disabled={locked} />
            </Tooltip>
          </div>
        {:else}
          <div class="none">Not an instance — defines everything itself.</div>
        {/if}
        <div class="row">
          <input class="mi" placeholder="delegate to a ref, e.g. #7" bind:value={archDraft} onkeydown={(e) => e.key === 'Enter' && setArchetype()} disabled={locked} />
          <Button size="sm" onclick={setArchetype} label={obj.archetype ? 'Change' : 'Set'} disabled={locked} />
        </div>
        {#if obj.instance_count > 0}
          <div class="inst">{obj.instance_count} object{obj.instance_count === 1 ? '' : 's'} delegate to this one.</div>
        {/if}
      </section>
    {/if}

    <section>
      <h3>Tags</h3>
      <p class="hint">A <code>category:key</code> label — used by locks, queries, and behavior (e.g. <code>system:hidden</code>).</p>
      <div class="tags">
        {#each ownTagList as tag}
          <span class="tag">{tag}<Tooltip text="Remove tag"><button aria-label="Remove tag" onclick={() => removeTag(tag)} disabled={locked}>×</button></Tooltip></span>
        {/each}
        {#each inheritedTags as t}
          <Tooltip text={`Inherited from ${t.source} — remove it on the archetype`}><span class="tag inh-tag">{t.tag}<span class="src">{t.source}</span></span></Tooltip>
        {/each}
        {#if !ownTagList.length && !inheritedTags.length}<span class="none">no tags</span>{/if}
      </div>
      <div class="row">
        <input class="mi" placeholder="category:key" bind:value={newTag} onkeydown={(e) => e.key === 'Enter' && addTag()} disabled={locked} />
        <Button size="sm" onclick={addTag} label="Add" disabled={locked} />
      </div>
    </section>

    <section>
      <h3>Aliases</h3>
      <div class="tags">
        {#each obj.aliases || [] as a}
          <span class="tag">{a}<Tooltip text="Remove alias"><button aria-label="Remove alias" onclick={() => removeAlias(a)} disabled={locked}>×</button></Tooltip></span>
        {/each}
        {#if !(obj.aliases || []).length}<span class="none">no aliases</span>{/if}
      </div>
      <div class="row">
        <input class="mi" placeholder={isExit ? 'alt direction (e.g. n)' : 'name people can use'} bind:value={newAlias} onkeydown={(e) => e.key === 'Enter' && addAlias()} disabled={locked} />
        <Button size="sm" onclick={addAlias} label="Add" disabled={locked} />
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
            {@const ov = overridesInherited(key)}
            <tr>
              <td class="ak">{key}{#if ov}<Tooltip text="Overrides an inherited value"><span class="ov">↑</span></Tooltip>{/if}</td>
              {#if editingAttr === key}
                <td class="av"><textarea bind:value={editValue} onkeydown={(e) => attrKeydown(e, key)}></textarea></td>
                <td class="aa"><button onclick={() => saveAttr(key)}>Save</button></td>
              {:else}
                <td class="av" ondblclick={() => startEdit(key)} title={locked ? 'Locked — edit the source file' : 'Double-click to edit'}>{disp}</td>
                <td class="aa">
                  <button onclick={() => startEdit(key)} disabled={locked}>Edit</button>
                  {#if ov}
                    <Tooltip text="Revert to the inherited value"><button class="del" onclick={() => deleteAttr(key)} disabled={locked}>Revert</button></Tooltip>
                  {:else}
                    <Tooltip text="Delete attribute"><button class="del" onclick={() => deleteAttr(key)} disabled={locked}>Del</button></Tooltip>
                  {/if}
                </td>
              {/if}
            </tr>
          {/each}
          {#if !userAttrs.length}<tr><td colspan="3" class="none pad">No attributes</td></tr>{/if}
        </tbody>
      </table>

      {#if inheritedAttrs.length}
        <div class="inh-h">Inherited</div>
        <table class="attrs">
          <tbody>
            {#each inheritedAttrs as key}
              {@const r = resolved[key]}
              {@const disp = typeof r.value === 'object' ? JSON.stringify(r.value) : String(r.value)}
              <tr class="inh">
                <td class="ak">{key}</td>
                <td class="av">{disp}</td>
                <td class="aa">
                  <span class="src">{r.source}</span>
                  <Tooltip text="Copy this value down as an own attribute you can edit"><button onclick={() => overrideAttr(key)} disabled={locked}>Override</button></Tooltip>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
      <div class="row">
        <input class="mi kw" placeholder="key" bind:value={newAttrKey} disabled={locked} />
        <input class="mi" placeholder="value (JSON or string)" bind:value={newAttrValue} onkeydown={(e) => e.key === 'Enter' && addAttr()} disabled={locked} />
        <Button size="sm" onclick={addAttr} label="Add" disabled={locked} />
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
                    <td class="av" ondblclick={() => startEdit(key)} title={locked ? 'Locked — edit the source file' : 'Double-click to edit'}>{disp}</td>
                    <td class="aa">
                      <button onclick={() => startEdit(key)} disabled={locked}>Edit</button>
                      <Tooltip text="Delete attribute"><button class="del" onclick={() => deleteAttr(key)} disabled={locked}>Del</button></Tooltip>
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

    {#if !locked}
      <section class="danger">
        {#if confirmDelete}
          <span class="confirm">Delete {obj.key} permanently?</span>
          <Button size="sm" tone="critical" onclick={doDelete} label="Delete" />
          <Button size="sm" onclick={() => (confirmDelete = false)} label="Cancel" />
        {:else}
          <button class="del-link" onclick={() => (confirmDelete = true)}>Delete {obj.kind}</button>
        {/if}
      </section>
    {/if}
  </div>
{/if}

<style>
  .pp { display: flex; flex-direction: column; gap: 12px; padding: 12px; }
  /* Locked: a prominent banner, plus disabled controls read as "read-only",
     not "broken". The banner explains where the source of truth actually is. */
  .lock-banner { display: flex; gap: 10px; align-items: flex-start; background: color-mix(in srgb, var(--accent-amber, #c9956b) 12%, var(--bg-surface, #17140f)); border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 40%, transparent); border-radius: 10px; padding: 10px 12px; }
  .lock-ico { font-size: 15px; line-height: 1.3; }
  .lock-banner strong { display: block; font-size: 12.5px; color: var(--accent-amber, #c9956b); }
  .lock-sub { display: block; margin-top: 2px; font-size: var(--fs-meta); line-height: 1.5; color: var(--text-secondary, #b6a888); }
  .lock-sub code { font-family: var(--font-mono, ui-monospace, monospace); color: var(--text-primary, #ece0c8); }
  /* Disabled controls: dim them so the whole panel reads as read-only. */
  .is-locked .fi:disabled, .is-locked .ft:disabled, .is-locked .mi:disabled { opacity: 0.55; cursor: not-allowed; }
  .is-locked .aa button:disabled, .is-locked .tag button:disabled { opacity: 0.4; cursor: not-allowed; }
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
  /* Player's-view preview: a recessed "screen" (darker than the card, no
     border) so it reads as the game window, not a nested card. Renders the
     description through the same BBCode classes the live output uses. */
  .pview { display: flex; flex-direction: column; gap: 5px; margin-top: 2px; }
  .pv-cap { font-size: var(--fs-label); font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-muted, #8c8378); }
  .pv-screen { background: var(--bg-primary, #0e0c0a); border-radius: 8px; padding: 14px 16px; }
  .pv-name { color: var(--accent-amber, #c9956b); font-weight: 700; font-size: 14px; margin-bottom: 7px; }
  .pv-desc { color: var(--text-primary, #ece0c8); font-size: 13px; line-height: 1.65; max-width: 66ch; white-space: pre-wrap; }
  /* Commands render as they look in play, but they aren't clickable in a
     preview — don't imply they are. */
  .pv-desc :global(.cmd) { cursor: default; }
  .pv-none { color: var(--text-muted, #8c8378); font-style: italic; font-size: 12.5px; }
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
  /* Archetype delegation */
  .arch-cur { display: flex; align-items: center; gap: 6px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12.5px; }
  .arch-ico { color: var(--accent-amber, #c9956b); }
  .arch-name { color: var(--text-primary, #ece0c8); }
  .arch-ref { color: var(--text-muted, #8c8378); font-size: 11px; }
  .inst { font-size: var(--fs-meta); font-style: italic; color: var(--text-muted, #8c8378); }
  /* Inherited title/description: the placeholder shows the effective value the
     archetype supplies, tinted amber so it reads as a real inherited value
     rather than a greyed-out hint. */
  .inh-field::placeholder { color: color-mix(in srgb, var(--accent-amber, #c9956b) 60%, transparent); font-style: italic; opacity: 1; }
  .inh-note { font-size: var(--fs-meta); font-style: italic; color: var(--text-muted, #8c8378); margin-top: -2px; }
  /* Inherited tags read muted and carry their source ref; they can't be
     removed here (they live on the archetype). */
  .tag.inh-tag { opacity: 0.7; }
  .tag.inh-tag .src { color: var(--text-muted, #8c8378); font-size: 10px; margin-left: 2px; }
  /* Inherited attributes read muted; own values that shadow them get a ↑ mark. */
  .inh-h { margin-top: 8px; font-size: var(--fs-label); font-weight: 700; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #8c8378); }
  tr.inh .ak, tr.inh .av { opacity: 0.7; }
  .src { color: var(--text-muted, #8c8378); font-size: 11px; margin-right: 6px; }
  .ov { color: var(--accent-amber, #c9956b); margin-left: 4px; cursor: help; }
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
