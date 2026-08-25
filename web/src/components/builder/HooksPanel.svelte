<script>
  import { Typeahead, Tooltip, showFlash } from '@kenn-io/kit-ui';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import FileCodeIcon from '@lucide/svelte/icons/file-code';
  import PackageIcon from '@lucide/svelte/icons/package';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import { api } from '../../lib/api.js';
  import { loadHooks, hookOptions, isValidHookName } from '../../lib/hooks.js';

  // An object has ONE script; its hooks are the top-level functions defined in
  // it, sharing scope. This panel is the launcher into that single script
  // editor: it lists the hooks the script defines (open one to jump there),
  // lets you add a new hook (seeds a `function <name>(...)` stub in the script),
  // or open the whole script. `Kind::Code` objects also list their require()able
  // lib modules here. The editor itself (CodeOverlay) does the editing.
  let {
    obj = null,
    activeHook = null,
    onopen = () => {},      // (hook|null) → open the object script, optionally focused on a hook
    onopenlib = () => {},   // (name) → open a lib module editor
    onchanged = () => {},
  } = $props();

  // resolved_hooks carries every hook the object responds to (own + inherited)
  // with the ref each resolves from ("own", or an ancestor ref). Fall back to
  // the plain own-hook list when the examine payload predates that field.
  const resolvedHooks = $derived(obj?.resolved_hooks || []);
  const hooks = $derived(
    resolvedHooks.length
      ? resolvedHooks.filter((h) => h.source === 'own').map((h) => h.hook).sort()
      : [...(obj?.hooks || [])].sort(),
  );
  const inheritedHooks = $derived(
    resolvedHooks.filter((h) => h.source !== 'own').sort((a, b) => a.hook.localeCompare(b.hook)),
  );
  const libs = $derived([...(obj?.libs || [])].sort());
  const isCode = $derived(obj?.kind === 'code' || libs.length > 0);

  let hooksData = $state(null);
  let hookErr = $state('');
  const hookOpts = $derived(hookOptions(hooksData));
  loadHooks().then((d) => (hooksData = d));

  // Validate the picked/typed name against the engine's vocabulary before it
  // reaches the editor. Returning false keeps the Typeahead open with the error.
  function pickHook(v) {
    if (!isValidHookName(v, hooksData)) {
      hookErr = `“${v}” isn't a valid hook — use a known hook or an on_/cmd_ name`;
      return false;
    }
    hookErr = '';
    onopen(v); // opens the script; if the hook isn't defined yet, a stub is seeded
  }

  let newLib = $state('');
  function addLib() {
    const n = newLib.trim();
    if (!n) return;
    if (!/^[a-zA-Z_]\w*$/.test(n)) { showFlash('Module name must be a bare identifier', { tone: 'danger' }); return; }
    newLib = '';
    onopenlib(n); // opens a fresh module editor; Save creates it
  }

  let confirmClear = $state(false);
  async function clearScript() {
    const res = await api('clear_script', { ref_id: obj.ref_id });
    confirmClear = false;
    if (res?.ok) { showFlash('Script removed', { tone: 'success' }); onchanged(); }
    else showFlash(res?.error || 'Failed', { tone: 'danger' });
  }
</script>

