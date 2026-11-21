/**
 * SearchableSelect.test.ts — #3171 PR 2, C51/C57.
 *
 * SearchableSelect is the generic extraction of SearchableModelSelect's
 * core mechanics (search input, fixed-position dropdown, selection,
 * close-on-outside-click) — item-agnostic: callers supply `filterItem`/
 * `sortItems`/`renderItem`/`getDisplayText`. This file covers the generic
 * mechanics directly with a minimal `{id, label}` item shape (no model
 * concerns — those stay covered by SearchableModelSelect.test.ts, which
 * must keep passing unchanged against this component as its backing
 * implementation).
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

describe('SearchableSelect (generic core, #3171 PR 2)', () => {
  it('renders the placeholder and no value when nothing is selected', () => {
    const { container } = render(SearchableSelect, {
      props: {
        items,
        selectedId: '',
        placeholder: 'Pick one...',
        onSelect: vi.fn(),
        getDisplayText: byLabel,
        filterItem: (item: any, lower: string) => item.label.toLowerCase().includes(lower),
        sortItems: (list: any[]) => [...list].sort((a, b) => a.label.localeCompare(b.label)),
        renderItem: labelSnippet(),
      },
    });

    const input = container.querySelector('.select-input-field') as HTMLInputElement;
    expect(input.placeholder).toBe('Pick one...');
    expect(input.value).toBe('');
  });

  it('uses getDisplayText to resolve the closed-state value for a selected id', () => {
    const { container } = render(SearchableSelect, {
      props: {
        items,
        selectedId: 'b',
        onSelect: vi.fn(),
        getDisplayText: byLabel,
        filterItem: (item: any, lower: string) => item.label.toLowerCase().includes(lower),
        sortItems: (list: any[]) => list,
        renderItem: labelSnippet(),
      },
    });

    const input = container.querySelector('.select-input-field') as HTMLInputElement;
    expect(input.value).toBe('Bravo');
    expect(input.title).toBe('Bravo');
  });

  it('opens on focus and lists items sorted via the supplied sortItems', async () => {
    const { container } = render(SearchableSelect, {
      props: {
        items,
        selectedId: '',
        onSelect: vi.fn(),
        getDisplayText: byLabel,
        filterItem: (item: any, lower: string) => item.label.toLowerCase().includes(lower),
        sortItems: (list: any[]) => [...list].sort((a, b) => b.label.localeCompare(a.label)), // reverse
        renderItem: labelSnippet(),
      },
    });

    await fireEvent.focus(container.querySelector('.select-input-field') as HTMLInputElement);

    const labels = Array.from(container.querySelectorAll('.item-label')).map((el) => el.textContent);
    expect(labels).toEqual(['Charlie', 'Bravo', 'Alpha']);
  });

  it('filters items via the supplied filterItem predicate while searchText is non-empty', async () => {
    const filterItem = vi.fn((item: any, lower: string) => item.label.toLowerCase().includes(lower));
    const { container } = render(SearchableSelect, {
      props: {
        items,
        selectedId: '',
        onSelect: vi.fn(),
        getDisplayText: byLabel,
        filterItem,
        sortItems: (list: any[]) => list,
        renderItem: labelSnippet(),
      },
    });

    const input = container.querySelector('.select-input-field') as HTMLInputElement;
    await fireEvent.focus(input);
    await fireEvent.input(input, { target: { value: 'bra' } });

    const labels = Array.from(container.querySelectorAll('.item-label')).map((el) => el.textContent);
    expect(labels).toEqual(['Bravo']);
    expect(filterItem).toHaveBeenCalled();
  });

  it('clicking an item calls onSelect with its id and closes the dropdown', async () => {
    const onSelect = vi.fn();
    const { container } = render(SearchableSelect, {
      props: {
        items,
        selectedId: '',
        onSelect,
        getDisplayText: byLabel,
        filterItem: (item: any, lower: string) => item.label.toLowerCase().includes(lower),
        sortItems: (list: any[]) => list,
        renderItem: labelSnippet(),
      },
    });

    const input = container.querySelector('.select-input-field') as HTMLInputElement;
    await fireEvent.focus(input);

    const charlieBtn = Array.from(container.querySelectorAll('.dropdown-item')).find(
      (el) => el.textContent?.includes('Charlie')
    ) as HTMLButtonElement;
    await fireEvent.mouseDown(charlieBtn);

    expect(onSelect).toHaveBeenCalledWith('c');
    expect(container.querySelector('.dropdown-list')).toBeNull();
  });

  it('renders the selected item with the "selected" class', async () => {
    const { container } = render(SearchableSelect, {
      props: {
        items,
        selectedId: 'b',
        onSelect: vi.fn(),
        getDisplayText: byLabel,
        filterItem: (item: any, lower: string) => item.label.toLowerCase().includes(lower),
        sortItems: (list: any[]) => list,
        renderItem: labelSnippet(),
      },
    });

    await fireEvent.focus(container.querySelector('.select-input-field') as HTMLInputElement);
    const items_ = Array.from(container.querySelectorAll('.dropdown-item'));
    const selected = items_.filter((el) => el.classList.contains('selected'));
    expect(selected.length).toBe(1);
    expect(selected[0].textContent).toContain('Bravo');
  });

  it('a footer snippet renders inside the dropdown and its closeDropdown callback closes it', async () => {
    const footerClick = vi.fn();
    const footer = createRawSnippet((close: () => () => void) => ({
      render: () => `<button type="button" class="footer-btn">Add new...</button>`,
      setup: (node: HTMLElement) => {
        node.addEventListener('mousedown', (e) => {
          e.preventDefault();
          footerClick();
          close()();
        });
      },
    }));

    const { container } = render(SearchableSelect, {
      props: {
        items,
        selectedId: '',
        onSelect: vi.fn(),
        getDisplayText: byLabel,
        filterItem: (item: any, lower: string) => item.label.toLowerCase().includes(lower),
        sortItems: (list: any[]) => list,
        renderItem: labelSnippet(),
        footer,
      },
    });

    const input = container.querySelector('.select-input-field') as HTMLInputElement;
    await fireEvent.focus(input);
    expect(container.querySelector('.footer-btn')).toBeTruthy();

    await fireEvent.mouseDown(container.querySelector('.footer-btn') as HTMLButtonElement);
    expect(footerClick).toHaveBeenCalled();
    expect(container.querySelector('.dropdown-list')).toBeNull();
  });

  it('with no searchText, filterItem is never called — the full (sorted) list renders unfiltered', async () => {
    const filterItem = vi.fn(() => false);
    const { container } = render(SearchableSelect, {
      props: {
        items,
        selectedId: '',
        onSelect: vi.fn(),
        getDisplayText: byLabel,
        filterItem,
        sortItems: (list: any[]) => list,
        renderItem: labelSnippet(),
      },
    });

    await fireEvent.focus(container.querySelector('.select-input-field') as HTMLInputElement);
    const labels = Array.from(container.querySelectorAll('.item-label')).map((el) => el.textContent);
    expect(labels).toEqual(['Alpha', 'Bravo', 'Charlie']);
    expect(filterItem).not.toHaveBeenCalled();
  });
});
