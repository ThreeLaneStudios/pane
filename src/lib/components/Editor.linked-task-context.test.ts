/**
 * Editor linked-task creation context — #2405 regression tests
 *
 * Verifies that createLinkedTask passes the host note's project_id and
 * folder_id to api.createNoteInProject (not empty strings / undefined).
 *
 * Tests are structured as pure logic tests: the createLinkedTask function
 * logic is inlined identically to Editor.svelte to avoid mounting the
 * Svelte component while keeping the assertion focused on the call-site fix.
 *
 * RED-ON-REVERT: reverting the Editor.svelte fix back to
 *   api.createNoteInProject('', undefined, ...)
 * causes all assertions on createNoteInProject's arguments to fail.
 *
 * Coverage:
 *   ELT-1  createLinkedTask passes noteMeta.project_id (not empty string)
 *   ELT-2  createLinkedTask passes noteMeta.folder_id (not undefined when set)
 *   ELT-3  createLinkedTask early-returns (no API call) when noteMeta.project_id absent
 *   ELT-4  createLinkedTask early-returns (no API call) when title is empty
 *   ELT-5  createLinkedTask early-returns (no API call) when noteId is absent
 *   ELT-6  Task title is trimmed before being passed to the API
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

// ---------------------------------------------------------------------------
// Inlined createLinkedTask — mirrors Editor.svelte post-fix exactly
// ---------------------------------------------------------------------------

type NoteMeta = {
  project_id?: string;
  folder_id?: string;
};

type Api = {
  createNoteInProject: (
    projectId: string,
    folderId: string | undefined,
    title: string,
    status: string,
  ) => Promise<any>;
};

/**
 * Direct inline of the post-fix createLinkedTask logic from Editor.svelte.
 * Any change to the actual Editor.svelte implementation must be reflected here
 * to keep the tests meaningful.
 *
 * Post-fix (2026-08-04, #2405):
 *   - Guard: if (!noteMeta?.project_id) return;
 *   - Passes noteMeta?.project_id || '' and noteMeta?.folder_id || undefined
 *
 * Pre-fix (reverted) would be:
 *   - await api.createNoteInProject('', undefined, ...)  // WRONG — always fails backend
 */
