import { writable, derived, get } from 'svelte/store';
import * as api from '$lib/api';
import { startAutoSync, stopAutoSync } from './syncStore';

interface CleanupPendingFlag {
    attempts: number;
    created_at: string;
    cleanup_encryption: boolean;
    cleanup_auth_session: boolean;
}

interface AuthSession {
    access_token: string;
    refresh_token: string;
    user_id: string;
    email: string;
    expires_at: number;
    device_id?: string;
}

export const authSession = writable<AuthSession | null>(null);
export const isAuthenticated = derived(authSession, $s => $s !== null);
export const userEmail = derived(authSession, $s => $s?.email ?? '');

// ADR-0070 Decision 3 (#3195 PR A): `account_enabled` is a runtime
// `ai_settings` row, no UI writes it. 'true' is on; absent, anything else,
// or a failed read (getAiSetting throws / returns anything but 'true') is
// off. One reader, used by both initAuth() below and startAutoSync()
// (syncStore.ts) — not duplicated per call site.
export async function isAccountEnabled(): Promise<boolean> {
    try {
        const value = await api.getAiSetting('account_enabled');
        return value === 'true';
    } catch (e) {
        console.warn('[AUTH] Failed to read account_enabled flag, treating as off:', e);
        return false;
    }
}

// Initialize from stored session on app start
export async function initAuth() {
    // ADR-0070 Decision 3 (#3195 PR B): the test-mode bypass that used to
    // sit here (`import.meta.env.DEV && __PANE_TEST_MODE__`) existed only
    // to satisfy the AuthScreen startup gate — the gate is gone (Decision
    // 1), so the harness boots to Home the same way production does, with
    // no mock session needed. Removed here, not left for later.
    //
    // Read the flag FIRST, before the 5s race / initAuthInternal is even
    // constructed. Off → no session, no getAuthSession, no cleanup-flag
    // read, no 5s race.
    if (!(await isAccountEnabled())) {
        console.log('[AUTH] account_enabled flag is off — skipping account init (ADR-0070)');
        authSession.set(null);
        return;
    }

    // Wrap auth init with 5-second timeout for local-first degradation
    const timeoutPromise = new Promise<void>((_, reject) => 
        setTimeout(() => reject(new Error('Auth timeout')), 5000)
    );
    
    try {
        await Promise.race([initAuthInternal(), timeoutPromise]);
    } catch (e) {
        console.warn('[AUTH] Init timeout, falling back to local-only mode:', e);
        authSession.set(null);
    }
}

async function initAuthInternal() {
    // Check for pending cleanup from prior failed sign-out (#1079)
    try {
        const flag = await api.checkCleanupPending() as CleanupPendingFlag | null;
        if (flag) {
            console.error(`[AUTH] Cleanup pending (attempt ${flag.attempts}/3)`);
            let success = true;
            if (flag.cleanup_encryption) {
                try { await api.clearEncryption(); } catch { success = false; }
            }
            if (flag.cleanup_auth_session) {
                try { await api.clearAuthSession(); } catch { success = false; }
            }
            if (success) {
                await api.clearCleanupFlag();
                console.error('[AUTH] Pending cleanup completed successfully');
            } else if (flag.attempts >= 3) {
                await api.clearCleanupFlag();
                console.error('[AUTH] Cleanup exhausted after 3 attempts, giving up');
            } else {
                await api.writeCleanupFlag(flag.cleanup_encryption, flag.cleanup_auth_session, flag.attempts + 1);
                console.error(`[AUTH] Cleanup retry ${flag.attempts} failed, will retry next launch`);
            }
        }
    } catch (e) {
        console.error('[AUTH] Error checking cleanup flag:', e);
    }

    try {
        const session = await api.getAuthSession() as AuthSession | null;
        if (session) {
            // Check if token is expired
            const now = Math.floor(Date.now() / 1000);
            if (session.expires_at && session.expires_at < now) {
                // Try to refresh
                try {
                    const refreshed = await api.authRefresh(session.refresh_token) as AuthSession;
                    // Preserve device_id when refreshing
                    refreshed.device_id = session.device_id;
                    await api.saveAuthSession(refreshed);
                    authSession.set(refreshed);
                    // Clear stale cleanup flag on successful signin (#1079)
                    try { await api.clearCleanupFlag(); } catch { /* non-critical */ }
                    
                    // Re-configure sync with refreshed token
                    if (refreshed.device_id) {
                        await api.syncConfigure(refreshed.user_id, refreshed.device_id, refreshed.access_token);
                    }
                    
                    // Start auto-sync after restore
                    startAutoSync();
                } catch {
                    // Refresh failed, clear session
                    await api.clearAuthSession();
                    authSession.set(null);
                }
            } else {
                authSession.set(session);
                // Clear stale cleanup flag on successful signin (#1079)
                try { await api.clearCleanupFlag(); } catch { /* non-critical */ }

                // Configure sync if device_id exists
                if (session.device_id) {
                    await api.syncConfigure(session.user_id, session.device_id, session.access_token);
                }
                
                // Start auto-sync after restore
                startAutoSync();
            }
        }
    } catch (e) {
        console.error('[AUTH] Failed to init auth:', e);
        throw e;
    }
}

