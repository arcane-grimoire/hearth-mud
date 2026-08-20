<script>
  import { Button } from '@kenn-io/kit-ui';
  import ChevronUp from '@lucide/svelte/icons/chevron-up';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import SendHorizontal from '@lucide/svelte/icons/send-horizontal';

  let { oncommand, commands = [], autocomplete = false } = $props();
  let inputEl;

  export function focus() {
    inputEl?.focus();
  }
  let value = $state('');
  let history = $state([]);
  let histIdx = $state(-1);

  let suggestions = $state([]);
  let selectedSuggestion = $state(0);

  let matches = $derived.by(() => {
    const prefix = value.toLowerCase();
    if (!prefix || prefix.includes(' ')) return [];
    return commands.filter(c => c.toLowerCase().startsWith(prefix) && c.toLowerCase() !== prefix);
  });

  function send() {
    const cmd = value.trim();
    if (cmd) {
      oncommand(cmd);
      history = [cmd, ...history.slice(0, 199)];
    }
    value = '';
    histIdx = -1;
    suggestions = [];
    inputEl?.focus();
  }

  function acceptSuggestion(s) {
    value = s + ' ';
    suggestions = [];
    selectedSuggestion = 0;
    inputEl?.focus();
  }

  function handleKeydown(e) {
    if (suggestions.length > 0) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        selectedSuggestion = (selectedSuggestion + 1) % suggestions.length;
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        selectedSuggestion = (selectedSuggestion - 1 + suggestions.length) % suggestions.length;
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        acceptSuggestion(suggestions[selectedSuggestion]);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        suggestions = [];
        selectedSuggestion = 0;
        return;
      }
    }

    if (e.key === 'Tab') {
      e.preventDefault();
      if (matches.length === 1) {
        acceptSuggestion(matches[0]);
      } else if (matches.length > 1) {
        suggestions = matches.slice(0, 10);
        selectedSuggestion = 0;
      }
      return;
    }

    if (e.key === 'Enter') {
      send();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (histIdx < history.length - 1) {
        histIdx++;
        value = history[histIdx];
      }
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (histIdx > 0) {
        histIdx--;
        value = history[histIdx];
      } else if (histIdx === 0) {
        histIdx = -1;
        value = '';
      }
    }
  }

  function handleInput() {
    if (autocomplete) {
      if (matches.length > 1) {
        suggestions = matches.slice(0, 10);
        selectedSuggestion = 0;
      } else {
        suggestions = [];
        selectedSuggestion = 0;
      }
    } else if (suggestions.length > 0) {
      if (matches.length > 1) {
        suggestions = matches.slice(0, 10);
        selectedSuggestion = 0;
      } else {
        suggestions = [];
        selectedSuggestion = 0;
      }
    }
  }

  function histPrev() {
    if (histIdx < history.length - 1) {
      histIdx++;
      value = history[histIdx];
    }
    inputEl?.focus();
  }

  function histNext() {
    if (histIdx > 0) {
      histIdx--;
      value = history[histIdx];
    } else if (histIdx === 0) {
      histIdx = -1;
      value = '';
    }
    inputEl?.focus();
  }
</script>

{#if suggestions.length > 0}
  <div class="suggestions">
    {#each suggestions as s, i}
      <button
        class="suggestion"
        class:selected={i === selectedSuggestion}
        onmousedown={(e) => { e.preventDefault(); acceptSuggestion(s); }}
      >{s}</button>
    {/each}
  </div>
{/if}
<div class="input-bar">
  <span class="prompt">&rsaquo;</span>
  <input
    bind:this={inputEl}
    bind:value
    onkeydown={handleKeydown}
    oninput={handleInput}
    type="text"
    autocomplete="off"
    spellcheck="false"
    placeholder="Enter command..."
    autofocus
  />
  <div class="buttons">
    <Button
      size="sm"
      onclick={histPrev}
      title="Previous command"
      disabled={history.length === 0 || histIdx >= history.length - 1}
    >
      <ChevronUp size={14} />
    </Button>
    <Button
      size="sm"
      onclick={histNext}
      title="Next command"
      disabled={histIdx < 0}
    >
      <ChevronDown size={14} />
    </Button>
    <Button size="sm" onclick={send} title="Send command">
      <SendHorizontal size={14} />
    </Button>
  </div>
</div>

<style>
  .input-bar {
    display: flex;
    align-items: center;
    background: var(--bg-inset, #0d0f14);
    border-top: 1px solid var(--border-default);
  }

  .prompt {
    color: var(--accent-amber, #c9956b);
    font-size: 20px;
    padding: 0 6px 0 14px;
    user-select: none;
    line-height: 1;
  }

  input {
    flex: 1;
    background: transparent;
    color: var(--text-primary);
    border: none;
    padding: 14px 8px;
    font-family: var(--font-mono);
    font-size: var(--font-size-md, 14px);
    outline: none;
    min-width: 0;
  }

  input::placeholder {
    color: var(--text-muted);
  }

  .buttons {
    display: flex;
    gap: 3px;
    padding: 6px 8px;
    flex-shrink: 0;
  }

  .suggestions {
    background: var(--bg-elevated, #1a1d24);
    border-top: 1px solid var(--border-default);
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 6px 12px;
  }

  .suggestion {
    background: transparent;
    border: 1px solid var(--border-default);
    border-radius: 4px;
    color: var(--text-secondary, #9ca3af);
    font-family: var(--font-mono);
    font-size: var(--font-size-sm, 12px);
    padding: 2px 8px;
    cursor: pointer;
  }

  .suggestion:hover,
  .suggestion.selected {
    background: var(--accent-amber, #c9956b);
    color: var(--bg-inset, #0d0f14);
    border-color: var(--accent-amber, #c9956b);
  }
</style>
