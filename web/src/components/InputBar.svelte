<script>
  let { oncommand } = $props();
  let inputEl;
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
    <button
      onclick={histPrev}
      title="Previous command"
      disabled={history.length === 0 || histIdx >= history.length - 1}
    >&uarr;</button>
    <button
      onclick={histNext}
      title="Next command"
      disabled={histIdx < 0}
    >&darr;</button>
    <button class="send-btn" onclick={send}>Send</button>
  </div>
</div>

<style>
  .input-bar {
    display: flex;
    align-items: center;
    background: var(--bg-input);
    border-top: 1px solid var(--border);
  }

  .prompt {
    color: var(--accent);
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
    font-size: var(--font-size);
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

  button {
    background: var(--bg-elevated);
    color: var(--text-secondary);
    border: 1px solid var(--border);
    padding: 6px 10px;
    border-radius: var(--radius);
    cursor: pointer;
    font-family: var(--font-mono);
    font-size: 13px;
    line-height: 1;
  }

  button:hover:not(:disabled) {
    color: var(--text-primary);
    border-color: var(--border-focus);
  }

  button:disabled {
    opacity: 0.3;
    cursor: default;
  }

  .send-btn {
    color: var(--accent);
    padding: 6px 16px;
  }

  .send-btn:hover {
    background: var(--accent-dim);
  }
</style>
