/**
 * Settings.templateConfirmDialog.test.ts — #2874 item G follow-up:
 * `Settings.svelte`'s template-delete hand-rolled confirm modal (\u00a732.4)
 * converted to the shared `ConfirmDialog.svelte`, found during the
 * required "search for other hand-rolled confirm modals" sweep for this PR
 * (see PR body for the full classification list).
 *
 * `Settings.svelte` has no existing render-test harness in this repo (no
 * `*.test.ts` file anywhere mounts the real component — it is a ~5000-line
 * file with heavy Tauri/update-checker/auth-store dependencies). Building
 * one from scratch is disproportionate for this secondary, opportunistic
 * conversion found alongside the PR's three required items (F/G/Kanban),
 * which already have full component-render tests
 * (Trash.confirmDialog.test.ts, CanvasRenderer.taskclick-completion.test.ts,
 * KanbanBoard.collapse-persist.test.ts). This file instead uses the same
 * source-text-extraction pattern already established in this directory for
 * hard-to-mount logic (see KanbanBoard.new-sheet-button-polish.test.ts,
 * KanbanBoard.folder-assignment.test.ts) as a named, honest limitation —
 * not a substitute for a real render test, a documented one.
 *
 * Fails against the pre-fix code: the old block rendered a
 * `.modal-overlay`/`.modal-content` pair with hard-coded Cancel/Delete
 * buttons and a `console.error`-only catch with no error prop of any kind.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'fs';
import { resolve } from 'path';

const COMPONENT_PATH = resolve(__dirname, 'Settings.svelte');
const SOURCE = readFileSync(COMPONENT_PATH, 'utf-8');

function extractBlock(marker: string): string {
  const start = SOURCE.indexOf(marker);
  expect(start).toBeGreaterThan(-1);
  const end = SOURCE.indexOf('{/if}', start);
  expect(end).toBeGreaterThan(start);
  return SOURCE.slice(start, end + '{/if}'.length);
}

describe('Settings.svelte template-delete confirm (#2874 item G)', () => {
  it('imports the shared ConfirmDialog component', () => {
    expect(SOURCE).toMatch(/import ConfirmDialog from '\.\/ConfirmDialog\.svelte';/);
  });

  it('the template-delete block renders <ConfirmDialog>, not a hand-rolled .modal-overlay/.modal-content pair', () => {
    const block = extractBlock('{#if tplDeleteConfirmId}');
    expect(block).toMatch(/<ConfirmDialog/);
    expect(block).not.toMatch(/class="modal-overlay"/);
    expect(block).not.toMatch(/class="modal-content"/);
    expect(block).toMatch(/error={tplDeleteConfirmError}/);
    expect(block).toMatch(/onConfirm={confirmDeleteTemplate}/);
    expect(block).toMatch(/onCancel={cancelDeleteTemplate}/);
  });

  it('confirmDeleteTemplate surfaces a failure into tplDeleteConfirmError instead of only console.error (dialog stays open, error visible)', () => {
    const fnStart = SOURCE.indexOf('async function confirmDeleteTemplate');
    expect(fnStart).toBeGreaterThan(-1);
    const fnEnd = SOURCE.indexOf('\n  }\n', fnStart);
    const fnBody = SOURCE.slice(fnStart, fnEnd === -1 ? undefined : fnEnd);
    expect(fnBody).toMatch(/catch \(e\) \{/);
    expect(fnBody).toMatch(/tplDeleteConfirmError = /);
  });
});
