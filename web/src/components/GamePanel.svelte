<script>
  import MapWidget from './widgets/MapWidget.svelte';
  import ListWidget from './widgets/ListWidget.svelte';
  import MeterWidget from './widgets/MeterWidget.svelte';
  import TextWidget from './widgets/TextWidget.svelte';

  let { channel = '', data = {}, sendCommand = () => {} } = $props();

  const widgets = { map: MapWidget, list: ListWidget, meter: MeterWidget, text: TextWidget };
  let Widget = $derived(widgets[data.widget] || null);
  let title = $derived(data.title || channel);
</script>

{#if Widget}
  <div class="game-panel">
    <h3 class="panel-label">{title}</h3>
    <div class="panel-body">
      <Widget {data} {sendCommand} />
    </div>
  </div>
{/if}

<style>
  .game-panel {
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
  }
</style>
