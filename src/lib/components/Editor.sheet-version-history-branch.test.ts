import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/**
 * #2339 PR 15b-beta — Editor.svelte version-history branch regression
 *
 * The project has no full Editor.svelte mount test harness (see
 * Editor.restore-markdown.test.ts's own header comment), so — like the
 * several other `*.test.ts` files in this directory that assert on the
 * component's own source text (AllTasksView.sheet-card-face.test.ts,
 * Onboarding.cardHeight.test.ts, pill-dropdown-position-contract.test.ts,
 * etc.) — this test statically inspects Editor.svelte's markup to lock in
 * the single most safety-critical invariant of the whole feature: a sheet
 * note's version-history UI must NEVER be able to call
 * `restore_note_version` (which would silently corrupt a sheet note by
 * writing a markdown snapshot's raw envelope JSON back as if it were the
 * note's markdown body), and a markdown note's version-history UI must
 * keep calling `restore_note_version` exactly as before this PR.
 *
 * The actual "sheet restore calls restore_sheet_snapshot and never
 * restore_note_version" behavioral proof lives in
 * ShVersionHistoryPanel.test.ts (mounted component, mocked `invoke`) —
 * this file instead locks in that Editor.svelte routes sheet notes to
 * THAT component at all, rather than ever reaching the markdown
 * `.vh-overlay` / `restoreFromVersion()` (which itself still hard-codes
 * `restore_note_version` and must keep doing so for non-sheet notes).
 */

const editorSource = readFileSync(
  join(__dirname, 'Editor.svelte'),
  'utf-8',
);

describe('Editor.svelte — version-history isSheet branch (source contract)', () => {
  it('branches the version-history overlay on isSheet', () => {
    expect(editorSource).toMatch(/\{#if showVersionHistory\}\s*\{#if isSheet\}/);
  });

  it('routes the sheet branch to ShVersionHistoryPanel, not the markdown .vh-overlay', () => {
    const match = editorSource.match(
      /\{#if showVersionHistory\}\s*\{#if isSheet\}([\s\S]*?)\{:else\}([\s\S]*?)\{\/if\}\s*\{\/if\}/,
    );
    expect(match).not.toBeNull();
    const [, sheetBranch, markdownBranch] = match!;

    // Sheet branch: mounts ShVersionHistoryPanel, contains no restore_note_version
    // call and no .vh-overlay markdown chrome.
    expect(sheetBranch).toContain('<ShVersionHistoryPanel');
    expect(sheetBranch).not.toContain('restore_note_version');
    expect(sheetBranch).not.toContain('vh-overlay');
    expect(sheetBranch).not.toContain('vh-preview-content');

    // Markdown (non-sheet) branch: unchanged — still the .vh-overlay chrome
    // calling restoreFromVersion(), which itself invokes restore_note_version.
    expect(markdownBranch).toContain('vh-overlay');
    expect(markdownBranch).toContain('restoreFromVersion');
    expect(markdownBranch).not.toContain('ShVersionHistoryPanel');
  });

  it('restoreFromVersion() (the markdown path) still hard-codes restore_note_version', () => {
    const fnMatch = editorSource.match(
      /async function restoreFromVersion\([\s\S]*?\n  \}/,
    );
    expect(fnMatch).not.toBeNull();
    expect(fnMatch![0]).toContain("invoke('restore_note_version'");
    // And, symmetrically, that function must never itself reach for the
    // sheet-only restore command — that would be just as wrong as the
    // inverse mistake this whole PR exists to prevent.
    expect(fnMatch![0]).not.toContain('restore_sheet_snapshot');
  });

  it('openVersionHistory() short-circuits for sheets instead of eagerly fetching note versions', () => {
    const fnMatch = editorSource.match(
      /async function openVersionHistory\([\s\S]*?\n  \}/,
    );
    expect(fnMatch).not.toBeNull();
    // The sheet short-circuit must appear BEFORE the get_note_versions call
    // (ShVersionHistoryPanel fetches its own list — this function must not
    // race it or fetch redundant markdown-version metadata for a sheet).
    const src = fnMatch![0];
    const sheetGuardIdx = src.indexOf('if (isSheet)');
    const fetchIdx = src.indexOf("invoke('get_note_versions'");
    expect(sheetGuardIdx).toBeGreaterThan(-1);
    expect(fetchIdx).toBeGreaterThan(-1);
    expect(sheetGuardIdx).toBeLessThan(fetchIdx);
  });

  it('passes reload() through to sheetGridRef so a restore refreshes the grid', () => {
    expect(editorSource).toContain('onRestored={() => { sheetGridRef?.reload(); }}');
  });
});
