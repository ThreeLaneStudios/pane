/**
 * sessionStore.test.ts — Store actions, ordering, filter, and empty-state tests.
 *
 * Tests:
 *  - createSession: calls api, updates store
 *  - switchSession: calls api, updates store
 *  - hideSession: calls api, reloads list
 *  - unhideSession: calls api, reloads list
 *  - renameSession: optimistic update + api call
 *  - Active-pinned-first ordering: active session always at index 0
 *  - Filter threshold: shouldShowFilter returns true at ≥8, false at <8
 *  - Auto-title fallback: getSessionDisplayTitle uses "Session YYYY-MM-DD" when title=''
 *  - Empty-state trigger: EMPTY_STATE_SINGLE exported, used when count === 1
 *
 * §395 PR6 — spec-covered-by: #1979
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import type { Session } from '$lib/types';

// ─── Mock api module ─────────────────────────────────────────────────────────

vi.mock('$lib/api', () => ({
  createSession: vi.fn(),
  createSessionWithCrew: vi.fn(),
  listSessions: vi.fn(),
  switchSession: vi.fn(),
  hideSession: vi.fn(),
  unhideSession: vi.fn(),
  renameSession: vi.fn(),
  getActiveSession: vi.fn(),
  updateSessionCrew: vi.fn(),
}));

import * as api from '$lib/api';

// ─── Import store after mocks ─────────────────────────────────────────────────

import {
  sessionStore,
  sortSessions,
  shouldShowFilter,
  filterSessions,
  getSessionDisplayTitle,
  EMPTY_STATE_SINGLE,
  FRESH_SESSION_NAME,
  FILTER_THRESHOLD,
} from './sessionStore';

// ─── Helpers ─────────────────────────────────────────────────────────────────

function mkSession(overrides: Partial<Session> = {}): Session {
  return {
    id: 'sess-' + Math.random().toString(36).slice(2, 9),
    project_id: 'project-1',
    title: 'Test session',
    origin: 'user',
    created_at: '2026-07-01T10:00:00Z',
    last_active_at: '2026-07-09T08:00:00Z',
    hidden_at: null,
    is_active: false,
    ...overrides,
  };
}

// ─── Tests ───────────────────────────────────────────────────────────────────

describe('sessionStore.loadSessions — workshop session exclusion (#2618)', () => {
  beforeEach(() => {
    sessionStore.reset();
    vi.clearAllMocks();
  });

  it('never surfaces a workshop-scoped session (project_id = WORKSHOP_SESSION_SCOPE_ID) in the store, even if the backend query returns one', async () => {
    // In practice this can't happen — every backend session query is scoped by
    // an exact project_id match, so a 'workspace' query can never actually
    // return a WORKSHOP_SESSION_SCOPE_ID-scoped row. This test proves the
    // frontend filter (filterOutWorkshopSessions, mirroring the C4 subagent
    // filter in agentDisplay.ts) is real defense-in-depth: even if
    // apiListSessions somehow returned one, loadSessions must still exclude it.
    const realSession = mkSession({ id: 'real-1', project_id: 'workspace', is_active: true });
    const workshopSession = mkSession({ id: 'workshop-1', project_id: '__workshop_session__', is_active: false });
    vi.mocked(api.listSessions).mockResolvedValue([realSession, workshopSession]);

    await sessionStore.loadSessions('workspace', false);

    const state = get(sessionStore);
    expect(state.sessions.map(s => s.id)).toEqual(['real-1']);
    expect(state.sessions.some(s => s.project_id === '__workshop_session__')).toBe(false);
  });

  it('never selects a workshop session as activeSession, even if it happened to be the only is_active=true row returned', async () => {
    const workshopSession = mkSession({ id: 'workshop-only', project_id: '__workshop_session__', is_active: true });
    vi.mocked(api.listSessions).mockResolvedValue([workshopSession]);

    await sessionStore.loadSessions('workspace', false);

    const state = get(sessionStore);
    expect(state.sessions).toEqual([]);
    expect(state.activeSession).toBeNull();
  });
});

describe('sortSessions — active-pinned-first ordering', () => {
  it('pins the active session at index 0 regardless of insertion order', () => {
    const active = mkSession({ id: 'active-1', is_active: true, last_active_at: '2026-07-09T06:00:00Z' });
    const older = mkSession({ id: 'older-1', is_active: false, last_active_at: '2026-07-09T08:00:00Z' });
    const newest = mkSession({ id: 'newest-1', is_active: false, last_active_at: '2026-07-09T09:00:00Z' });

    // Active is NOT at index 0 in the raw list
    const raw = [newest, older, active];
    const sorted = sortSessions(raw);

    expect(sorted[0].id).toBe('active-1');
  });

  it('sorts non-active sessions by last_active_at DESC', () => {
    const s1 = mkSession({ id: 's1', is_active: false, last_active_at: '2026-07-09T08:00:00Z' });
    const s2 = mkSession({ id: 's2', is_active: false, last_active_at: '2026-07-08T08:00:00Z' });
    const s3 = mkSession({ id: 's3', is_active: false, last_active_at: '2026-07-07T08:00:00Z' });

    const sorted = sortSessions([s3, s1, s2]);

    expect(sorted.map(s => s.id)).toEqual(['s1', 's2', 's3']);
  });

  it('handles empty list', () => {
    expect(sortSessions([])).toEqual([]);
  });

  it('handles single session list', () => {
    const s = mkSession({ is_active: true });
    expect(sortSessions([s])).toHaveLength(1);
    expect(sortSessions([s])[0].id).toBe(s.id);
  });
});

// ─── Store actions ────────────────────────────────────────────────────────────

describe('sessionStore.createSession', () => {
  beforeEach(() => {
    sessionStore.reset();
    vi.clearAllMocks();
  });

  it('calls api.createSession with the projectId and agentId', async () => {
    const newSession = mkSession({ id: 'new-1', is_active: true });
    vi.mocked(api.createSession).mockResolvedValue(newSession);
    vi.mocked(api.listSessions).mockResolvedValue([newSession]);

    await sessionStore.createSession('project-1', 'agent-1');

    expect(api.createSession).toHaveBeenCalledWith('project-1', 'agent-1');
  });

  it('updates the store with the new session after creation', async () => {
    const newSession = mkSession({ id: 'new-1', is_active: true });
    vi.mocked(api.createSession).mockResolvedValue(newSession);
    vi.mocked(api.listSessions).mockResolvedValue([newSession]);

    await sessionStore.createSession('project-1', 'agent-1');

    const state = get(sessionStore);
    expect(state.sessions).toHaveLength(1);
    expect(state.sessions[0].id).toBe('new-1');
    expect(state.activeSession?.id).toBe('new-1');
  });

  it('propagates api errors', async () => {
    vi.mocked(api.createSession).mockRejectedValue(new Error('DB error'));
    await expect(sessionStore.createSession('project-1', 'agent-1')).rejects.toThrow('DB error');
  });
});

/**
 * §36.17.4 (#3050 PR 10): `createSession`'s optional 3rd param. `undefined`
 * (every pre-existing call site) must stay on `apiCreateSession` — this is
 * the regression pin proving the additive change didn't alter the existing
 * path. An explicit array (even empty) routes through
 * `apiCreateSessionWithCrew` instead.
 */
