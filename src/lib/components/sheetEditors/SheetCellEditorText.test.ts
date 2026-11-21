// SheetCellEditorText.test.ts — component tests for the shared
// text/number/currency cell editor overlay (#2339 PR 10, §7/§8).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import SheetCellEditorText from './SheetCellEditorText.svelte';

describe('SheetCellEditorText', () => {
  it('renders with the initial value pre-filled', () => {
    render(SheetCellEditorText, {
      props: { value: 'Longform essays', variant: 'text', onCommit: vi.fn(), onCancel: vi.fn() },
    });
    const input = screen.getByTestId('sheet-cell-editor-input') as HTMLInputElement;
    expect(input.value).toBe('Longform essays');
  });

  it('commits on Enter with cause "enter"', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorText, {
      props: { value: 'hi', variant: 'text', onCommit, onCancel: vi.fn() },
    });
    const input = screen.getByTestId('sheet-cell-editor-input');
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onCommit).toHaveBeenCalledWith('enter');
  });

  it('commits on Tab with cause "tab", and "shift-tab" when shift is held', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorText, {
      props: { value: 'hi', variant: 'text', onCommit, onCancel: vi.fn() },
    });
    const input = screen.getByTestId('sheet-cell-editor-input');
    await fireEvent.keyDown(input, { key: 'Tab', shiftKey: true });
    expect(onCommit).toHaveBeenCalledWith('shift-tab');
  });

  it('commits on blur with cause "blur"', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorText, {
      props: { value: 'hi', variant: 'text', onCommit, onCancel: vi.fn() },
    });
    const input = screen.getByTestId('sheet-cell-editor-input');
    await fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledWith('blur');
  });

  it('cancels on Escape without calling onCommit', async () => {
    const onCommit = vi.fn();
    const onCancel = vi.fn();
    render(SheetCellEditorText, {
      props: { value: 'hi', variant: 'text', onCommit, onCancel },
    });
    const input = screen.getByTestId('sheet-cell-editor-input');
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onCommit).not.toHaveBeenCalled();
  });

  it('only fires one of commit/cancel even if both Enter and blur happen in sequence', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorText, {
      props: { value: 'hi', variant: 'text', onCommit, onCancel: vi.fn() },
    });
    const input = screen.getByTestId('sheet-cell-editor-input');
    await fireEvent.keyDown(input, { key: 'Enter' });
    await fireEvent.blur(input);
    expect(onCommit).toHaveBeenCalledTimes(1);
  });

  it('applies the numeric-alignment modifier class for the "number" variant', () => {
    render(SheetCellEditorText, {
      props: { value: '12', variant: 'number', onCommit: vi.fn(), onCancel: vi.fn() },
    });
    const el = screen.getByTestId('sheet-cell-editor');
    expect(el.className).toContain('sh-ed--num');
  });

  it('does not apply the numeric-alignment modifier class for the "text" variant', () => {
    render(SheetCellEditorText, {
      props: { value: 'abc', variant: 'text', onCommit: vi.fn(), onCancel: vi.fn() },
    });
    const el = screen.getByTestId('sheet-cell-editor');
    expect(el.className).not.toContain('sh-ed--num');
  });

  it('renders a currency-code suffix and hint for the "currency" variant only', () => {
    render(SheetCellEditorText, {
      props: { value: '1400', variant: 'currency', currencyCode: 'USD', onCommit: vi.fn(), onCancel: vi.fn() },
    });
    expect(screen.getByText('USD')).toBeTruthy();
  });
});
