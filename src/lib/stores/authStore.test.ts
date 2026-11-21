import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { authSession, isAuthenticated, signOut, initAuth } from './authStore';
import * as api from '$lib/api';

// Mock the API module
vi.mock('$lib/api', () => ({
  authSignOut: vi.fn(),
  clearEncryption: vi.fn(),
  clearAuthSession: vi.fn(),
  checkCleanupPending: vi.fn().mockResolvedValue(null),
  writeCleanupFlag: vi.fn().mockResolvedValue(undefined),
  clearCleanupFlag: vi.fn().mockResolvedValue(undefined),
  getAuthSession: vi.fn().mockResolvedValue(null),
  authRefresh: vi.fn(),
  saveAuthSession: vi.fn(),
  syncConfigure: vi.fn(),
  initEncryption: vi.fn(),
  // ADR-0070 Decision 3 (#3195 PR A): account_enabled flag reader.
  // Default to 'true' here so every PRE-EXISTING test in this file (written
  // before the flag existed) keeps exercising the on-path it was written
  // for, without having to touch every single test. Tests that care about
  // the off-path set this explicitly per-test.
  getAiSetting: vi.fn().mockResolvedValue('true'),
}));

// Mock syncStore to avoid side effects
vi.mock('./syncStore', () => ({
  stopAutoSync: vi.fn(),
  startAutoSync: vi.fn(),
}));

describe('authStore', () => {
  beforeEach(() => {
    // Reset store
    authSession.set(null);
    vi.clearAllMocks();
  });

  describe('signOut', () => {
    it('always sets authSession to null even when clearEncryption fails', async () => {
      // Set up initial authenticated state
      authSession.set({
        access_token: 'test_token',
        refresh_token: 'test_refresh',
        user_id: 'test_user',
        email: 'test@example.com',
        expires_at: Date.now() / 1000 + 3600,
      });

      // Verify we're authenticated
      expect(get(isAuthenticated)).toBe(true);

      // Mock clearEncryption to throw an error
      vi.mocked(api.clearEncryption).mockRejectedValue(new Error('Encryption clear failed'));
      vi.mocked(api.clearAuthSession).mockResolvedValue(undefined);

      // Call signOut
      await signOut();

      // CRITICAL: authSession must be null even though clearEncryption failed
      expect(get(authSession)).toBeNull();
      expect(get(isAuthenticated)).toBe(false);
    });

    it('always sets authSession to null even when clearAuthSession fails', async () => {
      // Set up initial authenticated state
      authSession.set({
        access_token: 'test_token',
        refresh_token: 'test_refresh',
        user_id: 'test_user',
        email: 'test@example.com',
        expires_at: Date.now() / 1000 + 3600,
      });

      expect(get(isAuthenticated)).toBe(true);

      // Mock clearAuthSession to throw an error
      vi.mocked(api.clearEncryption).mockResolvedValue(undefined);
      vi.mocked(api.clearAuthSession).mockRejectedValue(new Error('Auth session clear failed'));

      // Call signOut
      await signOut();

      // CRITICAL: authSession must be null even though clearAuthSession failed
      expect(get(authSession)).toBeNull();
      expect(get(isAuthenticated)).toBe(false);
    });

    it('always sets authSession to null even when both cleanup operations fail', async () => {
      // Set up initial authenticated state
      authSession.set({
        access_token: 'test_token',
        refresh_token: 'test_refresh',
        user_id: 'test_user',
        email: 'test@example.com',
        expires_at: Date.now() / 1000 + 3600,
      });

      expect(get(isAuthenticated)).toBe(true);

      // Mock both cleanup operations to fail
      vi.mocked(api.clearEncryption).mockRejectedValue(new Error('Encryption clear failed'));
      vi.mocked(api.clearAuthSession).mockRejectedValue(new Error('Auth session clear failed'));

      // Call signOut
      await signOut();

      // CRITICAL: authSession must be null even though all cleanup failed
      // This ensures the UI always navigates back to auth screen
      expect(get(authSession)).toBeNull();
      expect(get(isAuthenticated)).toBe(false);
    });

    it('successfully signs out when all operations succeed', async () => {
      // Set up initial authenticated state
      authSession.set({
        access_token: 'test_token',
        refresh_token: 'test_refresh',
        user_id: 'test_user',
        email: 'test@example.com',
        expires_at: Date.now() / 1000 + 3600,
      });

      expect(get(isAuthenticated)).toBe(true);

      // Mock all operations to succeed
      vi.mocked(api.authSignOut).mockResolvedValue(undefined);
      vi.mocked(api.clearEncryption).mockResolvedValue(undefined);
      vi.mocked(api.clearAuthSession).mockResolvedValue(undefined);

      // Call signOut
      await signOut();

      // Verify all cleanup operations were called
      expect(api.authSignOut).toHaveBeenCalledWith('test_token');
      expect(api.clearEncryption).toHaveBeenCalled();
      expect(api.clearAuthSession).toHaveBeenCalled();

      // Verify authSession is null
      expect(get(authSession)).toBeNull();
      expect(get(isAuthenticated)).toBe(false);
    });
  });
});

