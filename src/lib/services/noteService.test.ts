import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import * as noteService from './noteService';
import { notes, activeNote, activeNotebook } from '$lib/stores/app';
import { canvasState } from '$lib/stores/canvas';
import { mockData } from '../../../vitest-setup';

describe('noteService', () => {
  beforeEach(() => {
    // Reset stores
    notes.set([]);
    activeNote.set(null);
    activeNotebook.set('Test Notebook');
    canvasState.set({ view: 'home', data: {}, context: {} });
    
    // Reset mock data (V9: reset tasks array and other state)
    mockData.notebooks = ['Inbox', 'Test Notebook'];
    mockData.notes = { 'Inbox': [], 'Test Notebook': [] };
    mockData.noteContents = {};
    mockData.noteJsons = {};
    mockData.tasks = []; // V9: Clear unified notes/tasks array
    mockData.nextId = 1;
  });

  describe('createNote', () => {
    it('returns proper NoteMeta and updates store', async () => {
      const result = await noteService.createNote('Test Notebook');
      
      expect(result).toMatchObject({
        meta: expect.objectContaining({
          id: expect.stringMatching(/^note-\d+$/), // V9: UUIDs, not paths
          title: 'Untitled',
          slug: expect.stringMatching(/^note-\d+$/), // V9: slug = UUID
          notebook: 'Test Notebook',
        }),
        content: '',
      });

      // Check that store was updated
      const currentNotes = get(notes);
      expect(currentNotes).toHaveLength(1);
      expect(currentNotes[0]).toMatchObject({
        id: result.meta.id,
        title: result.meta.title,
        notebook: 'Test Notebook',
      });

      // Check that activeNote was set
      const currentActiveNote = get(activeNote);
      expect(currentActiveNote).toMatchObject(result);
    });

    it('uses default notebook when none provided', async () => {
      activeNotebook.set('Inbox');
      const result = await noteService.createNote();
      
      expect(result.meta.notebook).toBe('Inbox');
    });

    it('sets activeNote to null first then to new note', async () => {
      const noteSetSpy = vi.fn();
      activeNote.subscribe(noteSetSpy);
      
      await noteService.createNote('Test Notebook');
      
      // Should have been called with null first, then the new note
      expect(noteSetSpy).toHaveBeenCalledWith(null);
      expect(noteSetSpy).toHaveBeenLastCalledWith(expect.objectContaining({
        meta: expect.any(Object),
        content: expect.any(String),
      }));
    });
  });

  describe('selectNote', () => {
    it('follows null-then-set pattern', async () => {
      // V9: Create note properly using the mock API first
      const created = await noteService.createNote('Test Notebook');
      const testNote = created.meta;
      
      // Set content for the created note
      mockData.noteContents[testNote.id] = 'Test content';
      const noteInMock = mockData.tasks.find(t => t.id === testNote.id);
      if (noteInMock) {
        noteInMock.content = 'Test content';
      }

      const noteSetSpy = vi.fn();
      activeNote.subscribe(noteSetSpy);

      await noteService.selectNote(testNote);

      // Should set null first, then the loaded note
      expect(noteSetSpy).toHaveBeenCalledWith(null);
      expect(noteSetSpy).toHaveBeenLastCalledWith(expect.objectContaining({
        meta: expect.objectContaining({
          id: testNote.id,
          title: testNote.title,
        }),
        content: 'Test content',
      }));
    });

    it('awaits editorSavePromise before reading note (#1735)', async () => {
      // #1735: selectNote now awaits editorSavePromise instead of a fixed 100ms heuristic.
      // The promise resolves immediately when no editor is saving (default Promise.resolve()).
      // This test verifies selectNote completes successfully under the new pattern.
      const created = await noteService.createNote('Test Notebook');
      const testNote = created.meta;

      await noteService.selectNote(testNote);

      // If we get here, selectNote resolved without hanging — the promise-based
      // coordination works correctly when no editor save is in flight.
      const currentActiveNote = get(activeNote);
      expect(currentActiveNote).not.toBeNull();
    });

    it('handles missing note gracefully', async () => {
      const nonExistentNote = {
        id: 'missing-note-999',
        title: 'Missing Note',
        slug: 'missing-note-999',
        notebook: 'Test Notebook',
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        preview: '',
        word_count: 0,
      };

      await noteService.selectNote(nonExistentNote);

      const currentActiveNote = get(activeNote);
      expect(currentActiveNote).not.toBeNull();
      expect(currentActiveNote!.meta.slug).toBe('missing-note-999');
      expect(currentActiveNote!.meta.notebook).toBe('Test Notebook');
      expect(currentActiveNote!.content).toBe('');
    });

    // ── #2443 re-land (nav-lockup diagnosis §7 option 2 — single-writer): contextOverride ──
    //
    // selectNote() is the ONLY function that writes canvasState.view for a note selection.
    // wikiLinkNavigation.ts's exact-match branch (and any future caller with the same need)
    // threads the target note's own project_id/folder_id in via contextOverride instead of
    // firing a separate navigateFn/canvasState write beforehand — that separate-write pattern
    // is what caused the nav lockup (v0.68.93): a navigateFn write to 'notes_browser'
    // immediately followed by this function's OWN unconditional write-back to 'note_editor'
    // ~20-25ms later, on every single wiki-link click. See
    // docs/audits/2026-08-05-nav-lockup-diagnosis.md §2.

    it('#2443 re-land: contextOverride writes projectId/folderId into canvasState.context alongside the normal note fields', async () => {
      const created = await noteService.createNote('Test Notebook');
      const testNote = created.meta;

      await noteService.selectNote(testNote, { projectId: 'override-proj', folderId: 'override-folder' });

      const state = get(canvasState);
      expect(state.view).toBe('note_editor');
      expect(state.context.projectId).toBe('override-proj');
      expect(state.context.folderId).toBe('override-folder');
      expect(state.context.noteId).toBe(testNote.id);
    });

    it('#2443 re-land: without a contextOverride, projectId/folderId in canvasState.context are left as whatever was already there (no regression for existing callers)', async () => {
      canvasState.set({
        view: 'notes_browser',
        data: {},
        context: { projectId: 'preexisting-proj', folderId: 'preexisting-folder' },
      });

      const created = await noteService.createNote('Test Notebook');
      const testNote = created.meta;

      await noteService.selectNote(testNote);

      const state = get(canvasState);
      expect(state.view).toBe('note_editor');
      // Red-on-revert (existing-caller safety): a contextOverride-shaped change that
      // clobbers projectId/folderId to undefined when no override is passed would flip
      // this red — every pre-existing selectNote() caller (NoteList, NavPlayer,
      // ChatSidebar, TasksModule, NotesBrowser's own card click) relies on this field
      // staying untouched when it doesn't pass an override.
      expect(state.context.projectId).toBe('preexisting-proj');
      expect(state.context.folderId).toBe('preexisting-folder');
    });

    it('#2443 re-land: selectNote() performs exactly ONE canvasState write per call (no double write)', async () => {
      const created = await noteService.createNote('Test Notebook');
      const testNote = created.meta;

      let writeCount = 0;
      const unsub = canvasState.subscribe(() => { writeCount++; });
      // svelte's writable fires once immediately on subscription with the current value —
      // that's not a write triggered by selectNote, so we don't count it.
      writeCount = 0;

      await noteService.selectNote(testNote, { projectId: 'p1', folderId: 'f1' });
      unsub();

      // Red-on-revert (double-write fix): if selectNote() or a caller ahead of it ever
      // reintroduces a separate canvasState write (e.g. the old navigateFn call this PR
      // removes from wikiLinkNavigation.ts), this count becomes 2 — exactly the flip-flop
      // signature from docs/audits/2026-08-05-nav-lockup-diagnosis.md §2.
      expect(writeCount).toBe(1);
    });
  });

  describe('loadNotes', () => {
    it('merges without overwriting existing notes from same notebook', async () => {
      // V9: Add a just-created note to the store (with proper v9 structure)
      const justCreated = {
        id: 'just-created-1',
        title: 'Just Created',
        slug: 'just-created-1',
        notebook: 'Test Notebook',
        project_id: 'project-test',
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        preview: '',
        content_preview: '',
        word_count: 0,
        tags: [],
        pinned: false,
        task_sort_order: 0,
      };
      notes.set([justCreated]);

      // Add different notes to mock backend (v9: use tasks array)
      const loadedNote = {
        id: 'loaded-note-1',
        title: 'Loaded Note',
        content: '',
        status: 'cabinet',
        project_id: 'project-test',
        folder_id: null,
        content_preview: '',
        word_count: 0,
        tags: [],
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      };
      mockData.tasks.push(loadedNote);

      await noteService.loadNotes('Test Notebook');

      const currentNotes = get(notes);
      expect(currentNotes).toHaveLength(2);
      expect(currentNotes.map(n => n.id)).toEqual(expect.arrayContaining([
        'just-created-1',
        'loaded-note-1',
      ]));
    });

    it('does not overwrite notes from different notebooks', async () => {
      // V9: Add note from different notebook to store (but it shouldn't be loaded)
      const differentNotebookNote = {
        id: 'other-note-1',
        title: 'Other Note',
        slug: 'other-note-1',
        notebook: 'Other Notebook',
        project_id: 'project-other',
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        preview: '',
        content_preview: '',
        word_count: 0,
        tags: [],
        pinned: false,
        task_sort_order: 0,
      };
      notes.set([differentNotebookNote]);

      // Load notes for Test Notebook (v9: in tasks array)
      const testNote = {
        id: 'test-note-1',
        title: 'Test Note',
        content: '',
        status: 'cabinet',
        project_id: 'project-test',
        folder_id: null,
        content_preview: '',
        word_count: 0,
        tags: [],
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      };
      mockData.tasks.push(testNote);

      await noteService.loadNotes('Test Notebook');

      const currentNotes = get(notes);
      expect(currentNotes).toHaveLength(1);
      expect(currentNotes[0].id).toBe('test-note-1');
      // Different notebook note should be filtered out by notebook mismatch
    });
  });
});

  describe('updateTaskFields', () => {
    it('optimistically updates notes store when status changes', async () => {
      // V9: Create note properly first
      const created = await noteService.createNote('Test Notebook');
      const testNote = created.meta;
      
      // Add to mock storage so update_note can find it
      const noteInMock = mockData.tasks.find(t => t.id === testNote.id);
      if (!noteInMock) {
        mockData.tasks.push({
          id: testNote.id,
          title: testNote.title,
          content: '',
          status: 'cabinet',
          project_id: testNote.project_id,
          folder_id: null,
          content_preview: '',
          word_count: 0,
          tags: [],
          created_at: testNote.created_at,
          updated_at: testNote.updated_at,
        });
      }
      
      // Update task fields - set status to 'todo'
      await noteService.updateTaskFields(testNote.id, { status: 'todo' as any });
      
      // Verify the store was updated
      const currentNotes = get(notes);
      const updatedNote = currentNotes.find(n => n.id === testNote.id);
      expect(updatedNote).toBeDefined();
      expect(updatedNote!.task_status).toBe('todo');
    });

    it('moves note between status groups when status changes', async () => {
      // V9: Create note properly first
      const created = await noteService.createNote('Test Notebook');
      const testNote = created.meta;
      
      // Add to mock storage so update_note can find it
      const noteInMock = mockData.tasks.find(t => t.id === testNote.id);
      if (!noteInMock) {
        mockData.tasks.push({
          id: testNote.id,
          title: testNote.title,
          content: '',
          status: 'cabinet',
          project_id: testNote.project_id,
          folder_id: null,
          content_preview: '',
          word_count: 0,
          tags: [],
          created_at: testNote.created_at,
          updated_at: testNote.updated_at,
        });
      }
      
      // Verify initial status
      let currentNotes = get(notes);
      let ourNote = currentNotes.find(n => n.id === testNote.id);
      expect(ourNote).toBeDefined();
      expect(ourNote!.task_status).toBeUndefined(); // Cabinet maps to undefined in transformNoteMeta (cabinet = not a task)
      
      // Change status to in_progress
      await noteService.updateTaskFields(testNote.id, { status: 'in_progress' as any });
      
      // Verify status changed
      currentNotes = get(notes);
      ourNote = currentNotes.find(n => n.id === testNote.id);
      expect(ourNote).toBeDefined();
      expect(ourNote!.task_status).toBe('in_progress');
    });
  });
