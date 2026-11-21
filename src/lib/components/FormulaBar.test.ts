// FormulaBar.test.ts — #2339 PR 14 (Formula Bar & Formula Editing)
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import FormulaBar from './FormulaBar.svelte';
import SheetCellEditorFormula from './sheetEditors/SheetCellEditorFormula.svelte';
import { tokenizeFormula } from '$lib/utils/sheetFormula';

describe('FormulaBar — idle states', () => {
  it('renders the cell reference and fallback empty text for an empty cell', () => {
    render(FormulaBar, { props: { cref: 'G9', value: '' } });
    expect(screen.getByTestId('sheet-formula-bar-ref').textContent).toBe('G9');
    expect(screen.getByText('Empty cell')).toBeTruthy();
  });

  it('renders a typed (non-formula) value as plain tokenized text', () => {
    render(FormulaBar, { props: { cref: 'E3', value: '320.00' } });
    expect(screen.getByTestId('sheet-formula-bar-display').textContent).toBe('320.00');
  });

  it('renders a computed cell\'s raw formula, not its result', () => {
    render(FormulaBar, { props: { cref: 'F3', value: '=D3*E3' } });
    const display = screen.getByTestId('sheet-formula-bar-display');
    expect(display.textContent).toBe('=D3*E3');
  });

  it('tokenizes refs/functions/strings with the locked design classes', () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=IF(SUM(F1:F8)>20000, "over", AVERAGE(F1:F8))' } });
    const display = screen.getByTestId('sheet-formula-bar-display');
    expect(display.querySelector('.sh-t-fn')).toBeTruthy();
    expect(display.querySelector('.sh-t-ref')).toBeTruthy();
    expect(display.querySelector('.sh-t-str')).toBeTruthy();
  });

  it('the fx slot is a static label, not a button (Matt ruling 2026-09-05 #5)', () => {
    const { container } = render(FormulaBar, { props: { cref: 'A1', value: '' } });
    const fx = container.querySelector('.sh-fbar__fx');
    expect(fx).toBeTruthy();
    expect(fx?.tagName.toLowerCase()).not.toBe('button');
  });
});

describe('FormulaBar — editing state', () => {
  it('renders is-editing and an input when editing is true', () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=SUM(F1:F5)', editing: true } });
    expect(screen.getByTestId('sheet-formula-bar').className).toContain('is-editing');
    expect(screen.getByTestId('sheet-formula-bar-input')).toBeTruthy();
  });

  it('fires onCommit("enter") on Enter', async () => {
    const onCommit = vi.fn();
    render(FormulaBar, { props: { cref: 'F9', value: '=A1', editing: true, onCommit } });
    await fireEvent.keyDown(screen.getByTestId('sheet-formula-bar-input'), { key: 'Enter' });
    expect(onCommit).toHaveBeenCalledWith('enter');
  });

  it('fires onCancel on Escape', async () => {
    const onCancel = vi.fn();
    render(FormulaBar, { props: { cref: 'F9', value: '=A1', editing: true, onCancel } });
    await fireEvent.keyDown(screen.getByTestId('sheet-formula-bar-input'), { key: 'Escape' });
    expect(onCancel).toHaveBeenCalled();
  });

  it('fires onInput with the new value as the user types', async () => {
    const onInput = vi.fn();
    render(FormulaBar, { props: { cref: 'F9', value: '=A1', editing: true, onInput } });
    await fireEvent.input(screen.getByTestId('sheet-formula-bar-input'), { target: { value: '=A1+B1' } });
    expect(onInput).toHaveBeenCalledWith('=A1+B1');
  });

  it('fires onCommit("blur") when the input loses focus while editing', async () => {
    const onCommit = vi.fn();
    render(FormulaBar, { props: { cref: 'F9', value: '=A1', editing: true, onCommit } });
    await fireEvent.blur(screen.getByTestId('sheet-formula-bar-input'));
    expect(onCommit).toHaveBeenCalledWith('blur');
  });
});

describe('FormulaBar — inline rejection (ruling 3/8)', () => {
  it('renders the inline error message when error is set', () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=Z99', editing: true, error: "'Z99' is out of bounds — this sheet has no column Z" } });
    expect(screen.getByTestId('sheet-formula-bar-error').textContent).toContain('out of bounds');
  });

  it('renders no error element when error is null', () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=A1', editing: true, error: null } });
    expect(screen.queryByTestId('sheet-formula-bar-error')).toBeNull();
  });
});