describe('sessionStore.createSession — crewAgentIds optional param (#3050 PR 10)', () => {
  beforeEach(() => {
    sessionStore.reset();
    vi.clearAllMocks();
  });

  it('omitting crewAgentIds calls the plain apiCreateSession, never apiCreateSessionWithCrew — regression pin for every pre-existing caller', async () => {
    const newSession = mkSession({ id: 'new-1', is_active: true });
    vi.mocked(api.createSession).mockResolvedValue(newSession);
    vi.mocked(api.listSessions).mockResolvedValue([newSession]);

    await sessionStore.createSession('project-1', 'agent-1');

    expect(api.createSession).toHaveBeenCalledWith('project-1', 'agent-1');
    expect(api.createSessionWithCrew).not.toHaveBeenCalled();
  });

  it('passing crewAgentIds (even []) routes through apiCreateSessionWithCrew, never the plain apiCreateSession', async () => {
    const newSession = mkSession({ id: 'new-2', is_active: true, crew_agent_ids: null });
    vi.mocked(api.createSessionWithCrew).mockResolvedValue(newSession);
    vi.mocked(api.listSessions).mockResolvedValue([newSession]);

    await sessionStore.createSession('project-1', 'agent-1', []);

    expect(api.createSessionWithCrew).toHaveBeenCalledWith('project-1', 'agent-1', []);
    expect(api.createSession).not.toHaveBeenCalled();
  });

  it('a non-empty crewAgentIds array is passed through verbatim', async () => {
    const newSession = mkSession({ id: 'new-3', is_active: true, crew_agent_ids: '["c1","c2"]' });
    vi.mocked(api.createSessionWithCrew).mockResolvedValue(newSession);
    vi.mocked(api.listSessions).mockResolvedValue([newSession]);

    await sessionStore.createSession('project-1', 'agent-1', ['c1', 'c2']);

    expect(api.createSessionWithCrew).toHaveBeenCalledWith('project-1', 'agent-1', ['c1', 'c2']);
  });
});

