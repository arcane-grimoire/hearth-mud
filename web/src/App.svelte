<script>
  import { TopBar, StatusDot, IconButton, FlashBanner, ThemeToggle } from '@kenn-io/kit-ui';
  import PanelRightOpen from '@lucide/svelte/icons/panel-right-open';
  import PanelRightClose from '@lucide/svelte/icons/panel-right-close';
  import SettingsIcon from '@lucide/svelte/icons/settings';
  import DatabaseIcon from '@lucide/svelte/icons/database';
  import MapIcon from '@lucide/svelte/icons/map';
  import WrenchIcon from '@lucide/svelte/icons/wrench';
  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import Maximize2 from '@lucide/svelte/icons/maximize-2';
  import Minimize2 from '@lucide/svelte/icons/minimize-2';
  import Output from './components/Output.svelte';
  import InputBar from './components/InputBar.svelte';
  import Sidebar from './components/Sidebar.svelte';
  import Editor from './components/Editor.svelte';
  import Admin from './components/Admin.svelte';
  import Settings from './components/Settings.svelte';
  import { setToken, getSavedToken } from './lib/api.js';
  import { navigate, matches } from './lib/router.svelte.js';
  import WaypointsIcon from '@lucide/svelte/icons/waypoints';
  import CodeIcon from '@lucide/svelte/icons/code';
  import ShieldIcon from '@lucide/svelte/icons/shield-alert';

  let output;
  let status = $state('connecting');
  let sidebarOpen = $state(true);
  let roomData = $state(null);
  let gamePanels = $state(new Map());
  let scopes = $state([]);
  let availableCommands = $state([]);
  let autocomplete = $state(localStorage.getItem('hearth-autocomplete') === 'true');
  let editingEntity = $state(null);
  let settingsOpen = $state(false);
  let adminOpen = $state(false);
  let mapsOpen = $state(false);
  let panelFull = $state(false);
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
          case 'text': output?.append(msg.text); break;
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
  function openEditor(entity) { editingEntity = entity; adminOpen = false; mapsOpen = false; }
  function closeEditor() { editingEntity = null; }
  function toggleAdmin() { adminOpen = !adminOpen; editingEntity = null; mapsOpen = false; toolsOpen = false; }
  function openMaps() { mapsOpen = true; adminOpen = false; editingEntity = null; toolsOpen = false; }
  function closeMaps() { mapsOpen = false; }
  function toggleSidebar() { sidebarOpen = !sidebarOpen; }
  function setAutocomplete(val) { autocomplete = val; localStorage.setItem('hearth-autocomplete', val); }

  let isBuilder = $derived(scopes.includes('builder') || scopes.includes('admin'));
  // Map writes are Admin-tier (a PUT writes files on the server), so the Map
  // builder entry is admin-only — a plain builder would load it and then fail
  // on save. Reads/Admin panel stay builder-gated.
  let isAdmin = $derived(scopes.includes('admin'));

  // Room builder is its own full-page route (/builder/rooms), lazy-loaded so
  // the play client never pulls it (or the Svelte Flow canvas) into its bundle.
  let onBuilderRoute = $derived(matches('/builder/rooms'));
  let RoomBuilder = $state(null);
  $effect(() => {
    if (onBuilderRoute && !RoomBuilder) {
      import('./components/RoomBuilder.svelte').then((m) => { RoomBuilder = m.default; });
    }
  });
  function openRoomBuilder() { navigate('/builder/rooms'); toolsOpen = false; }

  // Code editor — its own full-page route (/builder/code), also lazy-loaded so
  // CodeMirror never touches the play bundle.
  let onCodeRoute = $derived(matches('/builder/code'));
  let CodeWorkspace = $state(null);
  $effect(() => {
    if (onCodeRoute && !CodeWorkspace) {
      import('./components/CodeWorkspace.svelte').then((m) => { CodeWorkspace = m.default; });
    }
  });
  function openCodeEditor() { navigate('/builder/code'); toolsOpen = false; }

  // World check — the problems panel (/builder/problems).
  let onProblemsRoute = $derived(matches('/builder/problems'));
  let WorldCheck = $state(null);
  $effect(() => {
    if (onProblemsRoute && !WorldCheck) {
      import('./components/WorldCheck.svelte').then((m) => { WorldCheck = m.default; });
    }
  });
  function openWorldCheck() { navigate('/builder/problems'); toolsOpen = false; }
  let inputBar;

  function handleKeydown(e) {
    if (e.key === 'Escape') {
      if (panelFull) { panelFull = false; return; }  // Esc leaves full screen first
      if (editingEntity) closeEditor();
      inputBar?.focus();
    }
  }

  // Auto-clear full screen when no panel is open, and let the embedded map
  // builder (a same-origin iframe) drive full-screen/Esc via postMessage, since
  // its own keydowns don't reach this document.
  $effect(() => {
    if (!mapsOpen && !adminOpen && !editingEntity) panelFull = false;
  });
  $effect(() => {
    const onMsg = (e) => {
      if (e.origin !== location.origin) return;
      const t = e.data?.type;
      if (t === 'mapwright:toggle-fullscreen') panelFull = !panelFull;
      else if (t === 'mapwright:esc') { if (panelFull) panelFull = false; else closeMaps(); }
    };
    window.addEventListener('message', onMsg);
    return () => window.removeEventListener('message', onMsg);
  });

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
              <button class="tools-item" role="menuitem" onclick={toggleAdmin}>
                <DatabaseIcon size={14} /> World admin
              </button>
              {#if isAdmin}
                <button class="tools-item" role="menuitem" onclick={openMaps}>
                  <MapIcon size={14} /> Map builder
                </button>
                <button class="tools-item" role="menuitem" onclick={openRoomBuilder}>
                  <WaypointsIcon size={14} /> Room builder
                </button>
                <button class="tools-item" role="menuitem" onclick={openCodeEditor}>
                  <CodeIcon size={14} /> Code editor
                </button>
                <button class="tools-item" role="menuitem" onclick={openWorldCheck}>
                  <ShieldIcon size={14} /> World check
                </button>
              {/if}
            </div>
          {/if}
        </div>
      {/if}
      {#if mapsOpen || adminOpen || editingEntity}
        <IconButton ariaLabel={panelFull ? 'Exit full screen' : 'Full screen'} size="sm" onclick={() => panelFull = !panelFull}>
          {#if panelFull}<Minimize2 size={14} />{:else}<Maximize2 size={14} />{/if}
        </IconButton>
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
    {#if mapsOpen}
      <div class="pane maps-view" class:full={panelFull}>
        <div class="maps-back">
          <IconButton ariaLabel="Back to game" size="sm" onclick={closeMaps}>
            <ArrowLeftIcon size={14} />
          </IconButton>
        </div>
        <iframe class="maps-frame" src="/builder?embed=1" title="Map builder"></iframe>
      </div>
    {:else if adminOpen}
      <div class="pane" class:full={panelFull}>
        <Admin onclose={() => adminOpen = false} />
      </div>
    {:else if editingEntity}
      <div class="pane" class:full={panelFull}>
        <Editor entity={editingEntity} onclose={closeEditor} />
      </div>
    {:else if sidebarOpen}
      <Sidebar {status} room={roomData} {sendCommand} {isBuilder} onedit={openEditor} {gamePanels} />
    {/if}
  </div>

  <InputBar bind:this={inputBar} oncommand={handleCommand} commands={availableCommands} {autocomplete} />
</div>

<Settings open={settingsOpen} onclose={() => settingsOpen = false} {scopes} oncommand={handleCommand} {autocomplete} onautocomplete={setAutocomplete} />

{#if onBuilderRoute && RoomBuilder}
  <RoomBuilder onexit={() => navigate('/')} />
{/if}

{#if onCodeRoute && CodeWorkspace}
  <CodeWorkspace onexit={() => navigate('/')} />
{/if}

{#if onProblemsRoute && WorldCheck}
  <WorldCheck onexit={() => navigate('/')} />
{/if}

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

  /* embedded map builder fills the main area; the app supplies the back button */
  /* a panel (map builder, admin, or entity editor) that can go full screen */
  .pane { display: flex; min-width: 0; }
  .maps-view.pane { position: relative; flex: 1; }
  /* full screen: fill the window below the top bar, so the exit toggle up
     there stays reachable — works for every panel, not just the map builder */
  .pane.full { position: fixed; inset: 44px 0 0 0; z-index: 100; background: var(--bg-primary); }
  .pane.full :global(.admin),
  .pane.full :global(.editor) { width: 100%; max-width: 100%; min-width: 0; flex: 1; }
  .maps-frame { flex: 1; width: 100%; height: 100%; border: 0; background: var(--bg-primary); }
  .maps-back { position: absolute; top: 6px; left: 7px; z-index: 5; }

  @media (max-width: 700px) {
    .main :global(.sidebar),
    .main :global(.editor),
    .main :global(.admin) {
      position: absolute;
      right: 0;
      top: 44px;
      bottom: 0;
      z-index: 10;
      box-shadow: -4px 0 20px rgba(0, 0, 0, 0.4);
    }
  }
</style>
