<script>
  import { onMount } from 'svelte';
  import { EditorView, keymap, hoverTooltip } from '@codemirror/view';
  import { EditorState } from '@codemirror/state';
  import { basicSetup } from 'codemirror';
  import { StreamLanguage, HighlightStyle, syntaxHighlighting } from '@codemirror/language';
  import { lua } from '@codemirror/legacy-modes/mode/lua';
  import { autocompletion } from '@codemirror/autocomplete';
  import { linter, lintGutter } from '@codemirror/lint';
  import { tags as t } from '@lezer/highlight';
  import { api } from '../../lib/api.js';
  import { loadHooks } from '../../lib/hooks.js';
  import { API_FUNCTIONS, API_GLOBALS, OBJECT_MEMBERS } from './hearth-api.js';

  let { value = $bindable(''), onsave = () => {}, onchange = () => {}, onlookup = null, oncursor = null, hook = '' } = $props();

  let host;
  let view;
  let syncing = false;

  // Flat name → reference entry, so a bare identifier (emit, this, display_name)
  // resolves to its signature + doc for hover tooltips and right-click lookup.
  // Functions and globals win over object members on a name clash; the same
  // hearth-api.js lists power autocomplete, kept honest by the api.rs tests.
  const API_INDEX = (() => {
    const m = new Map();
    for (const [name, sig, doc] of API_FUNCTIONS) m.set(name, { sig, doc });
    for (const [name, sig, doc] of API_GLOBALS) if (!m.has(name)) m.set(name, { sig, doc });
    for (const [name, info] of OBJECT_MEMBERS) if (!m.has(name)) m.set(name, { sig: name, doc: info });
    return m;
  })();

  // The identifier under a document position, or null. Word chars are Luau
  // identifier chars ([A-Za-z0-9_]); we widen left/right from pos to the token.
  function wordAt(state, pos) {
    const line = state.doc.lineAt(pos);
    const text = line.text;
    let i = pos - line.from, a = i, b = i;
    const isW = (c) => c && /\w/.test(c);
    while (a > 0 && isW(text[a - 1])) a--;
    while (b < text.length && isW(text[b])) b++;
    if (a === b) return null;
    return { word: text.slice(a, b), from: line.from + a, to: line.from + b };
  }

  // Identifiers known to hold a GameObject, so `x.` completes object fields.
  // The three hook params are always objects; add any local bound from an
  // object-returning call or an object-list loop. A heuristic, not real type
  // inference — enough to stop offering object fields after every dot.
  function objectIdents(doc) {
    const set = new Set(['this', 'actor', 'room']);
    const assign = /\b([A-Za-z_]\w*)\s*=\s*(?:get_object|get_location|get_owner|spawn)\s*\(/g;
    const loop = /\bfor\b[^\n=]*?\b([A-Za-z_]\w*)\s+in\b[^\n]*?(?:get_room_contents|get_inventory|all_objects|find_by_tag|find_by_attr)\s*\(/g;
    let m;
    while ((m = assign.exec(doc))) set.add(m[1]);
    while ((m = loop.exec(doc))) set.add(m[1]);
    return set;
  }

  // The parameters a hook is called with, so completing a hook name after
  // `function` lands the right signature. Almost every hook is (this, actor,
  // room); on_tick gets a persistent `state` table instead of an actor, and
  // on_create only ever sees `this` — the two people always get wrong.
  function hookSignature(name) {
    if (name === 'on_tick') return '(this, state, room)';
    if (name === 'on_create') return '(this)';
    return '(this, actor, room)';
  }

  // Hook vocabulary (on_* events, can_* guards, cmd_* commands) with the
  // engine's own descriptions, loaded from the live `list_hooks` vocabulary.
  // Feeds hover tooltips and right-click lookup so the hook *functions* a
  // script defines are documented, not just the API calls inside them. Mutated
  // in place after the async load; the hover/contextmenu handlers read it at
  // event time, so a populated map is visible by the time anyone hovers.
  const HOOK_INDEX = new Map();
  loadHooks()
    .then(({ known }) => {
      for (const h of known || []) {
        HOOK_INDEX.set(h.name, {
          sig: `function ${h.name}${hookSignature(h.name)}`,
          doc: h.describes || '',
        });
      }
    })
    .catch(() => {});

  // A known symbol's reference entry: an API function/global/member, or a hook.
  function symbolInfo(word) {
    return API_INDEX.get(word) || HOOK_INDEX.get(word) || null;
  }

  // The hook function whose body `pos` sits in, or null. Scans upward for the
  // nearest top-level `function <name>(`; a top-level `end` above the cursor
  // (before any function) means we've left the enclosing scope. Reports a name
  // only when it reads as a hook (known, or an on_/cmd_ prefix) so a plain
  // top-level helper doesn't masquerade as the "current" hook.
  function enclosingHook(state, pos) {
    const cur = state.doc.lineAt(pos).number;
    for (let n = cur; n >= 1; n--) {
      const text = state.doc.line(n).text;
      if (n < cur && /^end\b/.test(text)) return null;
      const m = /^function\s+([A-Za-z_]\w*)\s*\(/.exec(text);
      if (m) {
        const name = m[1];
        const isHook = HOOK_INDEX.has(name) || name.startsWith('on_') || name.startsWith('cmd_');
        return isHook ? name : null;
      }
    }
    return null;
  }

  // --- Hearth API autocomplete ---
  async function hearthComplete(context) {
    // Member access: only offer object fields when the receiver is known to be
    // an object (a hook param or an object-bound local), not after any dot —
    // `str.` and nested tables like `actor.attrs.` shouldn't get object fields.
    const member = context.matchBefore(/[A-Za-z_]\w*\.\w*$/);
    if (member) {
      const recv = /([A-Za-z_]\w*)\.\w*$/.exec(member.text)?.[1];
      if (recv && objectIdents(context.state.doc.toString()).has(recv)) {
        return {
          from: member.from + member.text.indexOf('.') + 1,
          options: OBJECT_MEMBERS.map(([label, info]) => ({ label, type: 'property', info })),
        };
      }
      return null; // unknown receiver — don't guess object fields
    }
    // Defining a hook: after `function `, offer the engine's hook vocabulary,
    // each expanding to its full signature. Grouped guards/events; the hook
    // this editor is already editing floats to the top.
    const fn = context.matchBefore(/function\s+\w*$/);
    if (fn) {
      const partial = /function\s+(\w*)$/.exec(fn.text)?.[1] ?? '';
      const { known } = await loadHooks();
      if (!known.length) return null;
      return {
        from: fn.to - partial.length,
        options: known.map((h) => ({
          label: h.name,
          apply: h.name + hookSignature(h.name),
          type: 'function',
          detail: h.describes || '',
          section: h.name.startsWith('can_') ? 'Guards · can_' : 'Events · on_',
          boost: h.name === hook ? 99 : 0,
        })),
      };
    }
    const word = context.matchBefore(/\w*/);
    if (!word || (word.from === word.to && !context.explicit)) return null;
    return {
      from: word.from,
      options: [
        ...API_FUNCTIONS.map(([label, detail, info]) => ({ label, type: 'function', detail, info, apply: label + '(' })),
        ...API_GLOBALS.map(([label, detail, info]) => ({ label, type: 'variable', detail, info })),
      ],
    };
  }

  // --- hover tooltips: signature + doc for a known API symbol ---
  const hearthHover = hoverTooltip((view, pos) => {
    const w = wordAt(view.state, pos);
    const entry = w && symbolInfo(w.word);
    if (!entry) return null;
    return {
      pos: w.from,
      end: w.to,
      above: true,
      create() {
        const dom = document.createElement('div');
        dom.className = 'cm-hearth-tip';
        const sig = document.createElement('code');
        sig.className = 'cm-hearth-tip-sig';
        sig.textContent = entry.sig;
        dom.appendChild(sig);
        if (entry.doc && entry.doc !== entry.sig) {
          const p = document.createElement('div');
          p.className = 'cm-hearth-tip-doc';
          p.textContent = entry.doc;
          dom.appendChild(p);
        }
        if (onlookup) {
          const hint = document.createElement('div');
          hint.className = 'cm-hearth-tip-hint';
          hint.textContent = 'Right-click → Look up in Help';
          dom.appendChild(hint);
        }
        return { dom };
      },
    };
  }, { hoverTime: 250 });

  // --- right-click a known symbol → open it in the Help panel ---
  // Only hijacks the native menu when there's actually something to look up;
  // right-clicking anything else leaves the browser's own menu alone.
  const hearthContextMenu = EditorView.domEventHandlers({
    contextmenu(e, view) {
      if (!onlookup) return false;
      const pos = view.posAtCoords({ x: e.clientX, y: e.clientY });
      if (pos == null) return false;
      const w = wordAt(view.state, pos);
      if (!w || !symbolInfo(w.word)) return false;
      e.preventDefault();
      onlookup(w.word);
      return true;
    },
  });

  // --- lint via the engine's compile-check ---
  const hearthLinter = linter(
    async (v) => {
      const code = v.state.doc.toString();
      if (!code.trim()) return [];
      let res;
      try { res = await api('check_program', { source: code }); } catch (e) { return []; }
      if (!res?.ok || res.data?.valid) return [];
      const msg = res.data?.error || 'Syntax error';
      // Luau errors read: 'syntax error: [string "…"]:LINE: message'. Read the
      // line number after the chunk-name bracket, not the one inside it.
      const m = msg.match(/\]:(\d+):\s*(.*)$/);
      const doc = v.state.doc;
      const line = m ? Math.min(Math.max(parseInt(m[1], 10), 1), doc.lines) : 1;
      const lineObj = doc.line(line);
      const message = m ? m[2] : msg.replace(/^syntax error:\s*/, '');
      return [{ from: lineObj.from, to: lineObj.to, severity: 'error', message }];
    },
    { delay: 500 },
  );

  // --- theme + syntax colours (kit-ui tokens, so they follow light/dark) ---
  const hearthHighlight = HighlightStyle.define([
    { tag: t.keyword, color: 'var(--accent-red, #c96a5a)' },
    { tag: t.controlKeyword, color: 'var(--accent-red, #c96a5a)' },
    { tag: [t.string, t.special(t.string)], color: 'var(--accent-green, #8fb877)' },
    { tag: [t.comment, t.lineComment, t.blockComment], color: 'var(--text-muted, #8c8378)', fontStyle: 'italic' },
    { tag: [t.number, t.bool, t.null], color: 'var(--accent-amber, #c9956b)' },
    { tag: [t.function(t.variableName), t.function(t.propertyName)], color: 'var(--accent-blue, #6ea3d0)' },
    { tag: t.operator, color: 'var(--text-secondary, #b6a888)' },
    { tag: t.propertyName, color: 'var(--text-primary, #ece0c8)' },
    { tag: t.variableName, color: 'var(--text-primary, #ece0c8)' },
  ]);

  const hearthTheme = EditorView.theme({
    '&': { color: 'var(--text-primary, #ece0c8)', backgroundColor: 'var(--bg-primary, #12100c)', height: '100%', fontSize: '13px' },
    '.cm-scroller': { fontFamily: 'var(--font-mono, ui-monospace, monospace)', lineHeight: '1.55' },
    '.cm-gutters': { backgroundColor: 'var(--bg-surface, #17140f)', color: 'var(--text-muted, #8c8378)', border: 'none' },
    '.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--accent-amber, #c9956b) 8%, transparent)' },
    '.cm-activeLineGutter': { backgroundColor: 'color-mix(in srgb, var(--accent-amber, #c9956b) 10%, transparent)' },
    '&.cm-focused .cm-cursor': { borderLeftColor: 'var(--accent-amber, #c9956b)' },
    '.cm-selectionBackground, .cm-content ::selection': { backgroundColor: 'color-mix(in srgb, var(--accent-amber, #c9956b) 24%, transparent)' },
    '&.cm-focused .cm-selectionBackground': { backgroundColor: 'color-mix(in srgb, var(--accent-amber, #c9956b) 28%, transparent)' },
    '.cm-tooltip': { backgroundColor: 'var(--bg-surface, #17140f)', border: '1px solid var(--border-default, #332c22)', color: 'var(--text-primary, #ece0c8)', borderRadius: '7px' },
    '.cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]': { backgroundColor: 'color-mix(in srgb, var(--accent-amber, #c9956b) 22%, transparent)', color: 'var(--text-primary, #ece0c8)' },
    '.cm-completionDetail': { color: 'var(--text-muted, #9a9186)', fontStyle: 'normal', marginLeft: '8px' },
    '.cm-tooltip .cm-hearth-tip': { padding: '7px 9px', maxWidth: '360px' },
    '.cm-hearth-tip-sig': { display: 'block', fontFamily: 'var(--font-mono, ui-monospace, monospace)', fontSize: '12px', color: 'var(--accent-green, #8fb877)', whiteSpace: 'pre-wrap', wordBreak: 'break-word' },
    '.cm-hearth-tip-doc': { marginTop: '5px', fontSize: '11.5px', lineHeight: '1.45', color: 'var(--text-secondary, #b6a888)' },
    '.cm-hearth-tip-hint': { marginTop: '7px', paddingTop: '6px', borderTop: '1px solid var(--border-muted, #2a2419)', fontSize: '10.5px', color: 'var(--text-muted, #8c8378)' },
  });

  const saveKeymap = keymap.of([
    { key: 'Mod-s', preventDefault: true, run: (v) => { onsave(v.state.doc.toString()); return true; } },
  ]);

  onMount(() => {
    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: value,
        extensions: [
          basicSetup,
          StreamLanguage.define(lua),
          syntaxHighlighting(hearthHighlight),
          autocompletion({ override: [hearthComplete] }),
          hearthHover,
          hearthContextMenu,
          hearthLinter,
          lintGutter(),
          saveKeymap,
          hearthTheme,
          EditorView.updateListener.of((u) => {
            // Report the hook function the cursor sits in, so the Help panel's
            // "current" card follows the caret through a multi-hook script.
            if ((u.selectionSet || u.docChanged) && oncursor) {
              oncursor(enclosingHook(u.state, u.state.selection.main.head));
            }
            // Skip doc changes we pushed in ourselves (loading a hook) — only
            // user edits mark dirty.
            if (!u.docChanged || syncing) return;
            syncing = true;
            value = u.state.doc.toString();
            onchange(value);
            syncing = false;
          }),
        ],
      }),
    });
    return () => view?.destroy();
  });

  // Push external value changes (switching hooks) into the editor.
  $effect(() => {
    const v = value;
    if (view && !syncing && v !== view.state.doc.toString()) {
      syncing = true;
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: v } });
      syncing = false;
    }
  });
</script>

<div class="cm-host" bind:this={host}></div>

<style>
  .cm-host { height: 100%; min-height: 0; overflow: hidden; }
  .cm-host :global(.cm-editor) { height: 100%; }
  .cm-host :global(.cm-editor.cm-focused) { outline: none; }
</style>
