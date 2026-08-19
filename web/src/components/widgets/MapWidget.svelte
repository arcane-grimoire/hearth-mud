<script>
  let { data = {}, sendCommand = () => {} } = $props();

  const CELL = 80;
  const GAP = 30;
  const NODE_W = 72;
  const NODE_H = 36;
  const CORNER = 6;

  let visited = $state(new Map());
  let currentRef = $state(null);

  $effect(() => {
    if (!data) return;
    currentRef = data.current;
    if (data.rooms) {
      for (const room of data.rooms) {
        visited.set(room.ref, room);
      }
    }
  });

  let nodes = $derived.by(() => {
    const entries = [...visited.values()];
    if (!entries.length) return [];
    return entries.map(r => ({
      ...r,
      cx: r.x * (CELL + GAP),
      cy: r.y * (CELL + GAP),
    }));
  });

  let edges = $derived.by(() => {
    if (!data?.edges) return [];
    const lookup = new Map(nodes.map(n => [n.ref, n]));
    return data.edges
      .filter(e => lookup.has(e.from) && lookup.has(e.to))
      .map(e => {
        const a = lookup.get(e.from);
        const b = lookup.get(e.to);
        return { x1: a.cx, y1: a.cy, x2: b.cx, y2: b.cy, dir: e.dir };
      });
  });

  let viewBox = $derived.by(() => {
    if (!nodes.length) return '-100 -100 200 200';
    const pad = 60;
    const xs = nodes.map(n => n.cx);
    const ys = nodes.map(n => n.cy);
    const x0 = Math.min(...xs) - pad;
    const y0 = Math.min(...ys) - pad;
    const w = Math.max(...xs) - Math.min(...xs) + pad * 2;
    const h = Math.max(...ys) - Math.min(...ys) + pad * 2;
    return `${x0} ${y0} ${w} ${h}`;
  });

  function handleClick(room) {
    if (room.ref === currentRef) return;
    const edge = data?.edges?.find(e => e.from === currentRef && e.to === room.ref);
    if (edge) sendCommand(edge.dir);
  }
</script>

{#if nodes.length}
  <svg viewBox={viewBox} xmlns="http://www.w3.org/2000/svg">
    {#each edges as e}
      <line
        x1={e.x1} y1={e.y1} x2={e.x2} y2={e.y2}
        stroke="var(--text-muted)"
        stroke-width="2"
        stroke-opacity="0.5"
      />
    {/each}
    {#each nodes as room}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <g
        class="node"
        class:current={room.ref === currentRef}
        class:clickable={room.ref !== currentRef && data?.edges?.some(e => e.from === currentRef && e.to === room.ref)}
        onclick={() => handleClick(room)}
      >
        <rect
          x={room.cx - NODE_W / 2}
          y={room.cy - NODE_H / 2}
          width={NODE_W}
          height={NODE_H}
          rx={CORNER}
        />
        <text x={room.cx} y={room.cy + 1} text-anchor="middle" dominant-baseline="middle">
          {room.short || room.name}
        </text>
      </g>
    {/each}
  </svg>
{:else}
  <span class="widget-empty">Move around to reveal the map</span>
{/if}

<style>
  svg {
    width: 100%;
    height: auto;
    max-height: 220px;
    display: block;
  }

  .node rect {
    fill: var(--bg-raised, #2a2a2a);
    stroke: var(--border-default);
    stroke-width: 1.5;
    transition: fill 0.15s, stroke 0.15s;
  }

  .node text {
    fill: var(--text-secondary);
    font-size: 9px;
    font-family: inherit;
    pointer-events: none;
  }

  .node.current rect {
    fill: var(--accent-amber, #c9956b);
    stroke: var(--accent-amber, #c9956b);
  }

  .node.current text {
    fill: var(--bg-base, #1a1a1a);
    font-weight: 700;
  }

  .node.clickable { cursor: pointer; }

  .node.clickable:hover rect {
    fill: var(--bg-overlay, #333);
    stroke: var(--accent-amber, #c9956b);
  }

  .widget-empty {
    color: var(--text-muted);
    font-size: var(--font-size-xs, 12px);
  }
</style>
