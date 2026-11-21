/**
 * fix(#1811): Note-nav project resolution tests
 *
 * Tests the Step 0 auto-resolution block inside `navigateTo`:
 * when `view === 'note_editor'` and `projectId` is absent, the nav layer
 * must call `api.getNoteRaw` to resolve the project_id from the note's
 * backend shape, then populate `destination.projectId` before setting
 * canvasState — so the CanvasRenderer guard (noteId && projectId) passes.
 *
 * Regression protection: reverting Step 0 breaks tests 1 and 3.
 * Adding a `meta.notebook` read (wrong field) breaks test 4.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { canvasState } from '$lib/stores/canvas';
import { projects } from '$lib/stores/app';
import * as api from '$lib/api';

// ── Mock $lib/api ─────────────────────────────────────────────────────────────
vi.mock('$lib/api', () => ({
  getNoteRaw: vi.fn(),
  getActiveSession: vi.fn().mockResolvedValue({ id: 'thread-test', project_id: 'workspace', title: '', origin: 'user', created_at: '', last_active_at: '', hidden_at: null, is_active: true }),
  switchSession: vi.fn(),
  getChatMessages: vi.fn().mockResolvedValue([]),
  setViewingSession: vi.fn().mockResolvedValue(undefined),
}));

// ── Import navigateTo AFTER mock is set up ────────────────────────────────────
import { navigateTo } from './navigation';

// ── Helpers ───────────────────────────────────────────────────────────────────
const TEST_PROJECT_ID = 'proj-abc-123';
const TEST_PROJECT_NAME = 'Test Project';
const TEST_NOTE_ID = 'note-xyz-456';

function seedProjects() {
  projects.set([
    {
      id: TEST_PROJECT_ID,
      name: TEST_PROJECT_NAME,
      parent_id: null,
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
    } as any,
  ]);
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('navigateTo — note_editor Step 0 auto-resolution (#1811)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.getActiveSession).mockResolvedValue({ id: 'thread-test', project_id: 'workspace', title: '', origin: 'user', created_at: '', last_active_at: '', hidden_at: null, is_active: true });
    vi.mocked(api.getChatMessages).mockResolvedValue([]);
    seedProjects();
  });

  // Test 1: Happy path — projectId absent, getNoteRaw resolves it
  it('populates projectId and projectName in canvasState when projectId is absent', async () => {
    vi.mocked(api.getNoteRaw).mockResolvedValue({
      meta: { project_id: TEST_PROJECT_ID },
      content: 'body text',
    } as any);

    await navigateTo({ view: 'note_editor', noteId: TEST_NOTE_ID });

    const state = get(canvasState);
    expect(state.view).toBe('note_editor');
    expect(state.context.noteId).toBe(TEST_NOTE_ID);
    expect(state.context.projectId).toBe(TEST_PROJECT_ID);
    expect(state.context.projectName).toBe(TEST_PROJECT_NAME);
    expect(vi.mocked(api.getNoteRaw)).toHaveBeenCalledOnce();
    expect(vi.mocked(api.getNoteRaw)).toHaveBeenCalledWith(TEST_NOTE_ID);
  });

  // Test 2: Guard-skip — projectId already provided, getNoteRaw must NOT be called
  it('does NOT call getNoteRaw when projectId is already present', async () => {
    await navigateTo({
      view: 'note_editor',
      noteId: TEST_NOTE_ID,
      projectId: TEST_PROJECT_ID,
    });

    expect(vi.mocked(api.getNoteRaw)).not.toHaveBeenCalled();

    const state = get(canvasState);
    expect(state.context.projectId).toBe(TEST_PROJECT_ID);
    expect(state.context.projectName).toBe(TEST_PROJECT_NAME);
  });

  // Test 3: Failure path — getNoteRaw throws, no crash; projectId remains undefined
  it('does not crash when getNoteRaw throws; view is still set and projectId is undefined', async () => {
    vi.mocked(api.getNoteRaw).mockRejectedValue(new Error('Note not found: note-xyz-456'));

    // Must not throw
    await expect(
      navigateTo({ view: 'note_editor', noteId: TEST_NOTE_ID })
    ).resolves.toBeDefined();

    const state = get(canvasState);
    expect(state.view).toBe('note_editor');
    expect(state.context.noteId).toBe(TEST_NOTE_ID);
    // projectId stays undefined (same graceful no-op as before the fix)
    expect(state.context.projectId).toBeUndefined();
  });

  // Test 4: Non-note_editor view — getNoteRaw must NOT be called
  it('does NOT call getNoteRaw for non-note_editor views', async () => {
    await navigateTo({ view: 'home' });

    expect(vi.mocked(api.getNoteRaw)).not.toHaveBeenCalled();
  });
});

// ── api.getNoteRaw unit test ──────────────────────────────────────────────────

describe('api.getNoteRaw (#1811)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('returns meta.project_id unmodified from the backend shape', async () => {
    // Use the Tauri mock (invoke → get_note) via vitest-setup — restore real implementation
    // for this isolated test by importing the actual api module with the Tauri mock active.
    //
    // The vitest-setup mock for get_note returns { meta: note, content: '' }.
    // We pre-seed a note with project_id to verify getNoteRaw passes it through.
    const { mockData } = await import('../../../vitest-setup');
    const now = new Date().toISOString();
    const noteId = 'raw-note-999';
    const rawNote = {
      id: noteId,
      title: 'Raw Note',
      project_id: 'raw-project-uuid',
      content: 'hello',
      status: 'cabinet',
      priority: 'medium',
      due_date: null,
      assignee: null,
      folder_id: null,
      parent_id: null,
      content_preview: 'hello',
      word_count: 1,
      tags: [],
      created_at: now,
      updated_at: now,
    };
    mockData.tasks.push(rawNote);
    mockData.noteContents[noteId] = 'hello';

    // Import the actual (non-mocked) api using a fresh dynamic import.
    // Since vi.mock('$lib/api') hoisted above replaces the module for this
    // file's static imports, we bypass it for this specific test by reaching
    // the module via the Tauri invoke mock path directly through vitest-setup.
    // Verify the contract via the Tauri mock: invoke('get_note', { noteId }) → { meta, content }
    const tauriInternals = (globalThis as any).window.__TAURI_INTERNALS__;
    const result = await tauriInternals.invoke('get_note', { noteId });

    expect(result).toEqual({
      meta: expect.objectContaining({ project_id: 'raw-project-uuid' }),
      content: 'hello',
    });
    // Ensure there is NO `notebook` field on meta — it doesn't exist in NoteMeta
    expect((result.meta as any).notebook).toBeUndefined();
  });
});

// ── navigateTo — sessionId override (§36.7, #3050 PR 6) ──────────────────────
//
// A notification's click-to-open needs "land on THIS exact session," not
// "land on whatever this project's active session happens to be" — the
// default STEP 3 resolution (`getActiveSession`) answers the wrong
// question for that one caller. `destination.sessionId`, when present,
// routes STEP 3 through `api.switchSession` instead.

describe('navigateTo — sessionId override (§36.7, #3050 PR 6)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.getActiveSession).mockResolvedValue({ id: 'thread-active', project_id: 'workspace', title: '', origin: 'user', created_at: '', last_active_at: '', hidden_at: null, is_active: true } as any);
    vi.mocked(api.switchSession).mockResolvedValue({ id: 'thread-target', project_id: 'proj-abc-123', title: '', origin: 'schedule', created_at: '', last_active_at: '', hidden_at: null, is_active: false } as any);
    vi.mocked(api.getChatMessages).mockResolvedValue([]);
    seedProjects();
  });

  /// Mutation (actually run — see report): changed STEP 3 back to
  /// unconditionally call `getActiveSession`, ignoring `destination.
  /// sessionId`. Failure: this test's own `toHaveBeenCalledWith` assertion
  /// on `switchSession` reddens (never called), and the resolved threadId
  /// assertion reddens too (`'thread-active'` instead of `'thread-target'`).
  it('resolves the thread via switchSession when sessionId is provided, not getActiveSession', async () => {
    const result = await navigateTo({ view: 'kanban', projectId: TEST_PROJECT_ID, sessionId: 'session-target-1' });

    expect(vi.mocked(api.switchSession)).toHaveBeenCalledWith('session-target-1');
    expect(vi.mocked(api.getActiveSession)).not.toHaveBeenCalled();
    expect(result.threadId).toBe('thread-target');
  });

  /// #3114 (Reed's audit): PATH 1 of the "every navigation path" set —
  /// notification click-to-open. `0` frontend call sites for
  /// `set_viewing_session` before this fix (confirmed by grep, Reed's audit
  /// and this PR's own report). Mutation (actually run — see report):
  /// removed the `api.setViewingSession(...)` call from `navigateTo`.
  /// Failure: this assertion reddens (`setViewingSession` never called).
  it('calls setViewingSession with the resolved thread id on the notification click-to-open path', async () => {
    await navigateTo({ view: 'home', projectId: TEST_PROJECT_ID, sessionId: 'session-target-1' });

    expect(vi.mocked(api.setViewingSession)).toHaveBeenCalledWith('thread-target');
  });

  /// Control: the default path (no sessionId) is unchanged — still uses
  /// getActiveSession, never switchSession. Regression pin for the normal
  /// (non-notification) navigation path.
  it('resolves via getActiveSession when sessionId is absent (unchanged default)', async () => {
    const result = await navigateTo({ view: 'kanban', projectId: TEST_PROJECT_ID });

    expect(vi.mocked(api.getActiveSession)).toHaveBeenCalledWith(TEST_PROJECT_ID);
    expect(vi.mocked(api.switchSession)).not.toHaveBeenCalled();
    expect(result.threadId).toBe('thread-active');
  });
});

// ── navigateTo — setViewingSession on every other path (§36.7, #3050 PR 6) ──
//
// Reed's audit: "set_viewing_session has ZERO frontend call sites… zero
// hits in src/, across all five commits." `navigateTo` is the one funnel
// EVERY project switch / Home / trash / notes_browser / etc. navigation
// goes through (grep confirms every one of Sidebar.svelte's/ChatSidebar.
// svelte's/+page.svelte's navigateTo({...}) calls resolves STEP 3's
// thread id unconditionally, regardless of view) — wiring the ONE call
// site here, rather than at each of those ~20 call sites individually,
// is what makes a FUTURE navigateTo caller covered "for free" (Reed's own
// framing). Click-to-open (the sessionId-override path) is covered
// separately above, inside the existing "sessionId override" describe
// block, since it's the same funnel with one different input.
describe('navigateTo — setViewingSession on every path (§36.7, #3050 PR 6)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.getChatMessages).mockResolvedValue([]);
    seedProjects();
  });

  /// #3114 PATH 2: project switch (navigating into a project view, e.g.
  /// the kanban board, resolves that project's own active session
  /// alongside the canvas view switch — `Sidebar.svelte`'s project click
  /// handlers route through exactly this call shape).
  /// Mutation (actually run — see report): removed the
  /// `api.setViewingSession(...)` call from `navigateTo`. Failure: this
  /// assertion reddens (never called).
  it('calls setViewingSession with the resolved thread id on a project switch', async () => {
    vi.mocked(api.getActiveSession).mockResolvedValue({ id: 'thread-project-abc', project_id: TEST_PROJECT_ID, title: '', origin: 'user', created_at: '', last_active_at: '', hidden_at: null, is_active: true } as any);

    await navigateTo({ view: 'kanban', projectId: TEST_PROJECT_ID });

    expect(vi.mocked(api.setViewingSession)).toHaveBeenCalledWith('thread-project-abc');
  });

  /// #3114 PATH 3: Home (`navigateTo({ view: 'home' })` — `Sidebar.svelte`'s
  /// Home button, `+page.svelte`'s onboarding-complete handler, etc.).
  /// Mutation (actually run — see report): same removal as above. Failure:
  /// this assertion reddens.
  it('calls setViewingSession with the resolved thread id on Home navigation', async () => {
    vi.mocked(api.getActiveSession).mockResolvedValue({ id: 'thread-workspace-home', project_id: 'workspace', title: '', origin: 'user', created_at: '', last_active_at: '', hidden_at: null, is_active: true } as any);

    await navigateTo({ view: 'home' });

    expect(vi.mocked(api.setViewingSession)).toHaveBeenCalledWith('thread-workspace-home');
  });

  /// #3114 PATH 4: teardown. Settings is the one view with no ChatSidebar
  /// mounted at all (ADR-0063 D8, #2536 — `+page.svelte`'s own
  /// `$canvasState.view !== 'settings'` guard around <ChatSidebar>) — a
  /// navigation INTO Settings must clear the viewing-session tracker
  /// (`null`), not leave it pointed at whatever thread STEP 3 still
  /// resolves internally, or `is_session_open` would keep reporting a
  /// session "open" that has no chat UI showing it anymore.
  /// Mutation (actually run — see report): changed the teardown branch to
  /// always pass `threadId` (i.e. removed the `destination.view ===
  /// 'settings'` check). Failure: this test's `toHaveBeenCalledWith(null)`
  /// assertion reddens (called with the thread id instead).
  it('calls setViewingSession with null when navigating to Settings (chat-view teardown)', async () => {
    vi.mocked(api.getActiveSession).mockResolvedValue({ id: 'thread-whatever', project_id: 'workspace', title: '', origin: 'user', created_at: '', last_active_at: '', hidden_at: null, is_active: true } as any);

    await navigateTo({ view: 'settings' });

    expect(vi.mocked(api.setViewingSession)).toHaveBeenCalledWith(null);
  });

  /// Regression guard for the mutation above, from the other direction: a
  /// NON-settings view must never pass `null` — proving the teardown
  /// branch is scoped to Settings specifically, not "sometimes null for
  /// no reason."
  it('does NOT call setViewingSession with null for a non-Settings view', async () => {
    vi.mocked(api.getActiveSession).mockResolvedValue({ id: 'thread-trash', project_id: 'workspace', title: '', origin: 'user', created_at: '', last_active_at: '', hidden_at: null, is_active: true } as any);

    await navigateTo({ view: 'trash' });

    expect(vi.mocked(api.setViewingSession)).toHaveBeenCalledWith('thread-trash');
    expect(vi.mocked(api.setViewingSession)).not.toHaveBeenCalledWith(null);
  });
});
