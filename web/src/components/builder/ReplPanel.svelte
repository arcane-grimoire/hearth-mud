<script>
  import { api } from '../../lib/api.js';
  import { repl } from '../../lib/repl.svelte.js';
  import CornerDownLeftIcon from '@lucide/svelte/icons/corner-down-left';
  import Trash2Icon from '@lucide/svelte/icons/trash-2';
  import TerminalIcon from '@lucide/svelte/icons/terminal';

  // The REPL: a prompt that PREVIEWS a line of Luau against the running world
  // through `eval_preview`. It runs the same one-shot path as `@eval` but reads
  // live state and DISCARDS the intent batch instead of applying it — so the
  // scrollback shows the return value and the exact writes it *would* make,
  // and the live world is never mutated (hence builder-, not admin-gated).
  //
  // Each submission is INDEPENDENT — `local x = 5` on one line is gone by the
  // next. Carrying state across lines needs a persistent engine-side session.
  // Scrollback + history persist across tab switches (see lib/repl.svelte.js).
  let input = $state('');
  let busy = $state(false);
  let histIx = $state(-1);  // -1 = editing a fresh line
  let scroller;
  let inputEl;

  async function submit() {
    const source = input.trim();
    if (!source || busy) return;
    busy = true;
    input = '';
    repl.history = [...repl.history, source];
    histIx = -1;
    const idx = repl.entries.length;
    repl.entries = [...repl.entries, { source, pending: true, ok: true }];
    scrollSoon();
    const res = await api('eval_preview', { source });
    const next = repl.entries.slice();
    next[idx] = res?.ok
      ? {
          source,
          ok: true,
          returned: res.data?.returned ?? null,
          writes: res.data?.writes || [],
          count: res.data?.write_count ?? 0,
        }
      : { source, ok: false, error: res?.error || 'Eval failed' };
    repl.entries = next;
    busy = false;
    scrollSoon();
    inputEl?.focus();
  }

  function scrollSoon() {
    requestAnimationFrame(() => { if (scroller) scroller.scrollTop = scroller.scrollHeight; });
  }
  // Reopening the tab remounts with restored scrollback — jump to the newest
  // line rather than the top. Runs once, when `scroller` is first bound.
  $effect(() => { if (scroller) scrollSoon(); });

  function onKey(e) {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); submit(); return; }
    // Up/down walk prior submissions — but only from the first/last line, so a
    // multi-line edit can still move the caret normally.
    if (e.key === 'ArrowUp' && !e.shiftKey && caretOnFirstLine(e.target)) {
      if (!repl.history.length) return;
      e.preventDefault();
      histIx = histIx === -1 ? repl.history.length - 1 : Math.max(0, histIx - 1);
      input = repl.history[histIx];
    } else if (e.key === 'ArrowDown' && !e.shiftKey && caretOnLastLine(e.target)) {
      if (histIx === -1) return;
      e.preventDefault();
      if (histIx >= repl.history.length - 1) { histIx = -1; input = ''; }
      else { histIx += 1; input = repl.history[histIx]; }
    }
  }
  const caretOnFirstLine = (ta) => ta.value.slice(0, ta.selectionStart).indexOf('\n') === -1;
  const caretOnLastLine = (ta) => ta.value.slice(ta.selectionEnd).indexOf('\n') === -1;

  function clear() { repl.entries = []; inputEl?.focus(); }
</script>

