import { writable, get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { authSession, isAccountEnabled } from './authStore';
import { subscribeDataChanged } from '$lib/refreshEvent';

export const autoSyncEnabled = writable(false);
export const lastSyncTime = writable<string | null>(null);

let syncTimeout: ReturnType<typeof setTimeout> | null = null;
let pushDebounceTimer: ReturnType<typeof setTimeout> | null = null;
let consecutiveFailures = 0;
const BASE_INTERVAL = 30000; // 30 seconds
const MAX_INTERVAL = 300000; // 5 minutes
const MAX_FAILURES = 10;

// #125: Don't auto-push for the first 60 seconds after launch
// Prevents propagating stale data before initialization stabilizes
const LAUNCH_TIME = Date.now();
const PUSH_DELAY = 60000; // 60 seconds

function getBackoffInterval(): number {
    if (consecutiveFailures === 0) return BASE_INTERVAL;
    return Math.min(BASE_INTERVAL * Math.pow(2, consecutiveFailures), MAX_INTERVAL);
}

function schedulePull() {
    syncTimeout = setTimeout(async () => {
        const session = get(authSession);
        if (!session) {
            if (get(autoSyncEnabled)) schedulePull();
            return;
        }

        try {
            const result = await invoke('sync_pull') as string;
            const fileResult = await invoke('sync_files_pull') as string;
            consecutiveFailures = 0;
            if (!result.includes('No pending') && !result.includes('skipped own')) {
                console.log('[auto-sync] Pull:', result, '| Files:', fileResult);
                lastSyncTime.set(new Date().toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit', hour12: true }));
                window.dispatchEvent(new CustomEvent('pane-sync-received'));
            }
        } catch (e) {
            consecutiveFailures++;
            console.error(`[auto-sync] Pull failed (${consecutiveFailures}/${MAX_FAILURES}):`, e);
            if (consecutiveFailures >= MAX_FAILURES) {
                console.error('[auto-sync] Too many consecutive failures, stopping auto-sync');
                stopAutoSync();
                return;
            }
        }

        if (get(autoSyncEnabled)) {
            schedulePull();
        }
    }, getBackoffInterval());
}

// Start auto-sync polling (with exponential backoff on failure).
//
// ADR-0070 Decision 3 (#3195 PR A, item 4): single choke point for the
// account_enabled flag. The ADR's own text claims startAutoSync() is "only
// ever reached from initAuthInternal / signIn" — that's false: Settings.svelte's
// SYNC card calls it directly from the Auto-sync toggle with no flag/
// $isAuthenticated check at all. Checking the flag HERE (rather than at each
// call site) protects both the existing callers (initAuthInternal/signIn) AND
// the Settings toggle, and any future caller inherits the protection too —
// one function per operation, not N copies of "is this allowed" scattered
// across call sites.
export async function startAutoSync() {
    if (!(await isAccountEnabled())) {
        console.log('[auto-sync] account_enabled flag is off — refusing to start (ADR-0070)');
        return;
    }
    // EMERGENCY early return removed — egress bug fixed in v11.28.70
    autoSyncEnabled.set(true);
    consecutiveFailures = 0;
    schedulePull();
    console.log('[auto-sync] Started');
}

export function stopAutoSync() {
    autoSyncEnabled.set(false);
    if (syncTimeout) {
        clearTimeout(syncTimeout);
        syncTimeout = null;
    }
    if (pushDebounceTimer) {
        clearTimeout(pushDebounceTimer);
        pushDebounceTimer = null;
    }
    console.log('[auto-sync] Stopped');
}

// Call this after any DB write (note save, project create, etc.)
export function notifyChange() {
    if (!get(autoSyncEnabled)) return;

    // Debounce: wait 3 seconds after last change before pushing
    if (pushDebounceTimer) {
        clearTimeout(pushDebounceTimer);
    }

    pushDebounceTimer = setTimeout(async () => {
        // #125: Skip push during startup stabilization period
        if (Date.now() - LAUNCH_TIME < PUSH_DELAY) {
            console.log('[auto-sync] Skipping push — startup stabilization period');
            return;
        }
        try {
            const result = await invoke('sync_push') as string;
            const fileResult = await invoke('sync_files_push') as string;
            if (!result.includes('No changes') || !fileResult.includes('0 files')) {
                console.log('[auto-sync] Push:', result, '| Files:', fileResult);
                lastSyncTime.set(new Date().toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit', hour12: true }));
            }
        } catch (e) {
            console.error('[auto-sync] Push failed:', e);
        }
    }, 3000);
}

// #2972 stage 3 (item 2): folded the old `pane-data-changed`/
// `pane-note-created` DOM-only listener pair into the single shared
// helper, which now also hears the BACKEND channel — closing the "sheet
// edits (and every other backend/agent-only mutation) never trigger
// auto-sync" gap the spec names (§34.2): previously this store only ever
// heard app-initiated DOM dispatches, so an agent tool-loop edit (backend
// `emit()` only) never queued a push at all.
if (typeof window !== 'undefined') {
    subscribeDataChanged(() => notifyChange());
}
