<script>
  import { Button } from '@kenn-io/kit-ui';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
  import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';
  import CheckIcon from '@lucide/svelte/icons/check';
  import { api } from '../lib/api.js';
  import { navigate } from '../lib/router.svelte.js';

  // Full-page world health check (/builder/problems): dangling exits,
  // unreachable/dead-end/description-less rooms, and hooks that don't compile —
  // one server-side pass (world_check). Each row jumps to where it can be fixed.
  let { onexit = () => {} } = $props();

  let problems = $state([]);
  let loading = $state(true);
  let live = $state(true);

  async function load() {
    loading = true;
    try {
      const r = await api('world_check');
      if (r?.ok) { problems = r.data.problems || []; live = true; }
      else { problems = []; live = false; }
    } catch (e) { problems = []; live = false; }
    loading = false;
  }
  load();

  const counts = $derived.by(() => {
    const c = { high: 0, medium: 0, low: 0 };
    for (const p of problems) c[p.severity] = (c[p.severity] || 0) + 1;
    return c;
  });

  const KIND = {
    broken_exit: 'broken exit',
    unreachable: 'unreachable',
    no_exits: 'dead end',
    no_description: 'no description',
    syntax_error: 'syntax error',
  };

  function open(p) {
    if (p.kind === 'syntax_error') {
      navigate(`/builder/code?ref=${encodeURIComponent(p.ref)}&hook=${encodeURIComponent(p.hook)}`);
      return;
    }
    const room = p.from || p.ref;
    if (room) navigate(`/builder/rooms?focus=${encodeURIComponent(room)}`);
  }
</script>

<div class="wc">
  <header class="wc-top">
    <button class="wc-back" onclick={onexit}><ArrowLeftIcon size={16} /> <span>Game</span></button>
    <div class="wc-title">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" aria-hidden="true">
        <path d="M12 3l8 4v5c0 4.5-3.2 7.7-8 9-4.8-1.3-8-4.5-8-9V7z" stroke-linejoin="round" />
      </svg>
      <h1>World check</h1>
      {#if !loading}
        <span class="wc-counts">
          <span class="wc-c high">{counts.high} high</span>
          <span class="wc-c medium">{counts.medium} med</span>
          <span class="wc-c low">{counts.low} low</span>
        </span>
      {/if}
    </div>
    <div class="wc-spacer"></div>
    <Button size="sm" onclick={load} disabled={loading}><RefreshIcon size={14} /> Recheck</Button>
  </header>

  <div class="wc-body">
    {#if loading}
      <div class="wc-msg">Checking the world…</div>
    {:else if problems.length === 0}
      <div class="wc-clean">
        <div class="wc-check"><CheckIcon size={26} /></div>
        <h2>All clear</h2>
        <p>No broken exits, unreachable rooms, or program errors found.</p>
      </div>
    {:else}
      <ul class="wc-list">
        {#each problems as p}
          <li class="wc-item sev-{p.severity}" onclick={() => open(p)} title="Go to fix">
            <span class="wc-sev">{p.severity}</span>
            <span class="wc-kind">{KIND[p.kind] || p.kind}</span>
            <span class="wc-where">
              {#if p.hook}<span class="wc-ref">{p.key || p.ref}</span> · <b>{p.hook}</b>
              {:else if p.dir}<span class="wc-ref">{p.from}</span> <span class="wc-dir">{p.dir}</span>→ {p.target}
              {:else}<span class="wc-ref">{p.ref}</span>{#if p.key} <span class="wc-k">{p.key}</span>{/if}{/if}
            </span>
            <span class="wc-msg-t">{p.message}</span>
            <ArrowRightIcon size={15} class="wc-go" />
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<style>
  .wc { position: fixed; inset: 0; z-index: 200; display: flex; flex-direction: column; background: var(--bg-primary, #0e0c0a); color: var(--text-primary, #ece0c8); }
  .wc-top { display: flex; align-items: center; gap: 14px; padding: 9px 14px; border-bottom: var(--border-width, 1px) solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); }
  .wc-back { display: inline-flex; align-items: center; gap: 6px; background: none; border: none; color: var(--text-primary, #ece0c8); cursor: pointer; font: inherit; font-size: 13px; padding: 5px 9px; border-radius: var(--radius-md, 8px); }
  .wc-back:hover { background: var(--bg-primary, rgba(255,255,255,.06)); }
  .wc-title { display: flex; align-items: center; gap: 10px; }
  .wc-title svg { color: var(--accent-amber, #c9956b); }
  .wc-title h1 { font-size: 15px; font-weight: 600; margin: 0; }
  .wc-counts { display: flex; gap: 6px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; }
  .wc-c { padding: 1px 7px; border-radius: 999px; border: 1px solid var(--border-default, #332c22); color: var(--text-muted, #9a9186); }
  .wc-c.high { color: var(--accent-red, #d07a5a); border-color: color-mix(in srgb, var(--accent-red, #d07a5a) 40%, transparent); }
  .wc-c.medium { color: var(--accent-amber, #c9956b); border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 40%, transparent); }
  .wc-c.low { color: var(--text-muted, #9a9186); }
  .wc-spacer { flex: 1; }

  .wc-body { flex: 1; overflow-y: auto; min-height: 0; }
  .wc-msg { padding: 40px; text-align: center; color: var(--text-muted, #9a9186); }
  .wc-clean { height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px; color: var(--text-muted, #9a9186); }
  .wc-check { width: 54px; height: 54px; border-radius: 50%; display: grid; place-items: center; color: var(--accent-green, #8fb877); border: 2px solid color-mix(in srgb, var(--accent-green, #8fb877) 40%, transparent); }
  .wc-clean h2 { margin: 6px 0 0; font-size: 18px; color: var(--text-primary, #ece0c8); }
  .wc-clean p { margin: 0; font-size: 13px; }

  .wc-list { list-style: none; margin: 0; padding: 6px 0; }
  .wc-item { display: grid; grid-template-columns: 62px 96px minmax(120px, 1fr) 2fr auto; align-items: center; gap: 12px; padding: 9px 16px; cursor: pointer; border-left: 3px solid transparent; }
  .wc-item:hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 8%, transparent); }
  .wc-item.sev-high { border-left-color: var(--accent-red, #d07a5a); }
  .wc-item.sev-medium { border-left-color: var(--accent-amber, #c9956b); }
  .wc-item.sev-low { border-left-color: var(--border-default, #332c22); }
  .wc-sev { font-family: var(--font-mono, ui-monospace, monospace); font-size: 9.5px; text-transform: uppercase; letter-spacing: .05em; color: var(--text-muted, #8c8378); }
  .wc-item.sev-high .wc-sev { color: var(--accent-red, #d07a5a); }
  .wc-item.sev-medium .wc-sev { color: var(--accent-amber, #c9956b); }
  .wc-kind { font-size: 11px; font-weight: 600; color: var(--text-secondary, #b6a888); text-transform: capitalize; }
  .wc-where { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-muted, #9a9186); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wc-ref { color: var(--accent-amber, #c9956b); }
  .wc-where b { color: var(--text-primary, #ece0c8); }
  .wc-dir { color: var(--edge, #9c8863); }
  .wc-k { color: var(--text-muted, #8c8378); }
  .wc-msg-t { font-size: 12.5px; color: var(--text-secondary, #b6a888); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wc-item :global(.wc-go) { color: var(--text-muted, #8c8378); opacity: 0; }
  .wc-item:hover :global(.wc-go) { opacity: 1; color: var(--accent-amber, #c9956b); }
</style>
