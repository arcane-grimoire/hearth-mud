<script>
  import Output from './components/Output.svelte';
  import InputBar from './components/InputBar.svelte';
  import Sidebar from './components/Sidebar.svelte';

  let output;
  let status = $state('connecting');
  let sidebarOpen = $state(true);
  let roomData = $state(null);
  let apiToken = $state(null);
  let ws;
  let reconnectTimer;

  function connect() {
    status = 'connecting';
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${proto}//${location.host}/ws`);

    ws.onopen = () => {
      status = 'connected';
    };

    ws.onclose = () => {
      status = 'disconnected';
      output?.append('<span class="system">\n[Connection closed]\n</span>');
      scheduleReconnect();
    };

    ws.onerror = () => {
      status = 'error';
    };

    ws.onmessage = (e) => {
      try {
        const msg = JSON.parse(e.data);
        switch (msg.type) {
          case 'text':
            output?.append(msg.text);
            break;
          case 'room':
            roomData = msg;
            break;
          case 'auth':
            apiToken = msg.token;
            break;
          case 'prompt':
            break;
          case 'game':
            break;
        }
      } catch {
        output?.append(e.data);
      }
    };
  }

  function scheduleReconnect() {
    clearTimeout(reconnectTimer);
    reconnectTimer = setTimeout(() => {
      output?.append('<span class="system">[Reconnecting...]\n</span>');
      connect();
    }, 3000);
  }

  function handleCommand(cmd) {
    if (ws?.readyState === WebSocket.OPEN) {
      ws.send(cmd);
    }
  }

  function toggleSidebar() {
    sidebarOpen = !sidebarOpen;
  }

  function sendCommand(cmd) {
    handleCommand(cmd);
  }

  $effect(() => {
    connect();
    return () => {
      clearTimeout(reconnectTimer);
      ws?.close();
    };
  });
</script>

<div class="app">
  <header class="topbar">
    <div class="topbar-left">
      <span class="brand">Hearth</span>
      {#if roomData}
        <span class="room-name">{roomData.name}</span>
      {/if}
    </div>
    <div class="topbar-right">
      <span class="status-dot" class:connected={status === 'connected'} class:error={status === 'error' || status === 'disconnected'}></span>
      <span class="status-label">{status}</span>
      <button class="toggle-btn" onclick={toggleSidebar} title={sidebarOpen ? 'Hide sidebar' : 'Show sidebar'}>
        {sidebarOpen ? '▸' : '◂'}
      </button>
    </div>
  </header>

  <div class="main">
    <Output bind:this={output} oncommand={handleCommand} />
    {#if sidebarOpen}
      <Sidebar {status} room={roomData} {sendCommand} />
    {/if}
  </div>

  <InputBar oncommand={handleCommand} />
</div>

<style>
  .app {
    height: 100%;
    display: flex;
    flex-direction: column;
  }

  .topbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 16px;
    height: 40px;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .topbar-left {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .brand {
    font-size: 13px;
    font-weight: 700;
    color: var(--accent);
    letter-spacing: 0.04em;
  }

  .room-name {
    font-size: 13px;
    color: var(--text-primary);
    font-weight: 600;
  }

  .topbar-right {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .status-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-muted);
  }

  .status-dot.connected {
    background: var(--green);
  }

  .status-dot.error {
    background: var(--red);
  }

  .status-label {
    font-size: 11px;
    color: var(--text-muted);
  }

  .toggle-btn {
    background: none;
    border: 1px solid var(--border);
    color: var(--text-secondary);
    width: 26px;
    height: 26px;
    border-radius: var(--radius);
    cursor: pointer;
    font-size: 11px;
    display: flex;
    align-items: center;
    justify-content: center;
    line-height: 1;
  }

  .toggle-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-focus);
  }

  .main {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  @media (max-width: 700px) {
    .main :global(.sidebar) {
      position: absolute;
      right: 0;
      top: 40px;
      bottom: 0;
      z-index: 10;
      box-shadow: -4px 0 20px rgba(0, 0, 0, 0.4);
    }
  }
</style>
