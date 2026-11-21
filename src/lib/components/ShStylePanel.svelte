<script lang="ts">
  // ShStylePanel.svelte — the Sheets cell-styling picker (#2339 PR
  // 15d-beta, `docs/design/sheets/sheets-formula.jsx` §3 "CELL STYLING",
  // `StylePanel` component ~L198, `docs/design/sheets/sheets.css:233-249`
  // `.sh-sty*`/`.sh-sw*`/`.sh-seg*`). "Two doors, one panel" (design Cap
  // prose): opened either from the toolbar's fill/text/border buttons or
  // from `ShCellContextMenu`'s "Style cells…" row — both entry points
  // mount THIS same component with the same props.
  //
  // Same "dumb popover shell with callback props" division of labor as
  // `ShSheetMenu.svelte`/`ShCellContextMenu.svelte`: this component owns
  // its own open/close-on-outside-click/Escape lifecycle, but every verb
  // (apply a fill, apply an ink, apply a border spec, clear all styling)
  // is a plain callback prop — all the actual selection-expansion/
  // per-cell-write logic lives in `SheetGrid.svelte`
  // (`applyStyleToSelection`, `sheetCellStyle.ts`).
  //
  // Positioning: unlike `ShSheetMenu`/`ShHeaderMenu` (anchor off a static
  // button ref) or `ShCellContextMenu` (anchor off a raw click
  // coordinate), this panel is positioned by the CALLER via plain `x`/`y`
  // props — `SheetGrid.svelte` computes a fixed offset near wherever it
  // was triggered from (toolbar button area or the context menu's own
  // click point) and hands over the final viewport coordinate directly,
  // since both entry points already have their own boundary-flip-aware
  // anchor math (the toolbar button's rect, or the context menu's own x/y)
  // and there's no benefit to re-deriving a second flip calculation here.
  //
  // Unanimity convention (Matt's ruling 4, scope report "Matt's Rulings"):
  // `currentBg`/`currentFg`/`currentBorder` are `null` whenever the active
  // selection's cells DISAGREE on that value — in that case no swatch/
  // segment shows `is-on` at all (the standard indeterminate-checkbox
  // convention), never a guess. `SheetGrid.svelte` computes these via
  // `detectUnanimity`/`detectBorderUnanimity` (sheetCellStyle.ts) before
  // passing them down; this component does zero unanimity logic itself.
  // #2874 item 29: border sides became independent additive toggles —
  // Top/Btm/Left/Rgt can be on simultaneously (e.g. a cell can have both
  // a top AND a left border at once, which the underlying `bd:
  // BorderSpec[]` data shape already supported — see sheetCellStyle.ts).
  // `currentBorder.sides` reports EVERY side the active selection is NOT
  // unanimously borderless on (not just the first match, as the old
  // single-side model did) — `SheetGrid.svelte`'s `panelBorder`
  // derivation computes this. `w`/`dashed` are nullable: Matt's ruling
  // (scope report `docs/audits/2026-09-13-item-29-scope.md`, "Matt's
  // Rulings" #1) is that when the non-absent sides disagree on
  // weight/dashed, NEITHER the weight nor the dashed control highlights
  // anything (option B, indeterminate) — `SheetGrid.svelte` passes
  // `w: null`/`dashed: null` in that case rather than guessing from one
  // side.
  //
  // #2874 item 45 (Matt's ruling, 2026-09-24): a side can now itself be
  // individually MIXED — some cells in the selection have that side
  // bordered and others don't, or all have it but disagree on w/c/dashed
  // (previously `detectBorderUnanimity` collapsed this into the same
  // "absent" signal as a genuinely borderless side, which is item 45's
  // bug). Matt's ruling is literal: for a mixed side, the side button
  // STAYS LIT (so it must still be included in `currentBorder.sides`,
  // same as a fully-unanimous side — this component does not
  // distinguish the two, by design: both just mean "this side has
  // border data, show it as on"), while "None" and every weight/dashed
  // control stay dark. "None" staying dark falls out of the existing
  // `borderState.mode === 'sides' && borderState.sides.size === 0` check
  // below (#2874 item 53 renamed this file's local state from a bare
  // `activeSides` Set to the tagged `borderState`; this paragraph's
  // reasoning is otherwise unchanged) once a mixed side is present in
  // `sides` (non-empty set); weight/dashed staying dark falls out of
  // `SheetGrid.svelte` reporting `w: null`/`dashed: null` whenever any
  // side is mixed — no `is-on` class here ever matches `null`. See
  // `handleSideClick`'s guard comment for the one behavior change this
  // DOES require in this file: a click must not silently resolve that
  // `null` to a fallback weight of 1 while it's still indeterminate.
  //
  // Delta-audit fix (Reed's 2026-09-24 audit of PR #2963, item 2 FAIL —
  // `docs/audits/2026-09-24-pr-2963-item45.md`): the guard above only
  // ever covered `handleSideClick`. `handleWeightClick`/
  // `handleDashedClick` were untouched by the original item-45 diff and
  // had no equivalent guard on the OTHER axis — clicking Dashed while
  // weight was still indeterminate (same-side mix, OR the pre-existing
  // item-29 cross-side mix) fired `applyCurrentBorder()`'s shared
  // `w: borderWeight ?? 1` fallback, silently clobbering every active
  // side's real, differing per-cell weight down to a flat 1px — the
  // EXACT clobber Matt's ruling prohibits, just reached through the
  // other button. Matt's authoritative fix (2026-09-24, quoted in full
  // in the fix commit): "A click in the border panel changes only what
  // was clicked, and every cell keeps its own value for everything
  // else. Clicking dashed on a mixed selection makes each cell dashed
  // while keeping its own weight and color; clicking a weight works the
  // same way for dashed. No overwrites, no dead clicks. Side-button
  // clicks keep the wait guard as built."
  //
  // Per that ruling, `handleWeightClick`/`handleDashedClick` no longer
  // route through `applyCurrentBorder()` (the shared-value, full-spec
  // apply `handleSideClick`/"All"/"None" still use, UNCHANGED — "side-
  // button clicks keep the wait guard as built") at all. They instead
  // fire their OWN dedicated callback props (`onApplyBorderWeight`/
  // `onApplyBorderDashed`) that carry ONLY the one attribute actually
  // clicked plus the active side set — `SheetGrid.svelte`'s
  // `applyStyleToSelection` resolves the PER-CELL write from there,
  // reading each cell's own existing weight/dashed/color for the
  // attribute NOT clicked rather than this panel's single shared
  // `borderWeight`/`borderDashed` `$state` (which is exactly the value
  // that can legitimately be `null`/indeterminate for a mixed
  // selection — using it as a fallback was the bug). This also means a
  // weight or dashed click can never be a "dead click" (it always fires,
  // regardless of whether the OTHER axis is resolved) while never
  // overwriting a real per-cell value it wasn't asked to change.
  interface BorderCurrentValue {
    sides: Array<'top' | 'bottom' | 'left' | 'right'>;
    w: number | null;
    dashed: boolean | null;
    /** #2874 item 53 (Matt's ruling, 2026-10-04): which of the panel's
     *  two modes this seed represents — `'all'` ONLY when every
     *  selected cell individually has all four border sides present;
     *  `'sides'` otherwise, including the case where each of the four
     *  directions is present on SOME cell in the selection (so this
     *  file's per-side unanimity loop reports all four as "active")
     *  but no single cell actually has all four at once — e.g. an
     *  outline shape. `sides.length === 4` cannot distinguish these two
     *  cases; `SheetGrid.svelte`'s `panelBorder` derivation computes
     *  `mode` by checking every cell's own `bd` directly. Absent
     *  (`undefined`) is treated the same as `'sides'` — the safe,
     *  outline-only default — so a caller that doesn't know better can
     *  never accidentally seed the fill-everything mode. */
    mode?: 'all' | 'sides';
  }

  interface Props {
    currentBg: string | null;
    currentFg: string | null;
    currentBorder: BorderCurrentValue | null;
    onApplyFill: (color: string) => void;
    onApplyInk: (color: string) => void;
    /** #2874 item 53 (Matt's ruling, 2026-10-04): carries the panel's
     *  tagged `mode` alongside the resolved side list — `'all'` means
     *  "fill every side on every cell, inner lines included" (the
     *  explicit All click, and ONLY that); `'sides'` means "outline
     *  only, via the existing outer-edge rule", even when `sides`
     *  happens to be all four (four individually-toggled sides).
     *  `SheetGrid.svelte`'s `applyStyleToSelection` gates its fast path
     *  on THIS field now, never on `sides.length === 4`. */
    onApplyBorder: (spec: { mode: 'all' | 'sides'; sides: string[]; w: number; dashed: boolean }) => void;
    /** #2874 item 59 (Matt's ruling, 2026-10-07 — supersedes the prior
     *  item 59 fix, which routed every ordinary individual-side click
     *  through `onApplyBorder`'s full `sides` snapshot and was found to
     *  rebuild/delete borders the user never touched). Fired by
     *  `handleSideClick` for an ORDINARY side toggle — any click that is
     *  NOT "All", "None", or the one-time redraw-as-outline transition
     *  OUT of "All" mode (those three still go through `onApplyBorder`
     *  above, since each of them is a genuine full-snapshot action, not
     *  a delta — see `handleSideClick`'s own doc comment). Carries
     *  exactly the two things Matt's ruling says the apply needs to
     *  know: `side` (which one was clicked) and `on` (which direction —
     *  `true` if this click just turned it on, `false` if off), computed
     *  by diffing the clicked side against `borderState`'s OWN set
     *  immediately before updating it (see `handleSideClick`). `w`/
     *  `dashed` are only meaningful when `on` is `true` (a fresh entry
     *  needs a concrete weight/dashed to write) — irrelevant when `on`
     *  is `false`, since turning a side off never writes a weight or
     *  dashed value at all, it only removes. `SheetGrid.svelte`'s
     *  `applyStyleToSelection` resolves the per-cell write from here:
     *  on preserves every other stored side untouched and only adds the
     *  clicked one where this cell is on that side's outer edge; off
     *  removes just the clicked side from every selected cell, also
     *  leaving every other stored side exactly as it was. */
    onApplyBorderSide: (spec: { side: 'top' | 'bottom' | 'left' | 'right'; on: boolean; w: number; dashed: boolean }) => void;
    /** Delta-audit fix (2026-09-24): fired by `handleWeightClick` ONLY —
     *  carries the one attribute actually clicked (`w`) plus the
     *  currently-active side set. Does NOT carry `dashed` —
     *  `SheetGrid.svelte`'s `applyStyleToSelection` preserves each
     *  cell's own existing dashed state per side rather than applying
     *  this panel's shared (possibly-indeterminate) value. */
    onApplyBorderWeight: (spec: { sides: string[]; w: number }) => void;
    /** Same as `onApplyBorderWeight`, mirrored for the dashed toggle —
     *  carries only `dashed`; `w`/`c` are preserved per cell per side by
     *  the caller. */
    onApplyBorderDashed: (spec: { sides: string[]; dashed: boolean }) => void;
    onClearStyling: () => void;
    onClose: () => void;
    x?: number;
    y?: number;
    /** #2874 item 16 Part B: the element that opened this panel (a toolbar
     *  fill/text/border button), when opened from the toolbar door —
     *  `undefined`/`null` for the context-menu door, which has no
     *  persistent anchor element (just a raw click coordinate, already
     *  folded into `x`/`y`). `handleWindowClick` excludes clicks landing
     *  on this element from the "outside" branch — the SAME guard its
     *  three sibling popovers (`ShSheetMenu`, `ShHeaderMenu`,
     *  `ShCellContextMenu`) already apply via `anchorEl?.contains(target)`.
     *  `ShStylePanel` was the only one of the four missing it (see
     *  `docs/audits/2026-09-08-item16-partB-focus-loss-diagnosis.md` §5) —
     *  added here as the same defensive hardening that diagnosis
     *  recommended, regardless of whether it was the ACTIVE cause of
     *  Matt's reported symptom in the packaged DMG build. */
    anchorEl?: HTMLElement | null;
  }

  let { currentBg, currentFg, currentBorder, onApplyFill, onApplyInk, onApplyBorder, onApplyBorderSide, onApplyBorderWeight, onApplyBorderDashed, onClearStyling, onClose, x = 0, y = 0, anchorEl = null }: Props = $props();

  // Seven fills, seven inks — hardcoded per the design's own `FILLS`/
  // `INKS` arrays (sheets-formula.jsx L188-195). "None" is fill index 0
  // (`transparent`) — clicking it applies `onApplyFill('transparent')`,
  // which `SheetGrid.svelte`'s `applyStyleToSelection` treats as "clear
  // the fill" (same convention `mergeStylePatch`'s `null` uses at the
  // data layer — the panel just needs a concrete sentinel string to pass
  // through the callback prop).
  const FILLS: Array<[string, string]> = [
    ['none', 'transparent'],
    ['slate', '#232327'],
    ['cyan', 'rgba(0,228,228,0.14)'],
    ['green', 'rgba(34,197,94,0.14)'],
    ['amber', 'rgba(245,158,11,0.14)'],
    ['red', 'rgba(239,68,68,0.13)'],
    ['violet', 'rgba(139,92,246,0.15)'],
  ];

  const INKS: Array<[string, string]> = [
    ['default', '#F0EDE8'],
    ['muted', '#A0A0A0'],
    ['cyan', '#00E4E4'],
    ['green', '#22C55E'],
    ['amber', '#F59E0B'],
    ['red', '#EF4444'],
    ['violet', '#a78bfa'],
  ];

  const BORDER_SIDES: Array<[string, string]> = [
    ['none', 'None'],
    ['all', 'All'],
    ['top', 'Top'],
    ['bottom', 'Btm'],
    ['left', 'Left'],
    ['right', 'Rgt'],
  ];

  const BORDER_WEIGHTS = [1, 2, 3];

  let panelEl: HTMLDivElement | null = $state(null);

  /** Working border state. #2874 item 29 made Top/Btm/Left/Rgt
   *  independent additive toggles rather than a single-select value;
   *  weight/dashed remain a SINGLE shared value applied to whichever
   *  side(s) are active (no per-side weight picker — scope report
   *  §3.5, not something Matt asked for).
   *
   *  #2874 item 53 (Matt's ruling, 2026-10-04 — REVERSES the paragraph
   *  this one replaces, from item 29's original fix, commit
   *  `889b88d2`): the border panel tracks TWO DISTINCT MODES, as a
   *  tagged value — not a side flag sitting beside a `Set`. The old
   *  state here was a bare `Set<Side>` (`activeSides`); a plain `Set`
   *  cannot tell "the user clicked All" apart from "the user happened
   *  to toggle all four directional buttons on one at a time" — both
   *  produced the identical `Set{top,bottom,left,right}`, which is
   *  exactly the equivalence item 29 deliberately built (and
   *  documented) and Matt's new ruling overturns: four individual
   *  clicks must draw an OUTLINE only (the existing outer-edge rule,
   *  unchanged) and must never light "All"; only an EXPLICIT "All"
   *  click fills inner lines and lights "All". `BorderPanelState`
   *  below is that tagged value — same discriminated-union posture as
   *  `sheetCellStyle.ts`'s `BorderSideResult` (item 45): `{ mode:
   *  'all' }` carries no side set at all (by construction it always
   *  means every side — there is nothing left to disambiguate); `{
   *  mode: 'sides', sides }` is the individual-toggle mode, carrying
   *  exactly the sides toggled on, independent of how many of the four
   *  that happens to be (0 through 4 — four individually-toggled sides
   *  is still `'sides'` mode, on purpose). Seeded from `currentBorder`
   *  (SheetGrid's unanimity result PLUS its own per-cell "does every
   *  cell have all four sides" check — see `panelBorder` there) when
   *  present, the safe outline-only default otherwise. */
  type Side = 'top' | 'bottom' | 'left' | 'right';
  type BorderPanelState = { mode: 'all' } | { mode: 'sides'; sides: Set<Side> };

  let borderState = $state<BorderPanelState>(
    currentBorder?.mode === 'all'
      ? { mode: 'all' }
      : { mode: 'sides', sides: new Set(currentBorder?.sides ?? []) }
  );

  /** The side list an apply actually needs to send — `'all'` mode
   *  always means every side (there is no separate set to read for
   *  it); `'sides'` mode means exactly what's currently toggled on. */
  function activeSidesArray(): Side[] {
    return borderState.mode === 'all' ? ['top', 'bottom', 'left', 'right'] : [...borderState.sides];
  }
  // Matt's ruling #1 (option B, indeterminate): when `currentBorder` is
  // present but its sides disagree on weight/dashed, `w`/`dashed` come in
  // as `null` — seed the local state to `null` too so NEITHER weight
  // button nor the dashed button shows `is-on` (the `class:is-on`
  // equality checks below never match `null`). Only fall back to the
  // sane 1px/solid defaults when there's no unanimous border at all
  // (`currentBorder` itself is `null`).
  let borderWeight = $state<number | null>(currentBorder ? currentBorder.w : 1);
  let borderDashed = $state<boolean | null>(currentBorder ? currentBorder.dashed : false);
  // #2874 item 54 (Matt's ruling, 2026-10-02): the seeded `1`/`false`
  // sane defaults above are the ACTIVE VALUE used to resolve an apply —
  // they are deliberately NOT cleared to `null` here, because a side
  // click on a genuinely borderless selection must still apply
  // immediately at 1px/solid (see `handleSideClick`'s
  // `weightDashedResolved` guard: it is already `true` at this seeded
  // default, so nothing else needs to change for that half of the
  // ruling). What changes is the LIT INDICATOR, not this value: a
  // borderless selection (`borderState.mode === 'sides' && sides.size
  // === 0`) must show ONLY "None" lit — the weight/dashed buttons must
  // not visually light off a default nothing has actually chosen. That
  // gate lives purely in the `class:is-on` checks below
  // (`(borderState.mode === 'all' || borderState.sides.size > 0) &&
  // ...`); it does not touch `borderWeight`/`borderDashed` themselves.
  // (#2874 item 53 renamed the underlying state from `activeSides` to
  // the tagged `borderState` — this gate's INTENT is unchanged.)

  function applyCurrentBorder() {
    // `borderWeight`/`borderDashed` can be `null` while indeterminate
    // (mixed unanimous sides, Matt's ruling #1; or a same-side mixed
    // border, #2874 item 45) — `onApplyBorder`'s wire format still needs
    // a concrete weight/dashed to write, so an apply fired while
    // indeterminate falls back to the same 1px/solid default
    // `currentBorder === null` already uses.
    //
    // Delta-audit fix (2026-09-24): this function — and its `?? 1`/`??
    // false` fallback — is now ONLY reached from `handleSideClick`
    // (a bare side toggle once `weightDashedResolved`, or "All"/"None",
    // per that function's own unchanged guard: "side-button clicks keep
    // the wait guard as built") — never from a weight or dashed click.
    // `handleWeightClick`/`handleDashedClick` fire their OWN
    // `onApplyBorderWeight`/`onApplyBorderDashed` callbacks instead,
    // which carry only the ONE attribute actually clicked — see those
    // functions' own doc comments. That split is exactly what closes
    // the gap Reed's audit of PR #2963 found: this function's shared
    // `w: borderWeight ?? 1, dashed: borderDashed ?? false` fallback
    // used to fire for a bare weight OR dashed click too, clobbering
    // whichever axis the user did NOT just click with a shared
    // (possibly wrong-for-this-cell) default.
    //
    // #2874 item 53: `mode` is threaded straight from `borderState` into
    // the wire payload — `SheetGrid.svelte` gates its fill-vs-outline
    // fast path on THIS field now, never on `sides.length === 4`.
    onApplyBorder({ mode: borderState.mode, sides: activeSidesArray(), w: borderWeight ?? 1, dashed: borderDashed ?? false });
  }

  /** Matt's ruling (scope report "Matt's Rulings" #2): "All"/"None" stay
   *  ABSOLUTE, never toggles. "All" always forces all four sides on;
   *  "None" always forces all four off — regardless of the set's current
   *  contents. Top/Btm/Left/Rgt toggle membership in the 'sides' mode's
   *  side set (add if absent, remove if present) — this is the one real
   *  behavior change item 29 made in this file: the old code REPLACED
   *  `borderSide` wholesale on every click, discarding whatever side was
   *  previously chosen. (#2874 item 53 adds the 'all'-vs-'sides' mode
   *  tag on top of that toggle set — see this function's body.)
   *
   *  #2874 item 59 (Matt's ruling, 2026-10-07 — reverses the PREVIOUS
   *  item 59 fix, which sent every individual-side click as a full
   *  `onApplyBorder({mode:'sides', sides:[...]})` snapshot and was found
   *  to delete borders the user never clicked on cells that were never
   *  touched by anything in `sides`). Matt's ruling (authoritative,
   *  quoted in full): "A side click changes only that side. Turning a
   *  side on adds it to cells on the selection's outer edge. Turning it
   *  off removes it from every selected cell. Every other side on every
   *  cell stays exactly as stored, whether or not it's lit in the
   *  panel. The apply needs to know which side was toggled, and whether
   *  on or off." This function is where that information already
   *  exists — `base.has(s)` BEFORE the toggle tells us the direction —
   *  so an ordinary side click now fires the dedicated `onApplyBorderSide
   *  ({side, on, w, dashed})` delta instead of folding back into
   *  `onApplyBorder`'s full-array shape.
   *
   *  ONE case still deliberately uses the full-array `onApplyBorder`
   *  path for an individual side: leaving "All" mode. `wasAllMode` below
   *  captures whether `borderState.mode` was `'all'` BEFORE this click —
   *  if so, per item 53's UNCHANGED ruling ("redraws the selection as an
   *  outline"), this click is a one-time full REDRAW of the whole
   *  selection down to an outline (discarding "All"'s blanket inner-line
   *  fill), not a delta on one side — so it still sends the complete
   *  reduced `sides` array via `applyCurrentBorder()`, same as "All"/
   *  "None" always have. Every OTHER individual-side click (already in
   *  'sides' mode, not transitioning) uses the new delta. */
  function handleSideClick(side: string) {
    if (side === 'all') {
      borderState = { mode: 'all' };
      applyCurrentBorder();
      return;
    }
    if (side === 'none') {
      borderState = { mode: 'sides', sides: new Set() };
      applyCurrentBorder();
      return;
    }

    // #2874 item 53 (Matt's ruling, verbatim): "Clicking any single
    // side switches to individual mode." True even starting FROM
    // 'all' mode — clicking a side while All is lit must leave All
    // mode and draw an outline. The starting point for the toggle is
    // "all four" when leaving 'all' mode (nothing was explicitly off
    // before this click) or the existing side set otherwise.
    const s = side as Side;
    const wasAllMode = borderState.mode === 'all';
    const base = borderState.mode === 'all' ? new Set<Side>(['top', 'bottom', 'left', 'right']) : new Set(borderState.sides);
    const turningOn = !base.has(s);
    if (turningOn) base.add(s); else base.delete(s);
    borderState = { mode: 'sides', sides: base };

    // #2874 item 45 guard (Matt's ruling, 2026-09-24 — "guard against a
    // click resetting the cells to weight 1"; scope report §5's
    // click-through footgun; "Side-button clicks keep the wait guard as
    // built" per item 59's own 2026-10-07 ruling above, verbatim). This
    // guard is UNCHANGED by item 59's delta rework — ANY bare side click
    // (on or off, transition-out-of-"All" or ordinary) still defers
    // while `borderWeight`/`borderDashed` are indeterminate, exactly the
    // same single condition as before. (Turning a side off doesn't
    // itself NEED a weight/dashed value to remove an entry — but the
    // guard is deliberately left exactly as built rather than carving
    // out a new asymmetric exception its own ruling never asked for.)
    const weightDashedResolved = borderWeight !== null && borderDashed !== null;
    if (!weightDashedResolved) return;

    if (wasAllMode) {
      // Leaving "All": a full redraw-as-outline snapshot, not a delta
      // (see this function's doc comment above).
      applyCurrentBorder();
      return;
    }

    // Ordinary individual-sides click: a delta on exactly one side.
    onApplyBorderSide({ side: s, on: turningOn, w: borderWeight ?? 1, dashed: borderDashed ?? false });
  }

  /** Delta-audit fix (2026-09-24, guarding the gap Reed's audit of PR
   *  #2963 found in `applyCurrentBorder`'s shared-fallback apply):
   *  clicking a weight changes ONLY weight. It no longer routes through
   *  `applyCurrentBorder()` (which would write `dashed: borderDashed ??
   *  false`, clobbering any cell whose real dashed state disagrees with
   *  this panel's shared, possibly-indeterminate `borderDashed`). Instead
   *  it fires `onApplyBorderWeight` with just `w` — `SheetGrid.svelte`
   *  preserves each cell's own existing dashed/color per side. Always
   *  fires once a side is active ("no dead clicks"): the OTHER axis being
   *  unresolved is no longer a reason to defer, because this write no
   *  longer touches that axis at all. */
  function handleWeightClick(w: number) {
    borderWeight = w;
    const sides = activeSidesArray();
    if (sides.length > 0) onApplyBorderWeight({ sides, w });
  }

  /** Mirror of `handleWeightClick` for the dashed toggle — changes ONLY
   *  `dashed`; `SheetGrid.svelte` preserves each cell's own existing
   *  weight/color per side. This is the exact path Reed's audit
   *  reproduced live as still-clobbering under the pre-fix code (dashed
   *  click from a same-side mixed-weight selection silently wrote
   *  `w: 1` over real, differing per-cell weights). */
  function handleDashedClick() {
    borderDashed = !borderDashed;
    const sides = activeSidesArray();
    if (sides.length > 0) onApplyBorderDashed({ sides, dashed: borderDashed });
  }

  function handleWindowClick(e: MouseEvent) {
    if (e.target instanceof Node && !e.target.isConnected) return;
    const target = e.target as Node;
    if (panelEl && panelEl.contains(target)) return;
    if (anchorEl && anchorEl.contains(target)) return;
    onClose();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
  }

  function handleClearStylingClick() {
    onClearStyling();
    onClose();
  }

  $effect(() => {
    if (panelEl) panelEl.focus();
  });
