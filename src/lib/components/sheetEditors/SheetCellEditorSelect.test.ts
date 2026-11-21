// SheetCellEditorSelect.test.ts — component tests for the select-column
// dropdown popover editor (#2339 PR 10, §2/select, §7/§8).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import SheetCellEditorSelect from './SheetCellEditorSelect.svelte';

// vitest-setup.ts replaces `globalThis.window` with a plain mock object
// lacking addEventListener/removeEventListener. This component uses
// <svelte:window onkeydown/onclick> for Escape/outside-click cancel, so the
// DOM listener API needs to exist on that mock for mount/unmount to
// succeed. Same patch as ProviderConnectionCard.test.ts.
if (typeof (globalThis.window as any).addEventListener !== 'function') {
  (globalThis.window as any).addEventListener = () => {};
  (globalThis.window as any).removeEventListener = () => {};
}

describe('SheetCellEditorSelect', () => {
  it('renders one row per choice', () => {
    render(SheetCellEditorSelect, {
      props: { value: 'In flight', choices: ['Draft', 'In flight', 'Shipped'], onCommit: vi.fn(), onCancel: vi.fn() },
    });
    const rows = screen.getAllByTestId('sheet-select-choice');
    expect(rows).toHaveLength(3);
  });

  it('marks the currently-selected choice with is-on', () => {
    render(SheetCellEditorSelect, {
      props: { value: 'In flight', choices: ['Draft', 'In flight', 'Shipped'], onCommit: vi.fn(), onCancel: vi.fn() },
    });
    const selected = screen.getByText('In flight').closest('button');
    expect(selected?.className).toContain('is-on');
  });

  it('commits the clicked choice', async () => {
    const onCommit = vi.fn();
    render(SheetCellEditorSelect, {
      props: { value: 'Draft', choices: ['Draft', 'In flight', 'Shipped'], onCommit, onCancel: vi.fn() },
    });
    await fireEvent.click(screen.getByText('Shipped'));
    expect(onCommit).toHaveBeenCalledWith('Shipped');
  });

  it('cancels on Escape without committing', async () => {
    const onCommit = vi.fn();
    const onCancel = vi.fn();
    render(SheetCellEditorSelect, {
      props: { value: 'Draft', choices: ['Draft', 'In flight'], onCommit, onCancel },
    });
    const pop = screen.getByTestId('sheet-cell-editor');
    await fireEvent.keyDown(pop, { key: 'Escape' });
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onCommit).not.toHaveBeenCalled();
  });

  it('renders a fallback message when the column has no configured choices', () => {
    render(SheetCellEditorSelect, {
      props: { value: '', choices: [], onCommit: vi.fn(), onCancel: vi.fn() },
    });
    expect(screen.getByText(/No options configured/)).toBeTruthy();
    expect(screen.queryAllByTestId('sheet-select-choice')).toHaveLength(0);
  });

  it('does not render an "Edit options…" row when onOptionsChange is omitted (backward-compatible default)', () => {
    render(SheetCellEditorSelect, {
      props: { value: 'Draft', choices: ['Draft'], onCommit: vi.fn(), onCancel: vi.fn() },
    });
    expect(screen.queryByText(/Edit options/)).toBeNull();
  });
});

describe('SheetCellEditorSelect — Edit options… footer (#2339 PR 15b-beta)', () => {
  it('renders the "Edit options…" row with the live choice count when onOptionsChange is provided', () => {
    render(SheetCellEditorSelect, {
      props: { value: 'Draft', choices: ['Draft', 'Shipped'], onCommit: vi.fn(), onCancel: vi.fn(), onOptionsChange: vi.fn() },
    });
    const row = screen.getByTestId('sheet-select-edit-options');
    expect(row.textContent).toContain('Edit options');
    expect(row.textContent).toContain('2');
  });

  it('clicking it swaps the popover to the options panel, hiding the choice list', async () => {
    render(SheetCellEditorSelect, {
      props: { value: 'Draft', choices: ['Draft', 'Shipped'], onCommit: vi.fn(), onCancel: vi.fn(), onOptionsChange: vi.fn() },
    });
    await fireEvent.click(screen.getByTestId('sheet-select-edit-options'));
    expect(screen.getByTestId('sheet-options-panel')).toBeTruthy();
    expect(screen.queryAllByTestId('sheet-select-choice')).toHaveLength(0);
  });

  it('a change from the panel calls onOptionsChange, not onCommit', async () => {
    const onOptionsChange = vi.fn();
    const onCommit = vi.fn();
    render(SheetCellEditorSelect, {
      props: { value: 'Draft', choices: ['Draft', 'Shipped'], onCommit, onCancel: vi.fn(), onOptionsChange },
    });
    await fireEvent.click(screen.getByTestId('sheet-select-edit-options'));
    await fireEvent.click(screen.getByTestId('sheet-options-remove-0'));
    expect(onOptionsChange).toHaveBeenCalledWith(['Shipped']);
    expect(onCommit).not.toHaveBeenCalled();
  });

  it('the options panel back-chevron returns to the choice list', async () => {
    render(SheetCellEditorSelect, {
      props: { value: 'Draft', choices: ['Draft', 'Shipped'], onCommit: vi.fn(), onCancel: vi.fn(), onOptionsChange: vi.fn() },
    });
    await fireEvent.click(screen.getByTestId('sheet-select-edit-options'));
    await fireEvent.click(screen.getByTestId('sheet-options-back'));
    expect(screen.getAllByTestId('sheet-select-choice')).toHaveLength(2);
  });
});
