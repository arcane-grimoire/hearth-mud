<script>
  import { onMount } from 'svelte';
  import { EditorView, keymap } from '@codemirror/view';
  import { EditorState } from '@codemirror/state';
  import { basicSetup } from 'codemirror';
  import { StreamLanguage, HighlightStyle, syntaxHighlighting } from '@codemirror/language';
  import { autocompletion } from '@codemirror/autocomplete';
  import { linter, lintGutter } from '@codemirror/lint';
  import { tags as t } from '@lezer/highlight';
  import { api } from '../../lib/api.js';
  import { inkStreamParser, inkComplete, parseInkError } from './ink-mode.js';

  // The Ink counterpart to code/CodeEditor.svelte — same shell (theme built
  // from kit-ui tokens, Mod-S to save, external-value sync), but with the Ink
  // language mode, Ink autocomplete, and a linter backed by `ink_compile`.
  //
  // `minimal` strips the linter and autocomplete for a distraction-free "raw"
  // mode — highlighting only, nothing that pops up or underlines as you type.
  // `onfail` fires if CodeMirror can't initialise, so the parent can drop to a
  // plain-textarea fallback instead of showing a dead pane.
  let { value = $bindable(''), onsave = () => {}, onchange = () => {}, minimal = false, onfail = () => {} } = $props();

  let host;
  let view;
  let syncing = false;

  const inkLinter = linter(
    async (v) => {
      const code = v.state.doc.toString();
      if (!code.trim()) return [];
      let res;
      try {
        res = await api('ink_compile', { source: code });
      } catch (e) {
        return [];
      }
      if (!res?.ok || res.data?.valid) return [];
      const parsed = parseInkError(res.data?.errors);
      if (!parsed) return [];
      const doc = v.state.doc;
      const line = parsed.line ? Math.min(Math.max(parsed.line, 1), doc.lines) : 1;
      const lineObj = doc.line(line);
      return [{ from: lineObj.from, to: lineObj.to, severity: 'error', message: parsed.message }];
    },
    { delay: 500 },
  );

  const inkHighlight = HighlightStyle.define([
    { tag: t.heading, color: 'var(--accent-amber, #c9956b)', fontWeight: '700' },
    { tag: t.strong, color: 'var(--accent-amber, #c9956b)' },
    { tag: t.controlKeyword, color: 'var(--accent-blue, #6ea3d0)', fontWeight: '600' },
    { tag: t.keyword, color: 'var(--accent-red, #c96a5a)' },
    { tag: t.definitionKeyword, color: 'var(--accent-red, #c96a5a)' },
    { tag: t.meta, color: 'var(--accent-green, #8fb877)' },
    { tag: t.labelName, color: 'var(--accent-blue, #6ea3d0)' },
    { tag: t.brace, color: 'var(--text-secondary, #b6a888)' },
    { tag: [t.string, t.special(t.string)], color: 'var(--accent-green, #8fb877)' },
    { tag: [t.comment, t.lineComment, t.blockComment], color: 'var(--text-muted, #8c8378)', fontStyle: 'italic' },
    { tag: [t.number, t.bool, t.null], color: 'var(--accent-amber, #c9956b)' },
  ]);

  const inkTheme = EditorView.theme({
    '&': { color: 'var(--text-primary, #ece0c8)', backgroundColor: 'var(--bg-primary, #12100c)', height: '100%', fontSize: '13px' },
    '.cm-scroller': { fontFamily: 'var(--font-mono, ui-monospace, monospace)', lineHeight: '1.6' },
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
    // Smart extensions (autocomplete + engine-backed lint) are omitted in
    // `minimal` mode, leaving highlighting and the save keymap.
    const smart = minimal
      ? []
      : [autocompletion({ override: [inkComplete] }), inkLinter, lintGutter()];
    try {
      view = new EditorView({
        parent: host,
        state: EditorState.create({
          doc: value,
          extensions: [
            basicSetup,
            StreamLanguage.define(inkStreamParser),
            syntaxHighlighting(inkHighlight),
            ...smart,
            saveKeymap,
            inkTheme,
            EditorView.updateListener.of((u) => {
              if (!u.docChanged || syncing) return;
              syncing = true;
              value = u.state.doc.toString();
              onchange(value);
              syncing = false;
            }),
          ],
        }),
      });
    } catch (e) {
      console.error('Ink editor failed to initialise', e);
      onfail();
    }
    return () => view?.destroy();
  });

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