<div class="repl">
  <header>
    <TerminalIcon size={14} />
    <span class="title">REPL</span>
    <span class="hint">Luau · preview · reads live, writes discarded</span>
    <span class="sp"></span>
    {#if repl.entries.length}
      <button class="clr" onclick={clear} title="Clear scrollback"><Trash2Icon size={13} /> Clear</button>
    {/if}
  </header>

  <div class="scroll" bind:this={scroller}>
    {#if !repl.entries.length}
      <div class="welcome">
        <p>Preview Luau against the running world — reads are live, writes are shown but never applied.</p>
        <p class="dim">Try <code>return get_tick</code>, <code>return get_attr(actor, "hp")</code>, or <code>set_attr(actor, "hp", 5)</code> to see the write it <em>would</em> make. Each line runs on its own — locals don't carry over yet.</p>
      </div>
    {/if}
    {#each repl.entries as e, i (i)}
      <div class="entry">
        <div class="src"><span class="caret">»</span><pre>{e.source}</pre></div>
        {#if e.pending}
          <div class="res dim">…</div>
        {:else if !e.ok}
          <pre class="res err">{e.error}</pre>
        {:else}
          {#if e.returned != null}<pre class="res ret">⇒ {e.returned}</pre>{/if}
          {#if e.writes.length}
            <div class="writes">
              <div class="wh">would apply · {e.count} write{e.count === 1 ? '' : 's'} · not committed</div>
              {#each e.writes as w, wi (wi)}<div class="wrow">{w}</div>{/each}
            </div>
          {:else if e.returned == null}
            <div class="res dim">no return value · no writes</div>
          {/if}
        {/if}
      </div>
    {/each}
  </div>

  <div class="prompt" class:busy>
    <span class="pc">»</span>
    <textarea
      bind:this={inputEl}
      bind:value={input}
      onkeydown={onKey}
      placeholder="Enter to run · Shift+Enter for a new line · ↑ for history"
      rows="1"
      spellcheck="false"
      autocapitalize="off"
      autocomplete="off"
    ></textarea>
    <button class="go" onclick={submit} disabled={busy || !input.trim()} title="Run (Enter)">
      <CornerDownLeftIcon size={14} />
    </button>
  </div>
</div>

<style>
  .repl { display: flex; flex-direction: column; height: 100%; min-height: 0; background: var(--bg-primary, #12100c); }
  header { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); color: var(--text-muted, #9a9186); }
  .title { font-size: 12.5px; font-weight: 600; color: var(--text-secondary, #b6a888); }
  .hint { font-size: 11px; color: var(--text-muted, #8c8378); }
  .sp { flex: 1; }
  .clr { display: inline-flex; align-items: center; gap: 5px; font: inherit; font-size: 12px; color: var(--text-muted, #9a9186); background: none; border: 1px solid var(--border-default, #332c22); border-radius: var(--radius-md, 8px); padding: 4px 9px; cursor: pointer; }
  .clr:hover { border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }

  .scroll { flex: 1; min-height: 0; overflow: auto; padding: 10px 12px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12.5px; line-height: 1.5; }
  .welcome { color: var(--text-muted, #8c8378); padding: 8px 2px; }
  .welcome p { margin: 0 0 6px; }
  .welcome .dim { font-size: 12px; }
  .welcome code { color: var(--accent-amber, #c9956b); background: color-mix(in srgb, var(--accent-amber, #c9956b) 12%, transparent); padding: 1px 5px; border-radius: 4px; }

  .entry { margin-bottom: 10px; }
  .src { display: flex; gap: 7px; align-items: flex-start; }
  .caret { color: var(--accent-amber, #c9956b); flex: none; user-select: none; }
  .src pre { margin: 0; color: var(--text-primary, #ece0c8); white-space: pre-wrap; word-break: break-word; flex: 1; }
  .res { margin: 3px 0 0 16px; padding: 0; color: var(--text-secondary, #b6a888); white-space: pre-wrap; word-break: break-word; }
  .res.err { color: var(--accent-red, #d98d78); }
  .res.ret { color: var(--accent-green, #8fb877); }
  .res.dim { color: var(--text-muted, #8c8378); font-style: italic; }

  /* The would-apply list: real writes the line produced, shown but discarded. */
  .writes { margin: 4px 0 0 16px; }
  .wh { font-size: 10.5px; text-transform: uppercase; letter-spacing: 0.05em; color: var(--accent-amber, #c9956b); margin-bottom: 3px; }
  .wrow { position: relative; padding: 1px 0 1px 14px; color: var(--text-secondary, #b6a888); }
  .wrow::before { content: '+'; position: absolute; left: 0; color: var(--accent-amber, #c9956b); opacity: .8; }

  .prompt { display: flex; align-items: flex-start; gap: 7px; padding: 8px 12px; border-top: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); }
  .prompt.busy { opacity: .7; }
  .pc { color: var(--accent-amber, #c9956b); font-family: var(--font-mono, ui-monospace, monospace); font-size: 13px; padding-top: 5px; user-select: none; }
  textarea { flex: 1; resize: none; background: none; border: none; outline: none; color: var(--text-primary, #ece0c8); font-family: var(--font-mono, ui-monospace, monospace); font-size: 12.5px; line-height: 1.5; padding: 4px 0; max-height: 40vh; field-sizing: content; }
  textarea::placeholder { color: var(--text-muted, #6f675c); }
  .go { flex: none; display: inline-flex; align-items: center; justify-content: center; width: 30px; height: 30px; background: none; border: 1px solid var(--border-default, #332c22); border-radius: var(--radius-md, 8px); color: var(--text-muted, #9a9186); cursor: pointer; }
  .go:hover:not(:disabled) { border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }
  .go:disabled { opacity: .4; cursor: default; }
</style>
