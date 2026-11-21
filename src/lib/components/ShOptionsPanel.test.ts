// ShOptionsPanel.test.ts — component tests for the select-column choices
// editor's six states (#2339 PR 15b-beta, `docs/design/sheets/sheets-
// addendum.jsx:133-260`, `NOTES.md` §B).

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte/svelte5';
import ShOptionsPanel from './ShOptionsPanel.svelte';

function baseProps(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    choices: ['Committed', 'In flight', 'Proposed', 'Cut'],
    onChange: vi.fn(),
    onBack: vi.fn(),
    ...overrides,
  };
}

describe('ShOptionsPanel — list state', () => {
  it('renders one row per choice, in order', () => {
    render(ShOptionsPanel, { props: baseProps() });
    const list = screen.getByTestId('sheet-options-list');
    expect(list.textContent).toContain('Committed');
    expect(list.textContent).toContain('In flight');
    expect(list.textContent).toContain('Proposed');
    expect(list.textContent).toContain('Cut');
  });

  it('shows the choice count in the header', () => {
    render(ShOptionsPanel, { props: baseProps() });
    expect(screen.getByTestId('sheet-options-count').textContent).toBe('4');
  });

  it('back chevron calls onBack', async () => {
    const onBack = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onBack }) });
    await fireEvent.click(screen.getByTestId('sheet-options-back'));
    expect(onBack).toHaveBeenCalledOnce();
  });

  it('removing a choice calls onChange with a shorter array, and no other change', async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onChange }) });
    await fireEvent.click(screen.getByTestId('sheet-options-remove-1')); // "In flight"
    expect(onChange).toHaveBeenCalledWith(['Committed', 'Proposed', 'Cut']);
  });

  it("removal doesn't alter stored cell values — onChange only ever receives a choices array, never a cell/value payload", async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onChange }) });
    await fireEvent.click(screen.getByTestId('sheet-options-remove-0'));
    const [arg] = onChange.mock.calls[0];
    expect(Array.isArray(arg)).toBe(true);
    expect(arg.every((v: unknown) => typeof v === 'string')).toBe(true);
    // No row/column/cell identifiers were ever passed anywhere near this
    // call — the callback's ENTIRE payload is the plain string array.
    expect(onChange.mock.calls[0]).toHaveLength(1);
  });
});

describe('ShOptionsPanel — add state', () => {
  it('"Add choice" row switches to a focused input', async () => {
    render(ShOptionsPanel, { props: baseProps() });
    await fireEvent.click(screen.getByTestId('sheet-options-add'));
    expect(screen.getByTestId('sheet-options-add-input')).toBeTruthy();
  });

  it('Enter commits the new choice via onChange, appended at the end', async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onChange }) });
    await fireEvent.click(screen.getByTestId('sheet-options-add'));
    const input = screen.getByTestId('sheet-options-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Shipped' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onChange).toHaveBeenCalledWith(['Committed', 'In flight', 'Proposed', 'Cut', 'Shipped']);
  });

  it('Escape cancels the add without calling onChange', async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onChange }) });
    await fireEvent.click(screen.getByTestId('sheet-options-add'));
    const input = screen.getByTestId('sheet-options-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Shipped' } });
    await fireEvent.keyDown(input, { key: 'Escape' });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.queryByTestId('sheet-options-add-input')).toBeNull();
  });

  it('committing an empty/whitespace value is a no-op cancel, not an empty-string choice', async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onChange }) });
    await fireEvent.click(screen.getByTestId('sheet-options-add'));
    const input = screen.getByTestId('sheet-options-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '   ' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('ShOptionsPanel — rename state', () => {
  it('double-clicking a chip swaps it for an in-place input', async () => {
    render(ShOptionsPanel, { props: baseProps() });
    await fireEvent.dblClick(screen.getByText('In flight'));
    expect(screen.getByTestId('sheet-options-rename-input')).toBeTruthy();
  });

  it('Enter commits the rename via onChange at the same index', async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onChange }) });
    await fireEvent.dblClick(screen.getByText('In flight'));
    const input = screen.getByTestId('sheet-options-rename-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'On hold' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onChange).toHaveBeenCalledWith(['Committed', 'On hold', 'Proposed', 'Cut']);
  });

  it('renaming to its own current value (case change only) is allowed, not treated as a duplicate', async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onChange }) });
    await fireEvent.dblClick(screen.getByText('Committed'));
    const input = screen.getByTestId('sheet-options-rename-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'committed' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onChange).toHaveBeenCalledWith(['committed', 'In flight', 'Proposed', 'Cut']);
    expect(screen.queryByTestId('sheet-options-error')).toBeNull();
  });
});

