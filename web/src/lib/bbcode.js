// Client-side BBCode -> HTML, mirroring src/markup.rs::bbcode_to_html so the
// builder's description preview renders exactly as the running game does. It
// emits the same .b/.i/.u/.dim/.c-* and .cmd classes already defined in
// app.css, so the preview and live output share one visual language.
//
// The grammar is small and stable; keep this tag table in sync with markup.rs
// TAGS if the server ever adds a tag. Output is safe for {@html}: every text
// char is escaped and only known class names are emitted.

const TAG_CLASS = {
  b: 'b', dim: 'dim', i: 'i', u: 'u',
  red: 'c-red', green: 'c-green', yellow: 'c-yellow', blue: 'c-blue',
  magenta: 'c-magenta', cyan: 'c-cyan', white: 'c-white',
  bright_red: 'c-bright-red', bright_green: 'c-bright-green',
  bright_yellow: 'c-bright-yellow', bright_blue: 'c-bright-blue',
  bright_magenta: 'c-bright-magenta', bright_cyan: 'c-bright-cyan',
  bright_white: 'c-bright-white',
};

function escText(ch) {
  if (ch === '&') return '&amp;';
  if (ch === '<') return '&lt;';
  if (ch === '>') return '&gt;';
  return ch;
}
function escAttr(s) {
  let o = '';
  for (const c of s) {
    if (c === '"') o += '&quot;';
    else if (c === '&') o += '&amp;';
    else if (c === '<') o += '&lt;';
    else o += c;
  }
  return o;
}

export function bbcodeToHtml(text) {
  if (text == null) return '';
  const s = String(text);
  const n = s.length;
  let out = '';
  let open = 0;
  let i = 0;
  while (i < n) {
    const ch = s[i];
    if (ch === '[') {
      const end = s.indexOf(']', i + 1);
      if (end !== -1) {
        const tag = s.slice(i + 1, end);
        if (tag === '/') {                                 // [/] closes everything
          out += '</span>'.repeat(open);
          open = 0; i = end + 1; continue;
        }
        if (tag[0] === '/' && TAG_CLASS[tag.slice(1)] && open > 0) { // [/b] etc.
          out += '</span>'; open -= 1; i = end + 1; continue;
        }
        if (TAG_CLASS[tag]) {                              // [b], [red], …
          out += `<span class="${TAG_CLASS[tag]}">`;
          open += 1; i = end + 1; continue;
        }
        if (tag.startsWith('cmd=')) {                      // [cmd=go north]…
          out += `<span class="cmd" data-cmd="${escAttr(tag.slice(4))}">`;
          open += 1; i = end + 1; continue;
        }
        if (tag === '/cmd' && open > 0) {
          out += '</span>'; open -= 1; i = end + 1; continue;
        }
      }
      out += '&#91;'; // unmatched '[' -> literal, same as the server
      i += 1;
      continue;
    }
    out += escText(ch);
    i += 1;
  }
  out += '</span>'.repeat(open);
  return out;
}
