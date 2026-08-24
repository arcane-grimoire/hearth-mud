<script>
  import { Handle, Position } from '@xyflow/svelte';

  // A room plate. Four handles (one per cardinal side) double as the exit
  // origins — the side you drag from names the exit's direction. Connection
  // mode is Loose on the flow, so a source handle also accepts a drop.
  let { data, selected } = $props();
</script>

<div class="room-node" class:sel={selected}>
  <div class="rn-key">{data.key}</div>
  <div class="rn-title">{data.title}</div>
  {#if data.tags?.length}
    <div class="rn-tags">
      {#each data.tags as t}<span class="rn-tag">{t}</span>{/each}
    </div>
  {/if}

  <Handle type="source" position={Position.Top} id="n" />
  <Handle type="source" position={Position.Right} id="e" />
  <Handle type="source" position={Position.Bottom} id="s" />
  <Handle type="source" position={Position.Left} id="w" />
  <Handle type="source" position={Position.Top} id="ne" style="left: 82%" />
  <Handle type="source" position={Position.Top} id="nw" style="left: 18%" />
  <Handle type="source" position={Position.Bottom} id="se" style="left: 82%" />
  <Handle type="source" position={Position.Bottom} id="sw" style="left: 18%" />
</div>

<style>
  .room-node {
    width: 156px;
    padding: 8px 11px 9px;
    background: var(--bg-surface, #1b1712);
    border: 1.5px solid var(--border-default, #3a3227);
    border-radius: var(--radius-md, 9px);
    box-shadow: 0 3px 12px -8px rgba(0, 0, 0, 0.5);
    font-family: var(--font-sans, system-ui, sans-serif);
    transition: border-color 0.12s, box-shadow 0.12s;
  }
  .room-node:hover { border-color: var(--accent-amber, #c9956b); }
  .room-node.sel {
    border-color: var(--accent-amber, #c9956b);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent-amber, #c9956b) 40%, transparent),
      0 6px 18px -10px rgba(0, 0, 0, 0.6);
  }
  .rn-key {
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: var(--fs-label); letter-spacing: 0.06em; text-transform: uppercase;
    color: var(--text-muted, #8c8378);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .rn-title {
    font-size: 13px; font-weight: 600; line-height: 1.25;
    color: var(--text-primary, #ece0c8); margin-top: 1px;
  }
  .rn-tags { display: flex; flex-wrap: wrap; gap: 3px; margin-top: 6px; }
  .rn-tag {
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: var(--fs-meta); padding: 1px 4px; border-radius: 3px;
    background: var(--bg-primary, #12100c); color: var(--text-muted, #9a9186);
    border: 1px solid var(--border-muted, #2a2419);
  }
</style>
