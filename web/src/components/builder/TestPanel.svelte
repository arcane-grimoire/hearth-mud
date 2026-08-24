<script>
  import { api } from '../../lib/api.js';
  import PlayIcon from '@lucide/svelte/icons/play';
  import CheckIcon from '@lucide/svelte/icons/check';
  import XIcon from '@lucide/svelte/icons/x';
  import FlaskConicalIcon from '@lucide/svelte/icons/flask-conical';

  // The Tests panel: runs every `*.test.luau` in the game via `run_tests` and
  // shows the results as a per-file tree. Each test runs against a clone of the
  // world, so nothing here touches the live game. A file that won't even
  // compile comes back with a file-level `error` — shown as one failing row.
  let files = $state([]);
  let summary = $state(null); // { passed, failed }
  let running = $state(false);
  let error = $state(null);
  let ran = $state(false);

  async function run() {
    running = true;
    error = null;
    const res = await api('run_tests');
    if (res?.ok) {
      files = res.data?.files || [];
      summary = { passed: res.data?.passed ?? 0, failed: res.data?.failed ?? 0 };
    } else {
      error = res?.error || 'Test run failed';
    }
    running = false;
    ran = true;
  }

  const fileFailed = (f) => f.error || f.tests.some((t) => !t.passed);

  $effect(() => { run(); });
</script>

<div class="tp">
  <header>
    <span class="title">Tests</span>
    {#if summary && !running}
      <span class="pill pass">{summary.passed} passed</span>
      {#if summary.failed}<span class="pill fail">{summary.failed} failed</span>{/if}
    {/if}
    <span class="sp"></span>
    <button class="re" onclick={run} disabled={running} title="Run all .test.luau files">
      <PlayIcon size={13} /> {running ? 'Running…' : 'Run all'}
    </button>
  </header>

  <div class="body">
    {#if running && !ran}
      <div class="none">Running tests…</div>
    {:else if error}
      <div class="none err">{error}</div>
    {:else if !files.length}
      <div class="empty">
        <FlaskConicalIcon size={26} />
        <p>No tests found.</p>
        <span>Add a <code>*.test.luau</code> file with <code>test_*</code> functions under your game directory.</span>
      </div>
    {:else}
      {#each files as f (f.file)}
        <div class="file" class:failed={fileFailed(f)}>
          <div class="file-h">
            <span class="fi" data-ok={!fileFailed(f)}>{#if fileFailed(f)}<XIcon size={12} />{:else}<CheckIcon size={12} />{/if}</span>
            <span class="fname">{f.file}</span>
            <span class="ftally">{f.tests.filter((t) => t.passed).length}/{f.tests.length}</span>
          </div>
          {#if f.error}
            <div class="test fail">
              <span class="ti"><XIcon size={11} /></span>
              <span class="tn">won't compile</span>
              <pre class="terr">{f.error}</pre>
            </div>
          {/if}
          {#each f.tests as t (t.name)}
            <div class="test" class:fail={!t.passed}>
              <span class="ti" data-ok={t.passed}>{#if t.passed}<CheckIcon size={11} />{:else}<XIcon size={11} />{/if}</span>
              <span class="tn">{t.name}</span>
              {#if !t.passed && t.error}<pre class="terr">{t.error}</pre>{/if}
            </div>
          {/each}
        </div>
      {/each}
    {/if}
  </div>
</div>

<style>
  .tp { display: flex; flex-direction: column; height: 100%; min-height: 0; background: var(--bg-primary, #12100c); }
  header { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); }
  .title { font-size: 12.5px; font-weight: 600; color: var(--text-secondary, #b6a888); }
  .pill { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; padding: 1px 7px; border-radius: 999px; }
  .pill.pass { color: var(--accent-green, #8fb877); background: color-mix(in srgb, var(--accent-green, #8fb877) 14%, transparent); }
  .pill.fail { color: var(--accent-red, #d07a5a); background: color-mix(in srgb, var(--accent-red, #d07a5a) 14%, transparent); }
  .sp { flex: 1; }
  .re { display: inline-flex; align-items: center; gap: 5px; font: inherit; font-size: 12px; color: var(--text-muted, #9a9186); background: none; border: 1px solid var(--border-default, #332c22); border-radius: var(--radius-md, 8px); padding: 4px 9px; cursor: pointer; }
  .re:hover:not(:disabled) { border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }
  .re:disabled { opacity: .55; cursor: default; }

  .body { flex: 1; min-height: 0; overflow: auto; padding: 6px 0 14px; }
  .none { color: var(--text-muted, #8c8378); font-style: italic; padding: 14px; }
  .none.err { color: var(--accent-red, #d07a5a); font-style: normal; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; }
  .empty { display: flex; flex-direction: column; align-items: center; gap: 6px; text-align: center; padding: 40px 20px; color: var(--text-muted, #8c8378); }
  .empty p { margin: 4px 0 0; color: var(--text-secondary, #b6a888); font-size: 14px; }
  .empty span { font-size: 12px; max-width: 340px; }
  .empty code { font-family: var(--font-mono, ui-monospace, monospace); color: var(--accent-amber, #c9956b); font-size: 11.5px; }

  .file { margin: 4px 0; }
  .file-h { display: flex; align-items: center; gap: 7px; padding: 5px 12px; }
  .fname { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-secondary, #b6a888); flex: 1; overflow: hidden; text-overflow: ellipsis; }
  .ftally { font-family: var(--font-mono, ui-monospace, monospace); font-size: 10.5px; color: var(--text-muted, #8c8378); }
  .fi { line-height: 0; flex: none; color: var(--accent-green, #8fb877); }
  .fi[data-ok="false"] { color: var(--accent-red, #d07a5a); }

  .test { display: grid; grid-template-columns: auto 1fr; align-items: center; gap: 7px; padding: 3px 12px 3px 26px; }
  .ti { line-height: 0; color: var(--accent-green, #8fb877); }
  .ti[data-ok="false"] { color: var(--accent-red, #d07a5a); }
  .test.fail .ti { color: var(--accent-red, #d07a5a); }
  .tn { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-primary, #ece0c8); }
  .test.fail .tn { color: var(--accent-red, #d98d78); }
  .terr { grid-column: 2; margin: 2px 0 4px; padding: 6px 9px; background: color-mix(in srgb, var(--accent-red, #d07a5a) 9%, var(--bg-surface, #17140f)); border-radius: 5px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11.5px; color: var(--accent-red, #d98d78); white-space: pre-wrap; word-break: break-word; }
</style>
