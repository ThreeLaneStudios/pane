import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { 
  notes, 
  activeNote, 
  activeNotebook, 
  isSearching, 
  searchResults, 
  displayedNotes 
} from './app';
import type { NoteMeta, NoteContent } from '$lib/types';

describe('app stores', () => {
  beforeEach(() => {
    // Reset stores
    notes.set([]);
    activeNote.set(null);
    activeNotebook.set('');
    isSearching.set(false);
    searchResults.set([]);
  });

  describe('displayedNotes filtering by notebook', () => {
    const createTestNote = (id: string, title: string, notebook: string): NoteMeta => ({
      id,
      title,
      slug: title.toLowerCase().replace(/\s+/g, '-'),
      notebook,
      created_at: new Date().toISOString(),
      updated_at: new Date().toISOString(),
      preview: '',
      word_count: 0,
    });

    it('shows all notes when no active notebook', () => {
      const note1 = createTestNote('notebook1/note1', 'Note 1', 'Notebook 1');
      const note2 = createTestNote('notebook2/note2', 'Note 2', 'Notebook 2');
      const note3 = createTestNote('inbox/note3', 'Note 3', 'Inbox');

      notes.set([note1, note2, note3]);
      activeNotebook.set('');

      const displayed = get(displayedNotes);
      expect(displayed).toHaveLength(3);
      expect(displayed).toEqual(expect.arrayContaining([note1, note2, note3]));
    });

    it('filters notes by active notebook', () => {
      const note1 = createTestNote('notebook1/note1', 'Note 1', 'Notebook 1');
      const note2 = createTestNote('notebook1/note2', 'Note 2', 'Notebook 1');  
      const note3 = createTestNote('notebook2/note3', 'Note 3', 'Notebook 2');

      notes.set([note1, note2, note3]);
      activeNotebook.set('Notebook 1');

      const displayed = get(displayedNotes);
      expect(displayed).toHaveLength(2);
      expect(displayed).toEqual(expect.arrayContaining([note1, note2]));
      expect(displayed).not.toContain(note3);
    });

    it('filters by note ID starting with notebook path', () => {
      const note1 = createTestNote('Project Name/Notebook 1/note1', 'Note 1', 'Notebook 1');
      const note2 = createTestNote('Project Name/Notebook 1/note2', 'Note 2', 'Notebook 1');
      const note3 = createTestNote('Other Project/Notebook 1/note3', 'Note 3', 'Notebook 1');

      notes.set([note1, note2, note3]);
      activeNotebook.set('Project Name/Notebook 1');

      const displayed = get(displayedNotes);
      expect(displayed).toHaveLength(2);
      expect(displayed).toEqual(expect.arrayContaining([note1, note2]));
      expect(displayed).not.toContain(note3);
    });

    it('shows search results when searching', () => {
      const note1 = createTestNote('notebook1/note1', 'Note 1', 'Notebook 1');
      const note2 = createTestNote('notebook2/note2', 'Note 2', 'Notebook 2');
      const searchResult = createTestNote('notebook2/search', 'Search Result', 'Notebook 2');

      notes.set([note1, note2]);
      searchResults.set([searchResult]);
      activeNotebook.set('Notebook 1');
      isSearching.set(true);

      const displayed = get(displayedNotes);
      expect(displayed).toHaveLength(1);
      expect(displayed).toEqual([searchResult]);
      // Should show search results, not filtered notebook notes
    });

    it('prevents cross-folder bleed', () => {
      // Test case where notes from different notebooks might accidentally show
      const inboxNote = createTestNote('Inbox/inbox-note', 'Inbox Note', 'Inbox');
      const projectNote = createTestNote('Work Project/Notes/work-note', 'Work Note', 'Notes');
      const personalNote = createTestNote('Personal/Notes/personal-note', 'Personal Note', 'Notes');

      notes.set([inboxNote, projectNote, personalNote]);
      activeNotebook.set('Work Project/Notes');

      const displayed = get(displayedNotes);
      expect(displayed).toHaveLength(1);
      expect(displayed).toEqual([projectNote]);
      expect(displayed).not.toContain(inboxNote);
      expect(displayed).not.toContain(personalNote);
    });
  });

  describe('activeNote mutations', () => {
    it('maintains null state correctly', () => {
      const testNote: NoteContent = {
        meta: {
          id: 'test/note',
          title: 'Test Note',
          slug: 'test-note',
          notebook: 'Test',
          created_at: new Date().toISOString(),
          updated_at: new Date().toISOString(),
          preview: '',
          word_count: 0,
        },
        content: 'Test content',
      };

      // Start with null
      expect(get(activeNote)).toBeNull();

      // Set note
      activeNote.set(testNote);
      expect(get(activeNote)).toEqual(testNote);

      // Set back to null
      activeNote.set(null);
      expect(get(activeNote)).toBeNull();
    });

    it('handles note content updates', () => {
      const initialNote: NoteContent = {
        meta: {
          id: 'test/note',
          title: 'Initial Title',
          slug: 'test-note',
          notebook: 'Test',
          created_at: new Date().toISOString(),
          updated_at: new Date().toISOString(),
          preview: '',
          word_count: 0,
        },
        content: 'Initial content',
      };

      activeNote.set(initialNote);

      const updatedNote: NoteContent = {
        ...initialNote,
        meta: {
          ...initialNote.meta,
          title: 'Updated Title',
          updated_at: new Date().toISOString(),
        },
        content: 'Updated content',
      };

      activeNote.set(updatedNote);
      const currentNote = get(activeNote);
      
      expect(currentNote?.meta.title).toBe('Updated Title');
      expect(currentNote?.content).toBe('Updated content');
    });
  });
});