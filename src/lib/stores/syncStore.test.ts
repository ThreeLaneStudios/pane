import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { writable } from 'svelte/store';

// ADR-0070 Decision 3 (#3195 PR A, item 4): the Settings Auto-sync toggle
// calls startAutoSync() with no flag check of its own (Settings.svelte's
// SYNC card, `onclick={() => $autoSyncEnabled ? stopAutoSync() : startAutoSync()}`).
// The ADR's own text claims startAutoSync() is "only ever reached from
// initAuthInternal / signIn" — false. These tests exercise the single
// choke point inside startAutoSync() itself, which protects both that
// claimed call path AND the toggle, without Settings.svelte needing its
// own copy of the check.

const { isAccountEnabledMock } = vi.hoisted(() => ({ isAccountEnabledMock: vi.fn() }));

vi.mock('./authStore', () => ({
    authSession: writable(null),
    isAccountEnabled: isAccountEnabledMock,
}));

vi.mock('$lib/refreshEvent', () => ({
    subscribeDataChanged: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({
    invoke: vi.fn(),
}));

import { startAutoSync, stopAutoSync, autoSyncEnabled } from './syncStore';

describe('syncStore — startAutoSync account_enabled gate (ADR-0070 Decision 3, #3195 PR A)', () => {
    beforeEach(() => {
        autoSyncEnabled.set(false);
        isAccountEnabledMock.mockReset();
    });

    it('refuses to start when the flag is off — this is what the Settings toggle hits today with no check of its own', async () => {
        isAccountEnabledMock.mockResolvedValue(false);
        await startAutoSync();
        expect(get(autoSyncEnabled)).toBe(false);
    });

    it('starts normally when the flag is on (no regression for the initAuthInternal/signIn callers)', async () => {
        isAccountEnabledMock.mockResolvedValue(true);
        await startAutoSync();
        expect(get(autoSyncEnabled)).toBe(true);
        stopAutoSync();
    });
});
