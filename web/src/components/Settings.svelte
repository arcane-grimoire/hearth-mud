<script>
  import { DetailDrawer, ThemeToggle, Button, TextInput, showFlash } from '@kenn-io/kit-ui';
  import { api } from '../lib/api.js';

  let { open = false, onclose = () => {}, scopes = [], oncommand = () => {} } = $props();

  let tokenLabel = $state('');
  let tokens = $state([]);
  let loadingTokens = $state(false);
  let fontSize = $state(localStorage.getItem('hearth-font-size') || '14');

  $effect(() => {
    if (open) loadTokens();
  });

  $effect(() => {
    document.documentElement.style.setProperty('--font-size-output', fontSize + 'px');
    localStorage.setItem('hearth-font-size', fontSize);
  });

  async function loadTokens() {
    loadingTokens = true;
    oncommand('@token list');
    loadingTokens = false;
  }

  async function createToken() {
    const label = tokenLabel.trim();
    if (!label) return;
    oncommand(`@token create ${label}`);
    tokenLabel = '';
  }

  function revokeToken(label) {
    oncommand(`@token revoke ${label}`);
  }

  function changePassword() {
    oncommand('@password');
  }

  let isAdmin = $derived(scopes.includes('admin'));
</script>

{#if open}
  <DetailDrawer title="Settings" onclose={onclose} width="min(400px, 100vw)">
    <div class="settings">
      <section class="section">
        <h3 class="section-label">Appearance</h3>
        <div class="setting-row">
          <span class="setting-name">Theme</span>
          <ThemeToggle variant="segmented" size="sm" />
        </div>
        <div class="setting-row">
          <span class="setting-name">Font size</span>
          <div class="font-size-control">
            <input
              type="range"
              min="10"
              max="20"
              step="1"
              bind:value={fontSize}
            />
            <span class="font-size-value">{fontSize}px</span>
          </div>
        </div>
      </section>

      <section class="section">
        <h3 class="section-label">API Tokens</h3>
        <p class="section-desc">Tokens authenticate REST API requests. Create one for external tools or scripts.</p>
        <div class="token-create">
          <TextInput
            bind:value={tokenLabel}
            size="sm"
            placeholder="Token label..."
            block
            onkeydown={(e) => e.key === 'Enter' && createToken()}
          />
          <Button size="sm" onclick={createToken} label="Create" />
        </div>
        <p class="section-hint">Use <code>@token list</code> and <code>@token revoke</code> in the command input to manage tokens.</p>
      </section>

      <section class="section">
        <h3 class="section-label">Account</h3>
        <div class="setting-row">
          <span class="setting-name">Scopes</span>
          <span class="scope-list">{scopes.join(', ')}</span>
        </div>
        <div class="setting-row">
          <span class="setting-name">Password</span>
          <Button size="sm" onclick={changePassword} label="Change" />
        </div>
      </section>

      {#if isAdmin}
        <section class="section">
          <h3 class="section-label">Admin</h3>
          <div class="admin-actions">
            <Button size="sm" onclick={() => oncommand('@save')} label="Save world" />
            <Button size="sm" onclick={() => oncommand('@reload-world')} label="Reload files" />
            <Button size="sm" onclick={() => oncommand('who')} label="Who's online" />
          </div>
        </section>
      {/if}
    </div>
  </DetailDrawer>
{/if}

<style>
  .settings {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .section {
    padding: 12px 0;
    border-bottom: 1px solid var(--border-default);
  }

  .section:last-child {
    border-bottom: none;
  }

  .section-label {
    font-size: var(--font-size-2xs, 10px);
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    margin: 0 0 8px 0;
  }

  .section-desc {
    font-size: var(--font-size-xs, 12px);
    color: var(--text-secondary);
    margin: 0 0 8px 0;
    line-height: 1.5;
  }

  .section-hint {
    font-size: var(--font-size-2xs, 10px);
    color: var(--text-muted);
    margin: 8px 0 0 0;
    line-height: 1.5;
  }

  .section-hint code {
    background: var(--bg-inset);
    padding: 1px 4px;
    border-radius: 3px;
    font-size: inherit;
  }

  .setting-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 4px 0;
  }

  .setting-name {
    font-size: var(--font-size-sm, 13px);
    color: var(--text-primary);
  }

  .scope-list {
    font-size: var(--font-size-xs, 12px);
    color: var(--text-secondary);
    font-family: var(--font-mono);
  }

  .font-size-control {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .font-size-control input[type="range"] {
    width: 100px;
    accent-color: var(--accent-amber, #c9956b);
  }

  .font-size-value {
    font-size: var(--font-size-xs, 12px);
    color: var(--text-secondary);
    font-family: var(--font-mono);
    min-width: 32px;
  }

  .token-create {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .admin-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
</style>