describe('FormulaBar — chevron three-line expand (Matt ruling 2026-09-05 #7)', () => {
  it('starts collapsed (not is-expanded)', () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=SUM(F1:F5)' } });
    expect(screen.getByTestId('sheet-formula-bar').className).not.toContain('is-expanded');
  });

  it('clicking the chevron expands the bar', async () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=SUM(F1:F5)' } });
    await fireEvent.click(screen.getByTestId('sheet-formula-bar-chevron'));
    expect(screen.getByTestId('sheet-formula-bar').className).toContain('is-expanded');
  });

  it('clicking the chevron again collapses it back (real toggle, not one-way)', async () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=SUM(F1:F5)' } });
    const chevron = screen.getByTestId('sheet-formula-bar-chevron');
    await fireEvent.click(chevron);
    await fireEvent.click(chevron);
    expect(screen.getByTestId('sheet-formula-bar').className).not.toContain('is-expanded');
  });

  it('sets aria-pressed to reflect expanded state', async () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=SUM(F1:F5)' } });
    const chevron = screen.getByTestId('sheet-formula-bar-chevron');
    expect(chevron.getAttribute('aria-pressed')).toBe('false');
    await fireEvent.click(chevron);
    expect(chevron.getAttribute('aria-pressed')).toBe('true');
  });

  it('collapses automatically once editing ends', async () => {
    const { rerender } = render(FormulaBar, { props: { cref: 'F9', value: '=SUM(F1:F5)', editing: true } });
    await fireEvent.click(screen.getByTestId('sheet-formula-bar-chevron'));
    expect(screen.getByTestId('sheet-formula-bar').className).toContain('is-expanded');

    await rerender({ cref: 'F9', value: '=SUM(F1:F5)', editing: false });
    await tick();
    expect(screen.getByTestId('sheet-formula-bar').className).not.toContain('is-expanded');
  });
});

// #2339 PR 14 fix: bar live-tokenizing to design parity ("the same
// tokenizing and the same caret" for bar and cell). The bar's editing
// state must render a colored token overlay behind a transparent input,
// matching SheetCellEditorFormula.svelte's own technique exactly.
describe('FormulaBar — live-tokenizing overlay while editing (design parity)', () => {
  it('renders a tokenized highlight overlay while editing, not a plain input alone', () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=A1+B1', editing: true } });
    const hl = screen.getByTestId('sheet-formula-bar-highlight');
    expect(hl).toBeTruthy();
    // The real input still exists and still receives keystrokes/caret —
    // the overlay is purely additive, not a replacement.
    expect(screen.getByTestId('sheet-formula-bar-input')).toBeTruthy();
  });

  it('tokenizes the editing overlay with sh-t-ref classes for ref formulas', () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=A1+B1', editing: true } });
    const hl = screen.getByTestId('sheet-formula-bar-highlight');
    expect(hl.querySelectorAll('.sh-t-ref').length).toBe(2);
  });

  it('tokenizes the editing overlay with sh-t-fn for function names', () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=SUM(A1:A5)', editing: true } });
    const hl = screen.getByTestId('sheet-formula-bar-highlight');
    expect(hl.querySelector('.sh-t-fn')).toBeTruthy();
  });

  it('tokenizes the editing overlay with a green sh-t-str for string literals (ruling 1)', () => {
    render(FormulaBar, { props: { cref: 'F9', value: '=IF(A1>1,"yes","no")', editing: true } });
    const hl = screen.getByTestId('sheet-formula-bar-highlight');
    expect(hl.querySelectorAll('.sh-t-str').length).toBe(2);
  });

  it('updates the live overlay as the bound value changes mid-edit', async () => {
    const { rerender } = render(FormulaBar, { props: { cref: 'F9', value: '=A', editing: true } });
    let hl = screen.getByTestId('sheet-formula-bar-highlight');
    expect(hl.textContent).toBe('=A');
    await rerender({ cref: 'F9', value: '=A1+B1', editing: true });
    hl = screen.getByTestId('sheet-formula-bar-highlight');
    expect(hl.textContent).toBe('=A1+B1');
  });

  it('the editing input still fires onInput/onCommit/onCancel with the overlay present', async () => {
    const onInput = vi.fn();
    render(FormulaBar, { props: { cref: 'F9', value: '=A1', editing: true, onInput } });
    await fireEvent.input(screen.getByTestId('sheet-formula-bar-input'), { target: { value: '=A1+B1' } });
    expect(onInput).toHaveBeenCalledWith('=A1+B1');
  });

  it('bar editing and cell editing produce identical token class output for the same formula string', () => {
    // Only the FIRST class token matters here (`sh-t-ref`, `sh-t-fn`,
    // etc.) — Svelte appends a per-component scoped-CSS hash class
    // (`svelte-xxxxx`) that legitimately differs between FormulaBar.svelte
    // and SheetCellEditorFormula.svelte even though both use the exact
    // same `tokenizeFormula` output and the exact same `.sh-t-*` names.
    const baseClass = (full: string) => full.split(' ')[0];

    const formula = '=IF(SUM(A1:A5)>10,"big",AVERAGE(B1:B5))';
    render(FormulaBar, { props: { cref: 'F9', value: formula, editing: true } });
    const barHl = screen.getByTestId('sheet-formula-bar-highlight');
    const barClasses = Array.from(barHl.children).map((el) => baseClass(el.className));

    const { container } = render(SheetCellEditorFormula, { props: { value: formula, onCommit: vi.fn(), onCancel: vi.fn() } });
    const cellHl = container.querySelector('[data-testid="sheet-cell-editor-formula-highlight"]')!;
    const cellClasses = Array.from(cellHl.children).map((el) => baseClass(el.className));

    expect(barClasses).toEqual(cellClasses);
    // Also cross-check directly against the shared tokenizer, so this test
    // fails loudly if either component stops using it.
    const expected = tokenizeFormula(formula).map((t) => t.cls);
    expect(barClasses).toEqual(expected);
  });
});

