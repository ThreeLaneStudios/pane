# Pane Sheets — structured-table note type

Design for the full Sheets surface set, locked after review. Two views of the
same content:

- **`Sheets (scroll view).html`** — one scrolling dark page, all frames and
  annotations top to bottom, with a sticky section nav. Start here.
- **`Sheets.html`** — the same frames on a pannable canvas. Click a frame's
  label to focus it; ←/→ moves within a section, ↑/↓ across sections.

Six sections matching the brief: grid editor, formula experience, cell styling,
board card face, version history, CSV export. Everything is annotated in place —
the frames carry the spec, this file carries the rulings and the constraints.

```
Sheets (scroll view).html    the design doc as one scrolling page  ← start here
Sheets.html                  the same content as a pannable canvas
Sheets Addendum.html         addendum: New Sheet button + select choices editor
README.md                    this file — direction, rulings, constraints
NOTES.md                     addendum notes — treatment, behavior, rulings
sheets.css                   Sheets-specific styles, tokens only (incl. addendum section)
sheets-shared.jsx            app chrome, grid engine, toolbar / formula bar / status bar
sheets-grid.jsx              1 · grid editor screens
sheets-formula.jsx           2 · formula experience  ·  3 · cell styling
sheets-surfaces.jsx          4 · board card  ·  5 · version history  ·  6 · CSV export
sheets-addendum.jsx          A · New Sheet button pair  ·  B · select choices editor
addendum-deps.jsx            sample sheet data + Kanban card faces for addendum frames
kit.css                      Pane UI-kit stylesheet (copy of ui_kits/pane/kit.css)
pane-ui.jsx                  UI-kit primitives (Icon, Pill, Button, dot) — kit source
colors_and_type.css          design-system tokens (copy)
design-canvas.jsx            canvas wrapper (copy)
```

All paths are relative — keep the folder together and open either HTML file
directly in a browser. No build step, no server.

No new colors, no new fonts, no new radii. Everything resolves to
`colors_and_type.css`.

## What comes from the UI kit

The app context is the real kit, not a stand-in. The board beneath the overlay
is `KanbanBoard.jsx`'s markup and `kit.css` classes (`.kanban` / `.kcol` /
`.kcard` / `.dot` / `.iconbtn` / `.kcol__add`); the sheet card face is built
**on** `.kcard`, so it inherits the same padding, radius, status rail and hover
as a note card; priority and tag chips are the kit's `Pill`; the sheet toolbar
is the note editor's own `.etool` row (`.ebtn` / `.ebtn--active` / `.esep`);
Restore is `btn btn--primary`. Every glyph the kit has is the kit's `Icon`, at
the sizes the kit's own components use.

Sheets-only glyphs (type marks, fx, paint bucket, border, sort, funnel, wedge
markers) are drawn on `KitGlyph` — the kit's own 24px viewBox / 1.7 stroke /
round-cap base — so there is one icon language, not two.

Authored fresh, because the kit has nothing for them: the grid itself, the
corner-wedge language, the formula bar, the styling panel, the micro-grid card
preview, the read-only history grid, and the collapsed sidebar rail (the kit
ships only the expanded `.sidebar`).

---

## The one new idea: corner markers

Sheets needed three cell-level annotations that can't cost horizontal room and
can't be confused with cell styling. One system covers all three:

| Marker | Means |
|---|---|
| Bottom-left, cyan | Computed — this value came from a formula |
| Bottom-left, red | Formula error |
| Top-right, amber | Type mismatch — value stored as text |

**Position is the category** (bottom-left = formula, top-right = data),
**color is the meaning**. 5px wedges, so they survive any column width, any
cell fill, and a fully computed column still reads as a grid. Nothing else in a
cell ever uses a corner wedge. The status bar teaches the cyan one by putting it
next to the sheet's formula count.

## Section notes

**1 · Grid editor.** Sizes are kept; the grid overflows both axes with sticky
header band, sticky row-number gutter, and a corner cell sticky on both. Ghost
columns and rows always render past the last real one at 55% opacity with
one-third-weight hairlines; the first keystroke commits them and a new ghost
band appears. Selection is a cyan inset ring on the anchor plus an
`accent-soft` fill and solid outline for a range. Column resize grips the
header's right edge, row resize grips the *gutter* cell's lower edge only —
a grip in the cell area would fight click-drag range selection.

**2 · Formula experience.** The bar shows the raw formula, never the result.
Refs cyan, functions full-strength, numbers and strings recede — the same three
colors in the bar, in-cell, and in paused mode. Ranges referenced by the formula
being edited outline **dashed** so they can't read as the selection. Six error
codes, one treatment. Paused mode is a neutral notice between toolbar and
formula bar, leading with the count, with no action button.

**3 · Cell styling.** Toolbar carries fill / text / border persistently (the
fill button shows the last-used swatch); right-click reaches the same panel.
Seven fills at 13–15% alpha, seven inks at full strength, both from the status
and accent tokens — a red fill and red text are visibly the same red. The
second frame proves fills and wedges coexist: a fill paints the cell box, a
wedge occupies 5px of a corner the value never reaches, and error type stays
mono red on top of any fill in the palette.

