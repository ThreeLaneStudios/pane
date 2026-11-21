// ShStylePanel.test.ts — component tests for the Sheets cell-styling
// picker (#2339 PR 15d-beta).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import ShStylePanel from './ShStylePanel.svelte';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener — same patch as every
// other Sheets popover's own tests (ShSheetMenu.test.ts, ShCellContextMenu
// .test.ts).
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
  (globalThis.window as any).removeEventListener = () => {};
}

// #2874 item 16 Part B: the anchorEl-exclusion guard tests below need
// `handleWindowClick` (Svelte's `<svelte:window onclick>` listener) to
// ACTUALLY fire in response to `window.dispatchEvent(...)` — the shared
// no-op `addEventListener`/`dispatchEvent` patch above (same one every
// other Sheets popover test uses) deliberately makes that a no-op, since
// no other popover test in this codebase dispatches a real window click
// (see ShHeaderMenu.test.ts's own comment on this exact limitation).
// This file overrides all three with a genuine minimal pub/sub so the
// guard logic itself — not just the component's structural rendering —
// gets exercised end-to-end. Scoped to this file only; every sibling
// popover's own tests keep the shared no-op behavior unchanged.
const windowListeners: Record<string, Array<(e: Event) => void>> = {};
(globalThis.window as any).addEventListener = (type: string, fn: (e: Event) => void) => {
  (windowListeners[type] ||= []).push(fn);
};
(globalThis.window as any).removeEventListener = (type: string, fn: (e: Event) => void) => {
  windowListeners[type] = (windowListeners[type] || []).filter((f) => f !== fn);
};
(globalThis.window as any).dispatchEvent = (evt: Event) => {
  for (const fn of windowListeners[evt.type] || []) fn(evt);
  return true;
};

function baseProps(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    currentBg: null,
    currentFg: null,
    currentBorder: null,
    onApplyFill: vi.fn(),
    onApplyInk: vi.fn(),
    onApplyBorder: vi.fn(),
    onApplyBorderSide: vi.fn(),
    onApplyBorderWeight: vi.fn(),
    onApplyBorderDashed: vi.fn(),
    onClearStyling: vi.fn(),
    onClose: vi.fn(),
    ...overrides,
  };
}

describe('ShStylePanel — structure', () => {
  it('renders exactly 7 fill swatches', () => {
    render(ShStylePanel, { props: baseProps() });
    const names = ['none', 'slate', 'cyan', 'green', 'amber', 'red', 'violet'];
    for (const n of names) {
      expect(screen.getByTestId(`style-panel-fill-${n}`)).toBeTruthy();
    }
  });

  it('renders exactly 7 ink swatches', () => {
    render(ShStylePanel, { props: baseProps() });
    const names = ['default', 'muted', 'cyan', 'green', 'amber', 'red', 'violet'];
    for (const n of names) {
      expect(screen.getByTestId(`style-panel-ink-${n}`)).toBeTruthy();
    }
  });

  it('renders exactly 6 border side segments', () => {
    render(ShStylePanel, { props: baseProps() });
    const sides = ['none', 'all', 'top', 'bottom', 'left', 'right'];
    for (const s of sides) {
      expect(screen.getByTestId(`style-panel-border-side-${s}`)).toBeTruthy();
    }
  });

  it('renders 4 weight segments (1px, 2px, 3px, Dashed)', () => {
    render(ShStylePanel, { props: baseProps() });
    expect(screen.getByTestId('style-panel-border-weight-1')).toBeTruthy();
    expect(screen.getByTestId('style-panel-border-weight-2')).toBeTruthy();
    expect(screen.getByTestId('style-panel-border-weight-3')).toBeTruthy();
    expect(screen.getByTestId('style-panel-border-dashed')).toBeTruthy();
  });

  it('renders the Clear styling button', () => {
    render(ShStylePanel, { props: baseProps() });
    const btn = screen.getByTestId('style-panel-clear');
    expect(btn.textContent).toContain('Clear styling');
  });

  it('the "none" fill swatch shows an X icon', () => {
    render(ShStylePanel, { props: baseProps() });
    const btn = screen.getByTestId('style-panel-fill-none');
    expect(btn.textContent).toContain('✕');
    expect(btn.className).toContain('sh-sw__b--none');
  });

  it('ink swatches render the "A" glyph in their own color', () => {
    render(ShStylePanel, { props: baseProps() });
    const btn = screen.getByTestId('style-panel-ink-cyan') as HTMLElement;
    expect(btn.textContent).toContain('A');
    expect(btn.style.color).toBe('rgb(0, 228, 228)');
  });
});

