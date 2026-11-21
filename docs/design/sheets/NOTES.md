# Pane Sheets — addendum

Two designs only, both approved in review. They append to the locked Sheets
package; nothing already locked changes.

Open **`Sheets Addendum.html`** in a browser. All paths are relative — keep the
folder together. No build step, no server.

```
Sheets Addendum.html   the two frames, annotated
NOTES.md               this file
sheets.css             Sheets styles (tokens only)
sheets-shared.jsx      grid engine, toolbar, formula bar, popovers, annotations
sheets-addendum.jsx    the two frames
addendum-deps.jsx      sample sheet data + the Kanban card faces
kit.css                Pane UI-kit stylesheet
pane-ui.jsx            UI-kit primitives (Icon, Pill, Button, dot)
colors_and_type.css    design-system tokens
```

No new colors, no new fonts, no new radii. One new class, `.btn--outline`,
added to `sheets.css` for the New Sheet button.

---

## A · New Sheet button pair

`+ New Sheet` sits beside `+ New Note` in each host view's header — board,
project, folder and All Tasks. Same position, same order, everywhere.

**Treatment.** It takes the kit's `.btn` box unchanged — same height (32px),
padding, radius, gap, font and icon slot as New Note — and inverts only the
ground:

| | New Note | New Sheet |
|---|---|---|
| Background | filled `--accent` | transparent |
| Border | none | `rgba(0,228,228,0.42)` |
| Label | `--fg-on-accent` | `--accent` |
| Glyph | plus, 14px | plus, 14px |
| Height | 32px | 32px |

Identical geometry is what makes them read as a pair; the fill is what ranks
them. **New Note stays filled primary and stays rightmost** — it is the more
frequent action and already owns that position, so the primary button never
moves from where people reach for it.

**Ruled in review:** both buttons carry the **same plus glyph**, not the sheet
grid icon. Both create something, the plus says so, and the label is what
distinguishes them. The fill is the only other difference in the pair.

The kit's `.btn--ghost` was rejected — `fg-3` on a faint border reads as
tertiary, a "more options" affordance, not as the second of two create verbs.

**Narrow collapse**, in order, with the primary never giving way:

| Header width | Treatment |
|---|---|
| ≥ 900px | Both labelled |
| 720–900px | New Sheet drops its label, keeps its square 32px box |
| < 720px | New Sheet folds into a caret menu beside New Note, above New Folder |

Breakpoints are on the header's own width, not the viewport, so a narrow
project pane collapses the same way a narrow window does.

*One open flag:* the caret menu is a new affordance in the board header. If a
host view already has an overflow menu we haven't seen, New Sheet should join
that one rather than add a second caret.

---

## B · Select choices editor

Select columns needed a place to define their choices. `options_json` stores an
ordered list of strings; this is the UI over it.

**Entry, two doors to one panel.** The column header menu's five verbs gain a
conditional sixth row, **`Edit options…`** — present only when the column's
type is select, sitting directly under Type because it is the continuation of
that decision. It carries the current choice count (`Select · 4` answers what
the column is and whether it has been configured without opening anything) and
opens a nested panel to its right. The cell dropdown's existing `Edit options…`
row opens the same panel.

It stays inside the popover language — same surface, same border, same row
metrics as the menu it came from. A modal for four strings would be heavier
than the thing it edits.

**Choices render as the chips they will become**, in `options_json` order,
which is the order the cell dropdown shows. No alphabetical mode: a status list
is meaningful in sequence.

**Six states, drawn.**

| State | Behavior |
|---|---|
| List | Choices as chips, each with a drag grip and a hover ✕ |
| Add | Appends a focused empty field at the end; Enter commits |
| Rename | Swaps the chip for a field in place |
| Duplicate | Rejected **with signal** — see below |
| Reorder | Drag the six-dot grip, cyan drop line |
| Empty | What a type change to select produces today |

**Removal does not touch stored values.** A cell holding a removed choice keeps
its text and renders as a mismatch — amber top-right wedge, dotted underline,
left-aligned — exactly as a type change behaves. The panel footer states this
permanently, which is what makes the no-confirm removal safe.

**Duplicate names are rejected with a signal** (ruled in review). On a
duplicate commit attempt the field keeps focus and the typed text, takes the
danger ring with a 2px glow, shakes once, and a one-line reason names the
collision — *"Committed" is already a choice.* Same inline treatment as the
rename field, so the refusal explains itself rather than just not committing.

**The empty state matters most**, because changing a column to select is what
produces it. It says what the column does now ("cells are empty until you add
one"), what happened to what was there, and offers one button: *Add the first
choice*.

No colour-per-choice in v1. Cells render every choice in the neutral chip, and
per-cell styling already covers anyone who wants "Cut" to look red.

---

## Carried over from the locked package

Still true, and still binding on these frames:

- **Board column and card widths match the live app.** The kit's `.kanban`
  track `repeat(5, minmax(220px, 1fr))` at `gap: 16px`, and `.kcard`'s own
  width, padding and radius. Frame A shows the button in a real board header —
  nothing in it specifies a new column or card width.
- **Type changes and mismatches never destroy a value**, which is why removal
  in the choices editor is safe without a dialog.
- Sort and filter are view-only.
