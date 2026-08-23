<script>
  import XIcon from '@lucide/svelte/icons/x';
  import EyeIcon from '@lucide/svelte/icons/eye';
  import ZapIcon from '@lucide/svelte/icons/zap';
  import CornerDownLeftIcon from '@lucide/svelte/icons/corner-down-left';

  // A live playtest terminal that rides on top of the builder: it drives the
  // real game command loop (same WS the play client uses, via `oncommand`), so
  // you can walk into the room you're editing and watch its hooks fire. `feed`
  // is the running game output; `jumpRef` teleports the test character to the
  // room currently in focus.
  let { feed = [], roomData = null, jumpRef = '', oncommand = () => {}, onclose = () => {} } = $props();

  let cmd = $state('');
  let scroller;

  $effect(() => {
    feed.length; // re-run on new output
    if (scroller) requestAnimationFrame(() => { scroller.scrollTop = scroller.scrollHeight; });
  });

  function submit() {
    const c = cmd.trim();
    if (!c) return;
    oncommand(c);
    cmd = '';
  }
  const send = (c) => oncommand(c);
  const jump = () => jumpRef && oncommand(`@teleport ${jumpRef}`);
</script>

<div class="pt">
  <header class="pt-top">
    <span class="pt-title">Playtest</span>
    {#if roomData}<span class="pt-room">{roomData.name}</span>{/if}
    <span class="pt-spacer"></span>
    {#if jumpRef}<button class="pt-jump" onclick={jump}><ZapIcon size={13} /> Jump to {jumpRef}</button>{/if}
    <button class="pt-x" onclick={onclose} aria-label="Close playtest"><XIcon size={15} /></button>
  </header>

  <div class="pt-main">
    <div class="pt-feed" bind:this={scroller}>
      {#each feed as line}<span class="pt-line">{@html line}</span>{/each}
      {#if !feed.length}<span class="pt-dim">No output yet — type a command below, or Look.</span>{/if}
    </div>
    {#if roomData}
      <aside class="pt-side">
        <button class="pt-look" onclick={() => send('look')}><EyeIcon size={13} /> Look</button>
        <div class="pt-sec">Exits</div>
        <div class="pt-exits">
          {#if roomData.exits?.length}
            {#each roomData.exits as e}<button class="pt-exit" onclick={() => send(e.dir)} title={e.name || ''}>{e.dir}</button>{/each}
          {:else}<span class="pt-dim">none</span>{/if}
        </div>
        {#if roomData.contents?.length}
          <div class="pt-sec">Here</div>
          <div class="pt-here">
            {#each roomData.contents as c}<button class="pt-thing" onclick={() => send(`examine ${c.name || c.key || c}`)}>{c.name || c.key || c}</button>{/each}
          </div>
        {/if}
      </aside>
    {/if}
  </div>

  <form class="pt-input" onsubmit={(e) => { e.preventDefault(); submit(); }}>
    <span class="pt-prompt">&gt;</span>
    <input bind:value={cmd} placeholder="command — look, north, say hi, examine kael…" autocomplete="off" spellcheck="false" />
    <button type="submit" aria-label="Send"><CornerDownLeftIcon size={14} /></button>
  </form>
</div>

<style>
  .pt {
    position: fixed; left: 0; right: 0; bottom: 0; z-index: 320;
    height: min(42vh, 420px);
    display: flex; flex-direction: column;
    background: var(--bg-surface, #17140f);
    border-top: 1px solid var(--border-default, #2a2419);
    box-shadow: 0 -12px 40px -18px rgba(0, 0, 0, 0.7);
  }
  .pt-top { display: flex; align-items: center; gap: 10px; padding: 7px 12px; border-bottom: 1px solid var(--border-muted, #2a2419); }
  .pt-title { font-size: 12px; font-weight: 600; color: var(--accent-amber, #c9956b); letter-spacing: .03em; }
  .pt-room { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-secondary, #b6a888); }
  .pt-spacer { flex: 1; }
  .pt-jump { display: inline-flex; align-items: center; gap: 5px; font: inherit; font-size: 11.5px; color: var(--accent-amber, #c9956b); background: color-mix(in srgb, var(--accent-amber, #c9956b) 12%, transparent); border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 35%, transparent); border-radius: 6px; padding: 3px 9px; cursor: pointer; }
  .pt-jump:hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 20%, transparent); }
  .pt-x { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; padding: 2px; line-height: 0; border-radius: 5px; }
  .pt-x:hover { color: var(--text-primary, #ece0c8); }

  .pt-main { flex: 1; min-height: 0; display: grid; grid-template-columns: 1fr 168px; }
  .pt-feed { overflow-y: auto; padding: 10px 12px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12.5px; line-height: 1.5; color: var(--text-primary, #ece0c8); white-space: pre-wrap; word-break: break-word; }
  .pt-line { display: contents; }
  .pt-side { border-left: 1px solid var(--border-muted, #2a2419); padding: 10px; overflow-y: auto; display: flex; flex-direction: column; gap: 6px; }
  .pt-look { display: inline-flex; align-items: center; justify-content: center; gap: 6px; font: inherit; font-size: 12px; color: var(--text-primary, #ece0c8); background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 7px; padding: 5px; cursor: pointer; }
  .pt-look:hover { border-color: var(--accent-amber, #c9956b); }
  .pt-sec { font-size: 9.5px; text-transform: uppercase; letter-spacing: .07em; color: var(--text-muted, #8c8378); margin-top: 4px; }
  .pt-exits { display: flex; flex-wrap: wrap; gap: 4px; }
  .pt-exit { font-family: var(--font-mono, ui-monospace, monospace); font-size: 11px; color: var(--text-secondary, #b6a888); background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); border-radius: 5px; padding: 2px 8px; cursor: pointer; }
  .pt-exit:hover { border-color: var(--accent-amber, #c9956b); color: var(--accent-amber, #c9956b); }
  .pt-here { display: flex; flex-direction: column; gap: 3px; }
  .pt-thing { text-align: left; font-size: 11.5px; color: var(--text-secondary, #b6a888); background: none; border: none; cursor: pointer; padding: 1px 0; }
  .pt-thing:hover { color: var(--accent-amber, #c9956b); }
  .pt-dim { color: var(--text-muted, #8c8378); font-style: italic; font-size: 11.5px; }

  .pt-input { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-top: 1px solid var(--border-default, #2a2419); background: var(--bg-primary, #12100c); }
  .pt-prompt { color: var(--accent-amber, #c9956b); font-family: var(--font-mono, ui-monospace, monospace); }
  .pt-input input { flex: 1; background: none; border: none; outline: none; color: var(--text-primary, #ece0c8); font-family: var(--font-mono, ui-monospace, monospace); font-size: 13px; }
  .pt-input button { background: none; border: none; color: var(--text-muted, #9a9186); cursor: pointer; padding: 3px; line-height: 0; }
  .pt-input button:hover { color: var(--accent-amber, #c9956b); }
</style>
