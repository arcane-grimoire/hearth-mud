// Session-scoped REPL state, held outside the component so the scrollback and
// command history survive tab switches. The workspace's view switch destroys
// the inactive tab's component, so a plain `let` inside ReplPanel would reset
// every time you glanced at another tab — which a REPL must not do. Lives for
// the page's lifetime; a reload starts fresh (the eval'd world does not).
export const repl = $state({
  entries: [], // { source, output, ok }
  history: [], // submitted sources, for up/down recall
});
