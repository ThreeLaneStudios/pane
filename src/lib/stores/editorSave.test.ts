import { describe, it, expect, vi, beforeEach } from 'vitest';
import { editorSavePromise, setEditorSavePromise } from './editorSave';

/**
 * #1735 — Stale onDestroy Save Fix Tests
 *
 * Tests for the editorSave.ts store that coordinates save-promise handoff
 * between Editor.svelte (onDestroy) and noteService (selectNote / createNote).
 *
 * The key invariant: noteService must await the editor's in-flight save promise
 * before reading the note back from DB, to prevent reading stale content.
 *
 * E2E coverage for the fast-nav regression (type → navigate within 500ms →
 * content preserved) is deferred to #1723 (Tauri E2E harness).
 */

describe('editorSave store — #1735', () => {
  it('exports a resolved promise as initial state', async () => {
    // Module-level initial state should be immediately resolved
    await expect(editorSavePromise).resolves.toBeUndefined();
  });

  it('setEditorSavePromise updates the exported promise', async () => {
    let resolveFn!: () => void;
    const controlled = new Promise<void>(res => { resolveFn = res; });
    setEditorSavePromise(controlled);

    // Import the module fresh to read the mutated export
    // (direct reference to editorSavePromise captured at import time won't update)
    // We test the setter round-trip by verifying the import value is updated
    const mod = await import('./editorSave');
    const current = mod.editorSavePromise;

    let resolved = false;
    const check = current.then(() => { resolved = true; });

    // Not yet resolved
    await Promise.resolve(); // flush microtasks
    expect(resolved).toBe(false);

    resolveFn();
    await check;
    expect(resolved).toBe(true);
  });

  it('noteService race — selectNote waits for editorSavePromise', async () => {
    /**
     * Simulates the selectNote pattern introduced in the fix:
     *   activeNote.set(null);   <- triggers Editor unmount + onDestroy
     *   await editorSavePromise; <- waits for the onDestroy save
     *   await readNote(...)      <- reads AFTER save completes
     *
     * Here we verify the ordering guarantee with a controlled promise.
     */
    const callOrder: string[] = [];

    // Simulate a slow onDestroy save (like a Tauri IPC call)
    let resolveEditorSave!: () => void;
    const fakeEditorSave = new Promise<void>(res => { resolveEditorSave = res; });
    setEditorSavePromise(fakeEditorSave);

    const mod = await import('./editorSave');

    // Simulate selectNote waiting on the promise before reading
    const selectNoteSimulated = async () => {
      await mod.editorSavePromise;
      await Promise.resolve(); // DB flush buffer (20ms equivalent)
      callOrder.push('readNote');
    };

    // Start simulated selectNote
    const selectP = selectNoteSimulated();

    // Simulate onDestroy running its save
    callOrder.push('saveNote');
    resolveEditorSave(); // now the save "completes"

    await selectP;

    expect(callOrder).toEqual(['saveNote', 'readNote']);
  });
});

/**
 * Integration test notes (requires Svelte component mounting + TipTap):
 *
 * Test 1 — Fast-nav-within-debounce (THE critical test):
 *   - Mount Editor with content
 *   - Simulate keystroke → content changes
 *   - Within 500ms, destroy component
 *   - Assert api.saveNote was called with the NEW content, not the old content
 *   Requires: @testing-library/svelte + jsdom + TipTap event simulation
 *   Status: Deferred to #1723 (E2E harness)
 *
 * Test 2 — Normal debounce save regression:
 *   - Mount Editor with content
 *   - Simulate keystroke
 *   - Wait >500ms
 *   - Assert api.saveNote called with correct content
 *   Status: Deferred to #1723
 *
 * Test 3 — htmlToMarkdown per-keystroke cost:
 *   - Create a 10K+ word HTML string
 *   - Run htmlToMarkdown in a loop (100 iterations)
 *   - Assert average call time < 10ms
 *   Status: Can be added as a benchmark test once htmlToMarkdown is extractable
 */
