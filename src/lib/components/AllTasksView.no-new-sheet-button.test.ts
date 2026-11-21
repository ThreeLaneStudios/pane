/**
 * AllTasksView.no-new-sheet-button.test.ts — #2874 item 1 regression coverage.
 *
 * New Sheet was removed from AllTasksView entirely (not gated behind a
 * conditional) — mirroring the pre-existing pattern for New Note, which was
 * never built in this component in the first place (see the former
 * AllTasksView.new-sheet-button-polish.test.ts:150-155 absence assertion,
 * now superseded by this file since the whole New-Sheet-button-polish file
 * was deleted along with the feature it tested).
 *
 * Source-text-extraction / regex assertions, matching the house pattern for
 * this heavy Tauri/store-dependent component — see
 * AllTasksView.sheet-card-face.test.ts's own docstring on why full mounts
 * aren't used for this file's class of assertion.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const COMPONENT_PATH = resolve(__dirname, 'AllTasksView.svelte');
const SOURCE = readFileSync(COMPONENT_PATH, 'utf-8');

describe('AllTasksView — no New Sheet button (item 1)', () => {
  it('confirms there is no New Sheet button anywhere in this component', () => {
    expect(SOURCE).not.toMatch(/class="add-sheet-btn/);
    expect(SOURCE).not.toMatch(/>New Sheet</);
    expect(SOURCE).not.toMatch(/class="sheet-caret-btn"/);
  });

  it('confirms the New-Sheet-only state, handlers, and layout import are gone', () => {
    expect(SOURCE).not.toMatch(/sheetProjectPickerOpen/);
    expect(SOURCE).not.toMatch(/sheetProjectPickerProjects/);
    expect(SOURCE).not.toMatch(/creatingSheet/);
    expect(SOURCE).not.toMatch(/openSheetProjectPicker/);
    expect(SOURCE).not.toMatch(/handleCreateNewSheet/);
    expect(SOURCE).not.toMatch(/handleNewSheetFromCaret/);
    expect(SOURCE).not.toMatch(/sheetCaretOpen/);
    expect(SOURCE).not.toMatch(/tasksHeaderEl/);
    expect(SOURCE).not.toMatch(/tasksHeaderWidth/);
    expect(SOURCE).not.toMatch(/headerResizeObs/);
    expect(SOURCE).not.toMatch(/headerButtonMode/);
    expect(SOURCE).not.toMatch(/resolveHeaderButtonMode/);
    expect(SOURCE).not.toMatch(/newSheetButtonLayout/);
  });

  it('confirms there is still no New Note button anywhere in this component (unchanged from before item 1)', () => {
    expect(SOURCE).not.toMatch(/class="add-task-btn"/);
    expect(SOURCE).not.toMatch(/>New Note</);
  });

  it('confirms the task-count header survives the removal (header-right is not gutted entirely)', () => {
    expect(SOURCE).toMatch(/class="header-right"/);
    expect(SOURCE).toMatch(/class="task-count"/);
  });
});
