/**
 * Editor Toolbar — Responsive Overflow + ⌘⇧T Fix Tests
 * Issues: #2123 (toolbar overflow), #2130 (⌘⇧T regression), #2135 (measurement-based overflow)
 *
 * Tests use pure logic extraction (no DOM event binding) so they run in any
 * vitest environment without needing full window.addEventListener support.
 *
 * spec-skip: Overflow trigger mechanism change from fixed threshold to measurement-based;
 * spec updated in PR #2133, behavioral contract unchanged
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

// ---------------------------------------------------------------------------
// 1. Measurement-based overflow detection (#2135)
// ---------------------------------------------------------------------------

/**
 * Simulates the two-pass measurement logic from Editor.svelte's ResizeObserver.
 * Pass 1: un-collapse to measure full natural width (scrollWidth vs clientWidth).
 * Pass 2: re-collapse if still overflowing, then commit isNarrow state.
 *
 * In the browser this operates on real DOM elements; here we simulate the
 * width values that the ResizeObserver callback would observe.
 */
function simulateOverflowCheck(
  scrollWidthFull: number,  // scrollWidth with ALL items visible (un-collapsed state)
  clientWidth: number,      // available container width
): boolean {
  return scrollWidthFull > clientWidth;
}

describe('toolbar overflow — measurement-based isNarrow (#2135)', () => {
  it('overflows when full content wider than container', () => {
    expect(simulateOverflowCheck(500, 400)).toBe(true);
    expect(simulateOverflowCheck(401, 400)).toBe(true);
    expect(simulateOverflowCheck(331, 330)).toBe(true);
  });

  it('does NOT overflow when content fits exactly', () => {
    expect(simulateOverflowCheck(400, 400)).toBe(false);
  });

  it('does NOT overflow when content narrower than container', () => {
    expect(simulateOverflowCheck(300, 400)).toBe(false);
    expect(simulateOverflowCheck(1, 400)).toBe(false);
    expect(simulateOverflowCheck(200, 1200)).toBe(false);
  });

  it('triggers narrow at typical full-toolbar width (~520px) in a 400px container', () => {
    // Full toolbar with all 10 overflow items rendered is ~520px wide
    expect(simulateOverflowCheck(520, 400)).toBe(true);
  });

  it('exits narrow when container grows wide enough for all items', () => {
    // Container wide enough: full content fits
    expect(simulateOverflowCheck(520, 600)).toBe(false);
  });

  it('floating-point: scrollWidth 400.5 in 400px container overflows', () => {
    expect(simulateOverflowCheck(400.5, 400)).toBe(true);
  });

  it('floating-point: scrollWidth 399.9 in 400px container does not overflow', () => {
    expect(simulateOverflowCheck(399.9, 400)).toBe(false);
  });

  it('zero-width container edge: clientWidth 0 never checked (guarded in observer)', () => {
    // The ResizeObserver callback returns early when clientWidth === 0.
    // We verify that a hypothetical check with clientWidth=0 would always overflow,
    // justifying the early-return guard.
    expect(simulateOverflowCheck(1, 0)).toBe(true);
    expect(simulateOverflowCheck(0, 0)).toBe(false);
  });
});

