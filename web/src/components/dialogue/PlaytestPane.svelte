<script>
  import PlayIcon from '@lucide/svelte/icons/play';
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
  import { Tooltip } from '@kenn-io/kit-ui';
  import { api } from '../../lib/api.js';

  // A live playtest of the dialogue, driven by the ink_play_* REST actions.
  // It runs the CURRENT buffer (`source`) — not the saved copy — so you can
  // try edits before committing them, against a per-builder preview key that
  // never touches a real player's conversation with this NPC.
  let { refId = null, source = '' } = $props();

  // Transcript entries: { role: 'npc'|'you', text, tags? }
  let log = $state([]);
  let choices = $state([]);
  let ended = $state(false);
  let running = $state(false);
  let busy = $state(false);
  let error = $state(null);

  function apply(out) {
    if (out.text?.trim()) log = [...log, { role: 'npc', text: out.text.trim(), tags: out.tags || [] }];
    choices = out.choices || [];
    ended = !!out.ended;
  }

  async function start() {
    busy = true;
    error = null;
    log = [];
    choices = [];
    ended = false;
    const res = await api('ink_play_start', { ref_id: refId, source });
    busy = false;
    if (res.ok) {
      running = true;
      apply(res.data);
    } else {
      running = false;
      error = res.error || 'Could not start dialogue';
    }
  }

  async function choose(index, text) {
    if (busy) return;
    busy = true;
    log = [...log, { role: 'you', text }];
    choices = [];
    const res = await api('ink_play_choose', { ref_id: refId, index });
    busy = false;
    if (res.ok) apply(res.data);
    else error = res.error || 'Choice failed';
  }

  // End the preview conversation on the engine when we leave or restart, so
  // its runtime slot doesn't linger.
  function stop() {
    if (running && refId) api('ink_play_end', { ref_id: refId });
    running = false;
  }
  $effect(() => {
    const _ = refId; // re-run cleanup when the target changes
    return stop;
  });
</script>

<div class="pt">
  <div class="bar">
    <span class="lbl">Playtest</span>
    <span class="sp"></span>
    {#if running}
      <Tooltip text="Restart from the top" align="end">
        <button class="pt-btn" onclick={start} disabled={busy}>
          <RotateCcwIcon size={13} /> Restart
        </button>
      </Tooltip>
    {/if}
  </div>

  <div class="stage">
    {#if error}
      <div class="err">{error}</div>
    {/if}

    {#if !running}
      <div class="empty">
        <button class="play" onclick={start} disabled={busy || !refId}>
          <PlayIcon size={15} /> {busy ? 'Starting…' : 'Play from start'}
        </button>
        <p class="hint">Runs the current draft — no need to save first.</p>
      </div>
    {:else}
      <div class="transcript">
        {#each log as entry}
          <div class="line {entry.role}">
            <div class="txt">{entry.text}</div>
            {#if entry.tags?.length}
              <div class="tags">{#each entry.tags as tag}<span class="tag">#{tag}</span>{/each}</div>
            {/if}
          </div>
        {/each}

        {#if choices.length}
          <div class="choices">
            {#each choices as c}
              <button class="choice" disabled={busy} onclick={() => choose(c.index, c.text)}>
                {c.text}
              </button>
            {/each}
          </div>
        {:else if ended}
          <div class="end">— End —</div>
        {:else if busy}
          <div class="end thinking">…</div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .pt { display: flex; flex-direction: column; height: 100%; min-height: 0; background: var(--bg-surface, #17140f); }
  .bar { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--border-default, #2a2419); flex: none; }
  .lbl { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #8c8378); }
  .sp { flex: 1; }
  .pt-btn { display: inline-flex; align-items: center; gap: 5px; background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); color: var(--text-secondary, #b6a888); border-radius: 6px; padding: 4px 9px; cursor: pointer; font: inherit; font-size: 11.5px; }
  .pt-btn:hover { border-color: var(--accent-amber, #c9956b); color: var(--text-primary, #ece0c8); }

  .stage { flex: 1; min-height: 0; overflow-y: auto; padding: 14px; }
  .err { background: color-mix(in srgb, var(--accent-red, #c96a5a) 12%, transparent); border: 1px solid color-mix(in srgb, var(--accent-red, #c96a5a) 40%, transparent); color: var(--accent-red, #d98d78); border-radius: 7px; padding: 8px 10px; font-size: 12px; margin-bottom: 12px; white-space: pre-wrap; }

  .empty { height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px; }
  .play { display: inline-flex; align-items: center; gap: 7px; background: var(--accent-amber, #c9956b); color: var(--bg-primary, #12100c); border: none; border-radius: 8px; padding: 8px 16px; font: inherit; font-size: 13px; font-weight: 600; cursor: pointer; }
  .play:disabled { opacity: 0.5; cursor: default; }
  .hint { margin: 0; font-size: 11.5px; color: var(--text-muted, #8c8378); }

  .transcript { display: flex; flex-direction: column; gap: 10px; }
  .line .txt { font-size: 13.5px; line-height: 1.6; }
  .line.npc .txt { color: var(--text-primary, #ece0c8); }
  .line.you { align-self: flex-end; max-width: 85%; }
  .line.you .txt { color: var(--accent-blue, #6ea3d0); background: color-mix(in srgb, var(--accent-blue, #6ea3d0) 12%, transparent); border-radius: 10px 10px 2px 10px; padding: 5px 11px; font-size: 12.5px; }
  .tags { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 4px; }
  .tag { font-family: var(--font-mono, ui-monospace, monospace); font-size: var(--fs-meta); color: var(--accent-green, #8fb877); background: color-mix(in srgb, var(--accent-green, #8fb877) 12%, transparent); border-radius: 5px; padding: 0 5px; }

  .choices { display: flex; flex-direction: column; gap: 6px; margin-top: 6px; }
  .choice { text-align: left; background: var(--bg-primary, #12100c); border: 1px solid var(--border-default, #332c22); color: var(--text-secondary, #b6a888); border-radius: 8px; padding: 8px 12px; cursor: pointer; font: inherit; font-size: 12.5px; }
  .choice:hover:not(:disabled) { border-color: var(--accent-amber, #c9956b); color: var(--text-primary, #ece0c8); background: color-mix(in srgb, var(--accent-amber, #c9956b) 8%, transparent); }
  .choice:disabled { opacity: 0.5; cursor: default; }
  .end { text-align: center; color: var(--text-muted, #8c8378); font-size: 11.5px; margin-top: 8px; font-style: italic; }
  .thinking { letter-spacing: 2px; }
</style>
