/**
 * Tests for #2407 — Notebook String Integrity
 *
 * Verifies:
 *   1. readNote derives notebook from authoritative project_id + folder_id in meta,
 *      ignoring the passed-in bare notebook string.
 *   2. listNotesByProject emits a console.warn when a folder_id is not found in
 *      the projectMap (folder lookup miss instrumentation).
 *
 * Red-on-revert: reverting the derivation logic in readNote flips Test 1 red.
 * Reverting the warn in listNotesByProject flips Test 2 red.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { readNote, listNotesByProject } from '$lib/api';
import { mockData } from '../../vitest-setup';

// ─── helpers ──────────────────────────────────────────────────────────────────

function makeProject(id: string, name: string, parentId: string | null = null) {
  return {
    id,
    name,
    parent_id: parentId,
    description: null,
    color: null,
    sort_order: 0,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
  };
}

function makeNote(id: string, projectId: string, folderId: string | null = null, title = 'Test Note') {
  return {
    id,
    title,
    content: '',
    status: 'cabinet',
    priority: 'medium',
    due_date: null,
    assignee: null,
    project_id: projectId,
    folder_id: folderId,
    parent_id: null,
    content_preview: '',
    word_count: 0,
    tags: [],
    note_type: null,
    deleted_at: null,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
  };
}

// ─── Test 1: readNote derivation ───────────────────────────────────────────────

describe('readNote — notebook derivation from authoritative meta (#2407)', () => {
  const PROJECT_ID = 'proj-daily-tasks';
  const FOLDER_ID = 'folder-general';
  const NOTE_ID = 'note-2407-test-1';

  beforeEach(() => {
    // Reset mock state
    mockData.tasks = [];
    mockData.projects = [
      makeProject('project-inbox', 'Inbox'),
      makeProject(PROJECT_ID, 'Daiily Tasks'),
      makeProject(FOLDER_ID, 'General', PROJECT_ID),
    ];

    const note = makeNote(NOTE_ID, PROJECT_ID, FOLDER_ID, 'My Note');
    mockData.tasks = [note];
    mockData.nextId = 100;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('ignores the bare passed-in notebook string and derives "Project/Folder" from meta', async () => {
    // Simulate selectNote passing a bare "Daiily Tasks" string (the degraded scenario)
    const result = await readNote('Daiily Tasks', NOTE_ID);

    // The returned notebook must be the full path, NOT the bare input
    expect(result.meta.notebook).toBe('Daiily Tasks/General');
  });

  it('does NOT return the bare passed-in notebook when derivation succeeds', async () => {
    const result = await readNote('Daiily Tasks', NOTE_ID);

    expect(result.meta.notebook).not.toBe('Daiily Tasks');
  });

  it('falls back to passed-in notebook when project_id is missing from meta', async () => {
    // Note with no project_id — derivation impossible
    const bareNote = makeNote('note-no-project', '', null, 'Orphan Note');
    bareNote.project_id = '';  // empty — map lookup will miss
    mockData.tasks = [bareNote];

    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const result = await readNote('FallbackProject', 'note-no-project');

    expect(result.meta.notebook).toBe('FallbackProject');
    expect(warnSpy).toHaveBeenCalledWith(
      expect.stringContaining('[api.readNote]'),
      expect.objectContaining({ noteId: 'note-no-project', passedNotebook: 'FallbackProject' })
    );
  });

  it('returns bare project name (no folder) when note has no folder_id', async () => {
    const noFolderNote = makeNote('note-no-folder', PROJECT_ID, null, 'Root Note');
    mockData.tasks = [noFolderNote];

    const result = await readNote('Daiily Tasks', 'note-no-folder');
    expect(result.meta.notebook).toBe('Daiily Tasks');
  });
});

// ─── Test 2: builder warning on folder lookup miss ─────────────────────────────

describe('listNotesByProject — folder lookup miss emits console.warn (#2407)', () => {
  const PROJECT_ID = 'proj-warn-test';
  const MISSING_FOLDER_ID = 'folder-does-not-exist';
  const NOTE_ID = 'note-2407-test-2';

  beforeEach(() => {
    mockData.tasks = [];
    mockData.projects = [
      makeProject(PROJECT_ID, 'Warning Test Project'),
      // MISSING_FOLDER_ID is intentionally NOT in projects
    ];
    mockData.tasks = [makeNote(NOTE_ID, PROJECT_ID, MISSING_FOLDER_ID, 'Orphan Note')];
    mockData.nextId = 200;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('emits a console.warn naming the function and the missing folder_id', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

    await listNotesByProject(PROJECT_ID);

    expect(warnSpy).toHaveBeenCalled();
    const [message, details] = warnSpy.mock.calls[0];
    expect(message).toContain('[api.listNotesByProject]');
    expect(details).toMatchObject({
      folderId: MISSING_FOLDER_ID,
      noteId: NOTE_ID,
    });
    expect(typeof details.projectMapSize).toBe('number');
  });

  it('still returns the note with bare project name on folder lookup miss', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});

    const results = await listNotesByProject(PROJECT_ID);

    expect(results).toHaveLength(1);
    expect(results[0].notebook).toBe('Warning Test Project');
  });

  it('does NOT warn when folder_id is present and found in projectMap', async () => {
    const GOOD_FOLDER_ID = 'folder-exists';
    mockData.projects.push(makeProject(GOOD_FOLDER_ID, 'Real Folder', PROJECT_ID));
    const goodNote = makeNote('note-good', PROJECT_ID, GOOD_FOLDER_ID, 'Good Note');
    mockData.tasks = [goodNote];

    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

    const results = await listNotesByProject(PROJECT_ID);

    // No warning — folder was found
    const folderMissWarnings = warnSpy.mock.calls.filter(([msg]) =>
      typeof msg === 'string' && msg.includes('[api.listNotesByProject]')
    );
    expect(folderMissWarnings).toHaveLength(0);
    expect(results[0].notebook).toBe('Warning Test Project/Real Folder');
  });
});