describe('ShStylePanel — fill application', () => {
  it('clicking a fill swatch fires onApplyFill with the correct color', async () => {
    const onApplyFill = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyFill }) });
    await fireEvent.click(screen.getByTestId('style-panel-fill-cyan'));
    expect(onApplyFill).toHaveBeenCalledWith('rgba(0,228,228,0.14)');
  });

  it('clicking the "none" swatch fires onApplyFill with "transparent"', async () => {
    const onApplyFill = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyFill }) });
    await fireEvent.click(screen.getByTestId('style-panel-fill-none'));
    expect(onApplyFill).toHaveBeenCalledWith('transparent');
  });

  it('does not close the panel when a fill swatch is clicked', async () => {
    const onClose = vi.fn();
    render(ShStylePanel, { props: baseProps({ onClose }) });
    await fireEvent.click(screen.getByTestId('style-panel-fill-red'));
    expect(onClose).not.toHaveBeenCalled();
  });
});

describe('ShStylePanel — ink application', () => {
  it('clicking an ink swatch fires onApplyInk with the correct color', async () => {
    const onApplyInk = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyInk }) });
    await fireEvent.click(screen.getByTestId('style-panel-ink-red'));
    expect(onApplyInk).toHaveBeenCalledWith('#EF4444');
  });
});

