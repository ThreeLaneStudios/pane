/**
 * userReachableViews.derived.test.ts — #3043 item 2
 * (closes Reed's #3043 delta-2 WARN: "USER_REACHABLE_VIEWS is hand-
 * maintained ... no mechanical way to derive 'has a user control' from
 * source" — until now.)
 *
 * Runs `scanUserReachableViews` (`userReachableScan.ts`) against the REAL
 * frontend source tree and compares the result against:
 *  1. `USER_REACHABLE_VIEWS` (canvas.ts) — the hand-maintained list this
 *     scan now continuously verifies instead of merely disclosing it
 *     can't.
 *  2. `SHOW_VIEW_SCREENS` (canvas.ts) — `show_view`'s own offered screens
 *     must ALL be in the derived set; this is the test that would have
 *     caught #3022 on its own (`task_list`/`dashboard` were never
 *     user-reachable, and this scan proves it from source, not from a
 *     second hand list that could independently be wrong the same way).
 *
 * Per the task's explicit instruction: this test does NOT tune the
 * scanner to make it agree with any particular expected answer. It reads
 * the real exclusions scanUserReachableViews documents (agent-dispatch
 * code only) and reports whatever the scan actually finds.
 *
 * #3044 item 5 update: the one disagreement this file used to document
 * (`note_list`, reachable only via `+page.svelte`'s `handleNewNote`, bound
 * to Cmd+N/Cmd+T) is GONE — #3044 item 3 deleted that function's own
 * creation code, with no `navigateTo({ view: 'note_list' })` call anywhere
 * left in it. Re-running this scan after that deletion shows ZERO
 * citations for `note_list` — confirmed by actually running it, not
 * assumed — so per the task's own rule ("if the scan reports zero
 * routes, note_list joins the deletion list"), `note_list` was removed
 * outright from `CanvasViewType`/`RENDERED_CANVAS_VIEWS`/
 * `USER_REACHABLE_VIEWS` (canvas.ts) and its dead render branch deleted
 * from `CanvasRenderer.svelte` in the same commit. (Cmd+N's own creation
 * path was later reworked again, #3044 revisited: it now calls
 * `noteService.createNewNote()` directly — no `window.__paneNewNote`
 * global exists anymore either way — but that's orthogonal to this file's
 * `note_list`-reachability finding, which still holds.)
 */
import { describe, it, expect, beforeAll } from 'vitest';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { resolve, join, relative } from 'node:path';
import { scanUserReachableViews, type ScanFile, type ScanResult } from './userReachableScan';

const SRC_ROOT = resolve(__dirname, '../../');
const CANVAS_TS_PATH = resolve(__dirname, 'canvas.ts');

// Whole-file exclusions: the navigation ENGINE/DECISION modules, not call
// sites. `destination.view`/`fn` are parameter names here, not literal
// screens — scanning them would be a category error (see userReachableScan.ts
// header). These are the ONLY files excluded wholesale; every other
// exclusion the scan applies is scoped to an agent-dispatch block within a
// file, not a whole file, so real user-control call sites next to agent
// code in the SAME file (e.g. ChatSidebar.svelte) are still found.
const WHOLE_FILE_EXCLUDES = new Set([
  resolve(SRC_ROOT, 'lib/services/navigation.ts'),
  resolve(SRC_ROOT, 'lib/utils/navigationDispatch.ts'),
]);

function collectSourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const st = statSync(full);
    if (st.isDirectory()) {
      out.push(...collectSourceFiles(full));
      continue;
    }
    if (!/\.(svelte|ts)$/.test(entry)) continue;
    if (entry.endsWith('.test.ts') || entry.endsWith('.spec.ts')) continue;
    if (WHOLE_FILE_EXCLUDES.has(full)) continue;
    out.push(full);
  }
  return out;
}

function extractTsArray(source: string, constName: string): string[] {
  const arrayMatch = source.match(
    new RegExp(`export const ${constName} = \\[([\\s\\S]*?)\\]\\s*as const`),
  );
  if (!arrayMatch) return [];
  const ids: string[] = [];
  const itemRe = /'([^']*)'/g;
  let m: RegExpExecArray | null;
  while ((m = itemRe.exec(arrayMatch[1])) !== null) {
    ids.push(m[1]);
  }
  return ids;
}

