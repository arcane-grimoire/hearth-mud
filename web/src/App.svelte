<script>
  import { TopBar, StatusDot, IconButton, FlashBanner, ThemeToggle } from '@kenn-io/kit-ui';
  import PanelRightOpen from '@lucide/svelte/icons/panel-right-open';
  import PanelRightClose from '@lucide/svelte/icons/panel-right-close';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import WrenchIcon from '@lucide/svelte/icons/wrench';
  import Output from './components/Output.svelte';
  import InputBar from './components/InputBar.svelte';
  import Sidebar from './components/Sidebar.svelte';
  import Settings from './components/Settings.svelte';
  import { setToken, getSavedToken } from './lib/api.js';
  import { navigate, matches, route } from './lib/router.svelte.js';
  import TerminalIcon from '@lucide/svelte/icons/terminal';
  import LayersIcon from '@lucide/svelte/icons/layers';

  let output;
  let status = $state('connecting');
  let sidebarOpen = $state(true);
  let roomData = $state(null);
  let gamePanels = $state(new Map());
  let scopes = $state([]);
  let availableCommands = $state([]);
  let autocomplete = $state(localStorage.getItem('hearth-autocomplete') === 'true');
  let settingsOpen = $state(false);
  let toolsOpen = $state(false);
  let ws;
  let reconnectTimer;

  const statusDotMap = {
    connected: 'working',
    connecting: 'idle',
    disconnected: 'stale',
    error: 'unclean',
  };

  function escapeHtml(s) {
    return s.replace(/[&<>"']/g, (c) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
    })[c]);
  }

  function connect() {
    status = 'connecting';
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    ws = new WebSocket(`${proto}//${location.host}/ws`);

    ws.onopen = () => {
      status = 'connected';
      const saved = getSavedToken();
      if (saved) ws.send(`reconnect ${saved}`);
    };
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
          case 'text': output?.append(msg.text); feed = [...feed.slice(-300), msg.text]; break;
          case 'room': roomData = msg; break;
          case 'auth':
            setToken(msg.token);
            scopes = msg.scopes || [];
            break;
          case 'prompt': break;
          case 'commands':
            availableCommands = msg.commands || [];
            break;
          case 'game':
            // $state proxies Maps, so mutating in place is reactive without
            // cloning the whole map per update.
            if (msg.channel && msg.data?.widget) {
              gamePanels.set(msg.channel, msg.data);
            }
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
      output?.append(`<span class="echo">&gt; ${escapeHtml(cmd)}</span>\n`);
      ws.send(cmd);
    }
  }

  function sendCommand(cmd) { handleCommand(cmd); }
  function toggleSidebar() { sidebarOpen = !sidebarOpen; }
  function setAutocomplete(val) { autocomplete = val; localStorage.setItem('hearth-autocomplete', val); }

  let isBuilder = $derived(scopes.includes('builder') || scopes.includes('admin'));

  // Unified builder workspace (/builder/workspace) — the IDE: one shell with a
  // shared selection driving table/map + properties/hooks/dialogue. Lazy-loaded
  // so its CodeMirror/Svelte Flow weight never touches the play bundle.
  let onWorkspaceRoute = $derived(matches('/builder/workspace'));
  let BuilderWorkspace = $state(null);
  $effect(() => {
    if (onWorkspaceRoute && !BuilderWorkspace) {
      import('./components/BuilderWorkspace.svelte').then((m) => { BuilderWorkspace = m.default; });
    }
  });
  function openWorkspace() { navigate('/builder/workspace'); toolsOpen = false; }

  // Playtest console — a live game terminal riding on top of the builder
  // workspace, wired to the real command loop (handleCommand). `feed` mirrors the
  // game output. `jumpRef` teleports the test character to the room in focus.
  let feed = $state([]);
  let consoleOpen = $state(false);
  let PlaytestConsole = $state(null);
  let onAnyBuilder = $derived(onWorkspaceRoute);
  let jumpRef = $derived(route.query.focus || route.query.ref || '');
  $effect(() => {
    if (consoleOpen && !PlaytestConsole) {
      import('./components/PlaytestConsole.svelte').then((m) => { PlaytestConsole = m.default; });
    }
    if (!onAnyBuilder) consoleOpen = false; // leaving the builder closes it
  });

  let inputBar;

  function handleKeydown(e) {
    if (e.key === 'Escape') {
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
      {#if isBuilder}
        <div class="tools-menu">
          <IconButton ariaLabel="Builder tools" size="sm" onclick={() => toolsOpen = !toolsOpen}>
            <WrenchIcon size={14} />
          </IconButton>
          {#if toolsOpen}
            <button class="tools-backdrop" aria-label="Close menu" onclick={() => toolsOpen = false}></button>
            <div class="tools-dropdown" role="menu">
              <button class="tools-item unified" role="menuitem" onclick={openWorkspace}>
                <LayersIcon size={14} /> Builder <span class="tools-badge">unified</span>
              </button>
            </div>
          {/if}
        </div>
      {/if}
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
    {#if sidebarOpen}
      <Sidebar {status} room={roomData} {sendCommand} {isBuilder} {gamePanels} />
    {/if}
  </div>

  <InputBar bind:this={inputBar} oncommand={handleCommand} commands={availableCommands} {autocomplete} />
</div>

<Settings open={settingsOpen} onclose={() => settingsOpen = false} {scopes} oncommand={handleCommand} {autocomplete} onautocomplete={setAutocomplete} />

{#if onWorkspaceRoute && BuilderWorkspace}
  <BuilderWorkspace onexit={() => navigate('/')} />
{/if}

{#if onAnyBuilder && !consoleOpen}
  <button class="pt-toggle" onclick={() => (consoleOpen = true)} title="Open playtest console">
    <TerminalIcon size={15} /> Playtest
  </button>
{/if}
{#if consoleOpen && PlaytestConsole}
  <PlaytestConsole {feed} roomData={roomData} {jumpRef} oncommand={handleCommand} onclose={() => (consoleOpen = false)} />
{/if}

<style>
  .pt-toggle {
    position: fixed; right: 16px; bottom: 16px; z-index: 310;
    display: inline-flex; align-items: center; gap: 7px;
    font-size: 12.5px; font-weight: 500;
    color: var(--bg-primary, #12100c); background: var(--accent-amber, #c9956b);
    border: none; border-radius: 999px; padding: 8px 15px; cursor: pointer;
    box-shadow: 0 8px 24px -8px rgba(0, 0, 0, 0.5);
  }
  .pt-toggle:hover { filter: brightness(1.08); }
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

  /* builder-tools dropdown in the top bar */
  .tools-menu { position: relative; display: inline-flex; }
  .tools-backdrop {
    position: fixed; inset: 0; z-index: 40;
    background: transparent; border: 0; padding: 0; cursor: default;
  }
  .tools-dropdown {
    position: absolute; top: calc(100% + 6px); right: 0; z-index: 41;
    min-width: 170px; padding: 4px;
    display: flex; flex-direction: column; gap: 2px;
    background: var(--bg-surface);
    border: var(--border-width) solid var(--border-default);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-md);
  }
  .tools-item {
    display: flex; align-items: center; gap: 8px; width: 100%;
    padding: 7px 9px; border: 0; border-radius: var(--radius-sm);
    background: transparent; color: var(--text-primary);
    font-size: var(--font-size-md); text-align: left; cursor: pointer;
  }
  .tools-item:hover { background: var(--bg-surface-hover); }
  .tools-item.unified { color: var(--accent-amber, #c9956b); font-weight: 600; }
  .tools-badge { margin-left: auto; font-size: 9px; text-transform: uppercase; letter-spacing: 0.05em; background: color-mix(in srgb, var(--accent-amber, #c9956b) 18%, transparent); color: var(--accent-amber, #c9956b); border-radius: 8px; padding: 1px 6px; }

  @media (max-width: 700px) {
    .main :global(.sidebar) {
      position: absolute;
      right: 0;
      top: 44px;
      bottom: 0;
      z-index: 10;
      box-shadow: -4px 0 20px rgba(0, 0, 0, 0.4);
    }
  }
</style>
