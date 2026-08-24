<script>
  import { api } from '../../lib/api.js';

  // Reach ANY object directly (the table/graph only surface rooms; items/npcs
  // otherwise hide inside a room's Contents). Search all objects by name / key
  // / kind / ref, or type a raw ref (#351) to open it — which also works for
  // exits, since they're excluded from list_objects but examine handles them.
  // `extras` are non-object entries (e.g. maps) the host wants findable here —
  // each { id, label, kind } is matched by the same query and picked through
  // onpickextra. Empty by default, so other callers are unaffected.
  let { onpick = () => {}, onclose = () => {}, extras = [], onpickextra = () => {} } = $props();

  let objects = $state([]);
  let loading = $state(true);
  let q = $state('');
  let box;

  async function load() {
    try {
      const r = await api('list_objects'); // no location -> all (except exits)
      objects = r?.ok ? r.data : [];
    } catch (e) { objects = []; }
    loading = false;
  }
  load();
  $effect(() => { box?.focus(); });

  const asRef = $derived.by(() => {
    const t = q.trim();
    return /^#?[\w.-]+$/.test(t) && /\d/.test(t) ? (t.startsWith('#') ? t : `#${t}`) : '';
  });

  const results = $derived.by(() => {
    const needle = q.trim().toLowerCase();
    const base = needle
      ? objects.filter((o) => `${o.ref_id} ${o.key} ${o.title || ''} ${o.kind}`.toLowerCase().includes(needle))
      : objects;
    return [...base]
      .sort((a, b) => (a.title || a.key).localeCompare(b.title || b.key))
      .slice(0, 80);
  });

  const extraResults = $derived.by(() => {
    const needle = q.trim().toLowerCase();
    return (extras || [])
      .filter((x) => !needle || `${x.label} ${x.kind || ''}`.toLowerCase().includes(needle))
      .slice(0, 20);
  });

  function pick(ref) { onpick(ref); }
  function onKey(e) {
    if (e.key === 'Escape') { onclose(); return; }
    if (e.key === 'Enter') {
      e.preventDefault();
      // An explicit "#…" is a direct-open intent — honour it even when the
      // query also fuzzy-matches some objects (that's the top "Open #ref" row).
      const explicitRef = asRef && q.trim().startsWith('#');
      if (explicitRef) pick(asRef);
      else if (results.length) pick(results[0].ref_id);
      else if (extraResults.length) onpickextra(extraResults[0].id);
      else if (asRef) pick(asRef);
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && onclose()} />
<button class="of-backdrop" aria-label="Close" onclick={onclose}></button>
<div class="of" role="dialog" aria-label="Find object">
  <input bind:this={box} bind:value={q} onkeydown={onKey} spellcheck="false" autocomplete="off"
    placeholder="Find an object — name, key, kind, or ref (#351)…" />

  <div class="of-results">
    {#if asRef}
      <button class="of-item of-ref-open" onclick={() => pick(asRef)}>
        <span class="of-kind of-any">open</span>
        <span class="of-title">Open <b>{asRef}</b> directly</span>
        <span class="of-ref">↵</span>
      </button>
    {/if}
    {#each extraResults as x (x.id)}
      <button class="of-item" onclick={() => onpickextra(x.id)}>
        <span class="of-kind kind-map">{x.kind || 'map'}</span>
        <span class="of-title">{x.label}</span>
      </button>
    {/each}
    {#if loading}
      <div class="of-empty">Loading…</div>
    {:else}
      {#each results as o (o.ref_id)}
        <button class="of-item" onclick={() => pick(o.ref_id)}>
          <span class="of-kind kind-{o.kind}">{o.kind}</span>
          <span class="of-title">{o.title || o.key}</span>
          {#if o.location_ref}<span class="of-loc">in {o.location_ref}</span>{/if}
          <span class="of-ref">{o.ref_id}</span>
        </button>
      {:else}
        {#if !asRef}<div class="of-empty">No objects match “{q}”.</div>{/if}
      {/each}
    {/if}
  </div>
</div>

<style>
  .of-backdrop { position: fixed; inset: 0; z-index: 240; background: rgba(0, 0, 0, 0.45); border: none; cursor: default; }
  .of {
    position: fixed; left: 50%; top: 12vh; transform: translateX(-50%); z-index: 241;
    width: min(560px, calc(100vw - 32px));
    background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #2a2419);
    border-radius: 12px; box-shadow: 0 24px 60px -20px rgba(0, 0, 0, 0.7);
    overflow: hidden; display: flex; flex-direction: column;
  }
  .of > input {
    font: inherit; font-size: 15px; color: var(--text-primary, #ece0c8);
    background: none; border: none; outline: none;
    padding: 14px 16px; border-bottom: 1px solid var(--border-muted, #2a2419);
  }
  .of-results { max-height: 56vh; overflow-y: auto; padding: 6px; }
  .of-item {
    display: flex; align-items: center; gap: 10px; width: 100%; text-align: left;
    background: none; border: none; cursor: pointer; border-radius: 8px;
    padding: 8px 10px; color: var(--text-primary, #ece0c8);
  }
  .of-item:hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 12%, transparent); }
  .of-ref-open { color: var(--accent-amber, #c9956b); }
  .of-kind {
    font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-badge); text-transform: uppercase;
    letter-spacing: .04em; padding: 2px 6px; border-radius: 4px; flex: none; min-width: 42px; text-align: center;
    background: var(--bg-primary, #12100c); border: 1px solid var(--border-muted, #2a2419); color: var(--text-muted, #9a9186);
  }
  .kind-room { color: var(--accent-amber, #c9956b); }
  .kind-npc { color: var(--accent-green, #8fb877); }
  .kind-item { color: var(--accent-blue, #6ea3d0); }
  .kind-map { color: var(--accent-green, #8fb877); }
  .of-any { color: var(--bg-primary, #12100c); background: var(--accent-amber, #c9956b); border-color: var(--accent-amber, #c9956b); }
  .of-title { flex: 1; font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .of-loc { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--text-muted, #8c8378); }
  .of-ref { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--accent-amber, #c9956b); }
  .of-empty { padding: 18px; text-align: center; color: var(--text-muted, #9a9186); font-size: 13px; }
</style>
