<script>
  import { Typeahead, Tooltip, showFlash } from '@kenn-io/kit-ui';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import FileCodeIcon from '@lucide/svelte/icons/file-code';
  import PackageIcon from '@lucide/svelte/icons/package';
  import TrashIcon from '@lucide/svelte/icons/trash-2';
  import PlayIcon from '@lucide/svelte/icons/play';
  import CheckIcon from '@lucide/svelte/icons/check';
  import XIcon from '@lucide/svelte/icons/x';
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
  // A `system:locked` object is file-authoritative: its script is read-only to
  // in-game authoring. Hooks still open (view-only in the editor), but the
  // add/clear controls are hidden. See PropertiesPanel + docs/plans/archetypes.md.
  const locked = $derived(obj?.locked === true);

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

  // Run the `test_*` functions embedded in THIS object's script (co-located
  // tests — see docs/commands.md). `ctx.this` is bound to the object, so its
  // tests exercise itself. Results render inline below the hook list.
  let testResult = $state(null); // { passed, failed, tests, error }
  let testing = $state(false);
  async function runTests() {
    testing = true;
    const res = await api('run_tests', { ref_id: obj.ref_id });
    testing = false;
    if (res?.ok) {
      const file = (res.data?.files || [])[0] || { tests: [] };
      testResult = {
        passed: res.data?.passed ?? 0,
        failed: res.data?.failed ?? 0,
        tests: file.tests || [],
        error: file.error || null,
      };
    } else {
      testResult = null;
      showFlash(res?.error || 'Test run failed', { tone: 'danger' });
    }
  }
  // Drop stale results when the selected object changes.
  $effect(() => { obj?.ref_id; testResult = null; });

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
        <Tooltip text="Run the test_* functions in this object's script">
          <button class="run-tests" onclick={runTests} disabled={testing}>
            <PlayIcon size={12} /> {testing ? 'Running…' : 'Test'}
          </button>
        </Tooltip>
      {/if}
      {#if locked}
        <span class="lock-pill" title="Defined in a source file — edit the file and @reload-world">🔒 read-only</span>
      {:else if obj?.has_script}
        {#if confirmClear}
          <button class="confirm" onclick={clearScript}>Delete script?</button>
          <button class="cancel" onclick={() => (confirmClear = false)}>×</button>
        {:else}
          <Tooltip text="Remove this object's whole script"><button class="rm" aria-label="Remove script" onclick={() => (confirmClear = true)}><TrashIcon size={13} /></button></Tooltip>
        {/if}
      {/if}
    </div>
    {#if locked}
      <p class="hint">This object is <code>system:locked</code> — file-authoritative. Its script is read-only here; edit the source file and run <code>@reload-world</code>. Open a hook to view it.</p>
    {:else}
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
    {/if}

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

    {#if testResult}
      <div class="tests">
        <div class="tsum">
          <span class="pill pass">{testResult.passed} passed</span>
          {#if testResult.failed}<span class="pill fail">{testResult.failed} failed</span>{/if}
          {#if !testResult.tests.length && !testResult.error}
            <span class="tnone">no <code>test_*</code> functions in this script</span>
          {/if}
        </div>
        {#if testResult.error}<pre class="terr">{testResult.error}</pre>{/if}
        {#each testResult.tests as t (t.name)}
          <div class="trow" class:fail={!t.passed}>
            <span class="ti" data-ok={t.passed}>{#if t.passed}<CheckIcon size={11} />{:else}<XIcon size={11} />{/if}</span>
            <span class="tn">{t.name}</span>
            {#if !t.passed && t.error}<pre class="terr">{t.error}</pre>{/if}
          </div>
        {/each}
      </div>
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
      {#if !locked}
        <div class="add lib-add">
          <PackageIcon size={13} />
          <input class="lib-in" placeholder="new module name (e.g. combat)" bind:value={newLib}
            onkeydown={(e) => e.key === 'Enter' && addLib()} />
          <button class="lib-go" disabled={!newLib.trim()} onclick={addLib}>Add</button>
        </div>
      {/if}
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
  .lock-pill { flex: none; font-size: 11px; color: var(--accent-amber, #c9956b); background: color-mix(in srgb, var(--accent-amber, #c9956b) 14%, transparent); border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 35%, transparent); border-radius: 999px; padding: 1px 8px; }

  .run-tests { display: inline-flex; align-items: center; gap: 4px; flex: none; background: none; border: 1px solid var(--border-default, #332c22); color: var(--text-secondary, #b6a888); border-radius: 6px; padding: 3px 9px; cursor: pointer; font: inherit; font-size: 11.5px; }
  .run-tests:hover:not(:disabled) { color: var(--accent-green, #8fb877); border-color: color-mix(in srgb, var(--accent-green, #8fb877) 45%, transparent); }
  .run-tests:disabled { opacity: 0.55; cursor: default; }
  .run-tests :global(svg) { flex: none; }

  .tests { display: flex; flex-direction: column; gap: 4px; margin-top: 2px; padding: 8px 9px; background: var(--bg-inset, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 8px; }
  .tsum { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .pill { font-size: 11px; border-radius: 999px; padding: 1px 8px; border: 1px solid transparent; }
  .pill.pass { color: var(--accent-green, #8fb877); background: color-mix(in srgb, var(--accent-green, #8fb877) 14%, transparent); border-color: color-mix(in srgb, var(--accent-green, #8fb877) 35%, transparent); }
  .pill.fail { color: var(--accent-red, #e06c75); background: color-mix(in srgb, var(--accent-red, #e06c75) 14%, transparent); border-color: color-mix(in srgb, var(--accent-red, #e06c75) 35%, transparent); }
  .tnone { font-size: 11.5px; color: var(--text-muted, #8c8378); font-style: italic; }
  .tnone code { font-family: var(--font-mono, ui-monospace, monospace); font-style: normal; }
  .trow { display: flex; align-items: center; gap: 6px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-secondary, #b6a888); flex-wrap: wrap; }
  .trow .ti { line-height: 0; flex: none; }
  .trow .ti[data-ok="true"] :global(svg) { color: var(--accent-green, #8fb877); }
  .trow .ti[data-ok="false"] :global(svg) { color: var(--accent-red, #e06c75); }
  .trow.fail .tn { color: var(--accent-red, #e06c75); }
  .trow .tn { flex: 1 1 auto; min-width: 0; }
  .terr { flex-basis: 100%; margin: 2px 0 4px 17px; padding: 6px 8px; background: color-mix(in srgb, var(--accent-red, #e06c75) 8%, transparent); border-radius: 5px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--text-secondary, #b6a888); white-space: pre-wrap; overflow-x: auto; }
</style>