<div class="hp">
  <section class="sec">
    <div class="sec-h">
      <span class="sec-lbl">Script</span>
      {#if obj?.has_script}
        {#if confirmClear}
          <button class="confirm" onclick={clearScript}>Delete script?</button>
          <button class="cancel" onclick={() => (confirmClear = false)}>×</button>
        {:else}
          <Tooltip text="Remove this object's whole script"><button class="rm" aria-label="Remove script" onclick={() => (confirmClear = true)}><TrashIcon size={13} /></button></Tooltip>
        {/if}
      {/if}
    </div>
    <p class="hint">One Luau script per object. Its hooks are the top-level functions it defines (<code>on_enter</code>, <code>can_get</code>, <code>cmd_talk</code>…), sharing scope for helpers and constants.</p>

    <div class="add">
      <Typeahead
        options={hookOpts}
        fallbackLabel="new hook"
        placeholder="add a hook — on_enter, cmd_talk…"
        emptyLabel="No matching hook"
        allowCustom
        customLabel={'Use "{query}"'}
        onselect={pickHook}
      />
    </div>
    {#if hookErr}<div class="err">{hookErr}</div>{/if}

    {#if hooks.length}
      <div class="list">
        {#each hooks as h}
          <button class="row" class:active={h === activeHook} onclick={() => onopen(h)}>
            <FileCodeIcon size={13} />
            <span class="name">{h}</span>
            <ChevronRight size={13} class="chev" />
          </button>
        {/each}
      </div>
      <button class="open-all" onclick={() => onopen(null)}>Open full script →</button>
    {:else if obj?.has_script}
      <button class="open-all" onclick={() => onopen(null)}>Open script — it defines no recognized hooks yet →</button>
    {:else}
      <div class="none">No script yet. Add a hook above to start one.</div>
    {/if}

    {#if inheritedHooks.length}
      <div class="inh-lbl">Inherited<span class="inh-hint">click to override here</span></div>
      <div class="list">
        {#each inheritedHooks as h}
          <button class="row inh" title={`Defined by ${h.source} — click to override it on this object`} onclick={() => onopen(h.hook)}>
            <FileCodeIcon size={13} />
            <span class="name">{h.hook}</span>
            <span class="src">{h.source}</span>
          </button>
        {/each}
      </div>
    {/if}
  </section>

  {#if isCode}
    <section class="sec">
      <div class="sec-h"><span class="sec-lbl">Modules</span></div>
      <p class="hint">Reusable libraries loaded elsewhere with <code>require("name")</code>. Each is its own chunk that returns a table.</p>
      <div class="add lib-add">
        <PackageIcon size={13} />
        <input class="lib-in" placeholder="new module name (e.g. combat)" bind:value={newLib}
          onkeydown={(e) => e.key === 'Enter' && addLib()} />
        <button class="lib-go" disabled={!newLib.trim()} onclick={addLib}>Add</button>
      </div>
      {#if libs.length}
        <div class="list">
          {#each libs as m}
            <button class="row" onclick={() => onopenlib(m)}>
              <PackageIcon size={13} />
              <span class="name">{m}</span>
              <ChevronRight size={13} class="chev" />
            </button>
          {/each}
        </div>
      {:else}
        <div class="none">No modules yet.</div>
      {/if}
    </section>
  {/if}
</div>

<style>
  .hp { display: flex; flex-direction: column; gap: 16px; padding: 14px; }
  .sec { display: flex; flex-direction: column; gap: 9px; }
  .sec-h { display: flex; align-items: center; gap: 8px; }
  .sec-lbl { flex: 1; font-size: var(--fs-label); font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-muted, #8c8378); }
  .hint { margin: 0; font-size: var(--fs-meta); line-height: 1.5; color: var(--text-muted, #8c8378); }
  .hint code { font-family: var(--font-mono, ui-monospace, monospace); color: var(--text-secondary, #b6a888); }
  .add :global(.kit-typeahead) { width: 100%; }

  .hp :global(.kit-typeahead__panel) { min-width: 360px; }
  .hp :global(.kit-typeahead__option-label) { flex: none; }
  .hp :global(.kit-typeahead__option-meta) { flex: 1 1 auto; min-width: 0; margin-left: 12px; color: var(--text-secondary); }
  .err { color: var(--accent-red, #d07a5a); font-size: 11px; margin-top: -4px; }

  .list { display: flex; flex-direction: column; gap: 2px; }
  .row {
    display: flex; align-items: center; gap: 7px;
    background: none; border: 1px solid transparent; border-radius: 6px;
    padding: 5px 7px; color: var(--text-secondary, #b6a888); cursor: pointer;
    font-family: var(--font-mono, ui-monospace, monospace); font-size: 12.5px; text-align: left; width: 100%;
  }
  .row:hover { background: var(--bg-inset, #12100c); color: var(--text-primary, #ece0c8); }
  .row.active { background: color-mix(in srgb, var(--accent-amber, #c9956b) 14%, transparent); color: var(--accent-amber, #c9956b); border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 40%, transparent); }
  .row :global(svg) { color: var(--accent-blue, #6ea3d0); flex: none; }
  .row.active :global(svg) { color: var(--accent-amber, #c9956b); }
  .row .name { flex: 1; }
  .row :global(.chev) { color: var(--text-muted, #8c8378); flex: none; }

  .open-all { align-self: flex-start; background: none; border: none; color: var(--accent-amber, #c9956b); cursor: pointer; font: inherit; font-size: 12px; padding: 2px 0; }
  .open-all:hover { text-decoration: underline; }

  /* Hooks the object inherits from its archetype — muted, tagged with their
     source ref. Clicking seeds a stub here, overriding the inherited one. */
  .inh-lbl { display: flex; align-items: baseline; gap: 6px; margin-top: 4px; font-size: var(--fs-label); font-weight: 700; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #8c8378); }
  .inh-hint { font-weight: 400; text-transform: none; letter-spacing: 0; font-size: 11px; color: var(--text-muted, #8c8378); }
  .row.inh { color: var(--text-muted, #8c8378); }
  .row.inh .name { flex: 1; opacity: 0.85; }
  .row.inh .src { color: var(--text-muted, #8c8378); font-size: 11px; }
  .row.inh :global(svg) { color: var(--text-muted, #8c8378); opacity: 0.7; }

  .lib-add { display: flex; align-items: center; gap: 6px; }
  .lib-add :global(svg) { color: var(--accent-green, #8fb877); flex: none; }
  .lib-in { flex: 1; background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #332c22); border-radius: 6px; color: var(--text-primary, #ece0c8); padding: 5px 8px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; outline: none; }
  .lib-in:focus { border-color: var(--accent-amber, #c9956b); }
  .lib-go { background: none; border: 1px solid var(--border-default, #332c22); color: var(--text-secondary, #b6a888); border-radius: 6px; padding: 5px 10px; cursor: pointer; font: inherit; font-size: 12px; }
  .lib-go:disabled { opacity: 0.5; cursor: default; }

  .rm { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; padding: 3px; border-radius: 5px; line-height: 0; flex: none; }
  .rm:hover { color: var(--accent-red, #e06c75); background: color-mix(in srgb, var(--accent-red, #e06c75) 14%, transparent); }
  .confirm { background: none; border: none; color: var(--accent-red, #e06c75); cursor: pointer; font: inherit; font-size: 11.5px; padding: 2px 4px; flex: none; }
  .cancel { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; font-size: 14px; line-height: 1; padding: 0 4px; flex: none; }
  .none { color: var(--text-muted, #8c8378); font-style: italic; font-size: 12px; padding: 4px 2px; }
</style>
