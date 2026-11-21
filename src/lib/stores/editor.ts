import { writable } from 'svelte/store';

// Tracks the editor's text content before AI edits, for diff highlighting
export const editorPreEditText = writable<string>('');