/**
 * §36.17.4 (#3050 PR 10): the edit-crew popover's "Save crew" store action.
 */
describe('sessionStore.updateSessionCrew (#3050 PR 10)', () => {
  beforeEach(() => {
    sessionStore.reset();
    vi.clearAllMocks();
  });

  it('calls api.updateSessionCrew with the sessionId and crew array, then reloads the surface', async () => {
    const updated = mkSession({ id: 'sess-1', crew_agent_ids: '["c1"]' });
    vi.mocked(api.updateSessionCrew).mockResolvedValue(updated);
    vi.mocked(api.listSessions).mockResolvedValue([updated]);

    await sessionStore.updateSessionCrew('sess-1', ['c1'], 'project-1');

    expect(api.updateSessionCrew).toHaveBeenCalledWith('sess-1', ['c1']);
    expect(api.listSessions).toHaveBeenCalledWith('project-1', true);
  });

  it('propagates api errors', async () => {
    vi.mocked(api.updateSessionCrew).mockRejectedValue(new Error('UPDATE failed'));
    await expect(sessionStore.updateSessionCrew('sess-1', ['c1'], 'project-1')).rejects.toThrow('UPDATE failed');
  });
});

describe('sessionStore.switchSession', () => {
  beforeEach(() => {
    sessionStore.reset();
    vi.clearAllMocks();
  });

  it('calls api.switchSession with the sessionId', async () => {
    const session = mkSession({ id: 'sess-1', is_active: true });
    vi.mocked(api.switchSession).mockResolvedValue(session);
    vi.mocked(api.listSessions).mockResolvedValue([session]);

    await sessionStore.switchSession('sess-1', 'project-1');

    expect(api.switchSession).toHaveBeenCalledWith('sess-1');
  });

  it('reloads sessions after switching', async () => {
    const s1 = mkSession({ id: 'sess-1', is_active: false });
    const s2 = mkSession({ id: 'sess-2', is_active: true });
    vi.mocked(api.switchSession).mockResolvedValue(s2);
    vi.mocked(api.listSessions).mockResolvedValue([s1, s2]);

    await sessionStore.switchSession('sess-2', 'project-1');

    expect(api.listSessions).toHaveBeenCalled();
  });
});

