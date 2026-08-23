// A pragmatic Ink language mode for CodeMirror: a line-oriented StreamLanguage
// tokenizer plus divert-target autocomplete. It is not a full Ink parser (the
// engine's bladeink compiler is the source of truth, reached through the
// `ink_compile` linter) — just enough structure to make a script readable:
// knots, stitches, choices, gathers, diverts, logic, tags, and comments.
import { tags as t } from '@lezer/highlight';

const KEYWORDS =
  /^(VAR|CONST|LIST|INCLUDE|EXTERNAL|return|else|ref|temp|function|not|and|or|mod)\b/;

// Token names are mapped to highlight tags via `tokenTable` below, so the
// colours come from the editor's HighlightStyle (kit-ui theme tokens).
export const inkStreamParser = {
  startState: () => ({ inBlock: false }),
  token(stream, state) {
    // Block comments span lines — carry the flag in state.
    if (state.inBlock) {
      if (stream.match(/.*?\*\//)) state.inBlock = false;
      else stream.skipToEnd();
      return 'comment';
    }
    if (stream.match('/*')) {
      state.inBlock = true;
      return 'comment';
    }
    if (stream.match('//')) {
      stream.skipToEnd();
      return 'comment';
    }
    if (stream.match(/^TODO:.*/)) return 'comment';

    // Structural markers only count at the start of a line.
    if (stream.sol()) {
      stream.eatSpace();
      // Knot / stitch headers: === name ===  or  = name
      if (stream.match(/^={2,}[^\n]*/)) return 'knot';
      if (stream.match(/^=\s*[A-Za-z_]\w*/)) return 'stitch';
      // Choice markers (* and +, possibly nested) and their optional label.
      if (stream.match(/^[*+]+/)) return 'choice';
      // Gather points (- ... but never the -> divert).
      if (stream.match(/^-(?!>)[-\s]*/)) return 'gather';
    }

    // Diverts, threads, tunnel returns.
    if (stream.match(/->->|<-|->/)) return 'divert';
    // Tags run to end of line.
    if (stream.match(/#[^\n]*/)) return 'tag';
    // Logic / conditional braces and glue.
    if (stream.match(/[{}]|<>/)) return 'logic';
    if (stream.match(/"(?:[^"\\]|\\.)*"/)) return 'string';
    if (stream.match(/\b\d+(?:\.\d+)?\b/)) return 'number';
    if (stream.match(KEYWORDS)) return 'keyword';
    // A (label) on a choice or gather.
    if (stream.match(/\([A-Za-z_]\w*\)/)) return 'label';

    if (stream.match(/^[A-Za-z_]\w*/)) return null;
    stream.next();
    return null;
  },
  languageData: { commentTokens: { line: '//', block: { open: '/*', close: '*/' } } },
  tokenTable: {
    knot: t.heading,
    stitch: t.strong,
    choice: t.controlKeyword,
    gather: t.controlKeyword,
    divert: t.keyword,
    tag: t.meta,
    logic: t.brace,
    label: t.labelName,
    keyword: t.definitionKeyword,
    comment: t.lineComment,
    string: t.string,
    number: t.number,
  },
};

// Divert targets authored in the current document: knot names and their
// stitches (as `knot.stitch`), collected fresh on each completion request.
function collectTargets(doc) {
  const targets = [];
  let currentKnot = null;
  for (const raw of doc.split('\n')) {
    const line = raw.trim();
    let m;
    if ((m = /^={2,}\s*(?:function\s+)?([A-Za-z_]\w*)/.exec(line))) {
      currentKnot = m[1];
      targets.push(currentKnot);
    } else if ((m = /^=\s*([A-Za-z_]\w*)/.exec(line))) {
      targets.push(currentKnot ? `${currentKnot}.${m[1]}` : m[1]);
    }
  }
  return targets;
}

const INK_KEYWORDS = [
  ['VAR', 'Declare a story variable'],
  ['CONST', 'Declare a constant'],
  ['LIST', 'Declare a list'],
  ['INCLUDE', 'Include another ink file'],
  ['EXTERNAL', 'Declare an external function'],
  ['return', 'Return from a function/tunnel'],
  ['else', 'Conditional fallback'],
  ['END', 'End the story'],
  ['DONE', 'End this flow (thread-safe)'],
];

// Autocomplete: divert targets after `->`, otherwise Ink keywords.
export function inkComplete(context) {
  const arrow = context.matchBefore(/->\s*[\w.]*$/);
  if (arrow) {
    const name = /[\w.]*$/.exec(arrow.text)[0];
    const from = context.pos - name.length;
    const doc = context.state.doc.toString();
    const targets = collectTargets(doc).map((n) => ({ label: n, type: 'function' }));
    return {
      from,
      options: [
        ...targets,
        { label: 'END', type: 'constant', info: 'End the story' },
        { label: 'DONE', type: 'constant', info: 'End this flow' },
      ],
    };
  }
  const word = context.matchBefore(/\w*/);
  if (!word || (word.from === word.to && !context.explicit)) return null;
  return {
    from: word.from,
    options: INK_KEYWORDS.map(([label, info]) => ({ label, type: 'keyword', info })),
  };
}

// Parse the engine's Ink compile-error text into a { line, message }. bladeink
// errors carry a line number ("... line 4: ...") we surface on the right row;
// when we can't find one, the caller falls back to the first line.
export function parseInkError(text) {
  if (!text) return null;
  const m = /line[ :]\s*(\d+)/i.exec(text) || /:(\d+):/.exec(text);
  const firstLine = text.split('\n').find((l) => l.trim()) || text;
  return { line: m ? parseInt(m[1], 10) : null, message: firstLine.trim() };
}
