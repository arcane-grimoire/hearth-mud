<script>
  let { oncommand = () => {} } = $props();
  let container;
  let autoScroll = $state(true);
  let showJump = $state(false);

  function handleClick(e) {
    const el = e.target.closest('.cmd');
    if (el?.dataset?.cmd) {
      oncommand(el.dataset.cmd);
    }
  }

  // Incoming text is queued and flushed at most once per animation frame so
  // bursts of WebSocket messages cause a single DOM write + layout pass.
  let queue = [];
  let flushScheduled = false;

  export function append(html) {
    queue.push(html);
    if (!flushScheduled) {
      flushScheduled = true;
      requestAnimationFrame(flush);
    }
  }

  function flush() {
    flushScheduled = false;
    if (!container || queue.length === 0) return;
    const batch = queue;
    queue = [];
    container.insertAdjacentHTML('beforeend', batch.join(''));
    // Cap by node count; trim before scrolling so we don't lay out nodes
    // we're about to remove.
    let excess = container.childNodes.length - 8000;
    while (excess-- > 0) container.removeChild(container.firstChild);
    if (autoScroll) container.scrollTop = container.scrollHeight;
  }

  function handleScroll() {
    if (!container) return;
    const { scrollTop, scrollHeight, clientHeight } = container;
    const near = scrollHeight - scrollTop - clientHeight < 50;
    autoScroll = near;
    showJump = !near;
  }

  function jumpToBottom() {
    if (!container) return;
    container.scrollTop = container.scrollHeight;
    autoScroll = true;
    showJump = false;
  }
</script>

<div class="output-wrap">
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="output" bind:this={container} onscroll={handleScroll} onclick={handleClick}></div>
  {#if showJump}
    <button class="jump" onclick={jumpToBottom}>&#8595; new output</button>
  {/if}
</div>

<style>
  .output-wrap {
    position: relative;
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }

  .output {
    height: 100%;
    overflow-y: auto;
    padding: 12px 16px;
    white-space: pre-wrap;
    word-wrap: break-word;
    line-height: 1.6;
    font-size: var(--font-size-output, 14px);
  }

  .jump {
    position: absolute;
    bottom: 12px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--bg-elevated);
    color: var(--accent);
    border: 1px solid var(--border);
    padding: 5px 18px;
    border-radius: var(--radius);
    cursor: pointer;
    font-family: var(--font-mono);
    font-size: 12px;
    opacity: 0.9;
    z-index: 1;
  }

  .jump:hover {
    opacity: 1;
    background: var(--accent-dim);
  }
</style>