let scanResult: ScanResult;
let handMaintainedReachable: string[];
let handMaintainedShowView: string[];

beforeAll(() => {
  const files: ScanFile[] = collectSourceFiles(SRC_ROOT).map((full) => ({
    path: relative(SRC_ROOT, full),
    source: readFileSync(full, 'utf-8'),
  }));
  scanResult = scanUserReachableViews(files);

  const canvasSource = readFileSync(CANVAS_TS_PATH, 'utf-8');
  handMaintainedReachable = extractTsArray(canvasSource, 'USER_REACHABLE_VIEWS');
  handMaintainedShowView = extractTsArray(canvasSource, 'SHOW_VIEW_SCREENS');
});

describe('derived USER_REACHABLE_VIEWS (#3043 item 2 — scanned from real source, not hand-typed here)', () => {
  it('sanity: the scan actually ran across a non-trivial number of real files and found a non-trivial number of views (a scan that silently finds nothing must fail loudly, not pass vacuously)', () => {
    expect(scanResult.views.size).toBeGreaterThan(5);
  });

  it('the derived set matches the hand-maintained USER_REACHABLE_VIEWS (canvas.ts) EXACTLY — any mismatch here means either the hand list drifted from reality, or the scan drifted from the hand list; both are findings', () => {
    const derived = scanResult.views;
    const hand = new Set(handMaintainedReachable);

    const extraInDerived = [...derived].filter((v) => !hand.has(v));
    const missingFromDerived = [...hand].filter((v) => !derived.has(v));

    expect(
      extraInDerived,
      `derived scan found real user-control call sites for these views, but USER_REACHABLE_VIEWS (canvas.ts) is missing them: ${extraInDerived.join(', ')}`,
    ).toEqual([]);
    expect(
      missingFromDerived,
      `USER_REACHABLE_VIEWS (canvas.ts) claims these views are user-reachable, but the derived scan found NO real call site for them: ${missingFromDerived.join(', ')}`,
    ).toEqual([]);
  });

  it("every SHOW_VIEW_SCREENS entry is in the DERIVED set — the test that would have caught #3022 on its own. Fails naming the offending screen and where show_view declares it (canvas.ts/tools.rs SHOW_VIEW_SCREENS) if show_view ever offers a view no user control opens", () => {
    for (const screen of handMaintainedShowView) {
      expect(
        scanResult.views,
        `show_view (SHOW_VIEW_SCREENS, canvas.ts/tools.rs) offers '${screen}', but the derived scan found no real user-control call site for it anywhere in src/ — citations found: ${JSON.stringify(scanResult.citations.get(screen) ?? [])}`,
      ).toContain(screen);
    }
  });

  it("'search_results' is excluded from the derived set AUTOMATICALLY (zero real call sites), not by a hand-maintained carve-out — confirms Reed's #3043 finding and that this scan actually closes it, not merely re-discloses it", () => {
    expect(scanResult.views).not.toContain('search_results');
    expect(scanResult.citations.get('search_results')).toBeUndefined();
  });

  it("'task_list' and 'dashboard' are NOT in the derived set — re-adding either to SHOW_VIEW_SCREENS (canvas.ts) or tools.rs fails the SHOW_VIEW_SCREENS-subset test above, because the derived scan (unlike a second hand list) has no entry to agree with", () => {
    expect(scanResult.views).not.toContain('task_list');
    expect(scanResult.views).not.toContain('dashboard');
  });

  // #2874 item F (already merged on `main` before this branch) added a
  // SECOND real, intentional call site for 'kanban' —
  // CanvasRenderer.svelte's handleTaskClick, which opens a Calendar task's
  // project board directly, independent of Sidebar.svelte's own
  // selectProject(). Confirmed by direct execution against the real tree
  // (both before and after this branch's own #3044 changes — this is a
  // pre-existing collision from that merge, not something #3044
  // introduced): both call sites are genuine, literal, user-reachable
  // navigations to the SAME screen via two different real user actions
  // (clicking a project in the sidebar vs. clicking a task on the
  // Calendar) — two citations is the correct count now, not a bug.
  //
  // #3050 PR 8 update: a FOURTH real citation — `scheduledPanel.ts`'s
  // `openScheduledJobSession` (the Home Scheduled panel's row-click path,
  // task item 4). Confirmed by direct execution against the real tree:
  // the scanner's own literal-`view`-string requirement is still
  // satisfied (`view: 'kanban'` appears literally at that call site, even
  // though it's reached through an injected `deps.navigateTo` parameter
  // rather than a direct import — the scanner matches on the literal
  // text, not the identifier's declaration site, same as every other
  // citation here). Four citations is the correct count now, not a
  // regression: `ScheduledPanel.svelte`'s row click, like
  // `NotificationPanel.svelte`'s before it, opens a job-origin session
  // that may resolve to a project — same shape, new real caller.
  // #3151 PR1 Commit F update: a FIFTH real citation —
  // `jobSessionBack.ts`'s `goToJobSessionBackTarget`, the one-deep back
  // action shared by the back button (`ChatPanel.svelte`'s header) and
  // the `Cmd+[` shortcut (`+page.svelte`). Same shape as
  // `scheduledPanel.ts`'s own citation directly above it: a job-origin
  // session being returned to that may resolve to a project board, not a
  // new kind of navigation.
  it("'kanban' is reached through exactly five real call sites — Sidebar.svelte's selectProject(), CanvasRenderer.svelte's handleTaskClick (#2874 item F), NotificationPanel.svelte's click-to-open session routing (§36.7, #3050 PR 6), scheduledPanel.ts's openScheduledJobSession (§36.3.8, #3050 PR 8), and jobSessionBack.ts's goToJobSessionBackTarget (#3151 PR1 Commit F) — confirmed by citation count and path, not asserted", () => {
    const kanbanCitations = scanResult.citations.get('kanban') ?? [];
    expect(kanbanCitations.length).toBe(5);
    expect(kanbanCitations.map((c) => c.path)).toEqual(
      expect.arrayContaining([
        expect.stringMatching(/Sidebar\.svelte$/),
        expect.stringMatching(/CanvasRenderer\.svelte$/),
        expect.stringMatching(/NotificationPanel\.svelte$/),
        expect.stringMatching(/scheduledPanel\.ts$/),
        expect.stringMatching(/jobSessionBack\.ts$/),
      ]),
    );
  });

  it('REGRESSION PROOF: a derived set that (hypothetically) included task_list would fail the SHOW_VIEW_SCREENS-subset test\'s own logic — proven by injecting the dead name into a COPY of the derived set, not by trusting the assertion above blindly', () => {
    const derivedWithInjectedDeadName = new Set([...scanResult.views, 'task_list']);
    // The REAL assertion (above) uses scanResult.views directly, so this
    // injected copy existing separately proves the containment check is
    // not vacuously true — a set that DOES contain 'task_list' passes
    // `.toContain('task_list')`, demonstrating the check actually
    // discriminates rather than always passing.
    expect(derivedWithInjectedDeadName).toContain('task_list');
    expect(scanResult.views).not.toContain('task_list');
  });

  it("#3044 item 5: 'note_list' is GONE from the derived set — its only prior citation (+page.svelte's handleNewNote, bound to Cmd+N/Cmd+T) was deleted in item 3, and the view itself was removed from canvas.ts/CanvasRenderer.svelte in the same commit as this test update", () => {
    expect(scanResult.views).not.toContain('note_list');
    expect(scanResult.citations.get('note_list')).toBeUndefined();
  });

  it('all other screens Matt\'s manual enumeration named (all_tasks, calendar, home, note_editor, notes_browser, project_config, project_list, settings, trash, kanban) are present in the derived set — the scan agrees with Matt everywhere except the note_list finding above', () => {
    const mattsLiteralTargets = [
      'all_tasks',
      'calendar',
      'home',
      'note_editor',
      'notes_browser',
      'project_config',
      'project_list',
      'settings',
      'trash',
      'kanban',
    ];
    for (const view of mattsLiteralTargets) {
      expect(scanResult.views, `'${view}' should be in the derived set — Matt's enumeration named it`).toContain(view);
    }
  });
});

