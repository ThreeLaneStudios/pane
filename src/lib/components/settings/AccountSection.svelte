<script lang="ts">
  import { isAuthenticated, userEmail, signIn, signUp, signOut } from '$lib/stores/authStore';
  import { autoSyncEnabled, startAutoSync, stopAutoSync, lastSyncTime } from '$lib/stores/syncStore';
  import { getSyncHistory } from '$lib/sync';
  import * as api from '$lib/api';
  import { invoke } from '@tauri-apps/api/core';
  
  let email = $state('');
  let password = $state('');
  let loading = $state(false);
  let errorMessage = $state('');
  let successMessage = $state('');
  let deviceName = $state('');
  let syncInfo = $state<{ db_version: number; total_changes: number } | null>(null);
  let syncStatus = $state('');
  let syncMessage = $state('');
  let showSyncHistory = $state(false);
  let syncHistoryData = $state<Array<{timestamp: number; peer: string; changesSent: number; changesReceived: number; conflicts: number; error?: string}>>([]);
  
  // Bridge config
  let bridgeUrl = $state('');
  let bridgeToken = $state('');
  let bridgeStatus = $state<'not_configured' | 'connected' | 'error'>('not_configured');
  let bridgeMessage = $state('');
  let bridgeSaving = $state(false);
  
  async function getDeviceName() {
    try {
      const hostname = await invoke('get_hostname') as string;
      deviceName = hostname || 'Mac';
    } catch {
      deviceName = 'Mac';
    }
  }
  
  async function loadSyncInfo() {
    try {
      const info = await invoke('get_sync_info') as { db_version: number; total_changes: number };
      syncInfo = info;
    } catch (error) {
      console.error('Failed to load sync info:', error);
    }
  }
  
  async function handleSignIn() {
    if (!email || !password) {
      errorMessage = 'Email and password are required';
      return;
    }
    
    loading = true;
    errorMessage = '';
    successMessage = '';
    
    try {
      await signIn(email, password);
      password = '';
      successMessage = 'Signed in successfully';
      setTimeout(() => { successMessage = ''; }, 3000);
      await loadSyncInfo();
    } catch (error: any) {
      errorMessage = error.message || 'Failed to sign in. Check your credentials.';
    } finally {
      loading = false;
    }
  }
  
  async function handleSignUp() {
    if (!email || !password) {
      errorMessage = 'Email and password are required';
      return;
    }
    
    if (password.length < 8) {
      errorMessage = 'Password must be at least 8 characters';
      return;
    }
    
    loading = true;
    errorMessage = '';
    successMessage = '';
    
    try {
      await signUp(email, password);
      password = '';
      successMessage = 'Account created successfully';
      setTimeout(() => { successMessage = ''; }, 3000);
      await loadSyncInfo();
    } catch (error: any) {
      const msg = typeof error === 'string' ? error : error?.message || '';
      if (msg === 'CONFIRM_EMAIL' || msg.includes('CONFIRM_EMAIL')) {
        // Show success message instead of error
        successMessage = 'Check your email to confirm your account, then sign in.';
        errorMessage = '';
      } else {
        errorMessage = msg || 'Failed to create account.';
      }
    } finally {
      loading = false;
    }
  }
  
  async function handleSyncNow() {
    loading = true;
    syncStatus = "";
    syncMessage = "";
    
    try {
      const pushResult = await invoke("sync_push") as string;
      const filePushResult = await invoke("sync_files_push") as string;
      const pullResult = await invoke("sync_pull") as string;
      const filePullResult = await invoke("sync_files_pull") as string;
      syncStatus = "success";
      syncMessage = `Push: ${pushResult} | Files: ${filePushResult} | Pull: ${pullResult} | Files: ${filePullResult}`;
      lastSyncTime.set(new Date().toLocaleTimeString('en-US', { 
        hour: 'numeric', 
        minute: '2-digit',
        hour12: true 
      }));
      // Refresh sync info
      await loadSyncInfo();
    } catch (error: any) {
      syncStatus = 'error';
      syncMessage = typeof error === 'string' ? error : error?.message || 'Sync failed';
    } finally {
      loading = false;
    }
  }

  
  async function handleSignOut() {
    loading = true;
    errorMessage = '';
    
    try {
      await signOut();
      email = '';
      password = '';
      syncInfo = null;
      lastSyncTime.set(null);
    } catch (error: any) {
      errorMessage = error.message || 'Failed to sign out';
    } finally {
      loading = false;
    }
  }
  
  async function loadBridgeConfig() {
    try {
      const urlResult = await invoke('get_ai_setting', { key: 'bridge_url' }) as string | null;
      const tokenResult = await invoke('get_ai_setting', { key: 'bridge_token' }) as string | null;
      
      if (urlResult) {
        try {
          bridgeUrl = JSON.parse(urlResult);
        } catch {
          bridgeUrl = urlResult;
        }
      }
      
      if (tokenResult) {
        try {
          bridgeToken = JSON.parse(tokenResult);
        } catch {
          bridgeToken = tokenResult;
        }
      }
      
      // Set initial status
      if (bridgeUrl && bridgeToken) {
        bridgeStatus = 'connected';
      }
    } catch (error) {
      console.error('Failed to load bridge config:', error);
    }
  }
  
  async function saveBridgeConfig() {
    bridgeSaving = true;
    bridgeMessage = '';
    
    try {
      // Save to ai_settings
      await invoke('set_ai_setting', { key: 'bridge_url', value: JSON.stringify(bridgeUrl) });
      await invoke('set_ai_setting', { key: 'bridge_token', value: JSON.stringify(bridgeToken) });
      
      // Test the fixed Bridge health route from Rust. The WebView never receives
      // arbitrary fetch authority or forwards the token to a renderer-chosen URL.
      if (bridgeUrl && bridgeToken) {
        try {
          await invoke('bridge_health_check');
          bridgeStatus = 'connected';
          bridgeMessage = 'Connected successfully ✅';
        } catch (e: any) {
          bridgeStatus = 'error';
          bridgeMessage = `Unreachable: ${e.message || 'Network error'}`;
        }
      } else {
        bridgeStatus = 'not_configured';
        bridgeMessage = 'Config saved (no URL or token provided)';
      }
      
      setTimeout(() => { bridgeMessage = ''; }, 3000);
    } catch (error: any) {
      bridgeStatus = 'error';
      bridgeMessage = `Failed to save: ${error.message || error}`;
    } finally {
      bridgeSaving = false;
    }
  }
  
  $effect(() => {
    if ($isAuthenticated) {
      if (!deviceName) {
        getDeviceName();
      }
      if (!syncInfo) {
        loadSyncInfo();
      }
    }
    // Load bridge config on mount
    loadBridgeConfig();
  });
