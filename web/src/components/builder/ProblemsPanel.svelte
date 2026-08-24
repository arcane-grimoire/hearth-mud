<script>
  import { api } from '../../lib/api.js';
  import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
  import CheckCircle2Icon from '@lucide/svelte/icons/check-circle-2';
  import AlertTriangleIcon from '@lucide/svelte/icons/alert-triangle';

  // The Problems panel: one `world_check` pass surfaced as a click-through list.
  // A syntax error opens the offending hook; every other problem opens its
  // object. The engine does the whole-world walk (dangling exits, unreachable /
  // exitless / description-less rooms, uncompilable hooks) — we just render it.
  let { onopen = () => {}, onopenhook = () => {} } = $props();

  let loading = $state(true);
  let problems = $state([]);
  let error = $state(null);

  // Highest severity first is how the engine already sorts; keep that order but
  // give each tier a header so the eye lands on `high` before `low` noise.
  const TIERS = [
    { key: 'high', label: 'Errors' },
    { key: 'medium', label: 'Warnings' },
    { key: 'low', label: 'Hints' },
  ];
  const grouped = $derived(TIERS
    .map((tier) => ({ ...tier, items: problems.filter((p) => p.severity === tier.key) }))
    .filter((g) => g.items.length));

  async function run() {
    loading = true;
    error = null;
    const res = await api('world_check');
    if (res?.ok) problems = res.data?.problems || [];
    else error = res?.error || 'World check failed';
    loading = false;
  }

  function openProblem(p) {
    if (p.kind === 'syntax_error' && p.hook) onopenhook(p.ref, p.hook);
    else if (p.ref) onopen(p.ref);
  }

  $effect(() => { run(); });
</script>

<div class="pp">
  <header>
    <span class="title">Problems</span>
    {#if !loading && !error}
      <span class="count" class:clean={!problems.length}>{problems.length || 'none'}</span>
    {/if}
    <span class="sp"></span>
    <button class="re" onclick={run} disabled={loading} title="Re-run world check">
      <RefreshCwIcon size={13} class={loading ? 'spin' : ''} /> Re-check
    </button>
  </header>

  <div class="body">
    {#if loading}
      <div class="none">Checking the world…</div>
    {:else if error}
      <div class="none err">{error}</div>
    {:else if !problems.length}
      <div class="clean-state">
        <CheckCircle2Icon size={26} />
        <p>No problems found.</p>
        <span>No broken exits, unreachable rooms, or hooks that won't compile.</span>
      </div>
    {:else}
      {#each grouped as g (g.key)}
        <div class="grp">
          <div class="grp-h" data-sev={g.key}>{g.label}<span class="gc">{g.items.length}</span></div>
          {#each g.items as p, i (p.ref + p.kind + (p.hook || '') + i)}
            <button class="row" data-sev={p.severity} onclick={() => openProblem(p)}>
              <span class="ic"><AlertTriangleIcon size={13} /></span>
              <span class="meta">
                <span class="ref">{p.ref}{#if p.hook}<span class="hook"> · {p.hook}</span>{/if}</span>
                <span class="msg">{p.message}</span>
              </span>
              <span class="kind">{p.kind.replace(/_/g, ' ')}</span>
            </button>
          {/each}
        </div>
      {/each}
    {/if}
  </div>
</div>

<style>
  .pp { display: flex; flex-direction: column; height: 100%; min-height: 0; background: var(--bg-primary, #12100c); }
  header { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); }
  .title { font-size: 12.5px; font-weight: 600; color: var(--text-secondary, #b6a888); }
  .count { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--accent-red, #d07a5a); background: color-mix(in srgb, var(--accent-red, #d07a5a) 14%, transparent); padding: 1px 7px; border-radius: 999px; }
  .count.clean { color: var(--accent-green, #8fb877); background: color-mix(in srgb, var(--accent-green, #8fb877) 14%, transparent); }
  .sp { flex: 1; }
  .re { display: inline-flex; align-items: center; gap: 5px; font: inherit; font-size: 12px; color: var(--text-muted, #9a9186); background: none; border: 1px solid var(--border-default, #332c22); border-radius: var(--radius-md, 8px); padding: 4px 9px; cursor: pointer; }
  .re:hover:not(:disabled) { border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }
  .re:disabled { opacity: .55; cursor: default; }
  .re :global(.spin) { animation: sp 0.8s linear infinite; }
  @keyframes sp { to { transform: rotate(360deg); } }

  .body { flex: 1; min-height: 0; overflow: auto; padding: 6px 0 14px; }
  .none { color: var(--text-muted, #8c8378); font-style: italic; padding: 14px; }
  .none.err { color: var(--accent-red, #d07a5a); font-style: normal; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; }
  .clean-state { display: flex; flex-direction: column; align-items: center; gap: 6px; text-align: center; padding: 40px 20px; color: var(--accent-green, #8fb877); }
  .clean-state p { margin: 4px 0 0; color: var(--text-secondary, #b6a888); font-size: 14px; }
  .clean-state span { color: var(--text-muted, #8c8378); font-size: 12px; max-width: 320px; }

  .grp { margin-top: 4px; }
  .grp-h { display: flex; align-items: center; gap: 6px; padding: 6px 12px 3px; font-size: var(--fs-label, 10.5px); text-transform: uppercase; letter-spacing: 0.07em; color: var(--text-muted, #9a9186); }
  .grp-h[data-sev="high"] { color: var(--accent-red, #d07a5a); }
  .grp-h[data-sev="medium"] { color: var(--accent-amber, #c9956b); }
  .gc { font-family: var(--font-mono, ui-monospace, monospace); font-size: 10px; opacity: .7; }

  .row { display: flex; align-items: flex-start; gap: 9px; width: 100%; text-align: left; background: none; border: none; border-left: 2px solid transparent; padding: 6px 12px; cursor: pointer; color: inherit; font: inherit; }
  .row:hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 8%, transparent); }
  .row[data-sev="high"] { border-left-color: var(--accent-red, #d07a5a); }
  .row[data-sev="medium"] { border-left-color: var(--accent-amber, #c9956b); }
  .row[data-sev="low"] { border-left-color: var(--border-default, #332c22); }
  .ic { line-height: 0; padding-top: 2px; color: var(--text-muted, #8c8378); flex: none; }
  .row[data-sev="high"] .ic { color: var(--accent-red, #d07a5a); }
  .row[data-sev="medium"] .ic { color: var(--accent-amber, #c9956b); }
  .meta { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
  .ref { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11.5px; color: var(--text-secondary, #b6a888); }
  .hook { color: var(--accent-amber, #c9956b); }
  .msg { font-size: 12px; color: var(--text-primary, #ece0c8); overflow: hidden; text-overflow: ellipsis; }
  .kind { flex: none; font-size: 10px; text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-muted, #8c8378); font-family: var(--font-mono, ui-monospace, monospace); padding-top: 2px; }
</style>
