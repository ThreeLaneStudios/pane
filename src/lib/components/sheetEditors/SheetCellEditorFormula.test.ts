// SheetCellEditorFormula.test.ts — #2339 PR 14 in-cell formula editor.
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import { tick } from 'svelte';
import SheetCellEditorFormula from './SheetCellEditorFormula.svelte';

describe('SheetCellEditorFormula', () => {
  it('renders the formula variant editor with an input and highlight overlay', () => {
    render(SheetCellEditorFormula, { props: { value: '=A1+B1', onCommit: vi.fn(), onCancel: vi.fn() } });
    expect(screen.getByTestId('sheet-cell-editor').getAttribute('data-variant')).toBe('formula');
    expect(screen.getByTestId('sheet-cell-editor-formula-highlight')).toBeTruthy();
    expect(screen.getByTestId('sheet-cell-editor-input')).toBeTruthy();
  });

  it('tokenizes the highlight overlay with sh-t-ref/op classes for a ref formula', () => {
    render(SheetCellEditorFormula, { props: { value: '=A1+B1', onCommit: vi.fn(), onCancel: vi.fn() } });
    const hl = screen.getByTestId('sheet-cell-editor-formula-highlight');
    expect(hl.querySelectorAll('.sh-t-ref').length).toBe(2);
    expect(hl.querySelector('.sh-t-op')).toBeTruthy();
  });

  it('renders a green sh-t-str token for a string literal (ruling 1 deviation)', () => {
    render(SheetCellEditorFormula, { props: { value: '=IF(A1>1,"yes","no")', onCommit: vi.fn(), onCancel: vi.fn() } });
    const hl = screen.getByTestId('sheet-cell-editor-formula-highlight');
    expect(hl.querySelectorAll('.sh-t-str').length).toBe(2);
  });

  it('fires onCommit("enter") on Enter and stops further keydown propagation', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorFormula, { props: { value: '=A1', onCommit, onCancel: vi.fn() } });
    await fireEvent.keyDown(screen.getByTestId('sheet-cell-editor-input'), { key: 'Enter' });
    expect(onCommit).toHaveBeenCalledWith('enter');
  });

  it('fires onCommit("tab") / onCommit("shift-tab") appropriately', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorFormula, { props: { value: '=A1', onCommit, onCancel: vi.fn() } });
    await fireEvent.keyDown(screen.getByTestId('sheet-cell-editor-input'), { key: 'Tab', shiftKey: true });
    expect(onCommit).toHaveBeenCalledWith('shift-tab');
  });

  it('fires onCancel on Escape', async () => {
    const onCancel = vi.fn();
    render(SheetCellEditorFormula, { props: { value: '=A1', onCommit: vi.fn(), onCancel } });
    await fireEvent.keyDown(screen.getByTestId('sheet-cell-editor-input'), { key: 'Escape' });
    expect(onCancel).toHaveBeenCalled();
  });

  it('fires onCommit("blur") when the input loses focus', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorFormula, { props: { value: '=A1', onCommit, onCancel: vi.fn() } });
    await fireEvent.blur(screen.getByTestId('sheet-cell-editor-input'));
    expect(onCommit).toHaveBeenCalledWith('blur');
  });

  it('does not commit twice for a double signal (enter then blur)', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorFormula, { props: { value: '=A1', onCommit, onCancel: vi.fn() } });
    const input = screen.getByTestId('sheet-cell-editor-input');
    await fireEvent.keyDown(input, { key: 'Enter' });
    await fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledTimes(1);
  });

  it('updates the live token overlay as the bound value changes', async () => {
    const { rerender } = render(SheetCellEditorFormula, { props: { value: '=A', onCommit: vi.fn(), onCancel: vi.fn() } });
    let hl = screen.getByTestId('sheet-cell-editor-formula-highlight');
    expect(hl.textContent).toBe('=A');
    await rerender({ value: '=A1+B1', onCommit: vi.fn(), onCancel: vi.fn() });
    hl = screen.getByTestId('sheet-cell-editor-formula-highlight');
    expect(hl.textContent).toBe('=A1+B1');
  });
});

// #2339 PR 14 fix (bar/cell caret parity): the in-cell editor's own half
// of the shared `selectionStart`/`selectionEnd` contract — mirrors
// FormulaBar.test.ts's equivalent "caret position transfer" cases.
describe('SheetCellEditorFormula — caret position transfer (bar/cell parity)', () => {
  it('restores a specific caret position on mount when selectionStart/selectionEnd are provided', async () => {
    render(SheetCellEditorFormula, {
      props: { value: '=A1+B1', onCommit: vi.fn(), onCancel: vi.fn(), selectionStart: 2, selectionEnd: 2 },
    });
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.selectionStart).toBe(2);
    expect(input.selectionEnd).toBe(2);
  });

  it('falls back to end-of-text caret when selectionStart/selectionEnd are null (unchanged default)', async () => {
    render(SheetCellEditorFormula, {
      props: { value: '=A1+B1', onCommit: vi.fn(), onCancel: vi.fn(), selectionStart: null, selectionEnd: null },
    });
    await tick();
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.selectionStart).toBe(input.value.length);
    expect(input.selectionEnd).toBe(input.value.length);
  });

  it('reports caret position via onSelectionChange as the user types', async () => {
    const onSelectionChange = vi.fn();
    render(SheetCellEditorFormula, { props: { value: '=A1', onCommit: vi.fn(), onCancel: vi.fn(), onSelectionChange } });
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '=A1+B1' } });
    expect(onSelectionChange).toHaveBeenCalled();
  });

  it('reports a non-null selection range shape (start/end numbers) on click', async () => {
    const onSelectionChange = vi.fn();
    render(SheetCellEditorFormula, { props: { value: '=A1+B1', onCommit: vi.fn(), onCancel: vi.fn(), onSelectionChange } });
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    await fireEvent.click(input);
    expect(onSelectionChange).toHaveBeenCalledWith(expect.any(Number), expect.any(Number));
  });
});
