/**
 * SearchableSelect.inlineMode.test.ts — #3178 (C57, rewrite, Matt's
 * ruling).
 *
 * C57's history: five passes over this component's positioning math,
 * four of them pure measurement fixes (containing-block anchor, the
 * MIN_USABLE_SPACE_BELOW threshold, the 'below' height clamp — reverted
 * the commit immediately before this test was added), each correct for
 * the case that prompted it and wrong for the next. Matt's ruling: stop
 * patching the measurement, add a mode that doesn't need any —
 * `inline`. In inline mode the list renders in normal document flow
 * under its field; there is no `position: fixed`, no `dropdownTop`, no
 * `dropdownMaxHeight`, no `actualHeight` read, and no above/below
 * branch for a resize or reopen to ever leave stale.
 *
 * jsdom stubs `getBoundingClientRect()` to all-zero and has no CSS
 * layout engine (the exact reason the clamp commit this PR reverts, and
 * the threshold commit before it, both shipped with NO jsdom test for
 * their own positioning math — see those commits' own messages). That
 * limitation is exactly why this prop removes the positioning math for
 * its two callers instead of patching it again. What jsdom CAN verify
 * without any real layout — and what this file tests — is simpler and
 * layout-independent: which code PATH ran. Does the dropdown element
 * carry the fixed-position inline styles (`top`/`left`/`width`) and
 * `data-position` attribute at all, or none of them? Is
 * `updateDropdownPosition` (and therefore `dropdownTop`/
 * `dropdownMaxHeight`/`actualHeight`) ever invoked for an inline
 * instance? Both are structural, not geometric, and jsdom answers them
 * exactly as reliably as a real browser would.
 *
 * Real on-screen geometry (527/720 overflow, the resize sequence, the
 * reopen sequence, the cramped case) has **no jsdom test, measured
 * instead** — a disposable Playwright WebKit harness against a live
 * mount, same discipline as Reed's own #3180 audit harness and the
 * clamp commit's own stated verification method. See the PR body for
 * those numbers.
 *
 * Mutation (actually run, not just asserted): swapping the template's
 * `{#if inline}` for `{#if !inline}` (the branch picking which markup
 * — the fixed-position div with `data-position`/`top`/`left`/`width`,
 * or the plain `.dropdown-list-inline` div — renders for which prop
 * value) reddens BOTH of the first two tests below, inverted exactly
 * as expected: the default/floating-mode test fails on
 * `dropdown-list-inline` being present when it should be absent, and
 * the `inline={true}` test fails on `data-position` being present when
 * it should be absent. Verified directly (mutate, run, see 2 failures
 * with that exact shape; revert, run, see 3 passes) before writing this
 * comment — not inferred from reading the diff.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte/svelte5';
import { createRawSnippet } from 'svelte';
import SearchableSelect from './SearchableSelect.svelte';

(globalThis.window as any).addEventListener = () => {};
(globalThis.window as any).removeEventListener = () => {};

const items = [
  { id: 'a', label: 'Alpha' },
  { id: 'b', label: 'Bravo' },
  { id: 'c', label: 'Charlie' },
];

function labelSnippet() {
  return createRawSnippet((item: () => { id: string; label: string }) => ({
    render: () => `<span class="item-label">${item().label}</span>`,
  }));
}

function byLabel(id: string) {
  return items.find((i) => i.id === id)?.label ?? '';
}

function baseProps(extra: Record<string, unknown> = {}) {
  return {
    items,
    selectedId: '',
    onSelect: vi.fn(),
    getDisplayText: byLabel,
    filterItem: (item: any, lower: string) => item.label.toLowerCase().includes(lower),
    sortItems: (list: any[]) => list,
    renderItem: labelSnippet(),
    ...extra,
  };
}

describe('SearchableSelect inline prop (#3178, C57 rewrite)', () => {
  it('defaults to floating mode (omitted prop): fixed-position styles and data-position are present, list is NOT .dropdown-list-inline', async () => {
    const { container } = render(SearchableSelect, { props: baseProps() });

    await fireEvent.focus(container.querySelector('.select-input-field') as HTMLInputElement);

    const list = container.querySelector('.dropdown-list') as HTMLElement;
    expect(list).toBeTruthy();
    expect(list.classList.contains('dropdown-list-inline')).toBe(false);
    expect(list.hasAttribute('data-position')).toBe(true);
    // jsdom zeroes getBoundingClientRect, but the floating branch still
    // WRITES these inline styles — their presence (not their value) is
    // what distinguishes the floating code path from the inline one.
    expect(list.style.top).not.toBe('');
    expect(list.style.left).not.toBe('');
    expect(list.style.width).not.toBe('');
  });

  it('inline={true}: no fixed-position styles, no data-position, .dropdown-list-inline present', async () => {
    const { container } = render(SearchableSelect, { props: baseProps({ inline: true }) });

    await fireEvent.focus(container.querySelector('.select-input-field') as HTMLInputElement);

    const list = container.querySelector('.dropdown-list') as HTMLElement;
    expect(list).toBeTruthy();
    expect(list.classList.contains('dropdown-list-inline')).toBe(true);
    expect(list.hasAttribute('data-position')).toBe(false);
    // The entire positioning block is skipped for an inline instance —
    // none of dropdownTop/dropdownLeft/dropdownWidth/dropdownMaxHeight
    // are ever assigned, so none of their style bindings are present.
    expect(list.style.top).toBe('');
    expect(list.style.left).toBe('');
    expect(list.style.width).toBe('');
  });

  it('inline={true}: filtering, selection, and footer mechanics are unchanged — same shared implementation as floating mode', async () => {
    const onSelect = vi.fn();
    const { container } = render(SearchableSelect, {
      props: baseProps({ onSelect, inline: true }),
    });

    const input = container.querySelector('.select-input-field') as HTMLInputElement;
    await fireEvent.focus(input);
    await fireEvent.input(input, { target: { value: 'bra' } });

    const labels = Array.from(container.querySelectorAll('.item-label')).map((el) => el.textContent);
    expect(labels).toEqual(['Bravo']);

    const bravoBtn = container.querySelector('.dropdown-item') as HTMLButtonElement;
    await fireEvent.mouseDown(bravoBtn);
    expect(onSelect).toHaveBeenCalledWith('b');
    expect(container.querySelector('.dropdown-list')).toBeNull();
  });

  // #3182 (C64 sibling): drift tripwire ONLY — jsdom has no layout
  // engine (this file's own header comment already says so), so this
  // cannot prove the dialog fits on screen or that the list actually
  // scrolls at 5+ rows. It proves the SOURCE changed: `.dropdown-list-
  // inline`'s declared `max-height` is no longer the literal string
  // `none` (#3178's explicit no-cap opt-out). The real claim — ~5 rows,
  // scrolls, dialog stays on screen — is proven in
  // `tests/e2e/c57-inline-list-height-cap.spec.ts` (WebKit, real
  // computed layout), not here. Reverting the CSS fix (restoring
  // `max-height: none`) reddens this one assertion and nothing else in
  // this file — the literal-string regression is all it catches.
  it('#3182: .dropdown-list-inline no longer declares max-height: none (drift tripwire, not a layout proof)', async () => {
    const { container } = render(SearchableSelect, { props: baseProps({ inline: true }) });

    await fireEvent.focus(container.querySelector('.select-input-field') as HTMLInputElement);

    const list = container.querySelector('.dropdown-list-inline') as HTMLElement;
    expect(list).toBeTruthy();
    expect(getComputedStyle(list).maxHeight).not.toBe('none');
  });
});
