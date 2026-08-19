<script>
  import { Button, IconButton, TextInput, showFlash } from '@kenn-io/kit-ui';
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import Plus from '@lucide/svelte/icons/plus';
  import { api } from '../lib/api.js';

  let { entity = null, onclose = () => {} } = $props();

  let obj = $state(null);
  let programs = $state([]);
  let loading = $state(true);
  let error = $state(null);
  let title = $state('');
  let description = $state('');
  let titleDirty = $state(false);
  let descDirty = $state(false);
  let newHook = $state('');
  let saving = $state({});

  $effect(() => {
    if (entity?.ref_id) load(entity.ref_id);
  });

  async function load(refId) {
    loading = true;
    error = null;
    const res = await api('examine', { ref_id: refId });
    if (!res.ok) {
      error = res.error || 'Failed to load';
      loading = false;
      return;
    }
    obj = res.data;
    title = obj.title || obj.key || '';
    description = obj.description || '';
    titleDirty = false;
    descDirty = false;

    const progRes = await api('list_programs', { ref_id: refId });
    if (progRes.ok && progRes.data) {
      programs = progRes.data.map(p => ({
        hook: p.hook,
        source: p.source || '',
        original: p.source || '',
        dirty: false,
      }));
    } else {
      programs = [];
    }
    loading = false;
  }

  async function saveTitle() {
    const res = await api('set_title', { ref_id: obj.ref_id, title });
    if (res.ok) {
      titleDirty = false;
      showFlash('Title saved', { tone: 'success' });
    } else {
      showFlash(res.error || 'Failed to save', { tone: 'danger' });
    }
  }

  async function saveDescription() {
    const res = await api('set_description', { ref_id: obj.ref_id, description });
    if (res.ok) {
      descDirty = false;
      showFlash('Description saved', { tone: 'success' });
    } else {
      showFlash(res.error || 'Failed to save', { tone: 'danger' });
    }
  }

  async function saveProgram(idx) {
    const p = programs[idx];
    saving = { ...saving, [p.hook]: true };
    const res = await api('set_program', {
      ref_id: obj.ref_id,
      hook: p.hook,
      source: p.source,
    });
    saving = { ...saving, [p.hook]: false };
    if (res.ok) {
      programs[idx] = { ...p, original: p.source, dirty: false };
      showFlash(`${p.hook} saved`, { tone: 'success' });
    } else {
      showFlash(res.error || 'Failed to save', { tone: 'danger' });
    }
  }

  async function removeProgram(idx) {
    const p = programs[idx];
    const res = await api('remove_program', {
      ref_id: obj.ref_id,
      hook: p.hook,
    });
    if (res.ok) {
      programs = programs.filter((_, i) => i !== idx);
      showFlash(`${p.hook} removed`, { tone: 'info' });
    } else {
      showFlash(res.error || 'Failed to remove', { tone: 'danger' });
    }
  }

  function revertProgram(idx) {
    const p = programs[idx];
    programs[idx] = { ...p, source: p.original, dirty: false };
  }

  function addProgram() {
    const hook = newHook.trim();
    if (!hook) return;
    if (programs.some(p => p.hook === hook)) return;
    programs = [...programs, {
      hook,
      source: `function ${hook}(this, actor, room)\n  \nend`,
      original: '',
      dirty: true,
    }];
    newHook = '';
  }

  function updateSource(idx, value) {
    programs[idx] = {
      ...programs[idx],
      source: value,
      dirty: value !== programs[idx].original,
    };
  }
</script>