export async function signUp(email: string, password: string) {
    const session = await api.authSignUp(email, password) as AuthSession;
    
    // Register device and get device_id
    const deviceId = await api.registerDevice(session.access_token, session.user_id, 'Mac') as string;
    session.device_id = deviceId;
    
    // Configure sync manager
    await api.syncConfigure(session.user_id, deviceId, session.access_token);
    
    // Save session with device_id
    await api.saveAuthSession(session);
    authSession.set(session);
    // Clear stale cleanup flag on successful signup (#1079)
    try { await api.clearCleanupFlag(); } catch { /* non-critical */ }
    
    // Initialize encryption with the same password
    await api.initEncryption(email, password);
    
    // Start auto-sync
    startAutoSync();
    
    return session;
}

export async function signIn(email: string, password: string) {
    // ADR-0070 Decision 4 (#3195 PR C): auth_sign_in now returns a plain
    // AuthSession (GoTrue re-point) — no entitlement snapshot exists
    // anymore for a wrapper struct to carry.
    const session = await api.authSignIn(email, password) as AuthSession;

    // Save session immediately so user can get in
    // Session already saved by backend command, but authStore expects it to be set
    authSession.set(session);
    // Clear stale cleanup flag on successful signin (#1079)
    try { await api.clearCleanupFlag(); } catch { /* non-critical */ }
    
    // Non-blocking: set up sync (device registration, encryption, auto-sync)
    try {
        const deviceId = await api.registerDevice(session.access_token, session.user_id, 'Mac') as string;
        session.device_id = deviceId;
        await api.syncConfigure(session.user_id, deviceId, session.access_token);
        await api.saveAuthSession(session);
        await api.initEncryption(email, password);
        startAutoSync();
    } catch (syncErr) {
        console.warn('[AUTH] Sync setup failed (non-blocking):', syncErr);
    }
    
    return session;
}

export async function signOut() {
    const currentSession = get(authSession);
    
    if (currentSession?.access_token) {
        try { 
            await api.authSignOut(currentSession.access_token); 
        } catch (e) {
            console.error('Sign out error:', e);
        }
    }
    
    // Stop auto-sync
    stopAutoSync();
    
    // Clear encryption keys and auth session, tracking failures for retry (#1079)
    let encryptionFailed = false;
    let authSessionFailed = false;

    try {
        await api.clearEncryption();
    } catch (e) {
        console.error('[AUTH] Failed to clear encryption keys:', e);
        encryptionFailed = true;
    }
    
    try {
        await api.clearAuthSession();
    } catch (e) {
        console.error('[AUTH] Failed to clear auth session:', e);
        authSessionFailed = true;
    }

    // Write cleanup flag if any cleanup operation failed
    if (encryptionFailed || authSessionFailed) {
        try {
            await api.writeCleanupFlag(encryptionFailed, authSessionFailed);
            console.error('[AUTH] Cleanup flag written for retry on next launch');
        } catch (flagError) {
            console.error('[AUTH] Failed to write cleanup flag:', flagError);
        }
    }
    
    // ALWAYS update store to trigger navigation, even if cleanup failed
    authSession.set(null);
}
