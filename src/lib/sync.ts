import { invoke } from '@tauri-apps/api/core';

let lastAutoSync = 0;
const AUTO_SYNC_DEBOUNCE = 500; // 500ms debounce

// Retry state
let retryCount = 0;
const MAX_RETRIES = 5;
const RETRY_DELAYS = [2000, 4000, 8000, 16000, 30000]; // Exponential backoff (2s, 4s, 8s, 16s, 30s)
let retryTimeout: ReturnType<typeof setTimeout> | null = null;

// Sync history interface
interface SyncHistoryEntry {
  timestamp: number;
  peer: string;
  changesSent: number;
  changesReceived: number;
  conflicts: number;
  error?: string;
}

// Sync status interface
interface SyncStatus {
  status: string;
  conflictCount: number;
  lastSyncTime: number | null;
  isRetrying: boolean;
}

// Global sync status state
let syncStatusCallbacks: ((status: SyncStatus) => void)[] = [];

/**
 * Subscribe to sync status updates
 */
export function subscribeSyncStatus(callback: (status: SyncStatus) => void): () => void {
  syncStatusCallbacks.push(callback);
  return () => {
    const index = syncStatusCallbacks.indexOf(callback);
    if (index > -1) {
      syncStatusCallbacks.splice(index, 1);
    }
  };
}

/**
 * Notify all subscribers of sync status changes
 */
function notifySyncStatus(status: Partial<SyncStatus>) {
  const currentStatus = getCurrentSyncStatus();
  const newStatus = { ...currentStatus, ...status };
  syncStatusCallbacks.forEach(callback => callback(newStatus));
}

/**
 * Get current sync status from localStorage and runtime state
 */
function getCurrentSyncStatus(): SyncStatus {
  const lastSyncTime = localStorage.getItem('pane-last-sync-time');
  return {
    status: 'Ready',
    conflictCount: 0,
    lastSyncTime: lastSyncTime ? parseInt(lastSyncTime) : null,
    isRetrying: retryTimeout !== null
  };
}

/**
 * Add entry to sync history
 */
function addSyncHistory(entry: SyncHistoryEntry) {
  try {
    const historyJson = localStorage.getItem('pane-sync-history');
    let history: SyncHistoryEntry[] = historyJson ? JSON.parse(historyJson) : [];
    
    // Add new entry at beginning
    history.unshift(entry);
    
    // Keep only last 20 entries
    if (history.length > 20) {
      history = history.slice(0, 20);
    }
    
    localStorage.setItem('pane-sync-history', JSON.stringify(history));
  } catch (error) {
    console.error('[SYNC-HISTORY] Failed to save sync history:', error);
  }
}

/**
 * Get sync history from localStorage
 */
export function getSyncHistory(): SyncHistoryEntry[] {
  try {
    const historyJson = localStorage.getItem('pane-sync-history');
    return historyJson ? JSON.parse(historyJson) : [];
  } catch (error) {
    console.error('[SYNC-HISTORY] Failed to load sync history:', error);
    return [];
  }
}

/**
 * Parse sync result to extract change counts and conflicts
 */
function parseSyncResult(resultString: string): { changesSent: number; changesReceived: number; conflicts: number } {
  try {
    // Parse result string like "Bidirectional sync complete: received 5 changes from 192.168.1.100:21059, sent 3 changes to 192.168.1.100:21059"
    const receivedMatch = resultString.match(/received (\d+) changes/);
    const sentMatch = resultString.match(/sent (\d+) changes/);
    
    const conflictMatch = resultString.match(/conflicts (\d+)/);
    return {
      changesSent: sentMatch ? parseInt(sentMatch[1]) : 0,
      changesReceived: receivedMatch ? parseInt(receivedMatch[1]) : 0,
      conflicts: conflictMatch ? parseInt(conflictMatch[1]) : 0
    };
  } catch (error) {
    console.error('[SYNC] Failed to parse sync result:', error);
    return { changesSent: 0, changesReceived: 0, conflicts: 0 };
  }
}

/**
 * Detect conflicts in received changes
 */
async function detectConflicts(changes: any[]): Promise<number> {
  try {
    const conflictCount = await invoke('detect_sync_conflicts', { changes });
    return Number(conflictCount) || 0;
  } catch (error) {
    console.error('[SYNC] Failed to detect conflicts:', error);
    return 0;
  }
}

/**
 * Retry sync with exponential backoff
 */
