<script>
  import { Typeahead, Tooltip, showFlash } from '@kenn-io/kit-ui';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import { api } from '../../lib/api.js';
  import { loadHooks, hookOptions, isValidHookName } from '../../lib/hooks.js';

  // An object's hooks. Clicking one opens the SAME CodeMirror editor the code
  // workspace uses — so "edit programs" is one experience, reached from here,
  // the map, or ⌘K. (The old Editor.svelte used a raw textarea; this replaces
  // that path.) The "new hook" box autocompletes against the engine's live hook
  // vocabulary (list_hooks) and lets you type a custom on_/cmd_/lib_ name.
  let { obj = null, activeHook = null, onopen = () => {}, onchanged = () => {} } = $props();

  let confirmHook = $state(null); // hook name pending delete-confirm

  async function removeHook(hook, e) {
    e?.stopPropagation();
    const res = await api('remove_program', { ref_id: obj.ref_id, hook });
    if (res.ok) { confirmHook = null; onchanged(); } else showFlash(res.error || 'Failed', { tone: 'danger' });
  }

  let hooksData = $state(null);
  let hookErr = $state('');
  const hookOpts = $derived(hookOptions(hooksData));
  const programs = $derived([...(obj?.programs || [])].sort());

  loadHooks().then((d) => (hooksData = d));

  // Validate the picked/typed name against the engine's vocabulary before it
  // reaches the editor. Returning false keeps the Typeahead open with the error
  // so a typo can be corrected without a round-trip.
  function pickHook(v) {
    if (!isValidHookName(v, hooksData)) {
      hookErr = `“${v}” isn't a valid hook — use a known hook or an on_/cmd_/lib_ name`;
      return false;
    }
    hookErr = '';
    onopen(v); // opening a not-yet-existing hook seeds a starter in the editor
  }
</script>

<div class="hp">
  <div class="add">
    <span class="add-lbl">Add a hook</span>
    <p class="hint">Luau scripts that run on events (<code>on_enter</code>, <code>can_get</code>…) or as commands (<code>cmd_…</code>). Pick one or type a name.</p>
    <Typeahead
      options={hookOpts}
      fallbackLabel="new hook"
      placeholder="on_enter, cmd_talk…"
      emptyLabel="No matching hook"
      allowCustom
      customLabel={'Use "{query}"'}
      onselect={pickHook}
    />
  </div>
  {#if hookErr}<div class="err">{hookErr}</div>{/if}

  {#if programs.length}
    <div class="list">
      {#each programs as h}
        <div class="hook" class:active={h === activeHook}>
          <button class="open" onclick={() => onopen(h)}>
            <span class="name">{h}</span>
          </button>
          {#if confirmHook === h}
            <button class="confirm" onclick={(e) => removeHook(h, e)}>Delete?</button>
            <button class="cancel" onclick={(e) => { e.stopPropagation(); confirmHook = null; }}>×</button>
          {:else}
            <Tooltip text="Remove hook"><button class="rm" aria-label="Remove hook" onclick={(e) => { e.stopPropagation(); confirmHook = h; }}><TrashIcon size={13} /></button></Tooltip>
          {/if}
          <ChevronRight size={13} class="chev" />
        </div>
      {/each}
    </div>
  {:else}
    <div class="none">No programs yet. Type a hook name above to start one.</div>
  {/if}
</div>

<style>
  .hp { display: flex; flex-direction: column; gap: 10px; padding: 14px; }
  .add { display: flex; flex-direction: column; gap: 5px; }
  .add-lbl { font-size: var(--fs-label); font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-muted, #8c8378); }
  .hint { margin: 0; font-size: var(--fs-meta); line-height: 1.5; color: var(--text-muted, #8c8378); }
  .hint code { font-family: var(--font-mono, ui-monospace, monospace); color: var(--text-secondary, #b6a888); }
  .add :global(.kit-typeahead) { width: 100%; }

  /* The hook picker's default row layout lets the description (meta) hold its
     width and crush the hook NAME to an ellipsis. Flip it: the name keeps its
     full width, the description shrinks/ellipsizes instead. Also give the panel
     room and brighten the description so definitions are readable. */
  .hp :global(.kit-typeahead__panel) { min-width: 360px; }
  .hp :global(.kit-typeahead__option-label) { flex: none; }
  .hp :global(.kit-typeahead__option-meta) { flex: 1 1 auto; min-width: 0; margin-left: 12px; color: var(--text-secondary); }
  .err { color: var(--accent-red, #d07a5a); font-size: 11px; margin-top: -4px; }
  .list { display: flex; flex-direction: column; gap: 2px; }
  .hook {
    display: flex; align-items: center; gap: 4px;
    background: none; border: 1px solid transparent; border-radius: 6px;
    padding: 3px 7px 3px 2px; color: var(--text-secondary, #b6a888);
    font-family: var(--font-mono, ui-monospace, monospace); font-size: 12.5px;
  }
  .hook:hover { background: var(--bg-inset, #12100c); color: var(--text-primary, #ece0c8); }
  .hook.active { background: color-mix(in srgb, var(--accent-amber, #c9956b) 14%, transparent); color: var(--accent-amber, #c9956b); border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 40%, transparent); }
  .hook .open { flex: 1; text-align: left; background: none; border: none; cursor: pointer; color: inherit; font: inherit; padding: 4px 7px; }
  .hook :global(.chev) { color: var(--text-muted, #8c8378); flex: none; }
  .rm { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; padding: 3px; border-radius: 5px; line-height: 0; flex: none; }
  .rm:hover { color: var(--accent-red, #e06c75); background: color-mix(in srgb, var(--accent-red, #e06c75) 14%, transparent); }
  .confirm { background: none; border: none; color: var(--accent-red, #e06c75); cursor: pointer; font: inherit; font-size: 11.5px; padding: 2px 4px; flex: none; }
  .cancel { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; font-size: 14px; line-height: 1; padding: 0 4px; flex: none; }
  .none { color: var(--text-muted, #8c8378); font-style: italic; font-size: 12px; padding: 6px 2px; }
</style>