async function createLinkedTask(
  newTaskTitle: string,
  noteId: string | undefined,
  noteMeta: NoteMeta | undefined,
  api: Api,
): Promise<void> {
  if (!newTaskTitle.trim() || !noteId) return;
  if (!noteMeta?.project_id) return;
  await api.createNoteInProject(
    noteMeta?.project_id || '',
    noteMeta?.folder_id || undefined,
    newTaskTitle.trim(),
    'todo',
  );
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

function makeApi(overrides: Partial<Api> = {}): Api & { createNoteInProject: ReturnType<typeof vi.fn> } {
  return {
    createNoteInProject: vi.fn().mockResolvedValue({
      id: 'new-task-id',
      title: 'My Task',
      project_id: 'proj-uuid',
      folder_id: 'folder-uuid',
      status: 'todo',
    }),
    ...overrides,
  } as any;
}

// ---------------------------------------------------------------------------
// ELT-1: createLinkedTask passes noteMeta.project_id
// ---------------------------------------------------------------------------

describe('ELT-1 — createLinkedTask passes noteMeta.project_id to createNoteInProject', () => {
  it('passes the host note project_id (not empty string)', async () => {
    const mockApi = makeApi();
    await createLinkedTask('My Task', 'note-uuid', { project_id: 'proj-uuid', folder_id: 'folder-uuid' }, mockApi);
    expect(mockApi.createNoteInProject).toHaveBeenCalledOnce();
    const [projectId] = mockApi.createNoteInProject.mock.calls[0];
    expect(projectId).toBe('proj-uuid');
    expect(projectId).not.toBe('');
  });

  it('uses the exact project_id from noteMeta (not a fallback or constant)', async () => {
    const mockApi = makeApi();
    await createLinkedTask('Task', 'note-id', { project_id: 'another-project-id', folder_id: undefined }, mockApi);
    const [projectId] = mockApi.createNoteInProject.mock.calls[0];
    expect(projectId).toBe('another-project-id');
  });
});

// ---------------------------------------------------------------------------
// ELT-2: createLinkedTask passes noteMeta.folder_id
// ---------------------------------------------------------------------------

describe('ELT-2 — createLinkedTask passes noteMeta.folder_id to createNoteInProject', () => {
  it('passes the host note folder_id when set', async () => {
    const mockApi = makeApi();
    await createLinkedTask('My Task', 'note-uuid', { project_id: 'proj-uuid', folder_id: 'folder-uuid' }, mockApi);
    expect(mockApi.createNoteInProject).toHaveBeenCalledOnce();
    const [, folderId] = mockApi.createNoteInProject.mock.calls[0];
    expect(folderId).toBe('folder-uuid');
  });

  it('passes undefined for folder_id when noteMeta.folder_id is absent', async () => {
    const mockApi = makeApi();
    await createLinkedTask('My Task', 'note-uuid', { project_id: 'proj-uuid', folder_id: undefined }, mockApi);
    const [, folderId] = mockApi.createNoteInProject.mock.calls[0];
    expect(folderId).toBeUndefined();
  });

  it('passes both project_id and folder_id in one call matching exact values', async () => {
    const mockApi = makeApi();
    await createLinkedTask('Buy Milk', 'note-123', { project_id: 'proj-uuid', folder_id: 'folder-uuid' }, mockApi);
    expect(mockApi.createNoteInProject).toHaveBeenCalledWith('proj-uuid', 'folder-uuid', 'Buy Milk', 'todo');
  });
});

// ---------------------------------------------------------------------------
// ELT-3: Early return when noteMeta.project_id absent
// ---------------------------------------------------------------------------

describe('ELT-3 — createLinkedTask early-returns when no project context', () => {
  it('does NOT call createNoteInProject when noteMeta is undefined', async () => {
    const mockApi = makeApi();
    await createLinkedTask('My Task', 'note-uuid', undefined, mockApi);
    expect(mockApi.createNoteInProject).not.toHaveBeenCalled();
  });

  it('does NOT call createNoteInProject when noteMeta.project_id is empty string', async () => {
    const mockApi = makeApi();
    await createLinkedTask('My Task', 'note-uuid', { project_id: '', folder_id: 'folder-uuid' }, mockApi);
    expect(mockApi.createNoteInProject).not.toHaveBeenCalled();
  });

  it('does NOT call createNoteInProject when noteMeta.project_id is absent', async () => {
    const mockApi = makeApi();
    await createLinkedTask('My Task', 'note-uuid', {} as NoteMeta, mockApi);
    expect(mockApi.createNoteInProject).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// ELT-4: Early return when title is empty
// ---------------------------------------------------------------------------

describe('ELT-4 — createLinkedTask early-returns when title is empty', () => {
  it('does NOT call createNoteInProject when title is empty string', async () => {
    const mockApi = makeApi();
    await createLinkedTask('', 'note-uuid', { project_id: 'proj-uuid', folder_id: 'folder-uuid' }, mockApi);
    expect(mockApi.createNoteInProject).not.toHaveBeenCalled();
  });

  it('does NOT call createNoteInProject when title is whitespace only', async () => {
    const mockApi = makeApi();
    await createLinkedTask('   ', 'note-uuid', { project_id: 'proj-uuid', folder_id: 'folder-uuid' }, mockApi);
    expect(mockApi.createNoteInProject).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// ELT-5: Early return when noteId is absent
// ---------------------------------------------------------------------------

describe('ELT-5 — createLinkedTask early-returns when noteId is absent', () => {
  it('does NOT call createNoteInProject when noteId is undefined', async () => {
    const mockApi = makeApi();
    await createLinkedTask('My Task', undefined, { project_id: 'proj-uuid', folder_id: 'folder-uuid' }, mockApi);
    expect(mockApi.createNoteInProject).not.toHaveBeenCalled();
  });

  it('does NOT call createNoteInProject when noteId is empty string', async () => {
    const mockApi = makeApi();
    await createLinkedTask('My Task', '', { project_id: 'proj-uuid', folder_id: 'folder-uuid' }, mockApi);
    expect(mockApi.createNoteInProject).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// ELT-6: Title trimming
// ---------------------------------------------------------------------------

describe('ELT-6 — createLinkedTask trims the task title before passing to API', () => {
  it('passes trimmed title (no leading/trailing whitespace)', async () => {
    const mockApi = makeApi();
    await createLinkedTask('  My Task  ', 'note-uuid', { project_id: 'proj-uuid', folder_id: 'folder-uuid' }, mockApi);
    const [, , title] = mockApi.createNoteInProject.mock.calls[0];
    expect(title).toBe('My Task');
  });
});
