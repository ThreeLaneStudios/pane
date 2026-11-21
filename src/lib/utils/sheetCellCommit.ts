// sheetCellCommit.ts — pure value parsing/formatting helpers for the Sheets
// per-type cell editors (#2339 PR 10, §2/§7). DOM-free, unit-tested.
//
// Backend contract (confirmed against sheets.rs — see the PR 10 scope
// report §6): `update_sheet_cell`'s `value` is stored VERBATIM, with zero
// server-side type enforcement beyond the leading-`=` formula-parse check.
// Every typed-format concern here (date mask↔ISO, currency display
// formatting) is therefore a pure frontend authoring/display choice, not a
// backend requirement — these helpers exist so SheetGrid.svelte's editors
// don't hand-roll parsing inline, and so the logic is testable without
// mounting a component.

/**
 * Converts a user-typed date string into an ISO-8601 `YYYY-MM-DD` string,
 * per Matt's ruling 4 (masked `MM/DD/YYYY` text input, converts to ISO on
 * commit). Tolerant of `M/D/YY` per the design's stated leniency
 * (`ShEditors`: "accepts 08/04/2026 or 8/4/26"). Two-digit years use the
 * common pivot: `< 50` → `20xx`, `>= 50` → `19xx`.
 *
 * Returns `null` if the input doesn't parse as a real calendar date (e.g.
 * `13/40/2026`, non-numeric, wrong shape) — callers fall back to storing
 * the raw typed string (§33.3.1's "unparseable values render as text"
 * baseline; PR 10 doesn't build the `ShMismatch` indicator, Matt's ruling
 * 6, but the write must never be blocked or throw).
 */
export function parseDateMaskToIso(input: string): string | null {
  const trimmed = input.trim();
  const m = trimmed.match(/^(\d{1,2})\/(\d{1,2})\/(\d{2}|\d{4})$/);
  if (!m) return null;

  const month = parseInt(m[1], 10);
  const day = parseInt(m[2], 10);
  let year = parseInt(m[3], 10);
  if (m[3].length === 2) {
    year = year < 50 ? 2000 + year : 1900 + year;
  }

  if (month < 1 || month > 12 || day < 1 || day > 31) return null;

  // Round-trip through Date to reject calendar-invalid combinations (e.g.
  // 02/30/2026, 04/31/2026) — Date's own overflow normalization (Feb 30 ->
  // Mar 2) would otherwise silently accept them.
  const date = new Date(year, month - 1, day);
  if (date.getFullYear() !== year || date.getMonth() !== month - 1 || date.getDate() !== day) {
    return null;
  }

  const mm = String(month).padStart(2, '0');
  const dd = String(day).padStart(2, '0');
  return `${year}-${mm}-${dd}`;
}

/**
 * Converts a stored ISO-8601 `YYYY-MM-DD` string into the `MM/DD/YYYY`
 * mask the date editor pre-fills with on open. Falls back to returning the
 * input unchanged if it isn't ISO-shaped (defensive — a mismatched/legacy
 * value should still be visible and editable, not blanked).
 */
export function isoToDateMask(iso: string): string {
  const m = iso.trim().match(/^(\d{4})-(\d{2})-(\d{2})$/);
  if (!m) return iso;
  return `${m[2]}/${m[3]}/${m[1]}`;
}

/**
 * Display-only currency formatting (Matt's ruling 3: the STORED value is
 * always the raw typed string — this function is never called before a
 * commit, only when rendering the idle/read state). Formats a raw numeric
 * string to fixed 2-decimal-place, comma-grouped form (e.g. `"1400"` ->
 * `"1,400.00"`), matching the design's stated commit-time display
 * transform. Non-numeric input passes through unchanged (mismatch-safe —
 * never throws, never blanks a value that doesn't parse as a number).
 */
export function formatCurrencyDisplay(raw: string): string {
  if (raw === '' || raw == null) return '';
  const n = Number(raw);
  if (!Number.isFinite(n)) return raw;
  return n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

/**
 * Resolves the string an editor's commit handler should pass to
 * `update_sheet_cell`'s `value` param, given the column's type and the
 * editor's raw typed buffer.
 *
 * - `date`: converts a valid `MM/DD/YYYY`/`M/D/YY` mask to ISO-8601;
 *   unparseable input is stored VERBATIM (never blocked — matches every
 *   other type's backend permissiveness, §33.3.1's baseline).
 * - `text` / `number` / `currency` / `select`: stored verbatim — no
 *   client-side reformatting on commit (currency's `1,400.00` display
 *   form is a read-time transform only, via `formatCurrencyDisplay`,
 *   never applied to the stored value per ruling 3).
 * - `checkbox` has no editor (§2) and doesn't go through this path — see
 *   `toggleCheckboxValue` below.
 */
export function valueForCommit(colType: string, typed: string): string {
  if (colType === 'date') {
    const iso = parseDateMaskToIso(typed);
    return iso ?? typed;
  }
  return typed;
}

/**
 * The next value a checkbox toggle should commit, given the cell's current
 * `raw_value`. Mirrors the existing read-render "on" predicate
 * (`!!raw_value && raw_value !== '0'`) so toggling is the exact inverse of
 * what's currently displayed. Empty/`undefined`/`'0'` are all "off"; any
 * other non-empty string (including a legacy/mismatched value) is "on" and
 * toggles to `'0'`.
 */
export function toggleCheckboxValue(raw: string | null | undefined): string {
  const isOn = !!raw && raw !== '0';
  return isOn ? '0' : '1';
}
