<script>
  import { Chip, StatusDot } from '@kenn-io/kit-ui';
  import GamePanel from './GamePanel.svelte';

  let { status = 'disconnected', room = null, sendCommand = () => {}, isBuilder = false, gamePanels = new Map() } = $props();

  let grouped = $derived.by(() => {
    const g = { players: [], npcs: [], items: [] };
    for (const e of room?.contents ?? []) {
      if (e.kind === 'player') g.players.push(e);
      else if (e.kind === 'npc') g.npcs.push(e);
      else if (e.kind === 'item') g.items.push(e);
    }
    return g;
  });
  let players = $derived(grouped.players);
  let npcs = $derived(grouped.npcs);
  let items = $derived(grouped.items);
  let exits = $derived(room?.exits ?? []);

  const statusDotMap = {
    connected: 'working',
    connecting: 'idle',
    disconnected: 'stale',
    error: 'unclean',
  };
</script>

<aside class="sidebar">
  {#each [...gamePanels] as [channel, data]}
    <GamePanel {channel} {data} {sendCommand} />
  {/each}

  <div class="panel">
    <h3 class="panel-label">Who's Here</h3>
    <div class="panel-body">
      {#if players.length || npcs.length}
        {#each players as p}
          <div class="entity player">{p.name}</div>
        {/each}
        {#each npcs as n}
          <div class="entity npc">
            <span>{n.name}</span>
          </div>
        {/each}
      {:else}
        <span class="empty">Nobody</span>
      {/if}
    </div>
  </div>

  <div class="panel">
    <h3 class="panel-label">What's Here</h3>
    <div class="panel-body">
      {#if items.length}
        {#each items as item}
          <div class="entity item">
            <span>{item.name}</span>
          </div>
        {/each}
      {:else}
        <span class="empty">Nothing</span>
      {/if}
    </div>
  </div>

  <div class="panel">
    <h3 class="panel-label">Exits</h3>
    <div class="panel-body exits">
      {#if exits.length}
        {#each exits as exit}
          <Chip
            interactive
            uppercase={false}
            tone="neutral"
            size="sm"
            onclick={() => sendCommand(exit.dir)}
            title={exit.name}
          >
            {exit.dir} &rarr; {exit.name}
          </Chip>
        {/each}
      {:else}
        <span class="empty">None</span>
      {/if}
    </div>
  </div>

  <div class="spacer"></div>

  <div class="footer">
    <span class="footer-brand">Hearth</span>
    <StatusDot status={statusDotMap[status] || 'stale'} label={status} />
  </div>
</aside>

<style>
  .sidebar {
    width: 260px;
    min-width: 260px;
    background: var(--bg-surface);
    border-left: 1px solid var(--border-default);
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .panel {
    border-bottom: 1px solid var(--border-default);
  }

  .panel-label {
    font-size: var(--font-size-2xs, 10px);
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    padding: 12px 16px 4px;
    margin: 0;
  }

  .panel-body {
    padding: 4px 16px 12px;
    font-size: var(--font-size-sm, 13px);
    color: var(--text-primary);
    line-height: 1.6;
  }

  .entity {
    padding: 1px 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
  }

  .player { color: var(--accent-teal, #56b6c2); font-weight: 600; }
  .npc { color: var(--text-primary); }
  .item { color: var(--text-secondary); }

  .exits {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .empty { color: var(--text-muted); font-size: var(--font-size-xs, 12px); }
  .spacer { flex: 1; }

  .footer {
    padding: 12px 16px;
    border-top: 1px solid var(--border-default);
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .footer-brand {
    font-size: var(--font-size-xs, 12px);
    font-weight: 700;
    color: var(--accent-amber, #c9956b);
    letter-spacing: 0.04em;
  }
</style>
