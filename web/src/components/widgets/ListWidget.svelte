<script>
  let { data = {}, sendCommand = () => {} } = $props();
  let items = $derived(data.items ?? []);
</script>

{#if items.length}
  <ul class="widget-list">
    {#each items as item}
      <li
        class="widget-list-item"
        class:clickable={!!item.command}
        style:color={item.color || 'inherit'}
      >
        {#if item.command}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <span class="clickable-text" onclick={() => sendCommand(item.command)}>
            {item.label}
          </span>
        {:else}
          {item.label}
        {/if}
        {#if item.value != null}
          <span class="item-value">{item.value}</span>
        {/if}
      </li>
    {/each}
  </ul>
{:else}
  <span class="widget-empty">{data.empty || 'Nothing'}</span>
{/if}

<style>
  .widget-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .widget-list-item {
    padding: 2px 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: var(--font-size-sm, 13px);
    line-height: 1.5;
  }

  .item-value {
    color: var(--text-muted);
    font-size: var(--font-size-xs, 12px);
    flex-shrink: 0;
  }

  .clickable-text {
    cursor: pointer;
    text-decoration: underline;
    text-decoration-color: var(--border-default);
    text-underline-offset: 2px;
  }

  .clickable-text:hover {
    text-decoration-color: var(--accent-amber, #c9956b);
    color: var(--accent-amber, #c9956b);
  }

  .widget-empty {
    color: var(--text-muted);
    font-size: var(--font-size-xs, 12px);
  }
</style>
