<script>
  import { Handle, Position } from '@xyflow/svelte';

  // A boundary stub: a room just outside the current slice, reached by an exit
  // from inside it. Clicking it (handled on the flow) re-scopes the graph
  // around this room so you can walk outward without ever loading the whole
  // world.
  let { data } = $props();
</script>

<div class="stub-node" title="Load rooms around {data.key}">
  <span class="stub-plus">+</span>
  <span class="stub-label">{data.title || data.key}</span>

  <Handle type="source" position={Position.Left} id="w" />
  <Handle type="source" position={Position.Top} id="n" />
  <Handle type="source" position={Position.Right} id="e" />
  <Handle type="source" position={Position.Bottom} id="s" />
  <Handle type="source" position={Position.Top} id="ne" style="left: 78%" />
  <Handle type="source" position={Position.Top} id="nw" style="left: 22%" />
  <Handle type="source" position={Position.Bottom} id="se" style="left: 78%" />
  <Handle type="source" position={Position.Bottom} id="sw" style="left: 22%" />
</div>

<style>
  .stub-node {
    display: flex; align-items: center; gap: 6px;
    padding: 6px 10px;
    background: transparent;
    border: 1.5px dashed var(--border-default, #4a4030);
    border-radius: 999px;
    color: var(--text-muted, #9a9186);
    font-family: var(--font-sans, system-ui, sans-serif);
    font-size: 11px;
    cursor: pointer;
    transition: border-color 0.12s, color 0.12s, background 0.12s;
  }
  .stub-node:hover {
    border-color: var(--accent-amber, #c9956b);
    color: var(--text-primary, #ece0c8);
    background: color-mix(in srgb, var(--accent-amber, #c9956b) 10%, transparent);
  }
  .stub-plus {
    display: grid; place-items: center;
    width: 15px; height: 15px; border-radius: 50%;
    background: var(--accent-amber, #c9956b); color: var(--bg-primary, #12100c);
    font-weight: 700; font-size: 12px; line-height: 1;
  }
  .stub-label { white-space: nowrap; max-width: 130px; overflow: hidden; text-overflow: ellipsis; }
</style>