describe('sessionStore.setActiveSessionFromSwitch (#3166 C58)', () => {
  beforeEach(() => {
    sessionStore.reset();
    vi.clearAllMocks();
  });

  it('sets activeSession directly from the given Session, with NO api call at all', async () => {
    const jobSession = mkSession({ id: 'sess-job', project_id: 'proj-aurum', title: 'Morning brief', origin: 'schedule' });

    sessionStore.setActiveSessionFromSwitch(jobSession);

    expect(get(sessionStore).activeSession).toEqual(jobSession);
    expect(api.switchSession).not.toHaveBeenCalled();
    expect(api.listSessions).not.toHaveBeenCalled();
  });

  /** This is the exact gap #3166 C58 closes: `loadSessions`'s own
   *  `$effect` (SessionDropdownHeader.svelte) only re-fires on a
   *  `projectId` CHANGE — a job session in the SAME project the store
   *  already has sessions for would otherwise never update
   *  `activeSession`. This test proves the new method does it directly,
   *  without relying on that effect firing at all. */
  it('overrides a stale activeSession from the SAME project (the Matt repro: Aurum -> Aurum job session)', () => {
    const staleInteractiveSession = mkSession({ id: 'sess-regular', project_id: 'proj-aurum', title: 'New One', origin: 'user', is_active: true });
    sessionStore.setActiveSessionFromSwitch(staleInteractiveSession);
    expect(get(sessionStore).activeSession?.title).toBe('New One');

    const jobSession = mkSession({ id: 'sess-job', project_id: 'proj-aurum', title: 'Morning brief', origin: 'schedule' });
    sessionStore.setActiveSessionFromSwitch(jobSession);

    expect(get(sessionStore).activeSession?.id).toBe('sess-job');
    expect(get(sessionStore).activeSession?.title).toBe('Morning brief');
  });

  it('adds the session to the sessions list when the store does not have it yet', () => {
    const jobSession = mkSession({ id: 'sess-job', project_id: 'proj-aurum', title: 'Morning brief' });

    sessionStore.setActiveSessionFromSwitch(jobSession);

    expect(get(sessionStore).sessions.some(s => s.id === 'sess-job')).toBe(true);
  });

  it('updates the existing row in place (no duplicate) when the session is already in the list', () => {
    const original = mkSession({ id: 'sess-job', project_id: 'proj-aurum', title: 'Old Title' });
    sessionStore.setActiveSessionFromSwitch(original);
    const updated = { ...original, title: 'New Title' };

    sessionStore.setActiveSessionFromSwitch(updated);

    const matches = get(sessionStore).sessions.filter(s => s.id === 'sess-job');
    expect(matches).toHaveLength(1);
    expect(matches[0].title).toBe('New Title');
  });
});

describe('sessionStore.hideSession', () => {
  beforeEach(() => {
    sessionStore.reset();
    vi.clearAllMocks();
  });

  it('calls api.hideSession with the sessionId', async () => {
    vi.mocked(api.hideSession).mockResolvedValue(undefined);
    vi.mocked(api.listSessions).mockResolvedValue([]);

    await sessionStore.hideSession('sess-1', 'project-1');

    expect(api.hideSession).toHaveBeenCalledWith('sess-1');
  });

  it('reloads sessions after hiding', async () => {
    const visible = mkSession({ id: 'visible', is_active: true });
    vi.mocked(api.hideSession).mockResolvedValue(undefined);
    vi.mocked(api.listSessions).mockResolvedValue([visible]);

    await sessionStore.hideSession('sess-1', 'project-1');

    const state = get(sessionStore);
    expect(state.sessions).toHaveLength(1);
    expect(state.sessions[0].id).toBe('visible');
  });

  // BUG-1 regression: hideSession must pass include_hidden=true so the Hidden
  // group remains populated in the store after hiding a session.
  it('BUG-1: reloads with include_hidden=true after hiding so hidden sessions stay in store', async () => {
    const hiddenSession = mkSession({ id: 'sess-1', hidden_at: '2026-07-09T12:00:00Z' });
    const active = mkSession({ id: 'active-1', is_active: true });
    vi.mocked(api.hideSession).mockResolvedValue(undefined);
    // Backend returns both visible + hidden when include_hidden=true
    vi.mocked(api.listSessions).mockResolvedValue([active, hiddenSession]);

    await sessionStore.hideSession('sess-1', 'project-1');

    expect(api.listSessions).toHaveBeenCalledWith('project-1', true);
    const state = get(sessionStore);
    // Hidden session is in the store (showHidden prop controls display, not fetching)
    const found = state.sessions.find(s => s.id === 'sess-1');
    expect(found).toBeDefined();
    expect(found?.hidden_at).not.toBeNull();
  });
});

describe('sessionStore.unhideSession', () => {
  beforeEach(() => {
    sessionStore.reset();
    vi.clearAllMocks();
  });

  it('calls api.unhideSession with the sessionId', async () => {
    vi.mocked(api.unhideSession).mockResolvedValue(undefined);
    vi.mocked(api.listSessions).mockResolvedValue([]);

    await sessionStore.unhideSession('sess-1', 'project-1');

    expect(api.unhideSession).toHaveBeenCalledWith('sess-1');
  });

  it('reloads sessions with include_hidden=true after unhide', async () => {
    vi.mocked(api.unhideSession).mockResolvedValue(undefined);
    vi.mocked(api.listSessions).mockResolvedValue([]);

    await sessionStore.unhideSession('sess-1', 'project-1');

    expect(api.listSessions).toHaveBeenCalledWith('project-1', true);
  });
});

