<script>
  import { Button, showFlash, Tooltip, Typeahead } from '@kenn-io/kit-ui';
  import PlayIcon from '@lucide/svelte/icons/play';
  import ZapIcon from '@lucide/svelte/icons/zap';
  import SaveIcon from '@lucide/svelte/icons/save';
  import XIcon from '@lucide/svelte/icons/x';
  import BookOpenIcon from '@lucide/svelte/icons/book-open';
  import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
  import ClockIcon from '@lucide/svelte/icons/clock';
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
  import CodeEditor from '../code/CodeEditor.svelte';
  import HelpPanel from '../code/HelpPanel.svelte';
  import { api } from '../../lib/api.js';
  import { hookTemplate } from '../../lib/hook-templates.js';

  // The one code surface: full CodeMirror (lint, autocomplete, ⌘S) scoped to a
  // single object+hook, opened from the Hooks panel or a map node. Save writes
  // the program; Run evals the buffer as your character.
  let { refId = null, hook = null, objName = '', onclose = () => {}, onsaved = () => {} } = $props();

  let source = $state('');
  let original = $state('');
  let loading = $state(true);
  let saving = $state(false);
  let running = $state(false);
  let output = $state(null);
  const dirty = $derived(source !== original);

  // Collapsible scripting-reference panel docked on the right. Default closed;
  // remembered across visits. A slim rail keeps it discoverable when closed.
  let helpOpen = $state((typeof localStorage !== 'undefined' && localStorage.getItem('co-help')) === '1');
  $effect(() => { try { localStorage.setItem('co-help', helpOpen ? '1' : '0'); } catch {} });

  // Draggable width for the docked panel, remembered across visits. Bounded so
  // it can't swallow the editor (dynamic max leaves the editor a minimum) nor
  // collapse to a sliver.
  const MIN_HELP_W = 240, MAX_HELP_W = 760;
  const readW = () => {
    const n = typeof localStorage !== 'undefined' ? parseInt(localStorage.getItem('co-help-w') || '', 10) : NaN;
    return Number.isFinite(n) ? n : 320;
  };
  let helpWidth = $state(Math.max(MIN_HELP_W, Math.min(readW(), MAX_HELP_W)));
  let midEl;            // the editor+panel row, for measuring during a drag
  let resizing = $state(false);

  function clampW(w) {
    const rect = midEl?.getBoundingClientRect();
    const max = rect ? Math.min(MAX_HELP_W, rect.width - 280) : MAX_HELP_W; // keep the editor usable
    return Math.max(MIN_HELP_W, Math.min(w, Math.max(MIN_HELP_W, max)));
  }
  function onResizeMove(e) {
    if (!midEl) return;
    const rect = midEl.getBoundingClientRect();
    helpWidth = clampW(rect.right - e.clientX);
  }
  function endResize() {
    resizing = false;
    window.removeEventListener('pointermove', onResizeMove);
    window.removeEventListener('pointerup', endResize);
    try { localStorage.setItem('co-help-w', String(helpWidth)); } catch {}
  }
  function startResize(e) {
    e.preventDefault();
    resizing = true;
    window.addEventListener('pointermove', onResizeMove);
    window.addEventListener('pointerup', endResize);
  }
  // Keyboard resize for the separator (WAI-ARIA): arrows nudge, Home/End jump.
  function resizeKey(e) {
    const step = e.shiftKey ? 48 : 16;
    let w = helpWidth;
    if (e.key === 'ArrowLeft') w += step;       // grows the panel (it's on the right)
    else if (e.key === 'ArrowRight') w -= step;
    else if (e.key === 'Home') w = MAX_HELP_W;
    else if (e.key === 'End') w = MIN_HELP_W;
    else return;
    e.preventDefault();
    helpWidth = clampW(w);
    try { localStorage.setItem('co-help-w', String(helpWidth)); } catch {}
  }
  // What the panel highlights: the object + hook currently in the editor.
  const sel = $derived(refId && hook ? { hook, ref: refId, key: objName } : null);

  // Right-click "Look up in Help" from the editor: open the panel and seed its
  // search with the symbol. The nonce makes repeat lookups of the same word
  // re-trigger (the object identity changes even when the term repeats).
  let lookup = $state(null);
  let lookupN = 0;
  function lookupInHelp(word) {
    helpOpen = true;
    lookup = { term: word, n: ++lookupN };
  }

  $effect(() => {
    if (refId && hook) load(refId, hook);
  });

  async function load(ref, h) {
    loading = true;
    output = null;
    const res = await api('list_programs', { ref_id: ref });
    const existing = res?.ok ? res.data.find((x) => x.hook === h) : null;
    // A stub with no function definition → seed a real starter (mark dirty).
    const codeOnly = (existing?.source || '').replace(/--\[\[[\s\S]*?\]\]/g, '').replace(/--[^\n]*/g, '');
    if (/\bfunction\b/.test(codeOnly)) {
      source = original = existing.source;
    } else {
      source = hookTemplate(h);
      original = ''; // so it reads as dirty and Save persists the starter
    }
    loading = false;
  }

  async function save(src) {
    saving = true;
    const body = src ?? source;
    const res = await api('set_program', { ref_id: refId, hook, source: body });
    saving = false;
    if (res?.ok) {
      original = body;
      source = body;
      showFlash(`Saved ${hook}`, { tone: 'success' });
      onsaved();
    } else {
      output = { ok: false, text: res?.error || 'Save failed' };
    }
  }

  async function run() {
    running = true;
    fireOpen = true; // surface output in the test sidebar
    output = { ok: true, text: 'Running…' };
    const res = await api('eval', { source });
    output = res?.ok ? { ok: true, text: res.data?.output ?? '(no output)' } : { ok: false, text: res?.error || 'Eval failed' };
    running = false;
  }

  // Preview-fire: run THIS hook as if the event happened, with a chosen actor
  // and room, and see the writes/emits it would make — without committing. The
  // current (unsaved) buffer is what fires, so you can test before you save.
  let fireOpen = $state(false);
  let firing = $state(false);
  let fireActors = $state([]); // players + npcs, pickable as `actor`
  let fireRooms = $state([]);  // rooms, pickable as `room`
  let fireActor = $state('');  // '' → the caller's own character
  let fireRoom = $state('');   // '' → auto (actor's / this's location)
  let fireLoaded = false;

  async function loadFireTargets() {
    if (fireLoaded) return;
    fireLoaded = true;
    const res = await api('list_objects_full', { limit: 3000 });
    const objs = res?.ok ? (res.data?.objects || []) : [];
    fireActors = objs.filter((o) => o.kind === 'player' || o.kind === 'npc');
    fireRooms = objs.filter((o) => o.kind === 'room');
  }
  function toggleTest() {
    fireOpen = !fireOpen;
    if (fireOpen) loadFireTargets();
  }
  const labelFor = (ref, list) => {
    const o = list.find((x) => x.ref_id === ref);
    return o ? `${o.title || o.key} (${ref})` : ref;
  };
  // Typeahead options: a default row ('' → you / auto) plus every candidate,
  // searchable by title/key and ref (ref rides in `meta` so it's matched too).
  const actorOpts = $derived([
    { name: '', label: '(you)', meta: 'your character' },
    ...fireActors.map((a) => ({ name: a.ref_id, label: a.title || a.key, meta: a.ref_id })),
  ]);
  const roomOpts = $derived([
    { name: '', label: '(auto)', meta: "this's location" },
    ...fireRooms.map((r) => ({ name: r.ref_id, label: r.title || r.key, meta: r.ref_id })),
  ]);
  async function previewFire() {
    firing = true;
    output = { ok: true, text: 'Firing…' };
    const res = await api('preview_hook', {
      ref_id: refId,
      hook,
      source,
      actor_ref: fireActor || undefined,
      room_ref: fireRoom || undefined,
    });
    firing = false;
    if (!res?.ok) { output = { ok: false, text: res?.error || 'Preview failed' }; return; }
    const d = res.data;
    const who = fireActor ? labelFor(fireActor, fireActors) : '(you)';
    const where = fireRoom ? labelFor(fireRoom, fireRooms) : '(auto)';
    const lines = [`Preview-fire ${hook} · actor ${who} · room ${where}`];
    if (d.denied) lines.push('⚠ guard returned false — this action would be vetoed');
    if (!d.write_count) {
      lines.push('no writes — nothing would change');
    } else {
      lines.push(`${d.write_count} write${d.write_count === 1 ? '' : 's'} (not committed):`);
      for (const w of d.writes) lines.push('  + ' + w);
    }
    output = { ok: true, text: lines.join('\n') };
  }

  // Version history: every Save records a version (server-side, DB-backed), so
  // this drawer is the recovery path for "I broke it — put back what worked".
  // Restore is non-destructive: it appends the old source as a NEW version
  // rather than rewinding, matching `@program/restore`.
  let histOpen = $state(false);
  let history = $state([]);
  let histLoading = $state(false);
  let previewVer = $state(null); // the version whose source is expanded inline

  async function toggleHistory() {
    histOpen = !histOpen;
    if (histOpen) await loadHistory();
    else previewVer = null;
  }
  async function loadHistory() {
    if (!refId || !hook) return;
    histLoading = true;
    previewVer = null;
    const res = await api('program_history', { ref_id: refId, hook });
    history = res?.ok ? (res.data || []) : [];
    histLoading = false;
  }
  async function restore(v) {
    const res = await api('program_restore', { ref_id: refId, hook, version: v.version });
    if (res?.ok) {
      showFlash(`Restored ${hook} · v${v.version}`, { tone: 'success' });
      histOpen = false;
      previewVer = null;
      await load(refId, hook);
      onsaved();
    } else {
      showFlash(res?.error || 'Restore failed', { tone: 'danger' });
    }
  }
  // created_at may arrive as ISO text or a unix timestamp; render either, and
  // fall back to the raw value if it's neither.
  function fmtTime(t) {
    if (t == null) return '';
    const d = typeof t === 'number' ? new Date(t * 1000) : new Date(t);
    return isNaN(d.getTime()) ? String(t) : d.toLocaleString();
  }