describe('authStore — cleanup flag (#1079)', () => {
  beforeEach(() => {
    authSession.set(null);
    vi.clearAllMocks();
    // Default: no pending flag
    vi.mocked(api.checkCleanupPending).mockResolvedValue(null);
    vi.mocked(api.writeCleanupFlag).mockResolvedValue(undefined);
    vi.mocked(api.clearCleanupFlag).mockResolvedValue(undefined);
    vi.mocked(api.clearEncryption).mockResolvedValue(undefined);
    vi.mocked(api.clearAuthSession).mockResolvedValue(undefined);
  });

  describe('signOut cleanup flag writes', () => {
    it('writes cleanup flag when clearEncryption fails', async () => {
      vi.mocked(api.clearEncryption).mockRejectedValue(new Error('fail'));
      vi.mocked(api.clearAuthSession).mockResolvedValue(undefined);
      await signOut();
      expect(api.writeCleanupFlag).toHaveBeenCalledWith(true, false);
    });

    it('writes cleanup flag when clearAuthSession fails', async () => {
      vi.mocked(api.clearEncryption).mockResolvedValue(undefined);
      vi.mocked(api.clearAuthSession).mockRejectedValue(new Error('fail'));
      await signOut();
      expect(api.writeCleanupFlag).toHaveBeenCalledWith(false, true);
    });

    it('writes cleanup flag when both fail', async () => {
      vi.mocked(api.clearEncryption).mockRejectedValue(new Error('fail'));
      vi.mocked(api.clearAuthSession).mockRejectedValue(new Error('fail'));
      await signOut();
      expect(api.writeCleanupFlag).toHaveBeenCalledWith(true, true);
    });

    it('does NOT write cleanup flag when both succeed', async () => {
      vi.mocked(api.clearEncryption).mockResolvedValue(undefined);
      vi.mocked(api.clearAuthSession).mockResolvedValue(undefined);
      await signOut();
      expect(api.writeCleanupFlag).not.toHaveBeenCalled();
    });
  });

  describe('initAuth cleanup retry', () => {
    it('retries and clears flag on success', async () => {
      vi.mocked(api.checkCleanupPending).mockResolvedValue({
        attempts: 1,
        created_at: '2026-01-01T00:00:00Z',
        cleanup_encryption: true,
        cleanup_auth_session: true,
      } as any);
      vi.mocked(api.clearEncryption).mockResolvedValue(undefined);
      vi.mocked(api.clearAuthSession).mockResolvedValue(undefined);
      vi.mocked(api.getAuthSession).mockResolvedValue(null);

      await initAuth();

      expect(api.clearCleanupFlag).toHaveBeenCalled();
      expect(api.writeCleanupFlag).not.toHaveBeenCalled();
    });

    it('gives up after 3 attempts', async () => {
      vi.mocked(api.checkCleanupPending).mockResolvedValue({
        attempts: 3,
        created_at: '2026-01-01T00:00:00Z',
        cleanup_encryption: true,
        cleanup_auth_session: false,
      } as any);
      vi.mocked(api.clearEncryption).mockRejectedValue(new Error('still failing'));
      vi.mocked(api.getAuthSession).mockResolvedValue(null);

      await initAuth();

      expect(api.clearCleanupFlag).toHaveBeenCalled();
    });

    it('increments attempts on failure', async () => {
      vi.mocked(api.checkCleanupPending).mockResolvedValue({
        attempts: 1,
        created_at: '2026-01-01T00:00:00Z',
        cleanup_encryption: true,
        cleanup_auth_session: false,
      } as any);
      vi.mocked(api.clearEncryption).mockRejectedValue(new Error('fail'));
      vi.mocked(api.getAuthSession).mockResolvedValue(null);

      await initAuth();

      expect(api.writeCleanupFlag).toHaveBeenCalledWith(true, false, 2);
      expect(api.clearCleanupFlag).not.toHaveBeenCalled();
    });
  });
});