describe('ShOptionsPanel — duplicate rejection', () => {
  it('renaming to an existing choice (case-insensitive, trimmed) is rejected with the danger ring + message, keeping the typed text', async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onChange }) });
    await fireEvent.dblClick(screen.getByText('In flight'));
    const input = screen.getByTestId('sheet-options-rename-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '  committed  ' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    expect(onChange).not.toHaveBeenCalled();
    expect(input.className).toContain('is-error');
    expect(input.value).toBe('  committed  ');
    expect(screen.getByTestId('sheet-options-error').textContent).toContain("'committed' is already a choice.");
  });

  it('adding a duplicate (exact case) is rejected the same way', async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onChange }) });
    await fireEvent.click(screen.getByTestId('sheet-options-add'));
    const input = screen.getByTestId('sheet-options-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Cut' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    expect(onChange).not.toHaveBeenCalled();
    expect(input.className).toContain('is-error');
    expect(screen.getByTestId('sheet-options-error').textContent).toContain("'Cut' is already a choice.");
  });

  it('the field stays in edit mode after a rejected duplicate — the fix is to keep typing', async () => {
    render(ShOptionsPanel, { props: baseProps() });
    await fireEvent.click(screen.getByTestId('sheet-options-add'));
    const input = screen.getByTestId('sheet-options-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Committed' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(screen.getByTestId('sheet-options-add-input')).toBeTruthy();
  });
});

describe('ShOptionsPanel — empty state', () => {
  it('renders "No choices yet" copy and a single "Add the first choice" button when choices is []', () => {
    render(ShOptionsPanel, { props: baseProps({ choices: [] }) });
    expect(screen.getByTestId('sheet-options-empty')).toBeTruthy();
    expect(screen.getByText(/No choices yet/)).toBeTruthy();
    expect(screen.getByTestId('sheet-options-add-first')).toBeTruthy();
    expect(screen.queryByTestId('sheet-options-list')).toBeNull();
  });

  it('the header count reads 0 in the empty state', () => {
    render(ShOptionsPanel, { props: baseProps({ choices: [] }) });
    expect(screen.getByTestId('sheet-options-count').textContent).toBe('0');
  });

  it('"Add the first choice" opens the same add-input flow', async () => {
    render(ShOptionsPanel, { props: baseProps({ choices: [] }) });
    await fireEvent.click(screen.getByTestId('sheet-options-add-first'));
    expect(screen.getByTestId('sheet-options-add-input')).toBeTruthy();
  });

  it('adding the first choice from empty commits via onChange as a one-item array', async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ choices: [], onChange }) });
    await fireEvent.click(screen.getByTestId('sheet-options-add-first'));
    const input = screen.getByTestId('sheet-options-add-input') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'Draft' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(onChange).toHaveBeenCalledWith(['Draft']);
  });
});

describe('ShOptionsPanel — footer', () => {
  it('shows the removal-safety footer text whenever the list is non-empty', () => {
    render(ShOptionsPanel, { props: baseProps() });
    expect(screen.getByText(/Removing a choice leaves cell values untouched/)).toBeTruthy();
  });

  it('does not show the footer text in the empty state (it has its own copy instead)', () => {
    render(ShOptionsPanel, { props: baseProps({ choices: [] }) });
    expect(screen.queryByText(/Removing a choice leaves cell values untouched/)).toBeNull();
  });
});

describe('ShOptionsPanel — reorder (drag)', () => {
  it('dropping row 0 onto row 2 reorders via onChange, keeping the same 4 values', async () => {
    const onChange = vi.fn();
    render(ShOptionsPanel, { props: baseProps({ onChange }) });
    const row0 = screen.getByTestId('sheet-options-row-0');
    const row2 = screen.getByTestId('sheet-options-row-2');
    await fireEvent.dragStart(row0);
    await fireEvent.dragOver(row2);
    await fireEvent.drop(row2);
    expect(onChange).toHaveBeenCalledOnce();
    const [result] = onChange.mock.calls[0];
    // 'Committed' (index 0) moves to land directly before the ORIGINAL
    // index 2 ('Proposed') — i.e. between 'In flight' and 'Proposed'.
    expect(result).toEqual(['In flight', 'Committed', 'Proposed', 'Cut']);
  });

  it('shows a drop-line indicator while dragging over a row', async () => {
    render(ShOptionsPanel, { props: baseProps() });
    const row0 = screen.getByTestId('sheet-options-row-0');
    const row2 = screen.getByTestId('sheet-options-row-2');
    await fireEvent.dragStart(row0);
    await fireEvent.dragOver(row2);
    expect(screen.getByTestId('sheet-options-drop-line')).toBeTruthy();
  });
});