async function retrySync(peer: string, attempt: number = 0): Promise<void> {
  if (attempt >= MAX_RETRIES) {
    console.error('[SYNC-RETRY] Max retries exceeded');
    retryCount = 0;
    retryTimeout = null;
    notifySyncStatus({ status: 'Peer unreachable', isRetrying: false });
    
    // Add failed sync to history
    addSyncHistory({
      timestamp: Date.now(),
      peer,
      changesSent: 0,
      changesReceived: 0,
      conflicts: 0,
      error: 'Peer unreachable (max retries exceeded)'
    });
    return;
  }

  const delay = RETRY_DELAYS[Math.min(attempt, RETRY_DELAYS.length - 1)];
  
  retryTimeout = setTimeout(async () => {
    try {
      notifySyncStatus({ status: 'Reconnecting...', isRetrying: true });
      console.log(`[SYNC-RETRY] Attempt ${attempt + 1}/${MAX_RETRIES} after ${delay}ms delay`);
      
      const result = await invoke('sync_with_peer', { 
        peerAddress: peer.includes(':') ? peer : `${peer}:21059`
      });
      
      // Success - reset retry state
      retryCount = 0;
      retryTimeout = null;
      const parsedResult = parseSyncResult(String(result));
      const now = Date.now();
      
      localStorage.setItem('pane-last-sync-time', now.toString());
      notifySyncStatus({ 
        status: 'Last synced: just now', 
        lastSyncTime: now,
        isRetrying: false,
        conflictCount: parsedResult.conflicts
      });
      
      // Add successful sync to history
      addSyncHistory({
        timestamp: now,
        peer,
        changesSent: parsedResult.changesSent,
        changesReceived: parsedResult.changesReceived,
        conflicts: parsedResult.conflicts
      });
      
      console.log('[SYNC-RETRY] Retry successful:', result);
    } catch (error) {
      console.error(`[SYNC-RETRY] Attempt ${attempt + 1} failed:`, error);
      retrySync(peer, attempt + 1);
    }
  }, delay);
}

/**
 * Sync with peer with retry logic
 */
export async function syncWithPeer(peer: string): Promise<string> {
  try {
    notifySyncStatus({ status: 'Connecting…', isRetrying: false });
    
    const result = await invoke('sync_with_peer', { 
      peerAddress: peer.includes(':') ? peer : `${peer}:21059`
    });
    
    // Success - reset any existing retry state
    if (retryTimeout) {
      clearTimeout(retryTimeout);
      retryTimeout = null;
    }
    retryCount = 0;
    
    const parsedResult = parseSyncResult(String(result));
    const now = Date.now();
    
    if (parsedResult.conflicts > 0) {
      console.log(`[SYNC] ${parsedResult.conflicts} conflict(s) resolved (last-write-wins)`);
    }
    
    localStorage.setItem('pane-last-sync-time', now.toString());
    notifySyncStatus({ 
      status: 'Last synced: just now', 
      lastSyncTime: now,
      conflictCount: parsedResult.conflicts
    });
    
    // Add successful sync to history
    addSyncHistory({
      timestamp: now,
      peer,
      changesSent: parsedResult.changesSent,
      changesReceived: parsedResult.changesReceived,
      conflicts: parsedResult.conflicts
    });
    
    // Save the peer IP to localStorage after successful sync
    localStorage.setItem('pane-sync-peer', peer.includes(':') ? peer : `${peer}:21059`);
    
    return String(result);
  } catch (error) {
    console.error('[SYNC] Sync failed, starting retry sequence:', error);
    
    // Add failed sync to history
    addSyncHistory({
      timestamp: Date.now(),
      peer,
      changesSent: 0,
      changesReceived: 0,
      conflicts: 0,
      error: String(error)
    });
    
    // Start retry sequence
    retrySync(peer, 0);
    throw error;
  }
}

/**
 * Auto-sync with known peers after save operations
 * Fire-and-forget background sync with silent error handling
 */
export async function autoSync(): Promise<void> {
  try {
    // Debounce rapid saves to prevent spam
    const now = Date.now();
    if (now - lastAutoSync < AUTO_SYNC_DEBOUNCE) {
      return;
    }
    lastAutoSync = now;

    // Get saved peer from localStorage
    const peer = localStorage.getItem('pane-sync-peer');
    if (!peer) {
      // No saved peer, nothing to sync with
      return;
    }

    // Fire-and-forget sync in background - don't await or show errors
    setTimeout(async () => {
      try {
        await syncWithPeer(peer);
        console.log('[AUTO-SYNC] Background sync completed');
      } catch (error) {
        // Silent error handling - don't interrupt user experience
        console.log('[AUTO-SYNC] Background sync failed (silent):', error);
      }
    }, 100); // Small delay to not block save operations
  } catch (error) {
    // Silent error handling - auto-sync should never break the app
    console.log('[AUTO-SYNC] Auto-sync setup failed (silent):', error);
  }
}