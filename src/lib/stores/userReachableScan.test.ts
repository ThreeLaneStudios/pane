/**
 * userReachableScan.test.ts — #3043 item 2/3
 *
 * Unit tests for the scanner itself against SYNTHETIC in-memory fixtures
 * (not the real source tree — that's `userReachableViews.derived.test.ts`,
 * which is the test that actually closes Reed's WARN). These tests prove
 * the scanner's RULES hold in isolation: literal extraction, agent-code
 * exclusion, and — required by the task — that an unresolvable target
 * fails loudly, naming file and line, rather than being silently skipped.
 */
import { describe, it, expect } from 'vitest';
import {
  scanUserReachableViews,
  stripComments,
  UnresolvableViewTargetError,
  type ScanFile,
} from './userReachableScan';

describe('scanUserReachableViews — literal extraction', () => {
  it('finds a literal view in a navigateTo() call', () => {
    const files: ScanFile[] = [
      { path: 'Fake.svelte', source: `function onClick() {\n  navigateTo({ view: 'home' });\n}` },
    ];
    const { views, citations } = scanUserReachableViews(files);
    expect(views).toEqual(new Set(['home']));
    expect(citations.get('home')).toEqual([{ path: 'Fake.svelte', line: 2 }]);
  });

  it('finds a literal view in canvasState.update() too (the selectNote()-style helper path)', () => {
    const files: ScanFile[] = [
      {
        path: 'noteService.ts',
        source: `function selectNote() {\n  canvasState.update(state => ({ ...state, view: 'note_editor' }));\n}`,
      },
    ];
    const { views } = scanUserReachableViews(files);
    expect(views).toEqual(new Set(['note_editor']));
  });

  it('collects MULTIPLE citations for the same view across files — the mechanism a "is X genuinely helper-only" proof needs', () => {
    const files: ScanFile[] = [
      { path: 'A.svelte', source: `navigateTo({ view: 'settings' });` },
      { path: 'B.svelte', source: `navigateTo({ view: 'settings' });` },
    ];
    const { citations } = scanUserReachableViews(files);
    expect(citations.get('settings')).toEqual([
      { path: 'A.svelte', line: 1 },
      { path: 'B.svelte', line: 1 },
    ]);
  });

  it('a canvasState.set()/update() call with no `view` property at all is simply not a navigation — ignored, not an error', () => {
    const files: ScanFile[] = [
      { path: 'A.svelte', source: `canvasState.update(state => ({ ...state, data: {} }));` },
    ];
    const { views } = scanUserReachableViews(files);
    expect(views.size).toBe(0);
  });

  it('comment text mentioning navigateTo({view: \'x\'}) is NOT mistaken for a real call site', () => {
    const files: ScanFile[] = [
      {
        path: 'A.svelte',
        source: [
          '// old code used to call navigateTo({ view: \'dashboard\' }) here',
          '/* also mentions navigateTo({ view: \'task_list\' }) in a block comment */',
          'navigateTo({ view: \'home\' });',
        ].join('\n'),
      },
    ];
    const { views } = scanUserReachableViews(files);
    expect(views).toEqual(new Set(['home']));
  });
});