</script>

<section class="settings-section">
  <h4>Account</h4>
  <p class="section-desc">Your Pane account and connection status.</p>
  
  {#if $isAuthenticated}
    <!-- Logged in state -->
    <div class="account-connected">
      <div class="connected-status">
        <span class="status-dot-connected"></span>
        <span class="status-text">Connected</span>
      </div>
      <div class="account-email">{$userEmail}</div>
      
      <div class="sync-section" style="opacity: 0.4; pointer-events: none;">
        <h5>Sync</h5>
        <p style="font-size: 12px; color: #666; margin: -4px 0 12px 0;">Coming soon — sync across devices.</p>
        <div class="sync-info">
          <div class="sync-row">
            <span class="sync-label">Device:</span>
            <span class="sync-value">{deviceName}</span>
          </div>
          <div class="sync-row">
            <span class="sync-label">DB Version:</span>
            <span class="sync-value">{syncInfo?.db_version ?? '—'}</span>
          </div>
          <div class="sync-row">
            <span class="sync-label">Changes tracked:</span>
            <span class="sync-value">{syncInfo?.total_changes?.toLocaleString() ?? '—'}</span>
          </div>
          <div class="sync-row">
            <span class="sync-label">Auto-sync:</span>
            <label style="display: flex; align-items: center; gap: 8px; cursor: pointer;">
              <input 
                type="checkbox" 
                checked={$autoSyncEnabled}
                onchange={(e) => {
                  if (e.currentTarget.checked) {
                    startAutoSync();
                  } else {
                    stopAutoSync();
                  }
                }}
                style="accent-color: #00E4E4;"
              />
              <span style="color: #888888; font-size: 13px;">
                {$autoSyncEnabled ? 'On' : 'Off'}
              </span>
            </label>
          </div>
        </div>
        
        <div class="sync-buttons">
          <button
            class="btn-primary"
            onclick={handleSyncNow}
            disabled={loading}
          >
            {loading ? 'Syncing...' : 'Sync Now'}
          </button>
          
          <button
            class="btn-secondary"
            onclick={handleSignOut}
            disabled={loading}
          >
            Sign Out
          </button>
        </div>
        
        {#if syncStatus === 'success'}
          <div class="sync-status-success">Sync complete</div>
        {/if}
        
        
        
        <div class="last-sync">
          Last sync: {$lastSyncTime ?? 'Never'}
        </div>
        
        <button
          class="btn-secondary"
          onclick={() => { showSyncHistory = !showSyncHistory; if (showSyncHistory) syncHistoryData = getSyncHistory(); }}
          style="margin-top: 8px; width: 100%;"
        >
          {showSyncHistory ? 'Hide History' : 'Show History'}
        </button>
        
        {#if showSyncHistory}
          <div class="sync-history-panel">
            {#if syncHistoryData.length === 0}
              <p style="font-size: 12px; color: #666666; margin: 8px 0 0 0;">No sync history yet.</p>
            {:else}
              {#each syncHistoryData as entry (entry.timestamp)}
                <div class="sync-history-item">
                  <span style="font-size: 12px; color: #888888;">{new Date(entry.timestamp).toLocaleString()}</span>
                  <span style="font-size: 12px; color: #aaaaaa; margin-left: 8px;">
                    {#if entry.error}
                      <span style="color: #EF4444;">{entry.error}</span>
                    {:else}
                      Sent: {entry.changesSent} | Received: {entry.changesReceived}
                    {/if}
                  </span>
                </div>
              {/each}
            {/if}
          </div>
        {/if}
      </div>
    </div>
  {:else}
    <!-- Not logged in state -->
    <div class="account-auth">
      <p class="auth-description">Sign in to sync across devices</p>
      
      <div class="auth-form">
        <input
          type="email"
          class="auth-input"
          placeholder="Email"
          bind:value={email}
          disabled={loading}
        />
        
        <input
          type="password"
          class="auth-input"
          placeholder="Password"
          bind:value={password}
          disabled={loading}
          onkeydown={(e) => {
            if (e.key === 'Enter') handleSignIn();
          }}
        />
        
        {#if errorMessage}
          <div class="error-message">{errorMessage}</div>
        {/if}
        
        {#if successMessage}
          <div class="success-message">{successMessage}</div>
        {/if}
        
        <div class="auth-buttons">
          <button
            class="btn-primary"
            onclick={handleSignIn}
            disabled={loading}
          >
            {loading ? 'Signing in...' : 'Sign In'}
          </button>
          
          <button
class="btn-secondary"
            onclick={handleSignUp}
            disabled={loading}
          >
            Create Account
          </button>
        </div>
        
        <div class="auth-divider">
          <span>or continue without account</span>
        </div>
        
        <p class="auth-footer">
          Pane works fully offline.<br />
          Sync requires an account.
        </p>
      </div>
    </div>
  {/if}

  <div class="bridge-section">
    <h5>Bridge (File Sync)</h5>
    <p style="font-size: 12px; color: #666; margin: -4px 0 12px 0;">Sync system files with your OpenClaw machine</p>
    
    <div class="bridge-form">
      <label style="font-size: 13px; color: #888; margin-bottom: 4px; display: block;">Bridge URL</label>
      <input
        type="text"
        class="auth-input"
        placeholder="https://192.168.x.x:7342"
        bind:value={bridgeUrl}
        disabled={bridgeSaving}
        style="margin-bottom: 12px;"
      />
      
      <label style="font-size: 13px; color: #888; margin-bottom: 4px; display: block;">Auth Code</label>
      <input
        type="text"
        class="auth-input"
        placeholder="123456"
        bind:value={bridgeToken}
        disabled={bridgeSaving}
        style="margin-bottom: 12px;"
      />
      
      <button
        class="btn-primary"
        onclick={saveBridgeConfig}
        disabled={bridgeSaving}
        style="width: 100%;"
      >
        {bridgeSaving ? 'Saving...' : 'Save & Test'}
      </button>
      
      {#if bridgeMessage}
        <div class={bridgeStatus === 'connected' ? 'sync-status-success' : 'error-message'} style="margin-top: 8px;">
          {bridgeMessage}
        </div>
      {/if}
      
      {#if !bridgeMessage && bridgeStatus === 'connected' && bridgeUrl}
        <div class="bridge-status-indicator" style="margin-top: 8px;">
          <span class="status-dot-connected"></span>
          <span style="font-size: 13px; color: #10B981;">Connected</span>
        </div>
      {:else if !bridgeMessage && bridgeStatus === 'not_configured'}
        <div style="font-size: 13px; color: #666; margin-top: 8px;">Not configured</div>
      {:else if !bridgeMessage && bridgeStatus === 'error'}
        <div style="font-size: 13px; color: #EF4444; margin-top: 8px;">Unreachable ❌</div>
      {/if}
    </div>
  </div>
</section>

<style>
  .section-desc {
    font-size: 13px;
    color: #666666;
    margin: 0 0 20px 0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  .settings-section {
    margin-bottom: 24px;
  }
  
  h4 {
    font-size: 14px;
    font-weight: 600;
    color: #e0e0e0;
    margin: 0 0 16px 0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  h5 {
    font-size: 14px;
    font-weight: 600;
    color: #e0e0e0;
    margin: 0 0 12px 0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  /* Auth form (not logged in) */
  .account-auth {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  
  .auth-description {
    font-size: 14px;
    color: #888888;
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  .auth-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  
  .auth-input {
    width: 100%;
    padding: 10px 12px;
    background: #0a0a0f;
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 4px;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    font-size: 14px;
    outline: none;
    transition: border-color 0.15s;
    box-sizing: border-box;
  }
  
  .auth-input:focus {
    border-color: #00E4E4;
  }
  
  .auth-input::placeholder {
    color: #666666;
  }
  
  .auth-input:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  
  .error-message {
    font-size: 13px;
    color: #EF4444;
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  .success-message {
    font-size: 13px;
    color: #10B981;
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  .auth-buttons {
    display: flex;
    gap: 12px;
    margin-top: 4px;
  }
  
  .btn-primary {
    background: #00E4E4;
    color: #0a0a0f;
    font-weight: 600;
    padding: 10px 20px;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    font-size: 14px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    transition: background 0.15s;
    flex: 1;
  }
  
  .btn-primary:hover {
    background: #00C8C8;
  }
  
  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  
  .btn-secondary {
    background: transparent;
    color: #888888;
    font-weight: 400;
    padding: 10px 20px;
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 4px;
    cursor: pointer;
    font-size: 14px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    transition: border-color 0.15s, color 0.15s;
    flex: 1;
  }
  
  .btn-secondary:hover {
    border-color: #888888;
    color: #e0e0e0;
  }
  
  .btn-secondary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  
  .auth-divider {
    margin: 8px 0;
    text-align: center;
    position: relative;
  }
  
  .auth-divider span {
    font-size: 12px;
    color: #666666;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    background: rgba(255,255,255,0.05);
    padding: 0 12px;
    position: relative;
    z-index: 1;
  }
  
  .auth-divider::before {
    content: '';
    position: absolute;
    top: 50%;
    left: 0;
    right: 0;
    height: 1px;
    background: rgba(255,255,255,0.06);
  }
  
  .auth-footer {
    font-size: 13px;
    color: #666666;
    margin: 0;
    line-height: 1.5;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  /* Connected state */
  .account-connected {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  
  .connected-status {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  
  .status-dot-connected {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #10B981;
  }
  
  .status-text {
    font-size: 14px;
    color: #10B981;
    font-weight: 600;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  .account-email {
    font-size: 14px;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  .bridge-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding-top: 16px;
    border-top: 1px solid rgba(255,255,255,0.06);
  }
  
  .bridge-form {
    display: flex;
    flex-direction: column;
  }
  
  .bridge-status-indicator {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  
  .sync-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding-top: 16px;
    border-top: 1px solid rgba(255,255,255,0.06);
  }
  
  .sync-info {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  
  .sync-row {
    display: flex;
    gap: 8px;
    font-size: 14px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  .sync-label {
    color: #666666;
    min-width: 140px;
  }
  
  .sync-value {
    color: #888888;
  }
  
  .sync-buttons {
    display: flex;
    gap: 12px;
    margin-top: 8px;
  }
  
  .sync-status-success {
    font-size: 13px;
    color: #10B981;
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  .sync-status-error {
    font-size: 13px;
    color: #EF4444;
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
  
  .last-sync {
    font-size: 13px;
    color: #666666;
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }
.sync-history-panel {
    margin-top: 8px;
    max-height: 150px;
    overflow-y: auto;
  }
  
  .sync-history-item {
    padding: 4px 0;
    border-bottom: 1px solid rgba(255,255,255,0.04);
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  
</style>
