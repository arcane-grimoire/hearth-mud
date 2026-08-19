<script>
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
    if (res.ok) titleDirty = false;
    else alert(res.error || 'Failed to save');
  }

  async function saveDescription() {
    const res = await api('set_description', { ref_id: obj.ref_id, description });
    if (res.ok) descDirty = false;
    else alert(res.error || 'Failed to save');
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
    } else {
      alert(res.error || 'Failed to save');
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
    } else {
      alert(res.error || 'Failed to remove');
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
    <button class="back-btn" onclick={onclose}>&larr;</button>
    {#if obj}
      <span class="editor-title">{obj.title || obj.key}</span>
      <span class="editor-ref">{obj.ref_id}</span>
      <span class="editor-kind">{obj.kind}</span>
    {/if}
  </div>

  {#if loading}
    <div class="editor-body"><span class="muted">Loading...</span></div>
  {:else if error}
    <div class="editor-body"><span class="error">{error}</span></div>
  {:else if obj}
    <div class="editor-body">
      <div class="field">
        <label>Title</label>
        <div class="field-row">
          <input
            type="text"
            value={title}
            oninput={(e) => { title = e.target.value; titleDirty = title !== (obj.title || obj.key || ''); }}
          />
          {#if titleDirty}
            <button class="save-btn" onclick={saveTitle}>Save</button>
          {/if}
        </div>
      </div>

      <div class="field">
        <label>Description</label>
        <div class="field-row">
          <textarea
            rows="3"
            value={description}
            oninput={(e) => { description = e.target.value; descDirty = description !== (obj.description || ''); }}
          ></textarea>
          {#if descDirty}
            <button class="save-btn" onclick={saveDescription}>Save</button>
          {/if}
        </div>
      </div>

      <div class="programs-header">
        <label>Programs</label>
        <div class="add-program">
          <input
            type="text"
            bind:value={newHook}
            placeholder="hook name..."
            onkeydown={(e) => e.key === 'Enter' && addProgram()}
          />
          <button onclick={addProgram}>+</button>
        </div>
      </div>

      {#each programs as prog, idx}
        <div class="program">
          <div class="program-header">
            <span class="hook-name">{prog.hook}</span>
            <div class="program-actions">
              {#if prog.dirty}
                <button class="save-btn" onclick={() => saveProgram(idx)} disabled={saving[prog.hook]}>
                  {saving[prog.hook] ? '...' : 'Save'}
                </button>
                <button class="revert-btn" onclick={() => revertProgram(idx)}>Revert</button>
              {/if}
              <button class="delete-btn" onclick={() => removeProgram(idx)}>Delete</button>
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
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .editor-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .back-btn {
    background: none;
    border: 1px solid var(--border);
    color: var(--text-secondary);
    width: 26px;
    height: 26px;
    border-radius: var(--radius);
    cursor: pointer;
    font-size: 14px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .back-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-focus);
  }

  .editor-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .editor-ref {
    font-size: 11px;
    color: var(--text-muted);
  }

  .editor-kind {
    font-size: 10px;
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

  .field label {
    display: block;
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    margin-bottom: 4px;
  }

  .field-row {
    display: flex;
    gap: 4px;
  }

  .field input, .field textarea {
    flex: 1;
    background: var(--bg-input);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 6px 8px;
    font-family: var(--font-mono);
    font-size: 12px;
    outline: none;
    resize: vertical;
  }

  .field input:focus, .field textarea:focus {
    border-color: var(--border-focus);
  }

  .programs-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .programs-header label {
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
  }

  .add-program {
    display: flex;
    gap: 3px;
  }

  .add-program input {
    width: 110px;
    background: var(--bg-input);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 3px 6px;
    font-family: var(--font-mono);
    font-size: 11px;
    outline: none;
  }

  .add-program button {
    background: var(--bg-elevated);
    color: var(--accent);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 3px 8px;
    cursor: pointer;
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .program {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }

  .program-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 8px;
    background: var(--bg-elevated);
    border-bottom: 1px solid var(--border);
  }

  .hook-name {
    font-size: 12px;
    font-weight: 600;
    color: var(--cyan);
  }

  .program-actions {
    display: flex;
    gap: 3px;
  }

  .code {
    width: 100%;
    background: var(--bg-input);
    color: var(--text-primary);
    border: none;
    padding: 8px;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
    outline: none;
    resize: vertical;
    tab-size: 2;
  }

  button {
    font-family: var(--font-mono);
  }

  .save-btn {
    background: var(--accent-dim);
    color: var(--accent);
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    padding: 2px 8px;
    cursor: pointer;
    font-size: 11px;
  }

  .save-btn:hover { opacity: 0.9; }
  .save-btn:disabled { opacity: 0.5; cursor: default; }

  .revert-btn, .delete-btn {
    background: var(--bg-elevated);
    color: var(--text-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 2px 8px;
    cursor: pointer;
    font-size: 11px;
  }

  .delete-btn:hover {
    color: var(--red);
    border-color: var(--red);
  }

  .revert-btn:hover {
    color: var(--text-primary);
  }

  .no-programs {
    font-size: 12px;
    color: var(--text-muted);
    font-style: italic;
    padding: 8px 0;
  }

  .muted { color: var(--text-muted); }
  .error { color: var(--red); }
</style>
