// sheetOptions.ts — pure helpers for the select-column choices editor
// (`ShOptionsPanel.svelte`, #2339 PR 15b-beta, `docs/design/sheets/
// sheets-addendum.jsx:120-270`, NOTES.md §B). DOM-free, fully unit-tested,
// following the same "component owns interaction, this file owns the
// data rules" split as `sheetView.ts`/`sheetSelection.ts`.

import { normalizeInsertIndex } from './dragReorderIndex';

/** Trimmed, case-insensitive duplicate check — the exact rule NOTES.md §B
 *  specifies ("Duplicate names are rejected with a signal ... trimmed +
 *  case-insensitive"). `excludeIndex` lets a RENAME check against every
 *  choice except the one being edited (renaming "Cut" to "cut" is a no-op
 *  case-normalization, not a collision with itself). */
export function isDuplicateChoice(choices: string[], candidate: string, excludeIndex?: number): boolean {
  const norm = candidate.trim().toLowerCase();
  return choices.some((c, idx) => idx !== excludeIndex && c.trim().toLowerCase() === norm);
}

/** The one-line collision message. The design mockup
 *  (`sheets-addendum.jsx`) uses curly double quotes ("Committed" is
 *  already a choice.); this PR's task brief instead specifies straight
 *  single quotes around the name ('Committed' is already a choice.) —
 *  following the task brief's exact wording since it's the more recent,
 *  more specific instruction. */
export function duplicateChoiceMessage(candidate: string): string {
  return `'${candidate.trim()}' is already a choice.`;
}

/** Appends a new choice, trimmed. Caller is responsible for calling
 *  `isDuplicateChoice` first — this function does not itself reject
 *  duplicates (kept as a separate concern so the panel can show the
 *  rejection UI instead of this function silently no-oping or throwing). */
export function addChoice(choices: string[], value: string): string[] {
  return [...choices, value.trim()];
}

/** Renames the choice at `index` to `value`, trimmed. Same
 *  caller-checks-duplicates contract as `addChoice`. */
export function renameChoiceAt(choices: string[], index: number, value: string): string[] {
  const next = [...choices];
  next[index] = value.trim();
  return next;
}

/** Removes the choice at `index`. This ONLY ever touches the returned
 *  `options_json.choices` array — per NOTES.md §B ("Removal does not
 *  touch stored values"), there is no cell-value side effect anywhere in
 *  this module; a cell holding the removed choice keeps its raw stored
 *  text untouched and starts rendering as a mismatch purely because
 *  `isTypeMismatch` (sheetView.ts) is re-evaluated against the NEW,
 *  shorter choice list on the next render — see
 *  `sheetOptions.test.ts`'s dedicated "removal doesn't alter stored
 *  values" test, which exercises exactly that render-derived path. */
export function removeChoiceAt(choices: string[], index: number): string[] {
  return choices.filter((_, i) => i !== index);
}

/** Reorders `choices`: moves the item at `fromIndex` so it lands at
 *  `insertIndex` (an already-removal-adjusted index — i.e. the position
 *  the moved item should occupy in the ARRAY AFTER it's spliced out of
 *  `fromIndex`, exactly `normalizeInsertIndex`'s contract, reused
 *  verbatim from `dragReorderIndex.ts` so this drag gesture matches "the
 *  same gesture the Kanban cards use" per the design's own Cap prose). */
export function reorderChoices(choices: string[], fromIndex: number, insertIndex: number): string[] {
  const next = [...choices];
  const [moved] = next.splice(fromIndex, 1);
  next.splice(insertIndex, 0, moved);
  return next;
}

/** Convenience wrapper combining `normalizeInsertIndex` + `reorderChoices`
 *  for the component's drop handler — `rawDropIndex` is the raw "insert
 *  before this ORIGINAL index" position (before the dragged item is
 *  removed), matching `dragReorderIndex.ts`'s own `resolveDropIndex`
 *  convention. */
export function reorderChoicesByRawDrop(choices: string[], fromIndex: number, rawDropIndex: number): string[] {
  if (fromIndex === rawDropIndex || rawDropIndex === fromIndex + 1) return [...choices];
  const insertIndex = normalizeInsertIndex(fromIndex, rawDropIndex);
  return reorderChoices(choices, fromIndex, insertIndex);
}
