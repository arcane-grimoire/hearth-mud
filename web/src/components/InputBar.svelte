<script>
  import { Button } from '@kenn-io/kit-ui';
  import ChevronUp from '@lucide/svelte/icons/chevron-up';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import SendHorizontal from '@lucide/svelte/icons/send-horizontal';

  let { oncommand } = $props();
  let inputEl;

  export function focus() {
    inputEl?.focus();
  }
  let value = $state('');
  let history = $state([]);
  let histIdx = $state(-1);

  function send() {
    const cmd = value.trim();
    if (cmd) {
      oncommand(cmd);
      history = [cmd, ...history.slice(0, 199)];
    }
    value = '';
    histIdx = -1;
    inputEl?.focus();
  }

  function handleKeydown(e) {
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

<div class="input-bar">
  <span class="prompt">&rsaquo;</span>
  <input
    bind:this={inputEl}
    bind:value
    onkeydown={handleKeydown}
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
</style>
