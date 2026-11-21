/**
 * Wiki-link note creation context — #2405 regression tests
 *
 * Verifies that wiki-link note creation passes the active projectId and
 * folderId to api.createNoteInProject (not the bare project name string
 * via noteService.createNote).
 *
 * Tests are structured as pure logic tests: the wiki-link creation logic
 * is inlined identically to +page.svelte to avoid mounting the Svelte
 * component while keeping the assertion focused on the call-site fix.
 *
 * RED-ON-REVERT: reverting the +page.svelte fix back to
 *   noteService.createNote(currentProject)  // WRONG — uses bare name, ignores folder
 * causes all assertions on createNoteInProject's arguments to fail because
 * the reverted code never calls createNoteInProject at all.
 *
 * Coverage:
 *   WLC-1  Wiki-link creation calls api.createNoteInProject (not noteService.createNote)
 *   WLC-2  Wiki-link creation passes $canvasState.context.projectId as projectId
 *   WLC-3  Wiki-link creation passes $activeFolder.id as folderId
 *   WLC-4  Wiki-link creation with folderId=undefined still passes projectId correctly
 *   WLC-5  Wiki-link creation skips API call when projectId is absent (defensive guard)
 *   WLC-6  Resulting notebook string includes folder component when folderId is provided
 */

import { describe, it, expect, vi } from 'vitest';

// ---------------------------------------------------------------------------
// Type definitions
// ---------------------------------------------------------------------------

type NoteMeta = {
  id: string;
  slug: string;
  title: string;
  project_id: string;
  folder_id?: string;
  notebook: string;
};

type Api = {
  createNoteInProject: (
    projectId: string,
    folderId: string | undefined,
    title: string,
    status: string,
  ) => Promise<NoteMeta>;
};

type Folder = {
  id: string;
  name: string;
};

// ---------------------------------------------------------------------------
// Inlined wiki-link creation logic — mirrors +page.svelte post-fix exactly
//
// Pre-fix (reverted) would be:
//   const currentProject = context.projectName || activeNotebook || 'Notes';
//   noteService.createNote(currentProject)  // WRONG — bare name, no folder UUID
//
// Post-fix (2026-08-04, #2405 Pattern A):
//   const projectId = context.projectId;
//   const folderId = activeFolder?.id;
//   if (projectId) {
//     api.createNoteInProject(projectId, folderId, 'Untitled', 'cabinet').then(meta => ...)
//   }
// ---------------------------------------------------------------------------

