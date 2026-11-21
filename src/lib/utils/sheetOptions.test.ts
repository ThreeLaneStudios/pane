// sheetOptions.test.ts — unit tests for the select-column choices editor's
// pure data-rule helpers (#2339 PR 15b-beta).

import { describe, it, expect } from 'vitest';
import {
  isDuplicateChoice,
  duplicateChoiceMessage,
  addChoice,
  renameChoiceAt,
  removeChoiceAt,
  reorderChoices,
  reorderChoicesByRawDrop,
} from './sheetOptions';
import { isTypeMismatch } from './sheetView';

describe('isDuplicateChoice', () => {
  it('rejects an exact match', () => {
    expect(isDuplicateChoice(['Committed', 'Cut'], 'Committed')).toBe(true);
  });

  it('rejects a case-insensitive match', () => {
    expect(isDuplicateChoice(['Committed', 'Cut'], 'committed')).toBe(true);
    expect(isDuplicateChoice(['Committed', 'Cut'], 'COMMITTED')).toBe(true);
  });

  it('rejects a match with leading/trailing whitespace (trimmed)', () => {
    expect(isDuplicateChoice(['Committed'], '  Committed  ')).toBe(true);
    expect(isDuplicateChoice(['  Committed  '], 'Committed')).toBe(true);
  });

  it('allows a genuinely new value', () => {
    expect(isDuplicateChoice(['Committed', 'Cut'], 'Proposed')).toBe(false);
  });

  it('excludes the given index (rename-against-self is not a collision)', () => {
    expect(isDuplicateChoice(['Committed', 'Cut'], 'committed', 0)).toBe(false);
    // still catches a collision against a DIFFERENT existing row
    expect(isDuplicateChoice(['Committed', 'Cut'], 'cut', 0)).toBe(true);
  });

  it('empty list never has a duplicate', () => {
    expect(isDuplicateChoice([], 'anything')).toBe(false);
  });
});

describe('duplicateChoiceMessage', () => {
  it('quotes the trimmed candidate in straight single quotes', () => {
    expect(duplicateChoiceMessage('Committed')).toBe("'Committed' is already a choice.");
  });

  it('trims before quoting', () => {
    expect(duplicateChoiceMessage('  Committed  ')).toBe("'Committed' is already a choice.");
  });
});

describe('addChoice', () => {
  it('appends the trimmed value at the end', () => {
    expect(addChoice(['A', 'B'], '  C  ')).toEqual(['A', 'B', 'C']);
  });

  it('does not mutate the input array', () => {
    const input = ['A'];
    addChoice(input, 'B');
    expect(input).toEqual(['A']);
  });
});

describe('renameChoiceAt', () => {
  it('replaces only the targeted index, trimmed', () => {
    expect(renameChoiceAt(['A', 'B', 'C'], 1, '  Bee  ')).toEqual(['A', 'Bee', 'C']);
  });

  it('does not mutate the input array', () => {
    const input = ['A', 'B'];
    renameChoiceAt(input, 0, 'Z');
    expect(input).toEqual(['A', 'B']);
  });
});

describe('removeChoiceAt', () => {
  it('removes exactly the targeted index', () => {
    expect(removeChoiceAt(['A', 'B', 'C'], 1)).toEqual(['A', 'C']);
  });

  it('does not mutate the input array', () => {
    const input = ['A', 'B'];
    removeChoiceAt(input, 0);
    expect(input).toEqual(['A', 'B']);
  });

  it("removal doesn't alter stored values — a cell holding the removed choice renders as a mismatch AFTER removal, but its own raw text is untouched", () => {
    const before = ['Committed', 'In flight', 'Proposed'];
    const cellRawValue = 'In flight';

    // Before removal: not a mismatch, the value is a real choice.
    expect(isTypeMismatch(cellRawValue, 'select', before)).toBe(false);

    // Remove "In flight" — the OPTIONS list changes...
    const after = removeChoiceAt(before, 1);
    expect(after).toEqual(['Committed', 'Proposed']);

    // ...but the cell's own raw value is a plain string a caller owns —
    // removeChoiceAt has no cell-mutation side channel; nothing here can
    // reach into sheet_cells. The ONLY thing that changed is the mismatch
    // classification, purely because it's re-evaluated against the new,
    // shorter list on the next render.
    expect(cellRawValue).toBe('In flight');
    expect(isTypeMismatch(cellRawValue, 'select', after)).toBe(true);
  });
});

describe('reorderChoices', () => {
  it('moves an item forward to a later index', () => {
    expect(reorderChoices(['A', 'B', 'C'], 0, 1)).toEqual(['B', 'A', 'C']);
  });

  it('moves an item backward to an earlier index', () => {
    expect(reorderChoices(['A', 'B', 'C'], 2, 0)).toEqual(['C', 'A', 'B']);
  });

  it('does not mutate the input array', () => {
    const input = ['A', 'B', 'C'];
    reorderChoices(input, 0, 2);
    expect(input).toEqual(['A', 'B', 'C']);
  });
});

describe('reorderChoicesByRawDrop', () => {
  it('dragging index 0 to drop-before index 2 lands it between 1 and 2', () => {
    // ['A','B','C'] — drag 'A' (0), drop before raw index 2 ('C') — 'A'
    // should end up directly before 'C', i.e. ['B','A','C'].
    expect(reorderChoicesByRawDrop(['A', 'B', 'C'], 0, 2)).toEqual(['B', 'A', 'C']);
  });

  it('is a no-op when dropped on its own current slot', () => {
    expect(reorderChoicesByRawDrop(['A', 'B', 'C'], 1, 1)).toEqual(['A', 'B', 'C']);
  });

  it('is a no-op when dropped on the identity "just after itself" slot', () => {
    expect(reorderChoicesByRawDrop(['A', 'B', 'C'], 1, 2)).toEqual(['A', 'B', 'C']);
  });

  it('dragging the last item to the front', () => {
    expect(reorderChoicesByRawDrop(['A', 'B', 'C'], 2, 0)).toEqual(['C', 'A', 'B']);
  });

  it('does not mutate the input array', () => {
    const input = ['A', 'B', 'C'];
    reorderChoicesByRawDrop(input, 0, 2);
    expect(input).toEqual(['A', 'B', 'C']);
  });
});
