<script>
  let { data = {} } = $props();
</script>

<div class="widget-text">
  {#if data.html}
    {@html data.html}
  {:else if data.lines}
    {#each data.lines as line}
      <p style:color={line.color || 'inherit'}>{line.text || line}</p>
    {/each}
  {:else if data.text}
    <p>{data.text}</p>
  {/if}
</div>

<style>
  .widget-text {
    font-size: var(--font-size-sm, 13px);
    color: var(--text-primary);
    line-height: 1.5;
  }

  .widget-text :global(p) {
    margin: 0 0 4px;
  }

  .widget-text :global(p:last-child) {
    margin-bottom: 0;
  }
</style>
