<script>
  import { onMount } from 'svelte';
  import { EditorView, keymap } from '@codemirror/view';
  import { EditorState } from '@codemirror/state';
  import { basicSetup } from 'codemirror';
  import { StreamLanguage, HighlightStyle, syntaxHighlighting } from '@codemirror/language';
  import { lua } from '@codemirror/legacy-modes/mode/lua';
  import { autocompletion } from '@codemirror/autocomplete';
  import { linter, lintGutter } from '@codemirror/lint';
  import { tags as t } from '@lezer/highlight';
  import { api } from '../../lib/api.js';
  import { API_FUNCTIONS, API_GLOBALS, OBJECT_MEMBERS } from './hearth-api.js';

  let { value = $bindable(''), onsave = () => {}, onchange = () => {} } = $props();

  let host;
  let view;
  let syncing = false;

  // --- Hearth API autocomplete ---
  function hearthComplete(context) {
    const member = context.matchBefore(/\.\w*$/);
    if (member && member.from < member.to - 0) {
      return {
        from: member.from + 1,
        options: OBJECT_MEMBERS.map(([label, info]) => ({ label, type: 'property', info })),
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
          hearthLinter,
          lintGutter(),
          saveKeymap,
          hearthTheme,
          EditorView.updateListener.of((u) => {
            // Skip changes we pushed in ourselves (loading a hook) — only user
            // edits mark dirty.
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
