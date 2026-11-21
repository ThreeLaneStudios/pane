/**
 * uxPolishBatch3120pr3.sourcePins.test.ts — #3120 PR 3 UX polish batch
 * (C10, C12 dropdown-height half, C15) — the three pure-CSS items in
 * Reed's PR 3 brief, which explicitly says a behavioral/DOM assertion is
 * NOT possible for CSS-only fixes like these (no computed-layout engine in
 * jsdom worth trusting for z-index stacking or real rendered contrast) —
 * "say so rather than inventing a DOM-measurement test that proves
 * nothing." Source-level assertions instead, same established pattern
 * `Settings.test.ts` already uses for this exact reason (see that file's
 * own header comment).
 *
 * A real visual confirmation (screenshot) is Gate 4d territory, covered by
 * Live test #3 at the end of the four-PR sprint, not a per-PR walkthrough
 * (every PR in this sprint carries `4d-skip: Matt`) — not attempted here.
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const pageSource = readFileSync(resolve(__dirname, '../../routes/+page.svelte'), 'utf-8');
const sheetSource = readFileSync(resolve(__dirname, './NewSessionSheet.svelte'), 'utf-8');
const panelSource = readFileSync(resolve(__dirname, './ScheduledPanel.svelte'), 'utf-8');
const notificationPanelSource = readFileSync(resolve(__dirname, './NotificationPanel.svelte'), 'utf-8');
const sidebarSource = readFileSync(resolve(__dirname, './Sidebar.svelte'), 'utf-8');

describe('C10 — New Session sheet header no longer clipped under a project toolbar', () => {
  /// Root cause (confirmed by reading, not a guess): `.main-content`
  /// (`.chat-panel`'s own sibling in `+page.svelte`) has no `position`/
  /// `z-index` of its own, so it never forms a stacking context — every
  /// z-index inside it (e.g. `NotesBrowser.svelte`'s sticky
  /// `.grid-folder-toolbar`, z-index: 5) compared DIRECTLY against
  /// `.chat-panel`'s old z-index: 1 and won, painting over the entire
  /// `.chat-panel` stacking context including `NewSessionSheet.svelte`'s
  /// fixed-position `.ns-sheet` nested deep inside it. Mutation: revert
  /// `.chat-panel`'s z-index back to 1 — reddens.
  it('.chat-panel z-index is raised above every in-project toolbar/dropdown z-index found in this tree (max 1000)', () => {
    const match = pageSource.match(/\.chat-panel\s*\{[^}]*z-index:\s*(\d+)/);
    expect(match, '.chat-panel must declare an explicit z-index').toBeTruthy();
    const zIndex = Number(match![1]);
    expect(zIndex).toBeGreaterThan(1000);
    // Regression guard: still well below the app's own top-level overlays
    // (QuickSwitcher/Templates/auth-loading, all z-index: 9999, true
    // siblings of `.app` rather than descendants of `.chat-panel`).
    expect(zIndex).toBeLessThan(9999);
  });
});

describe('C12 — New job dialog dropdowns no longer render too short to read', () => {
  /// Mutation: remove the `.inp:not(.ta) { min-height: ... }` rule —
  /// reddens (every <select>/<input> in the New schedule form loses its
  /// explicit minimum height, reverting to the cramped browser default).
  it('.inp (every <select>/<input> except the textarea) has an explicit min-height', () => {
    const match = panelSource.match(/\.inp:not\(\.ta\)\s*\{[^}]*min-height:\s*(\d+)px/);
    expect(match, '.inp:not(.ta) must declare an explicit min-height').toBeTruthy();
    expect(Number(match![1])).toBeGreaterThanOrEqual(36);
  });

  /// Root cause, confirmed by §8a sibling-parity check (#3128): the 3
  /// <select class="inp">s (Project/Agent/Frequency) never got
  /// `appearance: none`, so WKWebView's native select chrome ignored
  /// min-height above for the select's own rendered box. Mutation
  /// (actually run — see report): remove the `select.inp { appearance:
  /// none; ... }` rule — reddens (declaration absent).
  it('select.inp declares appearance: none (and -webkit-appearance: none)', () => {
    const match = panelSource.match(/select\.inp\s*\{[^}]*\}/);
    expect(match, 'select.inp rule must exist').toBeTruthy();
    expect(match![0]).toMatch(/appearance:\s*none/);
    expect(match![0]).toMatch(/-webkit-appearance:\s*none/);
  });
});

describe('C21 — widened ruling: the New job modal\'s own header, not just the panel strings', () => {
  /// PR 3's own regression test asserted `container.textContent` doesn't
  /// contain 'New schedule' but NEVER opened the modal (`{#if showModal}`),
  /// so it passed vacuously — it never had the chance to see the h3
  /// either way. #3128 widens the ruling to cover the modal header
  /// explicitly. **Widened again, #3134 C26:** the header is now a
  /// ternary (`editingJobId ? 'Edit job' : 'New job'`) so Edit mode gets
  /// its own header — this test's literal `<h3>New job</h3>` match no
  /// longer applies verbatim; updated to pin the ternary's own source
  /// text instead (still a plain substring check — ScheduledPanel.svelte's
  /// own component test, `ScheduledPanel.editJob.test.ts`, covers the
  /// REAL rendered DOM text in both modes). Mutation (actually run — see
  /// report): revert the ternary back to a literal `<h3>New schedule</h3>`
  /// — reddens (the `New schedule` guard below catches it; the
  /// `editingJobId ?` fragment check also reddens since the ternary is
  /// gone entirely).
  it('the modal header is the New-job/Edit-job ternary, never reverted to "New schedule"', () => {
    expect(panelSource).toContain("{editingJobId ? 'Edit job' : 'New job'}");
    expect(panelSource).not.toMatch(/<h3>New schedule<\/h3>/);
  });
});

describe('C12 — preset-prompt pass: Check-in\u2019s brief stays pinned to the spec\u2019s literal text', () => {
  /// Per C12's own instruction ("pass over the preset prompts while in
  /// there") and Reed's brief's "preset prompt text matches spec copy"
  /// test description: reviewed both non-Custom presets against
  /// `docs/spec/runtime.md`. Finding, not a guess: Check-in's brief IS the
  /// spec's literal pinned text (`runtime.md` \u00a736.3.2's `heartbeat` row);
  /// Weekly review's brief is explicitly flagged in `runtime.md` \u00a736.3.8 as
  /// "composed for this build, not sheet-sourced verbatim" — no drift to
  /// reconcile there. No wording change was warranted on either; this test
  /// is the regression guard that proves Check-in's text stays pinned to
  /// the spec going forward. Mutation: change one character of
  /// `HEARTBEAT_DEFAULT_BRIEF` — reddens.
  it('HEARTBEAT_DEFAULT_BRIEF matches runtime.md \u00a736.3.2\u2019s heartbeat row verbatim', () => {
    const SPEC_TEXT =
      "Review this project's open tasks and recent notes. If anything needs doing or deciding, do it or tell me. If nothing does, reply 'Nothing needed.' and stop.";
    const match = panelSource.match(/const HEARTBEAT_DEFAULT_BRIEF =\s*\n?\s*"([^"]+)"/);
    expect(match, 'HEARTBEAT_DEFAULT_BRIEF literal must be found').toBeTruthy();
    expect(match![1]).toBe(SPEC_TEXT);
  });
});

describe('C29 (#3128, dark-calendar half) — Once picker\'s date/time popups render dark', () => {
  /// Mutation (actually run — see report): remove the `color-scheme:
  /// dark;` declaration from the new rule — reddens (declaration absent).
  it('the Once picker\'s datetime-local/time inputs declare color-scheme: dark', () => {
    const match = panelSource.match(/\.when input\[type="datetime-local"\][^{]*\{[^}]*\}/);
    expect(match, 'the datetime-local/time rule must exist').toBeTruthy();
    expect(match![0]).toMatch(/color-scheme:\s*dark/);
  });
});

describe('C25 (#3128) — Notifications panel no longer paints behind .chat-panel', () => {
  /// Root cause: C10 (#3125) raised `.chat-panel`'s own z-index to 1001
  /// to escape in-project toolbar stacking — but `.nt-pop`/`.panel-overlay`
  /// (NotificationPanel's own root + the Sidebar overlay that gates it)
  /// were left at their old 30/29, so they now paint BEHIND `.chat-panel`.
  /// Mutation (actually run — see report): revert both back to
  /// z-index: 30 / z-index: 29 — reddens (30/29 ≷ 1001).
  it('.nt-pop and .panel-overlay are both raised strictly above .chat-panel\'s 1001', () => {
    const chatPanelMatch = pageSource.match(/\.chat-panel\s*\{[^}]*z-index:\s*(\d+)/);
    expect(chatPanelMatch, '.chat-panel must declare an explicit z-index').toBeTruthy();
    const chatPanelZ = Number(chatPanelMatch![1]);

    const ntPopMatch = notificationPanelSource.match(/\.nt-pop\s*\{[^}]*z-index:\s*(\d+)/);
    expect(ntPopMatch, '.nt-pop must declare an explicit z-index').toBeTruthy();
    expect(Number(ntPopMatch![1])).toBeGreaterThan(chatPanelZ);

    const overlayMatch = sidebarSource.match(/\.panel-overlay\s*\{[^}]*z-index:\s*(\d+)/);
    expect(overlayMatch, '.panel-overlay must declare an explicit z-index').toBeTruthy();
    expect(Number(overlayMatch![1])).toBeGreaterThan(chatPanelZ);
  });
});

describe('C15 — New Session sheet: highlighted captain row\u2019s model subtitle is readable on teal', () => {
  /// Root cause (confirmed by hex comparison, not a guess): `--fg-4`
  /// (`#444444` in the dark theme) is readable on the plain dark surface
  /// it's meant for, but the selected row's background lightens to the
  /// teal-tinted `--accent-soft` overlay, shrinking contrast further.
  /// `--fg-2` (`#A0A0A0`) is the next brighter token this same file
  /// already uses for secondary-but-legible text. Mutation: remove the
  /// `.ns-ag--on .ns-ag-t code` override — reddens (rule absent).
  it('.ns-ag--on .ns-ag-t code overrides to a higher-contrast token than the unselected --fg-4', () => {
    expect(sheetSource).toMatch(/\.ns-ag--on \.ns-ag-t code\s*\{\s*color:\s*var\(--fg-2\);?\s*\}/);
  });
});