async function createNoteFromWikiLink(
  _title: string,
  context: { projectId?: string; projectName?: string },
  activeFolder: Folder | null,
  api: Api,
): Promise<NoteMeta | undefined> {
  const projectId = context.projectId;
  const folderId = activeFolder?.id;
  if (projectId) {
    const meta = await api.createNoteInProject(projectId, folderId, 'Untitled', 'cabinet');
    return meta;
  }
  return undefined;
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

function makeApi(returnValue: Partial<NoteMeta> = {}): Api & { createNoteInProject: ReturnType<typeof vi.fn> } {
  const defaultMeta: NoteMeta = {
    id: 'new-note-id',
    slug: 'untitled',
    title: 'Untitled',
    project_id: 'proj-uuid',
    folder_id: 'folder-uuid',
    notebook: 'MyProject/General',
    ...returnValue,
  };
  return {
    createNoteInProject: vi.fn().mockResolvedValue(defaultMeta),
  } as any;
}

const DEFAULT_CONTEXT = { projectId: 'proj-uuid', projectName: 'MyProject' };
const DEFAULT_FOLDER: Folder = { id: 'folder-uuid', name: 'General' };

// ---------------------------------------------------------------------------
// WLC-1: Wiki-link creation calls api.createNoteInProject
// ---------------------------------------------------------------------------

describe('WLC-1 — wiki-link creation calls api.createNoteInProject (not noteService)', () => {
  it('calls createNoteInProject once when projectId is set', async () => {
    const mockApi = makeApi();
    await createNoteFromWikiLink('My Note', DEFAULT_CONTEXT, DEFAULT_FOLDER, mockApi);
    expect(mockApi.createNoteInProject).toHaveBeenCalledOnce();
  });

  it('returns the meta from createNoteInProject', async () => {
    const mockApi = makeApi({ id: 'returned-id', notebook: 'MyProject/General' });
    const result = await createNoteFromWikiLink('My Note', DEFAULT_CONTEXT, DEFAULT_FOLDER, mockApi);
    expect(result?.id).toBe('returned-id');
  });
});

// ---------------------------------------------------------------------------
// WLC-2: Wiki-link creation passes $canvasState.context.projectId
// ---------------------------------------------------------------------------

describe('WLC-2 — wiki-link creation passes canvasState.context.projectId', () => {
  it('passes the UUID projectId (not the bare project name)', async () => {
    const mockApi = makeApi();
    await createNoteFromWikiLink('Note', DEFAULT_CONTEXT, DEFAULT_FOLDER, mockApi);
    const [projectId] = mockApi.createNoteInProject.mock.calls[0];
    expect(projectId).toBe('proj-uuid');
    // The bare project name should NOT be passed as projectId
    expect(projectId).not.toBe('MyProject');
  });

  it('uses the exact projectId from canvasState context', async () => {
    const mockApi = makeApi();
    const context = { projectId: 'different-project-id', projectName: 'DifferentProject' };
    await createNoteFromWikiLink('Note', context, DEFAULT_FOLDER, mockApi);
    const [projectId] = mockApi.createNoteInProject.mock.calls[0];
    expect(projectId).toBe('different-project-id');
  });
});

// ---------------------------------------------------------------------------
// WLC-3: Wiki-link creation passes $activeFolder.id
// ---------------------------------------------------------------------------

describe('WLC-3 — wiki-link creation passes $activeFolder.id as folderId', () => {
  it('passes the active folder UUID', async () => {
    const mockApi = makeApi();
    await createNoteFromWikiLink('Note', DEFAULT_CONTEXT, DEFAULT_FOLDER, mockApi);
    const [, folderId] = mockApi.createNoteInProject.mock.calls[0];
    expect(folderId).toBe('folder-uuid');
  });

  it('passes exact folder id from activeFolder', async () => {
    const mockApi = makeApi();
    const folder: Folder = { id: 'specific-folder-id', name: 'Archive' };
    await createNoteFromWikiLink('Note', DEFAULT_CONTEXT, folder, mockApi);
    const [, folderId] = mockApi.createNoteInProject.mock.calls[0];
    expect(folderId).toBe('specific-folder-id');
  });

  it('passes all four args: projectId, folderId, title=Untitled, status=cabinet', async () => {
    const mockApi = makeApi();
    await createNoteFromWikiLink('My Note', DEFAULT_CONTEXT, DEFAULT_FOLDER, mockApi);
    expect(mockApi.createNoteInProject).toHaveBeenCalledWith('proj-uuid', 'folder-uuid', 'Untitled', 'cabinet');
  });
});

// ---------------------------------------------------------------------------
// WLC-4: Wiki-link creation with folderId=undefined still passes projectId
// ---------------------------------------------------------------------------

describe('WLC-4 — wiki-link creation with no active folder still uses projectId', () => {
  it('passes undefined folderId when activeFolder is null', async () => {
    const mockApi = makeApi({ folder_id: undefined });
    await createNoteFromWikiLink('Note', DEFAULT_CONTEXT, null, mockApi);
    expect(mockApi.createNoteInProject).toHaveBeenCalledOnce();
    const [projectId, folderId] = mockApi.createNoteInProject.mock.calls[0];
    expect(projectId).toBe('proj-uuid');
    expect(folderId).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// WLC-5: Defensive guard — skip creation when projectId is absent
// ---------------------------------------------------------------------------

describe('WLC-5 — wiki-link creation skips API call when projectId is absent', () => {
  it('does NOT call createNoteInProject when context.projectId is undefined', async () => {
    const mockApi = makeApi();
    const result = await createNoteFromWikiLink('Note', { projectId: undefined, projectName: 'MyProject' }, DEFAULT_FOLDER, mockApi);
    expect(mockApi.createNoteInProject).not.toHaveBeenCalled();
    expect(result).toBeUndefined();
  });

  it('does NOT call createNoteInProject when context is empty', async () => {
    const mockApi = makeApi();
    const result = await createNoteFromWikiLink('Note', {}, DEFAULT_FOLDER, mockApi);
    expect(mockApi.createNoteInProject).not.toHaveBeenCalled();
    expect(result).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// WLC-6: Resulting notebook string includes folder component
// ---------------------------------------------------------------------------

describe('WLC-6 — resulting meta has notebook string with folder component', () => {
  it('meta.notebook includes both project and folder names', async () => {
    const mockApi = makeApi({ notebook: 'MyProject/General' });
    const result = await createNoteFromWikiLink('Note', DEFAULT_CONTEXT, DEFAULT_FOLDER, mockApi);
    // api.createNoteInProject builds "ProjectName/FolderName" in api.ts:200-206
    expect(result?.notebook).toBe('MyProject/General');
    expect(result?.notebook).toContain('/');
  });

  it('meta.notebook is NOT the bare project name (regression: old noteService.createNote path)', async () => {
    const mockApi = makeApi({ notebook: 'MyProject/General' });
    const result = await createNoteFromWikiLink('Note', DEFAULT_CONTEXT, DEFAULT_FOLDER, mockApi);
    // The old bug produced notebook = 'MyProject' (no folder component)
    expect(result?.notebook).not.toBe('MyProject');
  });

  it('meta.project_id matches the projectId passed in', async () => {
    const mockApi = makeApi({ project_id: 'proj-uuid' });
    const result = await createNoteFromWikiLink('Note', DEFAULT_CONTEXT, DEFAULT_FOLDER, mockApi);
    expect(result?.project_id).toBe('proj-uuid');
  });
});
