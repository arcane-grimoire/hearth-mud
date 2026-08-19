<script>
  import { TopBar, StatusDot, IconButton, FlashBanner, ThemeToggle } from '@kenn-io/kit-ui';
  import PanelRightOpen from '@lucide/svelte/icons/panel-right-open';
  import PanelRightClose from '@lucide/svelte/icons/panel-right-close';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import Output from './components/Output.svelte';
  import InputBar from './components/InputBar.svelte';
  import Sidebar from './components/Sidebar.svelte';
  import Editor from './components/Editor.svelte';
  import Settings from './components/Settings.svelte';
  import { setToken } from './lib/api.js';

  let output;
  let status = $state('connecting');
  let sidebarOpen = $state(true);
  let roomData = $state(null);
  let scopes = $state([]);
  let editingEntity = $state(null);
  let settingsOpen = $state(false);
  let ws;
  let reconnectTimer;

  const statusDotMap = {
    connected: 'working',
    connecting: 'idle',
    disconnected: 'stale',
    error: 'unclean',
  };

  function connect() {
    status = 'connecting';
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${proto}//${location.host}/ws`);

    ws.onopen = () => { status = 'connected'; };
    ws.onclose = () => {
      status = 'disconnected';
      output?.append('<span class="system">\n[Connection closed]\n</span>');
      scheduleReconnect();
    };
    ws.onerror = () => { status = 'error'; };
    ws.onmessage = (e) => {
      try {
        const msg = JSON.parse(e.data);
        switch (msg.type) {
          case 'text': output?.append(msg.text); break;
          case 'room': roomData = msg; break;
          case 'auth':
            setToken(msg.token);
            scopes = msg.scopes || [];
            break;
          case 'prompt': break;
          case 'game': break;
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
      output?.append(`<span class="echo">&gt; ${cmd.replace(/&/g,'&amp;').replace(/</g,'&lt;')}</span>\n`);
      ws.send(cmd);
    }
  }

  function sendCommand(cmd) { handleCommand(cmd); }
  function openEditor(entity) { editingEntity = entity; }
  function closeEditor() { editingEntity = null; }
  function toggleSidebar() { sidebarOpen = !sidebarOpen; }

  let isBuilder = $derived(scopes.includes('builder') || scopes.includes('admin'));
  let inputBar;

  function handleKeydown(e) {
    if (e.key === 'Escape') {
      if (editingEntity) closeEditor();
      inputBar?.focus();
    }
  }

  $effect(() => {
    connect();
    return () => {
      clearTimeout(reconnectTimer);
      ws?.close();
    };
  });
</script>

<svelte:window onkeydown={handleKeydown} />
<FlashBanner top="44px" />
<div class="app">
  <TopBar>
    {#snippet left()}
      <span class="brand">Hearth</span>
      {#if roomData}
        <span class="room-name">{roomData.name}</span>
      {/if}
    {/snippet}
    {#snippet right()}
      <IconButton ariaLabel="Settings" size="sm" onclick={() => settingsOpen = true}>
        <SettingsIcon size={14} />
      </IconButton>
      <StatusDot status={statusDotMap[status] || 'stale'} label={status} />
      <span class="status-label">{status}</span>
      <IconButton
        ariaLabel={sidebarOpen ? 'Hide sidebar' : 'Show sidebar'}
        size="sm"
        onclick={toggleSidebar}
      >
        {#if sidebarOpen}
          <PanelRightClose size={14} />
        {:else}
          <PanelRightOpen size={14} />
        {/if}
      </IconButton>
    {/snippet}
  </TopBar>

  <div class="main">
    <Output bind:this={output} oncommand={handleCommand} />
    {#if editingEntity}
      <Editor entity={editingEntity} onclose={closeEditor} />
    {:else if sidebarOpen}
      <Sidebar {status} room={roomData} {sendCommand} {isBuilder} onedit={openEditor} />
    {/if}
  </div>

  <InputBar bind:this={inputBar} oncommand={handleCommand} />
</div>

<Settings open={settingsOpen} onclose={() => settingsOpen = false} {scopes} oncommand={handleCommand} />

<style>
  .app {
    height: 100%;
    display: flex;
    flex-direction: column;
  }

  .brand {
    font-size: var(--font-size-md);
    font-weight: 700;
    color: var(--accent-amber, #c9956b);
    letter-spacing: 0.04em;
  }

  .room-name {
    font-size: var(--font-size-md);
    color: var(--text-primary);
    font-weight: 600;
  }

  .status-label {
    font-size: var(--font-size-xs);
    color: var(--text-muted);
  }

  .main {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  @media (max-width: 700px) {
    .main :global(.sidebar),
    .main :global(.editor) {
      position: absolute;
      right: 0;
      top: 44px;
      bottom: 0;
      z-index: 10;
      box-shadow: -4px 0 20px rgba(0, 0, 0, 0.4);
    }
  }
</style>