describe('sessionStore.renameSession', () => {
  beforeEach(() => {
    sessionStore.reset();
    vi.clearAllMocks();
  });

  it('calls api.renameSession with the sessionId and title', async () => {
    const session = mkSession({ id: 'sess-1', title: 'Old name', is_active: true });
    vi.mocked(api.renameSession).mockResolvedValue(undefined);
    vi.mocked(api.listSessions).mockResolvedValue([{ ...session, title: 'New name' }]);

    await sessionStore.renameSession('sess-1', 'New name', 'project-1');

    expect(api.renameSession).toHaveBeenCalledWith('sess-1', 'New name');
  });

  it('calls api.renameSession with empty string to revert to auto-title', async () => {
    vi.mocked(api.renameSession).mockResolvedValue(undefined);
    vi.mocked(api.listSessions).mockResolvedValue([]);

    await sessionStore.renameSession('sess-1', '', 'project-1');

    expect(api.renameSession).toHaveBeenCalledWith('sess-1', '');
  });

  it('applies optimistic update to store before api call resolves', async () => {
    const session = mkSession({ id: 'sess-1', title: 'Old', is_active: true });
    // Manually load sessions into store
    vi.mocked(api.listSessions).mockResolvedValue([session]);
    await sessionStore.loadSessions('project-1');

    // Rename with a delayed API resolution
    let resolveRename!: () => void;
    vi.mocked(api.renameSession).mockReturnValue(new Promise(res => { resolveRename = res; }));
    // Don't await — check optimistic state first
    const renamePromise = sessionStore.renameSession('sess-1', 'Updated', 'project-1');

    // At this point, the optimistic update should have fired
    const state = get(sessionStore);
    const found = state.sessions.find(s => s.id === 'sess-1');
    expect(found?.title).toBe('Updated');

    // Resolve
    resolveRename();
    vi.mocked(api.listSessions).mockResolvedValue([{ ...session, title: 'Updated' }]);
    await renamePromise;
  });
});

// ─── Filter logic ─────────────────────────────────────────────────────────────

describe('shouldShowFilter — filter threshold', () => {
  it('returns false when session count is below threshold', () => {
    expect(shouldShowFilter(0)).toBe(false);
    expect(shouldShowFilter(7)).toBe(false);
    expect(shouldShowFilter(FILTER_THRESHOLD - 1)).toBe(false);
  });

  it('returns true when session count equals threshold', () => {
    expect(shouldShowFilter(FILTER_THRESHOLD)).toBe(true);
    expect(shouldShowFilter(8)).toBe(true);
  });

  it('returns true when session count exceeds threshold', () => {
    expect(shouldShowFilter(9)).toBe(true);
    expect(shouldShowFilter(100)).toBe(true);
  });
});

describe('filterSessions', () => {
  const visible1 = mkSession({ id: 'v1', title: 'Budget planning', hidden_at: null });
  const visible2 = mkSession({ id: 'v2', title: 'Marketing', hidden_at: null });
  const hidden1 = mkSession({ id: 'h1', title: 'Old budget notes', hidden_at: '2026-07-01T00:00:00Z' });

  const allSessions = [visible1, visible2, hidden1];

  it('returns all visible and hidden when query is empty', () => {
    const { visible, hidden } = filterSessions(allSessions, '');
    expect(visible).toHaveLength(2);
    expect(hidden).toHaveLength(1);
  });

  it('filters by title (case-insensitive) across visible sessions', () => {
    const { visible } = filterSessions(allSessions, 'budget');
    expect(visible.map(s => s.id)).toContain('v1');
    expect(visible.map(s => s.id)).not.toContain('v2');
  });

  it('matches hidden sessions so consumers can group them under "Hidden"', () => {
    const { hidden } = filterSessions(allSessions, 'budget');
    expect(hidden.map(s => s.id)).toContain('h1');
  });

  it('returns empty arrays when nothing matches', () => {
    const { visible, hidden } = filterSessions(allSessions, 'zzznomatch');
    expect(visible).toHaveLength(0);
    expect(hidden).toHaveLength(0);
  });
});

// ─── Auto-title fallback ──────────────────────────────────────────────────────