</script>

<div class="co">
  <header>
    <div class="cur">
      <span class="obj">{objName || refId}</span>
      <span class="sep">·</span>
      <b>{hook}</b>
      {#if dirty}<span class="dot">●</span>{/if}
    </div>
    <span class="sp"></span>
    <Tooltip text="Run — evaluate the buffer as your character"><Button size="sm" onclick={run} disabled={running}><PlayIcon size={13} /> Run</Button></Tooltip>
    <Tooltip text="Preview-fire this hook — see what it would do, uncommitted">
      <button class="help-btn" class:on={fireOpen} aria-expanded={fireOpen} onclick={toggleTest}>
        <ZapIcon size={14} /> <span>Preview-fire</span>
      </button>
    </Tooltip>
    <Tooltip text="Save program (⌘S)"><Button size="sm" tone="accent" onclick={() => save()} disabled={saving || !dirty}><SaveIcon size={13} /> Save</Button></Tooltip>
    <Tooltip text="Version history">
      <button class="help-btn" class:on={histOpen} aria-expanded={histOpen} onclick={toggleHistory}>
        <ClockIcon size={14} /> <span>History</span>
      </button>
    </Tooltip>
    <Tooltip text={helpOpen ? 'Hide scripting reference' : 'Show scripting reference'}>
      <button class="help-btn" class:on={helpOpen} aria-expanded={helpOpen}
        aria-controls={helpOpen ? 'co-help-panel' : undefined}
        onclick={() => (helpOpen = !helpOpen)}>
        <BookOpenIcon size={14} /> <span>Help</span>
      </button>
    </Tooltip>
    <Tooltip text="Close editor" align="end"><button class="x" aria-label="Close editor" onclick={onclose}><XIcon size={16} /></button></Tooltip>
  </header>

  <div class="mid" class:resizing bind:this={midEl}>
    {#if histOpen}
      <!-- Version drawer over the editor. Click a version to preview its source
           inline; Restore re-saves it as a new version (non-destructive). -->
      <aside class="hist" aria-label="Version history">
        <div class="hist-h">
          <ClockIcon size={13} /> <span>History</span>
          <span class="sp"></span>
          <button class="x" onclick={() => { histOpen = false; previewVer = null; }} aria-label="Close history"><XIcon size={14} /></button>
        </div>
        <div class="hist-list">
          {#if histLoading}
            <div class="none">Loading…</div>
          {:else if !history.length}
            <div class="none">No saved versions yet — Save this hook to start its history.</div>
          {:else}
            {#each [...history].reverse() as v (v.version)}
              <div class="hrow" class:sel={previewVer?.version === v.version} class:del={v.deleted}>
                <button class="hmeta" onclick={() => (previewVer = previewVer?.version === v.version ? null : v)}>
                  <span class="hv">v{v.version}</span>
                  <span class="hcol">
                    <span class="hwhen">{fmtTime(v.created_at)}</span>
                    <span class="hwho">{v.author || 'unknown'}{#if v.deleted} · deleted{/if}</span>
                  </span>
                </button>
                <Tooltip text="Restore as a new version"><button class="hrestore" onclick={() => restore(v)} aria-label="Restore version {v.version}"><RotateCcwIcon size={12} /></button></Tooltip>
              </div>
              {#if previewVer?.version === v.version}
                <pre class="hprev">{v.deleted ? '(this version deleted the program)' : v.source}</pre>
              {/if}
            {/each}
          {/if}
        </div>
      </aside>
    {/if}
    <div class="edit">
      {#if loading}
        <div class="none">Loading…</div>
      {:else}
        {#key refId + hook}
          <CodeEditor bind:value={source} {hook} onsave={save} onchange={() => {}} onlookup={lookupInHelp} />
        {/key}
      {/if}
    </div>

    {#if fireOpen || helpOpen}
      <!-- Drag (or arrow-key) to resize the docked sidebar. -->
      <div class="resizer" role="separator" aria-orientation="vertical" tabindex="0"
        aria-label="Resize sidebar" aria-valuenow={Math.round(helpWidth)}
        aria-valuemin={MIN_HELP_W} aria-valuemax={MAX_HELP_W}
        onpointerdown={startResize} onkeydown={resizeKey}></div>
      <div class="side" style="width: {helpWidth}px">
        {#if fireOpen}
          <!-- Preview-fire: pick the context, fire, read the result — all docked
               here so nothing runs against the live world. -->
          <section class="test-panel" aria-label="Preview-fire">
            <div class="tp-h">
              <ZapIcon size={13} /> <span>Preview-fire</span>
              <span class="sp"></span>
              <button class="x" onclick={() => (fireOpen = false)} aria-label="Close preview-fire"><XIcon size={14} /></button>
            </div>
            <div class="tp-ctx">
              <div class="tp-row"><span class="tp-lbl">this</span><span class="tp-this">{objName || refId}</span></div>
              <div class="tp-row">
                <span class="tp-lbl">actor</span>
                <div class="tp-ta"><Typeahead options={actorOpts} value={fireActor} fallbackLabel="(you)"
                  placeholder="search players / npcs…" emptyLabel="No matching object"
                  onselect={(v) => { fireActor = v; }} /></div>
              </div>
              <div class="tp-row">
                <span class="tp-lbl">room</span>
                <div class="tp-ta"><Typeahead options={roomOpts} value={fireRoom} fallbackLabel="(auto)"
                  placeholder="search rooms…" emptyLabel="No matching room"
                  onselect={(v) => { fireRoom = v; }} /></div>
              </div>
              <button class="fb-go" onclick={previewFire} disabled={firing}>
                <ZapIcon size={13} /> {firing ? 'Firing…' : 'Preview-fire'}
              </button>
            </div>
            <div class="tp-out">
              {#if output}
                <div class="oh">{output.ok ? 'result' : 'error'}<Tooltip text="Clear" align="end"><button aria-label="Clear output" onclick={() => (output = null)}>✕</button></Tooltip></div>
                <pre class:err={!output.ok}>{output.text}</pre>
              {:else}
                <div class="tp-empty">Fire the hook to see the writes and emits it would make. Nothing is committed to the live world.</div>
              {/if}
            </div>
          </section>
        {/if}
        {#if helpOpen}
          <div id="co-help-panel" class="help-wrap"><HelpPanel {sel} {lookup} open={helpOpen} onclose={() => (helpOpen = false)} /></div>
        {/if}
      </div>
    {:else}
      <!-- Slim rail so the reference stays discoverable when the sidebar's closed. -->
      <button class="rail" aria-expanded="false" aria-controls="co-help-panel"
        onclick={() => (helpOpen = true)} title="Show scripting reference">
        <ChevronLeftIcon size={14} />
        <span class="rail-label">Help</span>
      </button>
    {/if}
  </div>
</div>

<style>
  .co { display: flex; flex-direction: column; height: 100%; min-height: 0; background: var(--bg-primary, #12100c); }
  header { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--border-default, #2a2419); background: var(--bg-surface, #17140f); }
  .cur { display: flex; align-items: center; gap: 6px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12.5px; color: var(--text-muted, #9a9186); min-width: 0; }
  .cur .obj { color: var(--text-secondary, #b6a888); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cur b { color: var(--accent-amber, #c9956b); }
  .sep { color: var(--text-muted, #8c8378); }
  .dot { color: var(--accent-amber, #c9956b); }
  .sp { flex: 1; }
  .x { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; padding: 2px; line-height: 0; }
  .x:hover { color: var(--text-primary, #ece0c8); }

  .help-btn { display: inline-flex; align-items: center; gap: 5px; font: inherit; font-size: 12px; color: var(--text-muted, #9a9186); background: none; border: 1px solid var(--border-default, #332c22); border-radius: var(--radius-md, 8px); padding: 4px 9px; cursor: pointer; }
  .help-btn:hover, .help-btn.on { border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }

  /* Preview-fire context bar. */
  /* Editor + a single docked sidebar sit side by side. The sidebar stacks the
     Preview-fire panel and the Help reference; its width is set inline. */
  .mid { flex: 1; min-height: 0; display: flex; position: relative; }
  /* While dragging, suppress text selection and force the resize cursor. */
  .mid.resizing { cursor: col-resize; user-select: none; }
  .edit { flex: 1; min-width: 0; min-height: 0; overflow: hidden; }
  .side { flex: none; min-width: 0; min-height: 0; display: flex; flex-direction: column; background: var(--bg-surface, #17140f); }
  .help-wrap { flex: 1 1 0; min-height: 140px; display: flex; }
  .help-wrap :global(.hp) { flex: 1; min-height: 0; border-left: none; }

  /* Preview-fire sidebar panel: context pickers on top, results filling below. */
  .test-panel { flex: 1 1 0; min-height: 180px; display: flex; flex-direction: column; min-width: 0; }
  /* When Help is also open, the two panels share the column with a divider. */
  .test-panel:not(:last-child) { border-bottom: 1px solid var(--border-default, #2a2419); }
  .tp-h { display: flex; align-items: center; gap: 6px; padding: 8px 10px 8px 12px; border-bottom: 1px solid var(--border-default, #2a2419); font-size: 12px; color: var(--text-secondary, #b6a888); }
  .tp-h .sp { flex: 1; }
  .tp-h .x { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; padding: 2px; line-height: 0; }
  .tp-h .x:hover { color: var(--text-primary, #ece0c8); }
  .tp-ctx { display: flex; flex-direction: column; gap: 8px; padding: 10px 12px; border-bottom: 1px solid var(--border-muted, #211d16); }
  .tp-row { display: flex; align-items: center; gap: 8px; }
  .tp-lbl { flex: none; width: 44px; font-size: 11px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--text-muted, #8c8378); }
  .tp-this { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--text-secondary, #b6a888); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tp-ta { flex: 1; min-width: 0; }
  .fb-go { display: inline-flex; align-items: center; justify-content: center; gap: 6px; margin-top: 2px; font: inherit; font-size: 12.5px; color: var(--accent-amber, #c9956b); background: color-mix(in srgb, var(--accent-amber, #c9956b) 12%, transparent); border: 1px solid color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); border-radius: var(--radius-md, 8px); padding: 6px 11px; cursor: pointer; }
  .fb-go:hover:not(:disabled) { background: color-mix(in srgb, var(--accent-amber, #c9956b) 20%, transparent); }
  .fb-go:disabled { opacity: .55; cursor: default; }
  .tp-out { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
  .tp-out pre { margin: 0; padding: 10px 12px; overflow: auto; font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; line-height: 1.55; color: var(--text-primary, #ece0c8); white-space: pre-wrap; word-break: break-word; }
  .tp-out pre.err { color: var(--accent-red, #d98d78); }
  .tp-empty { padding: 14px 12px; font-size: 12px; line-height: 1.5; color: var(--text-muted, #8c8378); }

  /* The resizer is the divider now — drop the panel's own left border. */
  /* Drag handle between editor and panel. A hair-thin line that thickens and
     tints on hover/drag — no chunky gutter. */
  .resizer { flex: none; width: 5px; margin: 0 -2px; z-index: 4; cursor: col-resize; background: transparent; border: none; padding: 0; position: relative; }
  .resizer::before { content: ''; position: absolute; inset: 0 2px; background: var(--border-default, #2a2419); transition: background 0.12s; }
  .resizer:hover::before, .resizer:focus-visible::before, .mid.resizing .resizer::before { background: var(--accent-amber, #c9956b); }
  .resizer:focus-visible { outline: none; }
  .rail { position: absolute; right: 0; top: 50%; transform: translateY(-50%); z-index: 5; display: flex; flex-direction: column; align-items: center; gap: 6px; width: 26px; padding: 12px 0; background: var(--bg-surface, #17140f); border: 1px solid var(--border-default, #2a2419); border-right: none; border-radius: 8px 0 0 8px; color: var(--text-muted, #9a9186); cursor: pointer; box-shadow: -6px 0 18px -12px rgba(0,0,0,.6); }
  .rail:hover { color: var(--accent-amber, #c9956b); width: 30px; }
  .rail-label { writing-mode: vertical-rl; text-orientation: mixed; font-size: 11px; letter-spacing: .06em; text-transform: uppercase; }

  @media (max-width: 720px) {
    /* Too narrow to dock — float the sidebar over the editor. */
    .side { position: absolute; inset: 0 0 0 auto; width: min(340px, 100%); z-index: 6; box-shadow: -12px 0 28px -16px rgba(0,0,0,.7); }
  }
  /* Version-history drawer — floats over the editor's left edge. */
  .hist { position: absolute; inset: 0 auto 0 0; z-index: 7; display: flex; flex-direction: column; width: min(400px, 66%); background: var(--bg-surface, #17140f); border-right: 1px solid var(--border-default, #2a2419); box-shadow: 12px 0 28px -18px rgba(0,0,0,.7); }
  .hist-h { display: flex; align-items: center; gap: 6px; padding: 8px 10px 8px 12px; border-bottom: 1px solid var(--border-default, #2a2419); font-size: 12px; color: var(--text-secondary, #b6a888); }
  .hist-h .sp { flex: 1; }
  .hist-h .x { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; padding: 2px; line-height: 0; }
  .hist-h .x:hover { color: var(--text-primary, #ece0c8); }
  .hist-list { flex: 1; min-height: 0; overflow: auto; padding: 4px 0 12px; }
  .hrow { display: flex; align-items: stretch; gap: 2px; padding: 0 6px 0 0; }
  .hrow.sel { background: color-mix(in srgb, var(--accent-amber, #c9956b) 10%, transparent); }
  .hmeta { display: flex; align-items: center; gap: 9px; flex: 1; min-width: 0; text-align: left; background: none; border: none; border-left: 2px solid transparent; padding: 7px 8px 7px 12px; cursor: pointer; color: inherit; font: inherit; }
  .hrow.sel .hmeta { border-left-color: var(--accent-amber, #c9956b); }
  .hmeta:hover { background: color-mix(in srgb, var(--accent-amber, #c9956b) 7%, transparent); }
  .hv { font-family: var(--font-mono, ui-monospace, monospace); font-size: 12px; color: var(--accent-amber, #c9956b); flex: none; }
  .hcol { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
  .hwhen { font-size: 11.5px; color: var(--text-primary, #ece0c8); }
  .hwho { font-size: 10.5px; color: var(--text-muted, #8c8378); }
  .hrow.del .hwho { color: var(--accent-red, #d07a5a); }
  .hrestore { flex: none; display: inline-flex; align-items: center; align-self: center; justify-content: center; width: 26px; height: 26px; background: none; border: 1px solid var(--border-default, #332c22); border-radius: var(--radius-md, 8px); color: var(--text-muted, #9a9186); cursor: pointer; }
  .hrestore:hover { border-color: color-mix(in srgb, var(--accent-amber, #c9956b) 45%, transparent); color: var(--accent-amber, #c9956b); }
  .hprev { margin: 0 12px 8px; padding: 8px 10px; max-height: 220px; overflow: auto; background: var(--bg-primary, #12100c); border: 1px solid var(--border-muted, #211d16); border-radius: 6px; font-family: var(--font-mono, ui-monospace, monospace); font-size: 11.5px; line-height: 1.5; color: var(--text-secondary, #b6a888); white-space: pre-wrap; word-break: break-word; }

  .none { color: var(--text-muted, #8c8378); font-style: italic; padding: 14px; }
  .oh { display: flex; align-items: center; justify-content: space-between; flex: none; font-size: var(--fs-label); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted, #9a9186); padding: 6px 12px; border-bottom: 1px solid var(--border-muted, #211d16); }
  .oh button { background: none; border: none; color: var(--text-muted, #8c8378); cursor: pointer; }
</style>
