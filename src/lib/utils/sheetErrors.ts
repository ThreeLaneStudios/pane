// sheetErrors.ts — static error-code catalogue for the Sheets error
// detail popover (#2339 PR 15b-alpha, ShErrorPopover, §2c). Ported
// verbatim from the design's own `ERRS` array
// (docs/design/sheets/sheets-formula.jsx L83-90) — the six formula error
// codes as they appear in a cell's `computed_value` (produced by the Rust
// formula evaluator, sheets.rs).
//
// Deliberately static and code-keyed ONLY: NO culprit-cell naming (task
// hard constraint). The design's own mockup names a specific offending
// cell in its message ("B2 holds text" — see the Cap prose's own
// admission "The sentence names the culprit cell where we know it");
// this catalogue's messages omit that clause entirely, matching every
// code's message to ONLY the six lines the design shows in its own
// "the six codes · one treatment" list (sheets-formula.jsx's `ERRS`
// array itself, which is culprit-free — only the single illustrated
// `#VALUE!` popover mockup ADDS a culprit clause on top of it). The
// "raw formula" shown is likewise the SAME static demo formula the
// design pairs with each code in that array — this is a lookup table by
// error code, not a per-cell diagnostic (the frontend has no signal for
// which specific reference actually caused a given error; adding one
// would be a backend change, out of scope here).

export interface SheetErrorInfo {
  code: string;
  message: string;
  formula: string;
}

const ERRS: SheetErrorInfo[] = [
  { code: '#REF!', message: 'A reference points at a column or row that no longer exists.', formula: '=SUM(H1:H8)' },
  { code: '#VALUE!', message: 'A value in the formula is the wrong kind — usually text where a number is needed.', formula: '=D3*E3' },
  { code: '#DIV/0!', message: 'Something divides by zero, or by an empty cell.', formula: '=F5/D5' },
  { code: '#NAME?', message: "That function name isn't recognized.", formula: '=TOTAL(F1:F8)' },
  { code: '#PARSE!', message: "The formula can't be read — most often an unclosed bracket or quote.", formula: '=IF(F1>100, "over' },
  { code: '#CYCLE!', message: 'The formula depends on its own result, directly or through another cell.', formula: '=F9+1' },
];

/** Looks up the static message/example-formula pair for a computed error
 *  code (e.g. `'#VALUE!'`, taken verbatim from a cell's
 *  `computed_value`) — `null` for any code outside the six known ones
 *  (defensive; every error code the backend evaluator currently produces
 *  is one of these six, per sheets.rs, but this stays a lookup rather
 *  than an assumption). */
export function getErrorInfo(code: string): SheetErrorInfo | null {
  return ERRS.find((e) => e.code === code) ?? null;
}