describe('scanUserReachableViews — excludes the agent\'s own navigation code', () => {
  it('excludes a navigateTo() call inside a listen(\'pane-navigate\', ...) callback (the AI backend\'s own event — zero user controls fire it)', () => {
    const files: ScanFile[] = [
      {
        path: 'ChatSidebar.svelte',
        source: [
          "const unlisten = await listen('pane-navigate', async (event) => {",
          "  const { view } = event.payload;",
          "  await navigateTo({ view: 'dashboard' });",
          "});",
        ].join('\n'),
      },
    ];
    const { views } = scanUserReachableViews(files);
    expect(views.size).toBe(0);
  });

  it('excludes navigateTo() calls inside handleNavigateFromStream (the AI chat-stream navigate dispatcher)', () => {
    const files: ScanFile[] = [
      {
        path: 'ChatSidebar.svelte',
        source: [
          'async function handleNavigateFromStream(nav) {',
          "  await navigateTo({ view: 'task_list' });",
          '}',
        ].join('\n'),
      },
    ];
    const { views } = scanUserReachableViews(files);
    expect(views.size).toBe(0);
  });

  it('excludes navigateTo() calls inside handleCanvasCommand (the dead AI canvas-command dispatcher)', () => {
    const files: ScanFile[] = [
      {
        path: 'ChatSidebar.svelte',
        source: [
          'async function handleCanvasCommand(cmd) {',
          "  await navigateTo({ view: cmd.view });",
          '}',
        ].join('\n'),
      },
    ];
    // Note: this fixture's view is also dynamic (cmd.view) — proves exclusion
    // happens BEFORE the unresolvable-target check would otherwise fire.
    expect(() => scanUserReachableViews(files)).not.toThrow();
    const { views } = scanUserReachableViews(files);
    expect(views.size).toBe(0);
  });

  // #3044 item 4: the `if (navView) { ... }` exclusion marker this test
  // used to pin is REMOVED — the free-text chat-navigation block it
  // matched (ChatSidebar.svelte's handleSendMessage) was deleted outright,
  // not merely gated, per Matt's ruling ("every chat message goes to the
  // agent"). A synthetic fixture shaped like that block is no longer
  // special-cased by the scanner at all: its `navView` target is a plain
  // variable, not a literal, so it now correctly throws
  // UnresolvableViewTargetError like any other unresolvable call site
  // would — proving the marker's removal, not just its absence.
  it('a navView-shaped block (the kind the old #3043 chat-shortcut exclusion used to carve out) is NO LONGER excluded — it fails loudly as an unresolvable target, same as any other dynamic view', () => {
    const files: ScanFile[] = [
      {
        path: 'ChatSidebar.svelte',
        source: [
          "const navMap: Record<string, string> = { 'go home': 'home' };",
          'let navView = navMap[navLower];',
          'if (navView) {',
          "  await navigateTo({ view: navView as CanvasViewType });",
          '}',
        ].join('\n'),
      },
    ];
    expect(() => scanUserReachableViews(files)).toThrow(UnresolvableViewTargetError);
  });

  it('a REAL user-control call site elsewhere in the SAME file as an excluded agent block is still found — exclusion is scoped to the block, not the whole file', () => {
    const files: ScanFile[] = [
      {
        path: 'ChatSidebar.svelte',
        source: [
          'async function handleNavigateFromStream(nav) {',
          "  await navigateTo({ view: 'task_list' });",
          '}',
          '',
          "const onOpenSettings = () => { navigateTo({ view: 'settings' }); };",
        ].join('\n'),
      },
    ];
    const { views } = scanUserReachableViews(files);
    expect(views).toEqual(new Set(['settings']));
  });
});

describe('scanUserReachableViews — unresolvable targets fail loudly (#3043 item 3)', () => {
  it('a navigateTo() call whose `view` is a variable (not a literal) throws, naming the file and line — never silently skipped', () => {
    const files: ScanFile[] = [
      {
        path: 'Weird.svelte',
        source: ['function f(view) {', '  navigateTo({ view: view });', '}'].join('\n'),
      },
    ];
    let thrown: unknown;
    try {
      scanUserReachableViews(files);
    } catch (e) {
      thrown = e;
    }
    expect(thrown).toBeInstanceOf(UnresolvableViewTargetError);
    expect((thrown as Error).message).toContain('Weird.svelte:2');
  });

  it('a navigateTo() call whose `view` is a ternary expression also throws, naming file and line', () => {
    const files: ScanFile[] = [
      {
        path: 'Weird2.svelte',
        source: [
          'function f(isKanban) {',
          "  navigateTo({ view: isKanban ? 'kanban' : 'notes_browser' });",
          '}',
        ].join('\n'),
      },
    ];
    expect(() => scanUserReachableViews(files)).toThrow(UnresolvableViewTargetError);
    try {
      scanUserReachableViews(files);
    } catch (e) {
      expect((e as Error).message).toContain('Weird2.svelte:2');
    }
  });

  it('an unresolvable target in ONE file still fails the whole scan even when other files are fine — no partial/silent success', () => {
    const files: ScanFile[] = [
      { path: 'Good.svelte', source: "navigateTo({ view: 'home' });" },
      { path: 'Bad.svelte', source: 'navigateTo({ view: someDynamicView });' },
    ];
    expect(() => scanUserReachableViews(files)).toThrow(/Bad\.svelte:1/);
  });
});

describe('stripComments', () => {
  it('blanks // and /* */ comments but preserves string contents and line count', () => {
    const src = "// a comment\nconst x = 'keep this'; /* block */\nconst y = 1;";
    const stripped = stripComments(src);
    expect(stripped).toContain("'keep this'");
    expect(stripped).not.toContain('a comment');
    expect(stripped).not.toContain('block');
    expect(stripped.split('\n').length).toBe(src.split('\n').length);
  });
});
