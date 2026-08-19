<script>
  let { data = {} } = $props();
  let bars = $derived(data.bars ?? []);
</script>

{#if bars.length}
  <div class="meters">
    {#each bars as bar}
      <div class="meter-row">
        <span class="meter-label">{bar.label}</span>
        <div class="meter-track">
          <div
            class="meter-fill"
            style:width="{Math.max(0, Math.min(100, (bar.value / bar.max) * 100))}%"
            style:background={bar.color || 'var(--accent-amber, #c9956b)'}
          ></div>
        </div>
        <span class="meter-value">{bar.value}/{bar.max}</span>
      </div>
    {/each}
  </div>
{/if}

<style>
  .meters {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .meter-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .meter-label {
    font-size: var(--font-size-xs, 12px);
    color: var(--text-secondary);
    min-width: 28px;
    flex-shrink: 0;
  }

  .meter-track {
    flex: 1;
    height: 8px;
    background: var(--bg-raised, #2a2a2a);
    border-radius: 4px;
    overflow: hidden;
  }

  .meter-fill {
    height: 100%;
    border-radius: 4px;
    transition: width 0.3s ease;
  }

  .meter-value {
    font-size: var(--font-size-2xs, 10px);
    color: var(--text-muted);
    min-width: 40px;
    text-align: right;
    flex-shrink: 0;
  }
</style>