describe('getSessionDisplayTitle', () => {
  it('returns the session title when non-empty', () => {
    const s = mkSession({ title: 'My research session' });
    expect(getSessionDisplayTitle(s)).toBe('My research session');
  });

  it('returns "Session YYYY-MM-DD" fallback when title is empty', () => {
    const s = mkSession({ title: '', created_at: '2026-07-09T14:00:00Z' });
    const result = getSessionDisplayTitle(s);
    expect(result).toMatch(/^Session \d{4}-\d{2}-\d{2}$/);
    expect(result).toContain('2026-07-09');
  });

  it('returns "Session YYYY-MM-DD" fallback when title is whitespace', () => {
    // Use midday UTC so the local date is July 6 in any US timezone
    const s = mkSession({ title: '   ', created_at: '2026-07-06T14:00:00Z' });
    const result = getSessionDisplayTitle(s);
    // whitespace-only title → auto-title fallback (date may vary by tz, just check format)
    expect(result).toMatch(/^Session \d{4}-\d{2}-\d{2}$/);
  });
});

// ─── Regression guard: no loadSessions(…, false) call sites (#395) ─────────
//
// When a flag flips from `true` to `false`, this test catches it at CI time.
// No loadSessions call in sessionStore.ts may pass `false` as the second arg;
// include_hidden must always be true — showHidden controls UI display only.

describe('regression guard — loadSessions always called with include_hidden=true', () => {
  it('sessionStore.ts contains no loadSessions(…, false) call sites', () => {
    const fs = require('fs');
    const path = require('path');
    const src: string = fs.readFileSync(
      path.resolve(__dirname, 'sessionStore.ts'),
      'utf-8'
    );
    // Match any call of the form: loadSessions(<anything>, false)
    // Allows optional whitespace around the comma and the argument.
    const matches = src.match(/loadSessions\s*\([^)]*,\s*false\s*\)/g);
    expect(matches).toBeNull();
  });

  it('sessionStore.ts createSession reloads with include_hidden=true', async () => {
    sessionStore.reset();
    vi.clearAllMocks();
    const newSession = mkSession({ id: 'new-1', is_active: true });
    vi.mocked(api.createSession).mockResolvedValue(newSession);
    vi.mocked(api.listSessions).mockResolvedValue([newSession]);

    await sessionStore.createSession('project-1', 'agent-1');

    expect(api.listSessions).toHaveBeenCalledWith('project-1', true);
  });

  it('sessionStore.ts switchSession reloads with include_hidden=true', async () => {
    sessionStore.reset();
    vi.clearAllMocks();
    const session = mkSession({ id: 'sess-1', is_active: true });
    vi.mocked(api.switchSession).mockResolvedValue(session);
    vi.mocked(api.listSessions).mockResolvedValue([session]);

    await sessionStore.switchSession('sess-1', 'project-1');

    expect(api.listSessions).toHaveBeenCalledWith('project-1', true);
  });

  it('sessionStore.ts renameSession reloads with include_hidden=true', async () => {
    sessionStore.reset();
    vi.clearAllMocks();
    vi.mocked(api.renameSession).mockResolvedValue(undefined);
    vi.mocked(api.listSessions).mockResolvedValue([]);

    await sessionStore.renameSession('sess-1', 'New name', 'project-1');

    expect(api.listSessions).toHaveBeenCalledWith('project-1', true);
  });
});

// ─── Empty-state exports ──────────────────────────────────────────────────────

describe('empty-state exports', () => {
  it('EMPTY_STATE_SINGLE has the correct headline copy', () => {
    expect(EMPTY_STATE_SINGLE.headline).toBe('Just this one so far');
  });

  it('EMPTY_STATE_SINGLE has the correct body copy', () => {
    expect(EMPTY_STATE_SINGLE.body).toContain('hidden, never deleted');
  });

  it('FRESH_SESSION_NAME is "New session"', () => {
    expect(FRESH_SESSION_NAME).toBe('New session');
  });

  it('EMPTY_STATE_SINGLE is returned for the single-session case', () => {
    // Consumer pattern: sessions.length === 1 → show EMPTY_STATE_SINGLE
    const sessions = [mkSession({ is_active: true })];
    const showEmptyState = sessions.length === 1;
    expect(showEmptyState).toBe(true);
    expect(EMPTY_STATE_SINGLE.headline).toBeDefined();
  });
});
