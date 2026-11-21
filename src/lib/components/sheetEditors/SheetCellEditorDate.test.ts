// SheetCellEditorDate.test.ts — component tests for the masked date editor
// (#2339 PR 10, ruling 4/§7/§8). No calendar popover — plain masked text
// input only.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import SheetCellEditorDate from './SheetCellEditorDate.svelte';

describe('SheetCellEditorDate', () => {
  it('renders with the initial mask value pre-filled', () => {
    render(SheetCellEditorDate, { props: { value: '08/04/2026', onCommit: vi.fn(), onCancel: vi.fn() } });
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.value).toBe('08/04/2026');
  });

  it('renders the MM/DD/YYYY placeholder when empty', () => {
    render(SheetCellEditorDate, { props: { value: '', onCommit: vi.fn(), onCancel: vi.fn() } });
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.placeholder).toBe('MM/DD/YYYY');
  });

  it('does NOT render any calendar/popover markup (ruling 4: masked text only)', () => {
    render(SheetCellEditorDate, { props: { value: '', onCommit: vi.fn(), onCancel: vi.fn() } });
    expect(screen.queryByRole('grid')).toBeNull();
    expect(document.querySelector('.sh-pop')).toBeNull();
  });

  it('commits on Enter with cause "enter"', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorDate, { props: { value: '08/04/2026', onCommit, onCancel: vi.fn() } });
    const input = screen.getByTestId('sheet-cell-editor-input');
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onCommit).toHaveBeenCalledWith('enter');
  });

  it('commits on blur with cause "blur"', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorDate, { props: { value: '08/04/2026', onCommit, onCancel: vi.fn() } });
    const input = screen.getByTestId('sheet-cell-editor-input');
    await fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledWith('blur');
  });

  it('cancels on Escape without calling onCommit', async () => {
    const onCommit = vi.fn();
    const onCancel = vi.fn();
    render(SheetCellEditorDate, { props: { value: '08/04/2026', onCommit, onCancel } });
    const input = screen.getByTestId('sheet-cell-editor-input');
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onCommit).not.toHaveBeenCalled();
  });

  it('auto-inserts a slash after two typed digits', async () => {
    render(SheetCellEditorDate, { props: { value: '', onCommit: vi.fn(), onCancel: vi.fn() } });
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    input.value = '08';
    await fireEvent.input(input);
    expect(input.value).toBe('08/');
  });
});
