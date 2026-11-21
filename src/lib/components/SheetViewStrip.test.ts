// SheetViewStrip.test.ts — component tests for the sort/filter view strip
// (#2339 PR 12, §33.7). Pure props-in, callback-out component; no store,
// no api mock needed — `SheetGrid.svelte` owns `viewState` and passes
// resolved props straight through.

import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte/svelte5';
import SheetViewStrip from './SheetViewStrip.svelte';
import type { SheetColumn } from '$lib/types';

function col(overrides: Partial<SheetColumn> = {}): SheetColumn {
  return {
    id: overrides.id ?? 'col-a',
    note_id: 'note-1',
    name: overrides.name ?? 'Status',
    col_type: overrides.col_type ?? 'select',
    options_json: overrides.options_json ?? '{}',
    width: overrides.width ?? 148,
    sort_order: overrides.sort_order ?? 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

const noop = () => {};

describe('SheetViewStrip — presence gate', () => {
  it('renders nothing when both sortColumn and filterColumn are null', () => {
    render(SheetViewStrip, {
      props: {
        sortColumn: null,
        sortDirection: null,
        filterColumn: null,
        filterValues: null,
        onClearSort: noop,
        onClearFilter: noop,
      },
    });
    expect(screen.queryByTestId('sheet-view-strip')).toBeNull();
  });
});

describe('SheetViewStrip — sort chip', () => {
  it('renders only the sort chip when only a sort is active', () => {
    render(SheetViewStrip, {
      props: {
        sortColumn: col({ name: 'Total', col_type: 'number' }),
        sortDirection: 'desc',
        filterColumn: null,
        filterValues: null,
        onClearSort: noop,
        onClearFilter: noop,
      },
    });
    expect(screen.getByTestId('sheet-view-strip')).toBeTruthy();
    expect(screen.getByTestId('sheet-view-chip-sort')).toBeTruthy();
    expect(screen.queryByTestId('sheet-view-chip-filter')).toBeNull();
    expect(screen.getByTestId('sheet-view-chip-sort').textContent).toContain('Total');
    expect(screen.getByTestId('sheet-view-chip-sort').textContent).toContain('descending');
  });

  it('spells out "ascending" for an ascending sort', () => {
    render(SheetViewStrip, {
      props: {
        sortColumn: col({ name: 'Total', col_type: 'number' }),
        sortDirection: 'asc',
        filterColumn: null,
        filterValues: null,
        onClearSort: noop,
        onClearFilter: noop,
      },
    });
    expect(screen.getByTestId('sheet-view-chip-sort').textContent).toContain('ascending');
  });

  it('bolds only the column name inside the sort chip', () => {
    render(SheetViewStrip, {
      props: {
        sortColumn: col({ name: 'Total', col_type: 'number' }),
        sortDirection: 'desc',
        filterColumn: null,
        filterValues: null,
        onClearSort: noop,
        onClearFilter: noop,
      },
    });
    const chip = screen.getByTestId('sheet-view-chip-sort');
    const b = chip.querySelector('b');
    expect(b?.textContent).toBe('Total');
  });

  it('clicking the sort clear button calls onClearSort exactly once', async () => {
    const onClearSort = vi.fn();
    render(SheetViewStrip, {
      props: {
        sortColumn: col({ name: 'Total', col_type: 'number' }),
        sortDirection: 'desc',
        filterColumn: null,
        filterValues: null,
        onClearSort,
        onClearFilter: noop,
      },
    });
    const { fireEvent } = await import('@testing-library/svelte/svelte5');
    await fireEvent.click(screen.getByTestId('sheet-view-chip-sort-clear'));
    expect(onClearSort).toHaveBeenCalledTimes(1);
  });
});

describe('SheetViewStrip — filter chip', () => {
  it('renders only the filter chip when only a filter is active', () => {
    render(SheetViewStrip, {
      props: {
        sortColumn: null,
        sortDirection: null,
        filterColumn: col({ name: 'Status', col_type: 'select', options_json: '{"choices":["Lead","Contacted","Won","Cut"]}' }),
        filterValues: ['Lead', 'Contacted', 'Won'],
        onClearSort: noop,
        onClearFilter: noop,
      },
    });
    expect(screen.getByTestId('sheet-view-strip')).toBeTruthy();
    expect(screen.getByTestId('sheet-view-chip-filter')).toBeTruthy();
    expect(screen.queryByTestId('sheet-view-chip-sort')).toBeNull();
    // Matt's ruling 1: N-1 of N selected reads as exclusion.
    expect(screen.getByTestId('sheet-view-chip-filter').textContent).toContain('Status');
    expect(screen.getByTestId('sheet-view-chip-filter').textContent).toContain('is not Cut');
  });

  it('renders positive list phrasing when not N-1 of N', () => {
    render(SheetViewStrip, {
      props: {
        sortColumn: null,
        sortDirection: null,
        filterColumn: col({ name: 'Status', col_type: 'select', options_json: '{"choices":["Lead","Contacted","Won","Cut"]}' }),
        filterValues: ['Lead'],
        onClearSort: noop,
        onClearFilter: noop,
      },
    });
    expect(screen.getByTestId('sheet-view-chip-filter').textContent).toContain('is Lead');
  });

  it('renders True/False (not raw 1/0) for a checkbox filter chip', () => {
    render(SheetViewStrip, {
      props: {
        sortColumn: null,
        sortDirection: null,
        filterColumn: col({ name: 'Active', col_type: 'checkbox' }),
        filterValues: ['1'],
        onClearSort: noop,
        onClearFilter: noop,
      },
    });
    // N-1 of 2 (checkbox choices are ['0','1']) selected → exclusion phrasing.
    expect(screen.getByTestId('sheet-view-chip-filter').textContent).toContain('is not False');
  });

  it('bolds only the column name inside the filter chip', () => {
    render(SheetViewStrip, {
      props: {
        sortColumn: null,
        sortDirection: null,
        filterColumn: col({ name: 'Status', col_type: 'select', options_json: '{"choices":["Lead","Cut"]}' }),
        filterValues: ['Lead'],
        onClearSort: noop,
        onClearFilter: noop,
      },
    });
    const chip = screen.getByTestId('sheet-view-chip-filter');
    const b = chip.querySelector('b');
    expect(b?.textContent).toBe('Status');
  });

  it('clicking the filter clear button calls onClearFilter exactly once', async () => {
    const onClearFilter = vi.fn();
    render(SheetViewStrip, {
      props: {
        sortColumn: null,
        sortDirection: null,
        filterColumn: col({ name: 'Status', col_type: 'select', options_json: '{"choices":["Lead","Cut"]}' }),
        filterValues: ['Lead'],
        onClearSort: noop,
        onClearFilter,
      },
    });
    const { fireEvent } = await import('@testing-library/svelte/svelte5');
    await fireEvent.click(screen.getByTestId('sheet-view-chip-filter-clear'));
    expect(onClearFilter).toHaveBeenCalledTimes(1);
  });
});

describe('SheetViewStrip — both chips active', () => {
  it('renders both chips, sort ALWAYS left of filter, regardless of DOM query order (ruling 4)', () => {
    render(SheetViewStrip, {
      props: {
        sortColumn: col({ id: 'c1', name: 'Total', col_type: 'number' }),
        sortDirection: 'desc',
        filterColumn: col({ id: 'c2', name: 'Status', col_type: 'select', options_json: '{"choices":["Lead","Cut"]}' }),
        filterValues: ['Lead'],
        onClearSort: noop,
        onClearFilter: noop,
      },
    });
    const strip = screen.getByTestId('sheet-view-strip');
    const chips = strip.querySelectorAll('[data-testid^="sheet-view-chip-"]');
    // First two matches are the chips themselves (not their clear buttons,
    // which use a `-clear` suffix and thus also match `^=`, so filter to
    // just the chip-level testids).
    const chipIds = Array.from(chips)
      .map((el) => el.getAttribute('data-testid'))
      .filter((id) => id === 'sheet-view-chip-sort' || id === 'sheet-view-chip-filter');
    expect(chipIds).toEqual(['sheet-view-chip-sort', 'sheet-view-chip-filter']);
  });

  it('clearing sort while filter is active independently calls only onClearSort', async () => {
    const onClearSort = vi.fn();
    const onClearFilter = vi.fn();
    render(SheetViewStrip, {
      props: {
        sortColumn: col({ id: 'c1', name: 'Total', col_type: 'number' }),
        sortDirection: 'desc',
        filterColumn: col({ id: 'c2', name: 'Status', col_type: 'select', options_json: '{"choices":["Lead","Cut"]}' }),
        filterValues: ['Lead'],
        onClearSort,
        onClearFilter,
      },
    });
    const { fireEvent } = await import('@testing-library/svelte/svelte5');
    await fireEvent.click(screen.getByTestId('sheet-view-chip-sort-clear'));
    expect(onClearSort).toHaveBeenCalledTimes(1);
    expect(onClearFilter).not.toHaveBeenCalled();
  });
});

describe('SheetViewStrip — note text', () => {
  it('always shows the fixed "View only" note whenever the strip is visible', () => {
    render(SheetViewStrip, {
      props: {
        sortColumn: col({ name: 'Total', col_type: 'number' }),
        sortDirection: 'asc',
        filterColumn: null,
        filterValues: null,
        onClearSort: noop,
        onClearFilter: noop,
      },
    });
    expect(screen.getByText('View only — row order and stored data are unchanged.')).toBeTruthy();
  });
});
