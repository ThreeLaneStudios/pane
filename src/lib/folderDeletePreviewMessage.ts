// #2874 item 65 ruling 4: the delete-folder confirmation dialog must say
// where content goes — e.g. "3 notes and 2 sheets will move to Overview" —
// using the default folder's REAL, current name (read at display time via
// `api.getFolderDeletePreview`, never hardcoded "General"). Pure formatter,
// split out of Sidebar.svelte so it's directly unit-testable without
// rendering the component.
import type { FolderDeletePreview } from './api';

export function formatFolderDeleteMessage(preview: FolderDeletePreview): string {
  const parts: string[] = [];
  if (preview.note_count > 0) {
    parts.push(`${preview.note_count} note${preview.note_count === 1 ? '' : 's'}`);
  }
  if (preview.sheet_count > 0) {
    parts.push(`${preview.sheet_count} sheet${preview.sheet_count === 1 ? '' : 's'}`);
  }
  if (parts.length === 0) {
    return 'This folder is empty. This cannot be undone.';
  }
  return `${parts.join(' and ')} will move to ${preview.default_folder_name}. This cannot be undone.`;
}
