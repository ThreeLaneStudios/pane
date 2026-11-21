// Tracks the current editor's in-flight onDestroy save.
// noteService awaits this instead of the heuristic 100ms timeout.
export let editorSavePromise: Promise<void> = Promise.resolve();
export function setEditorSavePromise(p: Promise<void>) { editorSavePromise = p; }
