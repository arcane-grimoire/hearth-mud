<script>
  import { Typeahead } from '@kenn-io/kit-ui';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import { loadHooks, hookOptions, isValidHookName } from '../../lib/hooks.js';

  // An object's hooks. Clicking one opens the SAME CodeMirror editor the code
  // workspace uses — so "edit programs" is one experience, reached from here,
  // the map, or ⌘K. (The old Editor.svelte used a raw textarea; this replaces
  // that path.) The "new hook" box autocompletes against the engine's live hook
  // vocabulary (list_hooks) and lets you type a custom on_/cmd_/lib_ name.
  let { obj = null, activeHook = null, onopen = () => {} } = $props();

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
        <button class="hook" class:active={h === activeHook} onclick={() => onopen(h)}>
          <span class="name">{h}</span>
          <ChevronRight size={13} class="chev" />
        </button>
      {/each}
    </div>
  {:else}
    <div class="none">No programs yet. Type a hook name above to start one.</div>
  {/if}
</div>

<style>
  .hp { display: flex; flex-direction: column; gap: 10px; padding: 14px; }
  .add { display: flex; gap: 6px; }
  .add :global(.kit-typeahead) { flex: 1; }
  .err { color: var(--accent-red, #d07a5a); font-size: 11px; margin-top: -4px; }
  .list { display: flex; flex-direction: column; gap: 2px; }
  .hook {
    display: flex; align-items: center; justify-content: space-between;
    background: none; border: 1px solid transparent; border-radius: 6px; cursor: pointer;
    padding: 7px 9px; text-align: left; color: var(--text-secondary, #b6a888);
    font-family: var(--font-mono, ui-monospace, monospace); font-size: 12.5px;
  }
  .hook:hover { background: var(--bg-inset, #12100c); color: var(--text-primary, #ece0c8); }
  .hook.active { background: color-mix(in srgb, var(--accent-amber, #c9956b) 14%, transparent); color: var(--accent-amber, #c9956b); border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 40%, transparent); }
  .hook :global(.chev) { color: var(--text-muted, #8c8378); }
  .none { color: var(--text-muted, #8c8378); font-style: italic; font-size: 12px; padding: 6px 2px; }
</style>