**4 · Board card face.** The kit's `kcard` with three sheet-specific parts: a
cyan grid glyph beside the title, a micro-grid, and a footer metric. Revised to
**two data rows** — at equal board width the card now sits within a few pixels
of a three-line note card. The type-glyph header row always stays (it's the
informative part, drawn from real column config); below ~190px of card width
the data rows drop and the card is glyph plus header strip. Metadata is the
kit's `dot` + `Pill` in the note card's order; only the due-date chip adds a
tone the kit's pill set doesn't carry.

> **Do not change board column or card widths.** They match the live app and
> stay as they are: the kit's `.kanban` track `repeat(5, minmax(220px, 1fr))`
> at `gap: 16px`, and `.kcard`'s own width, padding and radius. The card
> revision in this package is **height only** — three micro-grid data rows
> reduced to two. No frame here specifies a new column or card width, and none
> should be built. The 176px narrow-card frame demonstrates where the compact
> treatment kicks in; it is a breakpoint, not a board width.

**5 · Version history.** Shell, source labels and Restore treatment unchanged.
The right pane is a faithful read-only rendering of the snapshot: its own
column headers with type glyphs, row numbers, values in their own columns and
alignments, and its cell styling. Read-only is expressed by removal — no ghost
space, no selection, no header hover — not by a badge.

**6 · CSV export.** Top of the toolbar's `⋯` menu, repeated in the cell context
menu. Trailing ellipsis on both, because the OS dialog is the next step.

---

## Rulings from review

All six open calls are closed plus two addendum rulings. Nothing in the package
is awaiting a decision.

1. **New-sheet seed stays five text columns** for v1. The three-column argument
   is parked — it's a one-line constant, revisitable with real usage. Headers
   seed **blank with a placeholder**, not "Column 1": accepted.
2. **Row resize ships v1; autofit does not.** Cells never wrap, so autofit has
   no height to compute, and it's the affordance people try first.
3. **No "compute once" button** in paused mode. The notice stands alone — v1
   isn't aimed at users who reach 500 formulas.
4. **The overlay opens wider for sheets** (~1000px vs the note editor's ~640).
   Same position, same behavior, different default width, because five default
   columns plus the gutter need the room. Resizable as today.
5. **All Tasks keeps both the glyph swap and the `cols × rows` metric.** The
   differentiation is the point; uniformity loses.
6. **Paused-sheet export writes formula strings, not values** — there are no
   values to write. Handled as a filename-convention line at the save dialog,
   not as UI.

### Addendum rulings (2026-09-05)

7. **New Sheet button pair** — accepted with amendments applied.
   Both buttons carry the **same plus glyph** (not the sheet grid icon). New
   Note stays filled primary and stays rightmost; New Sheet takes the kit's
   `.btn` box unchanged and inverts only the ground (`.btn--outline`:
   transparent fill, cyan border `rgba(0,228,228,0.42)`, cyan label). The
   kit's `.btn--ghost` was rejected — `fg-3` on a faint border reads as
   tertiary, not as the second of two create verbs. Narrow collapse in order:
   ≥900px both labelled → 720–900px New Sheet drops label (keeps square box) →
   <720px New Sheet folds into a caret menu beside New Note, above New Folder.
   Primary never gives way.
8. **Select choices editor** — accepted with the duplicate-rejected-with-signal
   state. Entry via the column header menu's conditional sixth row ("Edit
   options…", present only for select columns, shows current choice count) or
   the cell dropdown's existing "Edit options…" row. Both open the same nested
   panel. Six states drawn: list, add, rename, duplicate (rejected with danger
   ring + shake + collision message), reorder (drag grip, cyan drop line),
   empty (what type-change produces). Removal does not touch stored values —
   a cell holding a removed choice keeps its text and renders as a mismatch.
   No colour-per-choice in v1. No alphabetical mode — order is meaningful.

## Revisions after review

- **Card height** cut from three micro-grid rows to two, plus a compact
  glyph-plus-header treatment below ~190px card width. New frames show the
  revised card beside a note card at equal board width and in-column.
  **Widths did not change and must not change** — see the note under section 4.
- **History preview fidelity** fixed. The read-only grid was shearing: the grid
  template still reserved three ghost columns while read-only rows emitted only
  real cells, so every row began one column late. Read-only now reserves no
  ghost band, and the preview renders the snapshot's headers, values and
  styling in alignment.
- **New frame** in section 3 proving a styled + computed + error cell reads
  clearly, with the four fill/wedge combinations called out beside it.

## Build constraints — read before implementing

1. **Board column and card widths match the live app. Do not change them.**
   The kit's `.kanban` track `repeat(5, minmax(220px, 1fr))` at `gap: 16px`,
   and `.kcard`'s own width, padding and radius, all stay exactly as they are.
   The card revision in this package is **height only** — three micro-grid data
   rows reduced to two. No frame specifies a new column or card width, and none
   should be built. The 176px narrow-card frame demonstrates where the compact
   treatment kicks in; it is a breakpoint, not a board width.
2. **Sort and filter are view-only.** No frame implies otherwise, and the view
   strip's closing sentence is load-bearing copy.
3. **Type changes and mismatches never destroy a value.** A non-parsing value
   stays exactly as typed and renders as text with the amber marker.
4. **The invite to grow the grid is typing, not a button.** Explicit insert
   affordances exist for inserting *between* columns; they are never the
   required path.

---

## Out of scope, per brief

Mobile, multi-device conflict UI, import. Also not designed: drag-fill /
fill handle (deliberately omitted rather than implied — it promises a behavior
the brief doesn't include).
