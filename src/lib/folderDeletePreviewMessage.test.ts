/**
 * folderDeletePreviewMessage.test.ts — #2874 item 65 ruling 4.
 *
 * The delete-folder confirmation dialog must say where content goes,
 * using the default folder's REAL, current name (never hardcoded
 * "General") — this is the pure formatter Sidebar.svelte calls with the
 * backend's `get_folder_delete_preview` response.
 */
import { describe, it, expect } from 'vitest';
import { formatFolderDeleteMessage } from './folderDeletePreviewMessage';

describe('formatFolderDeleteMessage (#2874 item 65 ruling 4)', () => {
  it('matches the ruling\'s own example shape: "3 notes and 2 sheets will move to Overview"', () => {
    const message = formatFolderDeleteMessage({
      note_count: 3,
      sheet_count: 2,
      default_folder_name: 'Overview',
    });
    expect(message).toBe('3 notes and 2 sheets will move to Overview. This cannot be undone.');
  });

  it('uses the default folder\'s REAL name, not a hardcoded "General"', () => {
    const message = formatFolderDeleteMessage({
      note_count: 1,
      sheet_count: 0,
      default_folder_name: 'My Renamed Default Folder',
    });
    expect(message).toContain('My Renamed Default Folder');
    expect(message).not.toContain('General');
  });

  it('singularizes a single note correctly', () => {
    expect(
      formatFolderDeleteMessage({ note_count: 1, sheet_count: 0, default_folder_name: 'Overview' })
    ).toBe('1 note will move to Overview. This cannot be undone.');
  });

  it('singularizes a single sheet correctly', () => {
    expect(
      formatFolderDeleteMessage({ note_count: 0, sheet_count: 1, default_folder_name: 'Overview' })
    ).toBe('1 sheet will move to Overview. This cannot be undone.');
  });

  it('omits the note clause entirely when there are zero notes', () => {
    const message = formatFolderDeleteMessage({ note_count: 0, sheet_count: 4, default_folder_name: 'Overview' });
    expect(message).toBe('4 sheets will move to Overview. This cannot be undone.');
  });

  it('omits the sheet clause entirely when there are zero sheets', () => {
    const message = formatFolderDeleteMessage({ note_count: 4, sheet_count: 0, default_folder_name: 'Overview' });
    expect(message).toBe('4 notes will move to Overview. This cannot be undone.');
  });

  it('falls back to an empty-folder message when there is nothing to move', () => {
    const message = formatFolderDeleteMessage({ note_count: 0, sheet_count: 0, default_folder_name: 'Overview' });
    expect(message).toBe('This folder is empty. This cannot be undone.');
    // An empty folder still has nothing to move, so the default folder's
    // name is deliberately NOT mentioned in this branch.
    expect(message).not.toContain('Overview');
  });
});