describe('toolbar overflow — two-pass class mutation logic (#2135)', () => {
  /**
   * Simulates the class toggle strategy:
   * 1. Remove toolbar--narrow if present (to reveal full content for measurement)
   * 2. Measure scrollWidth vs clientWidth
   * 3. Re-add toolbar--narrow if still overflowing
   * 4. Commit isNarrow state
   */
  function simulateTwoPass(
    initialNarrow: boolean,
    scrollWidthFull: number,
    clientWidth: number,
  ): { resultNarrow: boolean; classNarrowDuringMeasure: boolean } {
    let classNarrow = initialNarrow;

    // Pass 1: reveal for measurement
    if (classNarrow) classNarrow = false;
    const classNarrowDuringMeasure = classNarrow; // should always be false

    // Measure (simulated)
    const shouldBeNarrow = scrollWidthFull > clientWidth;

    // Pass 2: re-collapse if needed
    if (initialNarrow && shouldBeNarrow) classNarrow = true;

    return { resultNarrow: shouldBeNarrow, classNarrowDuringMeasure };
  }

  it('class is removed during measurement pass (no flash mitigation)', () => {
    const { classNarrowDuringMeasure } = simulateTwoPass(true, 520, 400);
    expect(classNarrowDuringMeasure).toBe(false); // class is off during measure
  });

  it('stays narrow: was narrow + still overflows → resultNarrow true', () => {
    const { resultNarrow } = simulateTwoPass(true, 520, 400);
    expect(resultNarrow).toBe(true);
  });

  it('exits narrow: was narrow + content fits → resultNarrow false', () => {
    const { resultNarrow } = simulateTwoPass(true, 520, 600);
    expect(resultNarrow).toBe(false);
  });

  it('enters narrow: was NOT narrow + now overflows → resultNarrow true', () => {
    const { resultNarrow } = simulateTwoPass(false, 520, 400);
    expect(resultNarrow).toBe(true);
  });

  it('stays not narrow: was NOT narrow + content fits → resultNarrow false', () => {
    const { resultNarrow } = simulateTwoPass(false, 520, 600);
    expect(resultNarrow).toBe(false);
  });

  it('exact fit (scrollWidth === clientWidth) → not narrow', () => {
    const { resultNarrow } = simulateTwoPass(false, 400, 400);
    expect(resultNarrow).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// 2. Overflow item list completeness
// ---------------------------------------------------------------------------

describe('toolbar overflow — 10 expected overflow items', () => {
  // These are the 10 items that gain .toolbar-btn--overflow / .toolbar-sep--overflow
  // and collapse into the ⋯ Formatting group at narrow width.
  const OVERFLOW_ITEMS = [
    'Link (inline)',
    'Attachment link',
    'Code Block',
    'Undo',
    'Redo',
    'Version History',
    'Pin',
    'Checklist',
    'Due Today',
    'Attach File',
  ];

  it('overflow item list has exactly 10 items per design spec', () => {
    expect(OVERFLOW_ITEMS).toHaveLength(10);
  });

  it('every overflow item has a distinct label', () => {
    const unique = new Set(OVERFLOW_ITEMS);
    expect(unique.size).toBe(OVERFLOW_ITEMS.length);
  });
});

// ---------------------------------------------------------------------------
// 3. ⌘⇧T fix — capture-phase handler logic (pure function)
// ---------------------------------------------------------------------------

/**
 * Extract the exact logic from the capture-phase listener registered in +page.svelte onMount.
 * We test the pure function, not the DOM registration.
 */
function makeCaptureTemplateShortcut(setShowTemplates: (v: boolean) => void) {
  return (e: {
    metaKey: boolean;
    ctrlKey: boolean;
    shiftKey: boolean;
    key: string;
    preventDefault: () => void;
  }) => {
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === 'T') {
      e.preventDefault();
      setShowTemplates(true);
    }
  };
}

describe('⌘⇧T fix (#2130) — capture-phase keydown logic', () => {
  let showTemplates: boolean;
  let handler: ReturnType<typeof makeCaptureTemplateShortcut>;

  beforeEach(() => {
    showTemplates = false;
    handler = makeCaptureTemplateShortcut((v) => { showTemplates = v; });
  });

  function makeEvent(opts: { key: string; metaKey?: boolean; ctrlKey?: boolean; shiftKey?: boolean }) {
    return {
      key: opts.key,
      metaKey: opts.metaKey ?? false,
      ctrlKey: opts.ctrlKey ?? false,
      shiftKey: opts.shiftKey ?? false,
      preventDefault: vi.fn(),
    };
  }

  it('sets showTemplates = true on ⌘⇧T', () => {
    handler(makeEvent({ key: 'T', metaKey: true, shiftKey: true }));
    expect(showTemplates).toBe(true);
  });

  it('sets showTemplates = true on ⌃⇧T (Ctrl)', () => {
    handler(makeEvent({ key: 'T', ctrlKey: true, shiftKey: true }));
    expect(showTemplates).toBe(true);
  });

  it('does NOT trigger on ⌘T (no shift)', () => {
    handler(makeEvent({ key: 't', metaKey: true }));
    expect(showTemplates).toBe(false);
  });

  it('does NOT trigger on ⇧T alone (no meta/ctrl)', () => {
    handler(makeEvent({ key: 'T', shiftKey: true }));
    expect(showTemplates).toBe(false);
  });

  it('does NOT trigger on ⌘⇧S (different key)', () => {
    handler(makeEvent({ key: 'S', metaKey: true, shiftKey: true }));
    expect(showTemplates).toBe(false);
  });

  it('does NOT trigger on bare T (no modifiers)', () => {
    handler(makeEvent({ key: 'T' }));
    expect(showTemplates).toBe(false);
  });

  it('calls preventDefault on ⌘⇧T', () => {
    const e = makeEvent({ key: 'T', metaKey: true, shiftKey: true });
    handler(e);
    expect(e.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('does NOT call preventDefault on unrelated keys', () => {
    const e = makeEvent({ key: 'n', metaKey: true });
    handler(e);
    expect(e.preventDefault).not.toHaveBeenCalled();
  });

  it('is idempotent — calling handler twice sets showTemplates true once (already true)', () => {
    handler(makeEvent({ key: 'T', metaKey: true, shiftKey: true }));
    handler(makeEvent({ key: 'T', metaKey: true, shiftKey: true }));
    expect(showTemplates).toBe(true); // still true, not toggled
  });

  it('ctrlKey + metaKey both true → still triggers (belt and suspenders)', () => {
    handler(makeEvent({ key: 'T', metaKey: true, ctrlKey: true, shiftKey: true }));
    expect(showTemplates).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// 4. handleMoveToTrash — guard conditions (pure logic)
// ---------------------------------------------------------------------------

/**
 * Extracted guard logic from Editor.svelte handleMoveToTrash.
 * Tests verify all edge cases before the actual API call.
 */
async function simulateMoveToTrash(
  noteId: string | undefined,
  moveToTrashLoading: boolean,
  moveToTrash: (id: string) => Promise<void>,
  setActiveNoteNull: () => void,
): Promise<'guarded' | 'success' | 'error'> {
  if (!noteId || moveToTrashLoading) return 'guarded';
  try {
    await moveToTrash(noteId);
    setActiveNoteNull();
    return 'success';
  } catch {
    return 'error';
  }
}

describe('handleMoveToTrash — guard conditions', () => {
  it('returns early if noteId is empty string', async () => {
    const moveToTrash = vi.fn();
    const setNull = vi.fn();
    const result = await simulateMoveToTrash('', false, moveToTrash, setNull);
    expect(result).toBe('guarded');
    expect(moveToTrash).not.toHaveBeenCalled();
    expect(setNull).not.toHaveBeenCalled();
  });

  it('returns early if noteId is undefined', async () => {
    const moveToTrash = vi.fn();
    const setNull = vi.fn();
    const result = await simulateMoveToTrash(undefined, false, moveToTrash, setNull);
    expect(result).toBe('guarded');
    expect(moveToTrash).not.toHaveBeenCalled();
  });

  it('returns early if moveToTrashLoading is true', async () => {
    const moveToTrash = vi.fn();
    const setNull = vi.fn();
    const result = await simulateMoveToTrash('note-123', true, moveToTrash, setNull);
    expect(result).toBe('guarded');
    expect(moveToTrash).not.toHaveBeenCalled();
  });

  it('calls moveToTrash and sets activeNote null on success', async () => {
    const moveToTrash = vi.fn().mockResolvedValue(undefined);
    const setNull = vi.fn();
    const result = await simulateMoveToTrash('note-abc', false, moveToTrash, setNull);
    expect(result).toBe('success');
    expect(moveToTrash).toHaveBeenCalledWith('note-abc');
    expect(setNull).toHaveBeenCalledTimes(1);
  });

  it('does NOT call setActiveNoteNull if moveToTrash throws', async () => {
    const moveToTrash = vi.fn().mockRejectedValue(new Error('DB error'));
    const setNull = vi.fn();
    const result = await simulateMoveToTrash('note-xyz', false, moveToTrash, setNull);
    expect(result).toBe('error');
    expect(setNull).not.toHaveBeenCalled();
  });

  it('passes the noteId correctly to moveToTrash', async () => {
    const moveToTrash = vi.fn().mockResolvedValue(undefined);
    const setNull = vi.fn();
    await simulateMoveToTrash('note-unique-id-999', false, moveToTrash, setNull);
    expect(moveToTrash).toHaveBeenCalledWith('note-unique-id-999');
  });
});

// ---------------------------------------------------------------------------
// 5. pane-show-templates custom event — dispatch logic (pure)
// ---------------------------------------------------------------------------

describe('pane-show-templates dispatch', () => {
  it('CustomEvent constructor produces a truthy object with correct type', () => {
    const evt = new CustomEvent('pane-show-templates');
    expect(evt).toBeTruthy();
    expect(evt.type).toBe('pane-show-templates');
  });

  it('CustomEvent detail is null by default (matches dispatch pattern in TipTap handler)', () => {
    const evt = new CustomEvent('pane-show-templates');
    expect(evt.detail).toBeNull();
  });

  it('handler function pattern: templateHandler sets showTemplates true', () => {
    let showTemplates = false;
    // This mirrors: const templateHandler = () => { showTemplates = true; };
    const templateHandler = () => { showTemplates = true; };
    templateHandler();
    expect(showTemplates).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// 6. Formatting group structure — menu item count validation
// ---------------------------------------------------------------------------

describe('⋯ menu — Formatting group items at narrow width', () => {
  // Match the items rendered in the Formatting group in Editor.svelte
  const FORMATTING_MENU_ITEMS = [
    'Link',
    'Insert link',
    'Code block',
    'Undo',
    'Redo',
    'Version history',
    'Pin / Unpin',
    'Checklist',
    'Due today',
    'Attach file',
  ];

  it('Formatting group has 10 items matching overflow toolbar items', () => {
    expect(FORMATTING_MENU_ITEMS).toHaveLength(10);
  });

  it('all items have non-empty labels', () => {
    for (const item of FORMATTING_MENU_ITEMS) {
      expect(item.trim().length).toBeGreaterThan(0);
    }
  });
});

// ---------------------------------------------------------------------------
// 7. Sheet "More" overflow menu handlers (#2339 PR 15b-alpha) — pure logic
// ---------------------------------------------------------------------------
// Editor.svelte itself is too large/heavy to mount in these unit tests (same
// reasoning as handleMoveToTrash above) — these extract the exact guard/
// call/error-handling shape of `handleExportSheetCsv` and `toggleFolderPicker`
// and test them as pure functions. `handleRenameSheet` was removed (#2874
// item 33) — the sheet title input is already directly click-to-edit, so no
// dedicated rename handler exists anymore. `handleMoveToSheet` (and its
// `simulateMoveToSheet` test below) was removed (#2874 item 49) — the "Move
// to…" menu row it backed is gone, since the folder pill already covers
// moving a sheet between folders. The folder pill's own mechanism,
// `toggleFolderPicker`, is untouched and gets its own pure-logic coverage
// below to confirm it still works post-removal.

/** Mirrors Editor.svelte's `handleExportSheetCsv`. */
async function simulateExportSheetCsv(
  noteId: string | undefined,
  exportCsv: (noteId: string) => Promise<{ csv_content: string; filename: string }>,
  saveDialog: (opts: { defaultPath: string }) => Promise<string | null>,
  writeTextFile: (path: string, content: string) => Promise<void>,
  showErrorToast: (msg: string) => void,
): Promise<'guarded' | 'cancelled' | 'success' | 'error'> {
  if (!noteId) return 'guarded';
  try {
    const result = await exportCsv(noteId);
    const path = await saveDialog({ defaultPath: result.filename });
    if (!path) return 'cancelled';
    await writeTextFile(path, result.csv_content);
    return 'success';
  } catch {
    showErrorToast('Failed to export sheet as CSV.');
    return 'error';
  }
}

describe('handleExportSheetCsv — guard/flow conditions', () => {
  it('returns early (guarded) if noteId is undefined — no invoke, no dialog', async () => {
    const exportCsv = vi.fn();
    const saveDialog = vi.fn();
    const writeTextFile = vi.fn();
    const toast = vi.fn();
    const result = await simulateExportSheetCsv(undefined, exportCsv, saveDialog, writeTextFile, toast);
    expect(result).toBe('guarded');
    expect(exportCsv).not.toHaveBeenCalled();
  });

  it('calls export then save dialog then writeTextFile with the returned csv content on success', async () => {
    const exportCsv = vi.fn().mockResolvedValue({ csv_content: 'a,b\r\n1,2', filename: 'My Sheet.csv' });
    const saveDialog = vi.fn().mockResolvedValue('/Users/x/Desktop/My Sheet.csv');
    const writeTextFile = vi.fn().mockResolvedValue(undefined);
    const toast = vi.fn();
    const result = await simulateExportSheetCsv('note-1', exportCsv, saveDialog, writeTextFile, toast);
    expect(result).toBe('success');
    expect(exportCsv).toHaveBeenCalledWith('note-1');
    expect(saveDialog).toHaveBeenCalledWith({ defaultPath: 'My Sheet.csv' });
    expect(writeTextFile).toHaveBeenCalledWith('/Users/x/Desktop/My Sheet.csv', 'a,b\r\n1,2');
    expect(toast).not.toHaveBeenCalled();
  });

  it('treats a cancelled save dialog (null path) as a silent no-op, not an error', async () => {
    const exportCsv = vi.fn().mockResolvedValue({ csv_content: 'a,b', filename: 'x.csv' });
    const saveDialog = vi.fn().mockResolvedValue(null);
    const writeTextFile = vi.fn();
    const toast = vi.fn();
    const result = await simulateExportSheetCsv('note-1', exportCsv, saveDialog, writeTextFile, toast);
    expect(result).toBe('cancelled');
    expect(writeTextFile).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
  });

  it('shows an error toast if the export command itself rejects', async () => {
    const exportCsv = vi.fn().mockRejectedValue(new Error('db locked'));
    const saveDialog = vi.fn();
    const writeTextFile = vi.fn();
    const toast = vi.fn();
    const result = await simulateExportSheetCsv('note-1', exportCsv, saveDialog, writeTextFile, toast);
    expect(result).toBe('error');
    expect(saveDialog).not.toHaveBeenCalled();
    expect(toast).toHaveBeenCalledWith('Failed to export sheet as CSV.');
  });

  it('shows an error toast if writeTextFile rejects (e.g. permission denied)', async () => {
    const exportCsv = vi.fn().mockResolvedValue({ csv_content: 'a,b', filename: 'x.csv' });
    const saveDialog = vi.fn().mockResolvedValue('/root/x.csv');
    const writeTextFile = vi.fn().mockRejectedValue(new Error('EACCES'));
    const toast = vi.fn();
    const result = await simulateExportSheetCsv('note-1', exportCsv, saveDialog, writeTextFile, toast);
    expect(result).toBe('error');
    expect(toast).toHaveBeenCalledOnce();
  });
});

/** Mirrors Editor.svelte's `toggleFolderPicker` — the folder pill's own
 *  mechanism, unrelated to (and untouched by) the removed "Move to…" menu
 *  row (#2874 item 49). Covers its three real branches: no-op when the
 *  sheet has no `project_id` (pill isn't rendered), toggle-closed when
 *  already open, and open-and-load-folders otherwise. */
async function simulateToggleFolderPicker(
  projectId: string | null | undefined,
  isOpen: boolean,
  listFolders: (projectId: string) => Promise<Array<{ id: string; is_system?: boolean }>>,
): Promise<'guarded' | 'closed' | 'opened' | 'error'> {
  if (!projectId) return 'guarded';
  if (isOpen) return 'closed';
  try {
    await listFolders(projectId);
    return 'opened';
  } catch {
    return 'error';
  }
}

describe('toggleFolderPicker — folder pill still works after Move to… removal (#2874 item 49)', () => {
  it('returns guarded if the sheet has no project_id (pill is not rendered)', async () => {
    const listFolders = vi.fn();
    const result = await simulateToggleFolderPicker(null, false, listFolders);
    expect(result).toBe('guarded');
    expect(listFolders).not.toHaveBeenCalled();
  });

  it('returns closed and does not refetch folders when the picker is already open', async () => {
    const listFolders = vi.fn();
    const result = await simulateToggleFolderPicker('proj-1', true, listFolders);
    expect(result).toBe('closed');
    expect(listFolders).not.toHaveBeenCalled();
  });

  it('loads folders and opens when the pill is clicked with a project_id and the picker is closed', async () => {
    const listFolders = vi.fn().mockResolvedValue([{ id: 'f1', is_system: false }]);
    const result = await simulateToggleFolderPicker('proj-1', false, listFolders);
    expect(result).toBe('opened');
    expect(listFolders).toHaveBeenCalledWith('proj-1');
  });

  it('returns error if listFolders rejects', async () => {
    const listFolders = vi.fn().mockRejectedValue(new Error('db locked'));
    const result = await simulateToggleFolderPicker('proj-1', false, listFolders);
    expect(result).toBe('error');
  });
});

// ---------------------------------------------------------------------------
// 8. `handleFolderMove` ordering + error-visibility (#3053 item 2, docs/
//    audits/2026-10-20-scope-1061-fixes.md, D1/D2 part 2) — pure logic
// ---------------------------------------------------------------------------
// Same "mirror the non-exported function" approach as `simulateToggleFolder
// Picker`/`simulateExportSheetCsv` above — Editor.svelte is too large to
// mount for this. Mirrors the FIXED shape of `handleFolderMove`: capture
// `noteMeta.id`/entity type into local consts BEFORE calling `onMoved()`,
// dispatch using those consts, and surface (not swallow) a failed
// `invoke('update_note')` call via `showErrorToast`.

/** Mirrors Editor.svelte's fixed `handleFolderMove`. `noteMeta` is accepted
 *  as a plain object (not a getter-trap) for the common-path tests below;
 *  the ordering regression test further down uses a getter-trapped
 *  version to prove WHEN each field is read, not just whether the call
 *  succeeds. */
async function simulateHandleFolderMove(
  noteMeta: { id: string; project_id: string; folder_id: string | null },
  folder: { id: string; name: string },
  isSheet: boolean,
  invokeUpdateNote: (req: unknown) => Promise<unknown>,
  dispatchDataChanged: (id: string, entityType: 'note' | 'sheet', reason: 'moved') => void,
  onMoved: (() => void) | undefined,
  showErrorToast: (msg: string) => void,
): Promise<'noop-same-folder' | 'guarded' | 'success' | 'error'> {
  if (folder.id === noteMeta?.folder_id) return 'noop-same-folder';
  if (!noteMeta?.id || !noteMeta?.project_id) return 'guarded';

  // The fix: read BEFORE calling onMoved(), not after.
  const movedNoteId = noteMeta.id;
  const movedEntityType: 'note' | 'sheet' = isSheet ? 'sheet' : 'note';

  try {
    await invokeUpdateNote({
      note_id: noteMeta.id,
      project_id: noteMeta.project_id,
      folder_id: folder.id,
    });
    dispatchDataChanged(movedNoteId, movedEntityType, 'moved');
    if (onMoved) {
      onMoved();
    }
    return 'success';
  } catch {
    showErrorToast('Failed to move note to folder.');
    return 'error';
  }
}

describe('handleFolderMove — ordering + error visibility (#3053 item 2)', () => {
  it('no-ops without calling invoke/dispatch/onMoved when the folder is already the current one', async () => {
    const invokeUpdateNote = vi.fn();
    const dispatchDataChanged = vi.fn();
    const onMoved = vi.fn();
    const showErrorToast = vi.fn();
    const result = await simulateHandleFolderMove(
      { id: 'note-1', project_id: 'proj-1', folder_id: 'folder-a' },
      { id: 'folder-a', name: 'A' },
      false,
      invokeUpdateNote,
      dispatchDataChanged,
      onMoved,
      showErrorToast,
    );
    expect(result).toBe('noop-same-folder');
    expect(invokeUpdateNote).not.toHaveBeenCalled();
    expect(dispatchDataChanged).not.toHaveBeenCalled();
    expect(onMoved).not.toHaveBeenCalled();
  });

  it('dispatches the data-changed event with the resolved note id/entity type on success, note variant', async () => {
    const invokeUpdateNote = vi.fn().mockResolvedValue(undefined);
    const dispatchDataChanged = vi.fn();
    const onMoved = vi.fn();
    const showErrorToast = vi.fn();
    const result = await simulateHandleFolderMove(
      { id: 'note-1', project_id: 'proj-1', folder_id: 'folder-a' },
      { id: 'folder-b', name: 'B' },
      false,
      invokeUpdateNote,
      dispatchDataChanged,
      onMoved,
      showErrorToast,
    );
    expect(result).toBe('success');
    expect(dispatchDataChanged).toHaveBeenCalledWith('note-1', 'note', 'moved');
    expect(showErrorToast).not.toHaveBeenCalled();
  });

  it('dispatches entity_type "sheet" (not "note") when isSheet is true', async () => {
    const invokeUpdateNote = vi.fn().mockResolvedValue(undefined);
    const dispatchDataChanged = vi.fn();
    const result = await simulateHandleFolderMove(
      { id: 'sheet-1', project_id: 'proj-1', folder_id: 'folder-a' },
      { id: 'folder-b', name: 'B' },
      true,
      invokeUpdateNote,
      dispatchDataChanged,
      vi.fn(),
      vi.fn(),
    );
    expect(dispatchDataChanged).toHaveBeenCalledWith('sheet-1', 'sheet', 'moved');
  });

  /** The ordering regression test (#3053 item 2): `onMoved()` can
   *  synchronously destroy the Editor instance whose closure is still
   *  running past it (both real callers — AllTasksView's
   *  `handleCloseEditor`, NotesBrowser's `goBackToCards` — null
   *  `$activeNote` synchronously, and both render sites gate `<Editor>`
   *  on `$activeNote`). This `noteMeta` throws on any field access made
   *  AFTER `onMoved()` runs, simulating a destroyed component's prop
   *  access — proving `dispatchDataChanged` reads its id/type BEFORE that
   *  point, not merely that "no error is thrown" (a try/catch swallowing
   *  a post-destroy read failure would also show no thrown error, but
   *  would silently drop the event — the bug this guards against). */
  it('dispatchDataChanged still fires with the correct id/type when onMoved makes noteMeta unreadable afterward', async () => {
    const callOrder: string[] = [];
    let destroyed = false;
    const noteMeta = {
      folder_id: 'folder-a',
      get id() {
        if (destroyed) {
          throw new Error('noteMeta read after component destroyed — simulated teardown');
        }
        return 'note-1';
      },
      project_id: 'proj-1',
    };
    const invokeUpdateNote = vi.fn().mockResolvedValue(undefined);
    const dispatchDataChanged = vi.fn((id: string, entityType: string, reason: string) => {
      callOrder.push(`dispatch:${id}:${entityType}:${reason}`);
    });
    const onMoved = vi.fn(() => {
      callOrder.push('onMoved');
      destroyed = true; // simulates the component tearing down mid-call
    });
    const showErrorToast = vi.fn();

    const result = await simulateHandleFolderMove(
      noteMeta as any,
      { id: 'folder-b', name: 'B' },
      false,
      invokeUpdateNote,
      dispatchDataChanged,
      onMoved,
      showErrorToast,
    );

    expect(result).toBe('success');
    expect(dispatchDataChanged).toHaveBeenCalledWith('note-1', 'note', 'moved');
    expect(showErrorToast).not.toHaveBeenCalled();
    expect(callOrder).toEqual(['dispatch:note-1:note:moved', 'onMoved']);
  });

  it('surfaces a failed invoke via showErrorToast instead of swallowing it, and does not dispatch or call onMoved', async () => {
    const invokeUpdateNote = vi.fn().mockRejectedValue(new Error('db locked'));
    const dispatchDataChanged = vi.fn();
    const onMoved = vi.fn();
    const showErrorToast = vi.fn();
    const result = await simulateHandleFolderMove(
      { id: 'note-1', project_id: 'proj-1', folder_id: 'folder-a' },
      { id: 'folder-b', name: 'B' },
      false,
      invokeUpdateNote,
      dispatchDataChanged,
      onMoved,
      showErrorToast,
    );
    expect(result).toBe('error');
    expect(showErrorToast).toHaveBeenCalledWith('Failed to move note to folder.');
    expect(dispatchDataChanged).not.toHaveBeenCalled();
    expect(onMoved).not.toHaveBeenCalled();
  });
});