</script>

<svelte:window onclick={handleWindowClick} />

<div
  bind:this={panelEl}
  class="sh-sty"
  style:position="fixed"
  style:top="{y}px"
  style:left="{x}px"
  role="dialog"
  tabindex="-1"
  aria-label="Cell styling"
  onkeydown={handleKeydown}
  onclick={(e) => e.stopPropagation()}
  data-testid="style-panel"
>
  <div class="sh-sty__sec">
    <div class="sh-sty__lab">Cell background</div>
    <div class="sh-sw">
      {#each FILLS as [name, color]}
        <button
          type="button"
          class="sh-sw__b"
          class:sh-sw__b--none={color === 'transparent'}
          class:is-on={color === currentBg}
          style:background={color}
          title={name}
          onclick={() => onApplyFill(color)}
          data-testid="style-panel-fill-{name}"
        >{#if color === 'transparent'}✕{/if}</button>
      {/each}
    </div>
  </div>

  <div class="sh-sty__sec">
    <div class="sh-sty__lab">Text</div>
    <div class="sh-sw">
      {#each INKS as [name, color]}
        <button
          type="button"
          class="sh-sw__b sh-sw__b--txt"
          class:is-on={color === currentFg}
          style:color={color}
          title={name}
          onclick={() => onApplyInk(color)}
          data-testid="style-panel-ink-{name}"
        >A</button>
      {/each}
    </div>
  </div>

  <div class="sh-sty__sec">
    <div class="sh-sty__lab">Border</div>
    <div class="sh-seg" style:margin-bottom="6px">
      {#each BORDER_SIDES as [key, label]}
        <button
          type="button"
          class="sh-seg__b"
          class:is-on={key === 'none' ? (borderState.mode === 'sides' && borderState.sides.size === 0) : key === 'all' ? borderState.mode === 'all' : (borderState.mode === 'all' || borderState.sides.has(key as Side))}
          onclick={() => handleSideClick(key)}
          data-testid="style-panel-border-side-{key}"
        >{label}</button>
      {/each}
    </div>
    <div class="sh-seg">
      {#each BORDER_WEIGHTS as w}
        <button
          type="button"
          class="sh-seg__b"
          class:is-on={(borderState.mode === 'all' || borderState.sides.size > 0) && w === borderWeight}
          onclick={() => handleWeightClick(w)}
          data-testid="style-panel-border-weight-{w}"
        >
          <span class="sh-seg__bar" style:height="{w}px"></span>
        </button>
      {/each}
      <button
        type="button"
        class="sh-seg__b"
        class:is-on={(borderState.mode === 'all' || borderState.sides.size > 0) && borderDashed}
        style:flex="1.4"
        onclick={handleDashedClick}
        data-testid="style-panel-border-dashed"
      >Dashed</button>
    </div>
  </div>

  <button type="button" class="sh-sty__clear" onclick={handleClearStylingClick} data-testid="style-panel-clear">
    Clear styling
  </button>
</div>

<style>
  /* Ported verbatim from docs/design/sheets/sheets.css:233-249 — this is a
     new, single-consumer surface (Matt's "scoped single-consumer CSS for
     panel" endorsement, scope report), so these rules live locally rather
     than joining the shared global `.sh-pop*` block in src/app.css (this
     panel isn't a `.sh-pop` at all — distinct shell, distinct box model). */
  .sh-sty { width: 268px; box-sizing: border-box; background: #1c1c20; border: 1px solid var(--border-strong); border-radius: 10px; box-shadow: var(--shadow-pop); padding: 12px; z-index: 30; }
  .sh-sty__sec + .sh-sty__sec { margin-top: 14px; padding-top: 13px; border-top: 1px solid var(--border-faint); }
  .sh-sty__lab { font-size: 10px; font-weight: 600; letter-spacing: 0.08em; text-transform: uppercase; color: var(--fg-4); margin-bottom: 8px; }
  .sh-sw { display: grid; grid-template-columns: repeat(7, 1fr); gap: 6px; }
  .sh-sw__b { aspect-ratio: 1; border-radius: 5px; border: 1px solid var(--border); cursor: pointer; padding: 0; position: relative; }
  .sh-sw__b:hover { border-color: var(--border-strong); }
  .sh-sw__b.is-on { box-shadow: 0 0 0 1.5px var(--accent); border-color: transparent; }
  .sh-sw__b--none { background: transparent; display: flex; align-items: center; justify-content: center; color: var(--fg-4); }
  .sh-sw__b--txt { display: flex; align-items: center; justify-content: center; font-family: var(--font-sans); font-size: 12px; font-weight: 700; background: var(--surface-2); }
  .sh-seg { display: flex; gap: 4px; }
  .sh-seg__b { flex: 1 1 0; min-width: 0; height: 28px; border-radius: var(--radius-sm); border: 1px solid var(--border); background: transparent; color: var(--fg-3); font-family: var(--font-sans); font-size: 11.5px; font-weight: 500; cursor: pointer; display: flex; align-items: center; justify-content: center; }
  .sh-seg__b:hover { background: var(--overlay-hover); color: var(--fg-1); }
  .sh-seg__b.is-on { background: var(--accent-soft); border-color: rgba(0,228,228,0.4); color: var(--accent); }
  .sh-seg__bar { width: 18px; background: currentColor; border-radius: 1px; display: block; }
  .sh-sty__clear { width: 100%; margin-top: 12px; height: 30px; border-radius: var(--radius-sm); border: 1px solid var(--border); background: transparent; color: var(--fg-2); font-family: var(--font-sans); font-size: 12.5px; font-weight: 500; cursor: pointer; }
  .sh-sty__clear:hover { background: var(--overlay-hover); color: var(--fg-1); border-color: var(--border-strong); }
</style>