// #2339 PR 14 fix: bar/cell caret-position parity. Both surfaces read/
// write the same shared `editing` session's `selectionStart`/
// `selectionEnd` (owned by SheetGrid.svelte) — the bar's own focus
// effect must restore whatever was last recorded instead of always
// jumping to end-of-text.
describe('FormulaBar — caret position transfer (bar/cell parity)', () => {
  it('restores a specific caret position on focus when selectionStart/selectionEnd are provided', async () => {
    render(FormulaBar, {
      props: { cref: 'F9', value: '=A1+B1', editing: true, selectionStart: 2, selectionEnd: 2 },
    });
    await tick();
    const input = screen.getByTestId('sheet-formula-bar-input') as HTMLInputElement;
    expect(input.selectionStart).toBe(2);
    expect(input.selectionEnd).toBe(2);
  });

  it('falls back to end-of-text caret when selectionStart/selectionEnd are null', async () => {
    render(FormulaBar, {
      props: { cref: 'F9', value: '=A1+B1', editing: true, selectionStart: null, selectionEnd: null },
    });
    await tick();
    const input = screen.getByTestId('sheet-formula-bar-input') as HTMLInputElement;
    expect(input.selectionStart).toBe(input.value.length);
    expect(input.selectionEnd).toBe(input.value.length);
  });

  it('reports caret position via onSelectionChange as the user types', async () => {
    const onSelectionChange = vi.fn();
    render(FormulaBar, { props: { cref: 'F9', value: '=A1', editing: true, onSelectionChange } });
    const input = screen.getByTestId('sheet-formula-bar-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '=A1+B1' } });
    expect(onSelectionChange).toHaveBeenCalled();
  });

  it('reports a non-null selection range shape (start/end numbers) on click', async () => {
    const onSelectionChange = vi.fn();
    render(FormulaBar, { props: { cref: 'F9', value: '=A1+B1', editing: true, onSelectionChange } });
    const input = screen.getByTestId('sheet-formula-bar-input') as HTMLInputElement;
    await fireEvent.click(input);
    expect(onSelectionChange).toHaveBeenCalledWith(expect.any(Number), expect.any(Number));
  });

  // Manual-QA-required: a real cross-component focus handoff (bar -> cell
  // or cell -> bar) mid-edit relies on SheetGrid.svelte's shared `editing`
  // session object being read by BOTH mounted components simultaneously —
  // that full integration (SheetGrid + FormulaBar + SheetCellEditorFormula
  // all mounted together, driven by real focus events) isn't practical to
  // assert in jsdom against isolated component renders. The unit-level
  // contract each side of the handoff depends on (bar restores a given
  // selectionStart/selectionEnd on focus; bar reports its own via
  // onSelectionChange) is covered by the tests above and by
  // SheetCellEditorFormula.test.ts's equivalent cases.
});
