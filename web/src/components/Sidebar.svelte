<script>
  let { status = 'disconnected', room = null, sendCommand = () => {}, isBuilder = false, onedit = () => {} } = $props();

  let players = $derived(room?.contents?.filter(e => e.kind === 'player') ?? []);
  let npcs = $derived(room?.contents?.filter(e => e.kind === 'npc') ?? []);
  let items = $derived(room?.contents?.filter(e => e.kind === 'item') ?? []);
  let exits = $derived(room?.exits ?? []);
</script>

<aside class="sidebar">
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
            {#if n.owned || isBuilder}
              <button class="edit-btn" onclick={() => onedit(n)}>edit</button>
            {/if}
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
            {#if item.owned || isBuilder}
              <button class="edit-btn" onclick={() => onedit(item)}>edit</button>
            {/if}
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
          <button class="exit-chip" onclick={() => sendCommand(exit.dir)} title={exit.name}>
            {exit.dir} &rarr; {exit.name}
          </button>
        {/each}
      {:else}
        <span class="empty">None</span>
      {/if}
    </div>
  </div>

  <div class="spacer"></div>

  <div class="footer">
    <span class="brand">Hearth</span>
    <span class="dot" class:connected={status === 'connected'} class:error={status === 'error' || status === 'disconnected'}></span>
  </div>
</aside>

<style>
  .sidebar {
    width: 260px;
    min-width: 260px;
    background: var(--bg-surface);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .panel {
    border-bottom: 1px solid var(--border);
  }

  .panel-label {
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    padding: 12px 16px 4px;
    margin: 0;
  }

  .panel-body {
    padding: 4px 16px 12px;
    font-size: 13px;
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

  .player {
    color: var(--cyan);
    font-weight: 600;
  }

  .npc {
    color: var(--text-primary);
  }

  .item {
    color: var(--text-secondary);
  }

  .edit-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 10px;
    cursor: pointer;
    padding: 0 4px;
    text-decoration: underline;
    text-underline-offset: 2px;
    flex-shrink: 0;
  }

  .edit-btn:hover {
    color: var(--accent);
  }

  .exits {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .exit-chip {
    background: var(--bg-elevated);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 3px 10px;
    font-family: var(--font-mono);
    font-size: 12px;
    cursor: pointer;
    line-height: 1.4;
  }

  .exit-chip:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  .empty {
    color: var(--text-muted);
    font-size: 12px;
  }

  .spacer {
    flex: 1;
  }

  .footer {
    padding: 12px 16px;
    border-top: 1px solid var(--border);
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .brand {
    font-size: 12px;
    font-weight: 700;
    color: var(--accent);
    letter-spacing: 0.04em;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-muted);
  }

  .dot.connected {
    background: var(--green);
  }

  .dot.error {
    background: var(--red);
  }
</style>