describe('authStore — account_enabled flag (ADR-0070 Decision 3, #3195 PR A)', () => {
  beforeEach(() => {
    authSession.set(null);
    vi.clearAllMocks();
  });

  describe('T2a: initAuth off touches no session storage', () => {
    it('reads the flag first and returns before any session-storage call, even with an expired stored session waiting', async () => {
      // Flag absent/off
      vi.mocked(api.getAiSetting).mockResolvedValue(null);
      // Red-today setup: an EXPIRED stored session, which today's code would
      // reach via getAuthSession() and then clearAuthSession() on refresh
      // failure — none of that must happen with the flag off.
      vi.mocked(api.getAuthSession).mockResolvedValue({
        access_token: 'tok',
        refresh_token: 'ref',
        user_id: 'u1',
        email: 'e@example.com',
        expires_at: Math.floor(Date.now() / 1000) - 100, // expired
      } as any);

      await initAuth();

      expect(api.getAiSetting).toHaveBeenCalledWith('account_enabled');
      expect(api.checkCleanupPending).not.toHaveBeenCalled();
      expect(api.getAuthSession).not.toHaveBeenCalled();
      expect(api.clearAuthSession).not.toHaveBeenCalled();
      expect(api.authRefresh).not.toHaveBeenCalled();
      expect(get(authSession)).toBeNull();
    });

    it('treats any value other than the literal string "true" as off', async () => {
      vi.mocked(api.getAiSetting).mockResolvedValue('false');
      await initAuth();
      expect(api.getAuthSession).not.toHaveBeenCalled();
      expect(get(authSession)).toBeNull();
    });

    it('treats a failed flag read as off, not a thrown error', async () => {
      vi.mocked(api.getAiSetting).mockRejectedValue(new Error('IPC failure'));
      await expect(initAuth()).resolves.toBeUndefined();
      expect(api.getAuthSession).not.toHaveBeenCalled();
      expect(get(authSession)).toBeNull();
    });
  });

  it('flag on: normal init path still runs (no regression)', async () => {
    vi.mocked(api.getAiSetting).mockResolvedValue('true');
    vi.mocked(api.checkCleanupPending).mockResolvedValue(null);
    vi.mocked(api.getAuthSession).mockResolvedValue(null);
    await initAuth();
    expect(api.getAuthSession).toHaveBeenCalled();
  });

  describe('T4b (#3195 PR C): flag on — get_auth_session called exactly once at boot', () => {
    it('calls getAuthSession exactly once when there is no stored session', async () => {
      vi.mocked(api.getAiSetting).mockResolvedValue('true');
      vi.mocked(api.checkCleanupPending).mockResolvedValue(null);
      vi.mocked(api.getAuthSession).mockResolvedValue(null);

      await initAuth();

      expect(api.getAuthSession).toHaveBeenCalledTimes(1);
    });

    it('calls getAuthSession exactly once when a valid (non-expired) session is stored', async () => {
      vi.mocked(api.getAiSetting).mockResolvedValue('true');
      vi.mocked(api.checkCleanupPending).mockResolvedValue(null);
      vi.mocked(api.getAuthSession).mockResolvedValue({
        access_token: 'tok',
        refresh_token: 'ref',
        user_id: 'u1',
        email: 'e@example.com',
        expires_at: Math.floor(Date.now() / 1000) + 3600, // not expired
      } as any);

      await initAuth();

      expect(api.getAuthSession).toHaveBeenCalledTimes(1);
      expect(get(authSession)?.email).toBe('e@example.com');
    });
  });
});