<div class="editor">
  <div class="editor-header">
    <IconButton ariaLabel="Back" size="sm" onclick={onclose}>
      <ArrowLeft size={14} />
    </IconButton>
    {#if obj}
      <span class="editor-title">{obj.title || obj.key}</span>
      <span class="editor-ref">{obj.ref_id}</span>
      <span class="editor-kind">{obj.kind}</span>
    {/if}
  </div>

  {#if loading}
    <div class="editor-body"><span class="muted">Loading...</span></div>
  {:else if error}
    <div class="editor-body"><span class="err">{error}</span></div>
  {:else if obj}
    <div class="editor-body">
      <div class="field">
        <span class="field-label">Title</span>
        <div class="field-row">
          <TextInput
            bind:value={title}
            block
            size="sm"
            oninput={(v) => { title = v; titleDirty = title !== (obj.title || obj.key || ''); }}
          />
          {#if titleDirty}
            <Button size="sm" tone="success" surface="soft" onclick={saveTitle} label="Save" />
          {/if}
        </div>
      </div>

      <div class="field">
        <span class="field-label">Description</span>
        <div class="field-row">
          <textarea
            class="desc-area"
            rows="3"
            value={description}
            oninput={(e) => { description = e.target.value; descDirty = description !== (obj.description || ''); }}
          ></textarea>
          {#if descDirty}
            <Button size="sm" tone="success" surface="soft" onclick={saveDescription} label="Save" />
          {/if}
        </div>
      </div>

      <div class="programs-header">
        <span class="field-label">Programs</span>
        <div class="add-program">
          <TextInput
            bind:value={newHook}
            size="sm"
            placeholder="hook name..."
            onkeydown={(e) => e.key === 'Enter' && addProgram()}
          />
          <IconButton ariaLabel="Add program" size="sm" onclick={addProgram}>
            <Plus size={14} />
          </IconButton>
        </div>
      </div>

      {#each programs as prog, idx}
        <div class="program">
          <div class="program-header">
            <span class="hook-name">{prog.hook}</span>
            <div class="program-actions">
              {#if prog.dirty}
                <Button
                  size="sm" tone="success" surface="soft"
                  onclick={() => saveProgram(idx)}
                  disabled={saving[prog.hook]}
                  label={saving[prog.hook] ? '...' : 'Save'}
                />
                <Button size="sm" onclick={() => revertProgram(idx)} label="Revert" />
              {/if}
              <Button size="sm" tone="danger" onclick={() => removeProgram(idx)} label="Delete" />
            </div>
          </div>
          <textarea
            class="code"
            value={prog.source}
            oninput={(e) => updateSource(idx, e.target.value)}
            spellcheck="false"
            rows={Math.max(4, prog.source.split('\n').length + 1)}
          ></textarea>
        </div>
      {/each}

      {#if programs.length === 0}
        <div class="no-programs">No programs. Add a hook to get started.</div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .editor {
    width: 340px;
    min-width: 340px;
    background: var(--bg-surface);
    border-left: 1px solid var(--border-default);
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .editor-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border-default);
    flex-shrink: 0;
  }

  .editor-title {
    font-size: var(--font-size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .editor-ref {
    font-size: var(--font-size-xs);
    color: var(--text-muted);
  }

  .editor-kind {
    font-size: var(--font-size-2xs);
    text-transform: uppercase;
    color: var(--text-muted);
    letter-spacing: 0.05em;
  }

  .editor-body {
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .field-label {
    display: block;
    font-size: var(--font-size-2xs, 10px);
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    margin-bottom: 4px;
  }

  .field-row {
    display: flex;
    gap: 4px;
    align-items: flex-start;
  }

  .desc-area {
    flex: 1;
    background: var(--bg-inset, #0d0f14);
    color: var(--text-primary);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md, 4px);
    padding: 6px 8px;
    font-family: var(--font-mono);
    font-size: var(--font-size-xs, 12px);
    outline: none;
    resize: vertical;
  }

  .desc-area:focus {
    border-color: var(--accent-blue);
  }

  .programs-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .add-program {
    display: flex;
    gap: 3px;
    align-items: center;
  }

  .program {
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md, 4px);
    overflow: hidden;
  }

  .program-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 8px;
    background: var(--bg-inset);
    border-bottom: 1px solid var(--border-default);
  }

  .hook-name {
    font-size: var(--font-size-xs, 12px);
    font-weight: 600;
    color: var(--accent-teal, #56b6c2);
  }

  .program-actions {
    display: flex;
    gap: 3px;
  }

  .code {
    width: 100%;
    background: var(--bg-inset, #0d0f14);
    color: var(--text-primary);
    border: none;
    padding: 8px;
    font-family: var(--font-mono);
    font-size: var(--font-size-xs, 12px);
    line-height: 1.5;
    outline: none;
    resize: vertical;
    tab-size: 2;
  }

  .no-programs {
    font-size: var(--font-size-xs, 12px);
    color: var(--text-muted);
    font-style: italic;
    padding: 8px 0;
  }

  .muted { color: var(--text-muted); }
  .err { color: var(--accent-red); }
</style>