// #2874 item 29 — border sides became independent additive toggles.
// Top/Btm/Left/Rgt each toggle membership in a `sides` set and re-fire
// `onApplyBorder` with the FULL updated set on every click (additive, not
// replace-on-click as the old single-select model did); "All"/"None" stay
// absolute per Matt's ruling — always force all four on/off regardless of
// current toggle state.
describe('ShStylePanel — border application (#2874 item 29 toggles)', () => {
  // #2874 item 59 (Matt's ruling, 2026-10-07): an ORDINARY individual-
  // side click (not "All"/"None", not the one-time redraw leaving "All"
  // mode) now fires `onApplyBorderSide({side, on, w, dashed})` — a delta
  // naming the ONE side clicked and the direction — instead of folding
  // back into `onApplyBorder`'s full `sides` array. `onApplyBorder`
  // stays reserved for the three genuinely absolute/full-snapshot cases
  // (see the tests below this group for those).
  it('clicking a border side fires onApplyBorderSide with {side, on:true} — NOT onApplyBorder', async () => {
    const onApplyBorder = vi.fn();
    const onApplyBorderSide = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorder, onApplyBorderSide }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    expect(onApplyBorderSide).toHaveBeenCalledWith({ side: 'top', on: true, w: 1, dashed: false });
    expect(onApplyBorder).not.toHaveBeenCalled();
  });

  it('clicking Top then Left fires onApplyBorderSide twice, once per side, each on:true (additive, Top not discarded)', async () => {
    const onApplyBorderSide = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorderSide }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    expect(onApplyBorderSide).toHaveBeenCalledTimes(2);
    expect(onApplyBorderSide).toHaveBeenNthCalledWith(1, { side: 'top', on: true, w: 1, dashed: false });
    expect(onApplyBorderSide).toHaveBeenNthCalledWith(2, { side: 'left', on: true, w: 1, dashed: false });
  });

  it('clicking Top twice fires onApplyBorderSide with on:false the second time (toggle-off) — Left is never mentioned, since it was never clicked', async () => {
    const onApplyBorderSide = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorderSide }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top')); // toggle top OFF
    expect(onApplyBorderSide).toHaveBeenLastCalledWith({ side: 'top', on: false, w: 1, dashed: false });
  });

  it('"All" always sets all four sides regardless of current toggle state', async () => {
    const onApplyBorder = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorder }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-all'));
    const lastCall = onApplyBorder.mock.calls.at(-1)![0];
    expect(lastCall.sides.sort()).toEqual(['bottom', 'left', 'right', 'top']);
  });

  it('"All" clicked again when all four are already active still sets all four (not a toggle-off)', async () => {
    const onApplyBorder = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorder }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-all'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-all'));
    const lastCall = onApplyBorder.mock.calls.at(-1)![0];
    expect(lastCall.sides.sort()).toEqual(['bottom', 'left', 'right', 'top']);
  });

  it('"None" always clears all sides regardless of current toggle state', async () => {
    const onApplyBorder = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorder }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-none'));
    expect(onApplyBorder).toHaveBeenLastCalledWith(expect.objectContaining({ sides: [] }));
  });

  // #2874 item 53 (Matt's ruling, 2026-10-04): the border panel tracks
  // TWO MODES — 'all' and 'sides' — as a tagged value, and FOUR
  // INDIVIDUAL CLICKS must never collapse into 'all' mode, even though
  // the resulting side list is identical to what "All" would produce.
  // This is the component-level half of item 53's test (the write-path
  // half — does SheetGrid actually fill inner lines or just draw an
  // outline — is covered in SheetGrid.test.ts, since `ShStylePanel`
  // itself has no concept of inner/outer cells).
  it('clicking Top, Bottom, Left, Right one at a time fires onApplyBorderSide for EACH click (never onApplyBorder) and leaves "All" dark (#2874 item 53’s outline ruling + item 59’s delta ruling, combined)', async () => {
    const onApplyBorder = vi.fn();
    const onApplyBorderSide = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorder, onApplyBorderSide }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-bottom'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await fireEvent.click(screen.getByTestId('style-panel-border-side-right'));
    // BEFORE the item 59 rebuild fix (and before THIS fix): the 4th
    // click fired `onApplyBorder` with a full `mode:'sides'` snapshot
    // array of all four sides. AFTER this fix: each click is its own
    // delta via `onApplyBorderSide`, naming just the one side clicked —
    // `onApplyBorder` (the All/None/leaving-All snapshot path) never
    // fires for this bare toggle sequence at all.
    expect(onApplyBorder).not.toHaveBeenCalled();
    expect(onApplyBorderSide).toHaveBeenCalledTimes(4);
    expect(onApplyBorderSide.mock.calls.map((c) => (c[0] as { side: string }).side)).toEqual(['top', 'bottom', 'left', 'right']);
    for (const call of onApplyBorderSide.mock.calls) expect((call[0] as { on: boolean }).on).toBe(true);
    // "All" never lights for four individually-toggled sides, mode
    // tracking stays internal to the panel (item 53, unchanged).
    expect(screen.getByTestId('style-panel-border-side-all').className).not.toContain('is-on');
  });

  it('clicking "All" reports mode:"all" and lights the All segment (#2874 item 53)', async () => {
    const onApplyBorder = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorder }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-all'));
    const lastCall = onApplyBorder.mock.calls.at(-1)![0];
    expect(lastCall.mode).toBe('all');
    expect(lastCall.sides.sort()).toEqual(['bottom', 'left', 'right', 'top']);
    expect(screen.getByTestId('style-panel-border-side-all').className).toContain('is-on');
  });

  it('toggling a side after "All" switches mode back to "sides" and darkens "All" (#2874 item 53)', async () => {
    const onApplyBorder = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorder }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-all'));
    expect(screen.getByTestId('style-panel-border-side-all').className).toContain('is-on');
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top')); // toggle Top off from All mode
    const lastCall = onApplyBorder.mock.calls.at(-1)![0];
    // BEFORE this fix: there was no 'all' mode to leave — clicking Top
    // here just toggled membership in the same bare Set "All" clicking
    // had already populated, so "All" stayed lit (`sides.length` was
    // still being read, and toggling Top off would drop it to 3 — but
    // the underlying bug this test targets is that a fresh All-click
    // followed by re-toggling all four back on could never be told
    // apart from a real "All" state at all). AFTER: leaving 'all' mode
    // via any individual click is explicit and immediate.
    expect(lastCall.mode).toBe('sides');
    expect(lastCall.sides.sort()).toEqual(['bottom', 'left', 'right']);
    expect(screen.getByTestId('style-panel-border-side-all').className).not.toContain('is-on');
  });

  // Delta-audit fix (2026-09-24, docs/audits/2026-09-24-pr-2963-item45.md
  // item 2): weight/dashed clicks no longer route through the shared-value
  // `onApplyBorder` — they fire their OWN per-attribute callbacks
  // (`onApplyBorderWeight`/`onApplyBorderDashed`), carrying ONLY the
  // attribute actually clicked, so `SheetGrid.svelte` can preserve each
  // cell's own value for the attribute NOT clicked instead of overwriting
  // it with this panel's shared (possibly-indeterminate) state.
  it('choosing a side then a weight fires onApplyBorderWeight with just w — NOT onApplyBorder', async () => {
    const onApplyBorder = vi.fn();
    const onApplyBorderWeight = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorder, onApplyBorderWeight }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    await fireEvent.click(screen.getByTestId('style-panel-border-weight-3'));
    expect(onApplyBorderWeight).toHaveBeenLastCalledWith({ sides: ['top'], w: 3 });
    // The side toggle itself (#2874 item 59) fires `onApplyBorderSide`,
    // not `onApplyBorder` — and the WEIGHT click must not fire
    // `onApplyBorder` either, with a shared spec or otherwise.
    expect(onApplyBorder).not.toHaveBeenCalled();
  });

  it('toggling dashed after choosing a side fires onApplyBorderDashed with just dashed — NOT onApplyBorder', async () => {
    const onApplyBorder = vi.fn();
    const onApplyBorderDashed = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorder, onApplyBorderDashed }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-left'));
    await fireEvent.click(screen.getByTestId('style-panel-border-dashed'));
    expect(onApplyBorderDashed).toHaveBeenLastCalledWith({ sides: ['left'], dashed: true });
    expect(onApplyBorder).not.toHaveBeenCalled();
  });

  it('clicking a weight before any side is chosen does not fire onApplyBorderWeight (nothing to apply to)', async () => {
    const onApplyBorderWeight = vi.fn();
    render(ShStylePanel, { props: baseProps({ onApplyBorderWeight }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-weight-2'));
    expect(onApplyBorderWeight).not.toHaveBeenCalled();
  });

  it('a weight click fires immediately even while dashed is still indeterminate (no dead click, no guard on this axis)', async () => {
    // Matt's ruling: "No overwrites, no dead clicks." A weight click no
    // longer needs to wait for the dashed axis to resolve, because it no
    // longer touches the dashed axis at all.
    const onApplyBorderWeight = vi.fn();
    render(ShStylePanel, {
      props: baseProps({
        onApplyBorderWeight,
        // Mixed cross-side: sides present, dashed indeterminate (null).
        currentBorder: { sides: ['top', 'left'], w: null, dashed: null },
      }),
    });
    await fireEvent.click(screen.getByTestId('style-panel-border-weight-2'));
    expect(onApplyBorderWeight).toHaveBeenCalledWith({ sides: ['top', 'left'], w: 2 });
  });

  it('a dashed click fires immediately even while weight is still indeterminate (this is the exact gap Reed\'s audit reproduced live)', async () => {
    const onApplyBorderDashed = vi.fn();
    render(ShStylePanel, {
      props: baseProps({
        onApplyBorderDashed,
        currentBorder: { sides: ['top'], w: null, dashed: false },
      }),
    });
    await fireEvent.click(screen.getByTestId('style-panel-border-dashed'));
    expect(onApplyBorderDashed).toHaveBeenCalledWith({ sides: ['top'], dashed: true });
  });
});

describe('ShStylePanel — unanimity / is-on highlighting', () => {
  it('highlights the fill swatch matching currentBg', () => {
    render(ShStylePanel, { props: baseProps({ currentBg: '#232327' }) });
    expect(screen.getByTestId('style-panel-fill-slate').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-fill-cyan').className).not.toContain('is-on');
  });

  it('highlights no fill swatch when currentBg is null (heterogeneous selection)', () => {
    render(ShStylePanel, { props: baseProps({ currentBg: null }) });
    const names = ['none', 'slate', 'cyan', 'green', 'amber', 'red', 'violet'];
    for (const n of names) {
      expect(screen.getByTestId(`style-panel-fill-${n}`).className).not.toContain('is-on');
    }
  });

  it('highlights the ink swatch matching currentFg', () => {
    render(ShStylePanel, { props: baseProps({ currentFg: '#22C55E' }) });
    expect(screen.getByTestId('style-panel-ink-green').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-ink-default').className).not.toContain('is-on');
  });

  it('highlights no ink swatch when currentFg is null', () => {
    render(ShStylePanel, { props: baseProps({ currentFg: null }) });
    const names = ['default', 'muted', 'cyan', 'green', 'amber', 'red', 'violet'];
    for (const n of names) {
      expect(screen.getByTestId(`style-panel-ink-${n}`).className).not.toContain('is-on');
    }
  });

  it('seeds the border side/weight from currentBorder (unanimity result)', () => {
    render(ShStylePanel, { props: baseProps({ currentBorder: { sides: ['bottom'], w: 2, dashed: false } }) });
    expect(screen.getByTestId('style-panel-border-side-bottom').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).toContain('is-on');
  });

  it('seeds MULTIPLE unanimous sides at once (#2874 item 29) — Top and Left both light up', () => {
    render(ShStylePanel, { props: baseProps({ currentBorder: { sides: ['top', 'left'], w: 1, dashed: false } }) });
    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-left').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-bottom').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-right').className).not.toContain('is-on');
  });

  it('mixed weights across unanimous sides show NO active weight button (Matt\'s ruling #1, option B)', () => {
    // Top unanimous at weight 2, Left unanimous at weight 1 — SheetGrid's
    // panelBorder derivation reports both sides but w:null since they
    // disagree.
    render(ShStylePanel, { props: baseProps({ currentBorder: { sides: ['top', 'left'], w: null, dashed: false } }) });
    expect(screen.getByTestId('style-panel-border-side-top').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-side-left').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-1').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-3').className).not.toContain('is-on');
  });

  it('mixed dashed across unanimous sides shows NO active dashed button (Matt\'s ruling #1, option B)', () => {
    render(ShStylePanel, { props: baseProps({ currentBorder: { sides: ['top', 'left'], w: 1, dashed: null } }) });
    expect(screen.getByTestId('style-panel-border-dashed').className).not.toContain('is-on');
  });

  // #2874 item 53 (Matt's ruling, 2026-10-04): "All" lighting is now
  // gated on the tagged `mode` SheetGrid seeds it with, never on
  // `sides.length === 4` alone — those two can disagree (an outline
  // shape spread across a multi-cell selection can report all four
  // sides as "active" via the per-side unanimity loop above, without
  // ANY single cell having all four at once). BEFORE this fix: "All"
  // had no `mode` concept at all and lit purely off `sides.length ===
  // 4`, so the two cases below were indistinguishable.
  it('"All" segment lights up when mode is "all" (#2874 item 53)', () => {
    render(ShStylePanel, { props: baseProps({ currentBorder: { sides: ['top', 'bottom', 'left', 'right'], w: 1, dashed: false, mode: 'all' } }) });
    expect(screen.getByTestId('style-panel-border-side-all').className).toContain('is-on');
  });

  it('"All" segment stays DARK when mode is "sides" even though all four sides are individually present (#2874 item 53 — the exact case item 29 used to collapse into "All")', () => {
    render(ShStylePanel, { props: baseProps({ currentBorder: { sides: ['top', 'bottom', 'left', 'right'], w: 1, dashed: false, mode: 'sides' } }) });
    // BEFORE this fix: this asserted `.toContain('is-on')` (same as the
    // 'all'-mode case above, since both had `sides.length === 4`).
    // AFTER this fix: individual-sides mode never lights "All",
    // regardless of how many sides are set.
    expect(screen.getByTestId('style-panel-border-side-all').className).not.toContain('is-on');
  });

  it('"None" segment lights up when currentBorder.sides is empty', () => {
    render(ShStylePanel, { props: baseProps({ currentBorder: { sides: [], w: null, dashed: null } }) });
    expect(screen.getByTestId('style-panel-border-side-none').className).toContain('is-on');
  });

  // #2874 item 54 (Matt's ruling, 2026-10-02): "on a borderless selection
  // only None lights; no weight button does." Before this fix,
  // `borderWeight`'s seeded `1` default (and `borderDashed`'s seeded
  // `false`) lit the weight-1/dashed buttons' `is-on` class purely
  // because the SEEDED value happened to equal the button's own value —
  // regardless of whether any side was actually active. The seed is the
  // ACTIVE VALUE (needed so a side click still applies immediately at a
  // sane default), not a LIT INDICATOR — those are now gated separately
  // on `activeSides.size > 0`.
  it('defaults border side to "none" (lit) when currentBorder is null, but seeds weight 1 / dashed false as the underlying (unlit) default', () => {
    render(ShStylePanel, { props: baseProps({ currentBorder: null }) });
    expect(screen.getByTestId('style-panel-border-side-none').className).toContain('is-on');
    // BEFORE this fix: weight-1 (and dashed) carried `is-on` here too —
    // the exact #2874 item 54 bug ("only None lights" was violated).
    // AFTER this fix: neither lights; only "None" does.
    expect(screen.getByTestId('style-panel-border-weight-1').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-2').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-3').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-dashed').className).not.toContain('is-on');
  });

  it('a borderless selection reported via currentBorder.sides: [] (not just the null-prop case) also lights no weight/dashed button', () => {
    render(ShStylePanel, { props: baseProps({ currentBorder: { sides: [], w: 1, dashed: false } }) });
    expect(screen.getByTestId('style-panel-border-side-none').className).toContain('is-on');
    expect(screen.getByTestId('style-panel-border-weight-1').className).not.toContain('is-on');
    expect(screen.getByTestId('style-panel-border-dashed').className).not.toContain('is-on');
  });

  it('clicking a side on a plain borderless cell applies immediately at the seeded 1px/solid default via onApplyBorderSide (#2874 item 54’s seed-not-cleared ruling + item 59’s delta ruling)', async () => {
    const onApplyBorderSide = vi.fn();
    render(ShStylePanel, { props: baseProps({ currentBorder: null, onApplyBorderSide }) });
    await fireEvent.click(screen.getByTestId('style-panel-border-side-top'));
    // The apply fires immediately using the seeded underlying default —
    // unchanged by this fix, since only the `is-on` CSS class was gated,
    // never `borderWeight`/`borderDashed` themselves. A fix that cleared
    // the seed instead of gating the class would make this a no-op or
    // require a weight click first — both are explicitly wrong per
    // Matt's ruling. Carried via `onApplyBorderSide` (item 59's delta),
    // not the old full-array `onApplyBorder` shape.
    expect(onApplyBorderSide).toHaveBeenCalledWith({ side: 'top', on: true, w: 1, dashed: false });
  });
});

describe('ShStylePanel — Clear styling + lifecycle', () => {
  it('Clear styling calls onClearStyling then onClose', async () => {
    const onClearStyling = vi.fn();
    const onClose = vi.fn();
    render(ShStylePanel, { props: baseProps({ onClearStyling, onClose }) });
    await fireEvent.click(screen.getByTestId('style-panel-clear'));
    expect(onClearStyling).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('Escape calls onClose', async () => {
    const onClose = vi.fn();
    render(ShStylePanel, { props: baseProps({ onClose }) });
    const panel = screen.getByTestId('style-panel');
    await fireEvent.keyDown(panel, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('a click inside the panel does not close it', async () => {
    const onClose = vi.fn();
    render(ShStylePanel, { props: baseProps({ onClose }) });
    const panel = screen.getByTestId('style-panel');
    await fireEvent.click(panel);
    expect(onClose).not.toHaveBeenCalled();
  });

  it('positions the panel at the given x/y coordinates', () => {
    render(ShStylePanel, { props: baseProps({ x: 120, y: 240 }) });
    const panel = screen.getByTestId('style-panel') as HTMLElement;
    expect(panel.style.left).toBe('120px');
    expect(panel.style.top).toBe('240px');
  });
});

// #2874 item 16 Part B — anchorEl exclusion guard. `ShStylePanel` was the
// only Sheets popover missing this guard (docs/audits/2026-09-08-item16-
// partB-focus-loss-diagnosis.md §5); these tests exercise
// `handleWindowClick` directly by dispatching a real window click event
// with an explicit `target`, since jsdom's synchronous dispatch can't
// reproduce the actual same-click self-close bug (that requires real
// mid-bubble listener attachment timing) — what CAN be verified here is
// the guard logic itself: a click whose target is inside `anchorEl` must
// not close the panel, exactly like a click inside the panel itself.
describe('ShStylePanel — anchorEl exclusion guard (#2874 item 16 Part B)', () => {
  it('a window click targeting inside anchorEl does not close the panel', async () => {
    const onClose = vi.fn();
    const anchorEl = document.createElement('button');
    document.body.appendChild(anchorEl);
    render(ShStylePanel, { props: baseProps({ onClose, anchorEl }) });
    const evt = new MouseEvent('click', { bubbles: true });
    Object.defineProperty(evt, 'target', { value: anchorEl });
    window.dispatchEvent(evt);
    expect(onClose).not.toHaveBeenCalled();
    anchorEl.remove();
  });

  it('a click elsewhere (outside both panelEl and anchorEl) does close the panel', async () => {
    const onClose = vi.fn();
    const anchorEl = document.createElement('button');
    document.body.appendChild(anchorEl);
    const elsewhere = document.createElement('div');
    document.body.appendChild(elsewhere);
    render(ShStylePanel, { props: baseProps({ onClose, anchorEl }) });
    const evt = new MouseEvent('click', { bubbles: true });
    Object.defineProperty(evt, 'target', { value: elsewhere });
    window.dispatchEvent(evt);
    expect(onClose).toHaveBeenCalledOnce();
    anchorEl.remove();
    elsewhere.remove();
  });

  it('with no anchorEl prop, a click outside the panel still closes it (no regression)', async () => {
    const onClose = vi.fn();
    const elsewhere = document.createElement('div');
    document.body.appendChild(elsewhere);
    render(ShStylePanel, { props: baseProps({ onClose }) });
    const evt = new MouseEvent('click', { bubbles: true });
    Object.defineProperty(evt, 'target', { value: elsewhere });
    window.dispatchEvent(evt);
    expect(onClose).toHaveBeenCalledOnce();
    elsewhere.remove();
  });
});
