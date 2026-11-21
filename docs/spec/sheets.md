# Sheets

**Last Updated:** 2026-09-03
**Sections:** §33
**ADR:** ADR-0066

> **Cross-references:** core-data-model §1.12 (tables), §1.5 (note_versions),
> ai-integration §4.1 (tools), sync.md §9 (CRR), INVARIANTS.md (Sheets
> section), audits `docs/audits/2026-09-03-note-type-audit.md` and
> `2026-09-03-note-type-equality-audit.md`.

---

## 33. Sheets — Structured Tables

> **Status:** TARGET — NOT BUILT (#2339)

A sheet is a first-class structured table: typed columns, cell styling,
sort/filter, basic formulas, versioned snapshots, CSV export, AI tools.
Target uses: budgets, ad tracking, any structured list wanting live totals
plus AI analysis. Data model: ADR-0066 / §1.12. Design phase (grid,
interactions, cell/formula states, card face) happens in Claude Design
sessions after this spec and before any frontend PR; frontend builds to
locked design files.

### 33.1 Surfaces — where sheets appear

Sheets appear on **every task surface**: Kanban board, project view, folder
view, All Tasks, calendar, search, and due-date notifications. They carry
the full notes metadata surface (status, priority, due_date, tags, pinned).
They are **excluded from AI-agent system folders in v1** — enforced at write
time (§33.2.1), not by read-side filters.

#### 33.1.1 Canonical predicate map

The two 2026-09-03 audits enumerate all 37 `note_type` sites. Dispositions
under the ratified rulings:

**Widen `note_type IS NULL` → `(note_type IS NULL OR note_type = 'sheet')`:**

| Site | Surface |
| -------------------- | ---------------------------------------------------- |
| notes.rs:415 | folder `get_notes` |
| notes.rs:421 | project-root `get_notes` |
| notes.rs:474 | `KANBAN_NOTES_QUERY` |
| notes.rs:519 | `get_tasks` |
| notifications.rs:747 | `classify_due_tasks` |
| notifications.rs:941 | `check_due_dates_for_task` |
| notes.rs:4257–4453 | the seven kanban tests (mirror production predicate) |

Each widening lands with an inclusion regression test (a sheet-typed row
MUST appear) plus one exclusion test asserting a `note_type = 'daily'` row
still does not.

**Add `'sheet'` to the `NOT IN (...)` allowlist:**

| Site | Why |
| ------------------------------------------------------------ | ------------------------------------------------------------ |
| lib.rs:716 and lib.rs:725 (both — copy-pasted, keep in sync) | §486 integrity sweep repairs `body` from `file_path`; sheets are fileless (§33.4), so repair is meaningless and must never fire. |

**Explicitly unchanged (confirmed leave-as-is — do not "fix" in review):**

| Site | Reason |
| ------------------------------------- | ------------------------------------------------------------ |
| database.rs:632–634, 640–645, 648–666 | system-note fixups/dedup; sheets non-NULL-typed at creation, can never match |
| database.rs:409–417, 526, 577 | system-note resolver; `_ => None` fallthrough already correct |
| notes.rs:413 | system-folder branch; moot under write-time exclusion (§33.2.1) |
| notes.rs:637 | search exclusion list — sheets are searchable, stay OFF the list |
| notes.rs:1370–1377 | manual-version gate — sheets never route through it; dedicated snapshot path instead (§33.9) |
| notes.rs:1626–1631, 1676–1681 | delete-protection lists — sheets delete like any note |
| notes.rs:2753, 3132 | memory/daily single-purpose lookups |
| embeddings.rs:404 | COALESCE already includes sheets; body contract (§33.4) makes them embeddable |

**Invariant (INVARIANTS.md):** any future non-system content kind must
re-walk this map; `note_type IS NULL` alone no longer means "all user
content."

### 33.2 Creation

#### 33.2.1 Write-time rules

- `note_type = 'sheet'` is set atomically in the creating INSERT. No path
  creates a sheet row with `note_type` unset for later inference.
- `file_path = ''` always. No `.md` is created; sheets never enter the file
  watcher or flat-file sync.
- Creation into an `is_system` project/folder is rejected (v1 ruling). This
  is the sole enforcement point for system-folder exclusion.
- When `folder_id` is omitted (Kanban "New Sheet", AI `create_sheet` tool),
  `create_sheet_note_internal` auto-assigns the project's default folder,
  mirroring `create_note_internal`'s existing fallback. **TARGET — NOT BUILT
  (#2874 item 50a):** this resolves via `projects.default_folder_id`
  (core-data-model.md §1.7.1) — never a name lookup against `'General'`. An
  empty-string `folder_id` is treated identically to an omitted one (ruling 7,
  ai-integration.md §4.1). System folders (`is_system = 1`) are never
  eligible — Guards 2/3 reject them.
- A new sheet is born with a default column set (design phase decides count/
  types) and zero rows stored (sparse).

### 33.3 Grid model

#### 33.3.1 Typed columns

`col_type` ∈ text, number, currency, date, checkbox, select (canonical
list: `VALID_COLUMN_TYPES`, `sheets.rs`; UI mirror: `ShHeaderMenu.svelte`'s
`TYPES`, kept in sync by a parity test). Type governs cell editor, input
validation, display formatting, and sort comparator.
Values store as TEXT canonical forms: number = decimal string; currency =
decimal string + code in `options_json`; date = ISO 8601; checkbox = `'0'`/
`'1'`; select = the chosen option string. Type changes reinterpret in place;
unparseable values render as text with a type-mismatch indicator (design
phase), never destructively rewritten.

#### 33.3.2 `options_json`

Per-column config object: `{ "choices": [...] }` for select,
`{ "currency": "USD" }` for currency, `{}` otherwise. Opaque payload —
never queried by field.

#### 33.3.3 Ordering and sizing

Rows and columns order by `sort_order` (float-midpoint or integer-resequence
— match the Cabinet §1.11 mechanism). Column `width` and row `height` are
synced columns (Matt ruling 2026-09-03): layout travels with the sheet.
Cells keep their set sizes; the grid overflows with scrolling on both axes,
sticky column headers and row numbers. Never squish.

#### 33.3.4 Positional insert

Insert row above and Insert column left insert at a position. The toolbar add buttons, ghost commit, and agent-created rows and columns still append.

The IPC contract is the existing create_sheet_row and create_sheet_column commands with two optional fields: reference_id and position (Before or After). Both absent means append, unchanged. Before on a reference row or column places the new one immediately above or to the left of it.

Sort order for a positional insert is the integer midpoint between the reference and its predecessor. The first row or column uses the midpoint between 0 and the reference. When the two neighbours differ by less than 2, there is no integer slot: the sheet's rows (or columns) are resequenced to fresh gaps of 1000 and the midpoint is recomputed. Resequence and insert run in one transaction, with the same snapshot and recompute steps as any other structural mutation. This is the Cabinet §1.11 ordering convention applied to sheets.

After a positional insert, selection moves to the new row or column at its inserted position.

Only Before is exposed in the UI. After exists in the contract and is not wired.

Concurrent inserts from more than one device are undefined until sync ships. The tie-break is specified then.

#### 33.3.5 Ghost commit

> Status: BUILT (v1.0.54) — item 23 / #2874. Amended — item 44 / #2874: PENDING. I merge, then Stia.

Typing into a ghost cell N rows beyond the last real row creates N rows in one transaction, one snapshot, one recompute; the typed value lands in the Nth. Same for ghost columns. Both axes at once run as two transactions.

Typing into a ghost column never opens the column rename input. After the commit, the cell editor stays open on the new cell with the typed character in it, so typing continues in the cell exactly as it does for a ghost row. The new column keeps its empty name until the user renames it from the header menu (right-click → Rename). The same applies when a row and a column are both ghosts. Supersedes PR 10's rename-on-birth for ghost typing (Matt ruling, #2874 item 44).

#### 33.3.6 Column header interaction

> Status: BUILT (v1.0.54) — item 10 amendment / #2874

Left-click on a column header selects the whole column and opens nothing. Right-click opens the header menu (Rename, Type, Sort ascending, Sort descending, Filter, Delete column). Supersedes the Sep 4 one-click ruling.

### 33.4 Body contract — the flat-text copy

`notes.body` holds an auto-generated plain-text serialization of the grid:
title line, column-name header, one tab-separated line per row using
**computed** display values.

- **Written in the same transaction as every grid mutation** (cell, row,
  column, type, or style change). No async or debounced regeneration; no
  window where grid truth and `body` disagree locally.
- Consumers: FTS5 indexing (the every-write FTS invariant applies —
  INVARIANTS "Search Indexing"), embeddings (embeddings.rs:404 unchanged),
  and `content_preview`/`word_count` derivation.
- `body` is a derived read surface. Nothing ever parses `body` back into
  grid data. Cross-device LWW skew on `body` is self-healing: post-sync
  recompute (§33.5.4) regenerates it locally.
- **Write guard:** `apply_note_content_update` refuses direct `body` writes
  when `note_type = 'sheet'`, returning an error. This prevents the TipTap
  save path from overwriting the generated body when a sheet note is
  mistakenly opened in the plain-text editor (e.g. due to a missing
  `note_type` in the frontend's note metadata). The guard is a hard
  refusal, not a silent no-op.

### 33.5 Formula engine

Hand-rolled parser/evaluator. No crate with expression-language scope creep.

#### 33.5.1 Grammar (v1)

`=` prefix marks a formula. Supported: numeric literals, string and boolean
literals, cell references, ranges, parentheses, unary minus, binary
`+ - * /`, comparison operators, `SUM(range)`, `AVERAGE(range)`, and
`IF(condition, true_val, false_val)`. No other functions in v1. (Grammar
expanded beyond the original v1 minimum by Matt ruling 2026-09-03,
ratifying the shipped PR 6 engine; see #2851.)

Semantics rulings, same date: AVERAGE counts non-empty cells only; empty
range yields `#DIV/0!`. Checkbox values coerce in arithmetic (checked = 1,
unchecked = 0). Unary minus binds tighter than `* /`.

#### 33.5.2 Reference model — stable IDs stored, A1 displayed

Users type A1-style references. On commit, references resolve against the
current grid to `(row_id, column_id)` pairs and the formula is stored in
`sheet_cells.value` in canonical ID form. Display always re-renders
references as the referent's *current* A1 position — this applies to
every `SheetCell` returned to a caller, not only a `get_sheet_data` read:
a cell-write reply (`update_sheet_cell`, including a style-only write
that never touched `raw_value`, #2874 item 47) carries the SAME
A1-translated form a subsequent read would show, never the internal
`{column_id:row_id}` storage form.

Why: positional references are wrong under sync. Row `sort_order` can drift
transiently across devices (§33.10); a positional `=B2` would point at
different cells on different devices mid-convergence, and a local row insert
would silently retarget every formula below it. Stable IDs make a reference
mean "that cell" forever. They also make snapshot restore formula-safe
(§33.9.3).

Ranges store as `(column_id, from_row_id, to_row_id)`; membership is
computed at evaluation time as all rows ordered between the endpoints
(inclusive), so a row inserted inside a summed range is included — matching
spreadsheet intuition.

Ranges are stored as two cell refs in brace form: {col1:row1}:{col2:row2}.

Ranges may span a single column (A1:A5) or a single row (A1:C1). A range
spanning both is rejected inline. Row ranges follow the same rules as
column ranges: the referenced cells are the row's stored order at
evaluation time, and Insert column inside a row range extends it.

#### 33.5.3 Errors

Evaluation never throws; cells display error values: `#REF!` (referent
deleted), `#DIV/0!`, `#VALUE!` (non-numeric value in an arithmetic
context, or bad function arity), `#NAME?` (unknown function name),
`#PARSE!` (formulas arriving via sync/AI in unparseable form; local edits
with parse failures are rejected inline), `#CYCLE!` (dependency cycle;
every cell in the cycle shows it). Error values propagate through
referencing formulas.

#### 33.5.4 Recompute — the only computed-value path

Dependency graph built from stored references. Recompute triggers:
1. Local cell/row/column mutation (recompute dirty subgraph).
2. Sheet open/load (full recompute).
3. **Post-sync:** after `apply_changesets` returns, if any applied change
   touched `sheet_cells`/`sheet_rows`/`sheet_columns`, full recompute of
   affected sheets, then regenerate `body` (§33.4). `apply_changesets`
   itself stays a bare insert loop — the hook is at its call site, not
   inside it.

Computed values live only in the in-memory grid store. **Never persisted to
any CRR column, never synced** (ADR-0066 D4, structural per §1.12).

**Write replies always carry the current computed value (#2874 item 47,
Matt ruling 2026-09-24):** `update_sheet_cell`'s returned `SheetCell`
carries its cell's live computed value on EVERY write, including a
style-only one (`value: None` — border/fill/ink apply, clear styling) —
not only a value-changing write (item 25's original rule). This requires
a full `recompute_sheet` pass on every style-only write, the exact cost
an earlier PR amendment to item 25 had deliberately narrowed away;
Matt's ruling accepts that cost explicitly ("I accept the recompute
cost") rather than leave the reply stale or `None` — there is no cheaper
cache to consult instead, since computed values are never persisted
(above). **Body regeneration (§33.4) is unaffected and stays gated on
`value_changed` only** — a pure style change still never touches
`notes.body`/`updated_at` (Matt ruling 2026-09-03).

#### 33.5.5 Toolbar fx button

The toolbar fx button opens the formula editor on the anchor cell with `=`
prefilled, reusing the existing `clearWithChar` mechanism (same path as
typing `=` into a cell). Checkbox and select columns are excluded.

### 33.6 Cell styling

`style_json` per cell: background color, text color, border style/
thickness. Palette and application UI are design-phase. Styling a
valueless cell creates the sparse row; clearing value AND style deletes it.

The toolbar fill, text-color, and border buttons open the style panel
anchored to the clicked button. The panel's outside-click handler excludes
the anchor element — the same guard its sibling popovers use — preventing
a same-click self-close during native event dispatch.

A style-only write never guesses at a formula cell's computed value or
exposes its internal storage form — see §33.5.4's write-reply rule
(#2874 item 47). Styling is not a recompute-free path.

**Batched style applies recalculate once per apply, not once per cell
(#2874 item 51).** A selection's style apply (`applyStyleToSelection`) or
clear (`clearSelectedStyling`) sends every touched cell's style write in
ONE backend call, which runs in one transaction with one snapshot check
and one recalculation for the whole batch — not N transactions/
recalculations for an N-cell selection. Body regeneration stays skipped
for a style-only batch, exactly as it already is for a single style-only
write (unaffected by this item). Every returned cell still carries its
readable A1 formula and live computed value (item 47's guarantee,
unchanged) — the batch's recompute pass serves every cell's reply from
the same single computed-value map.

#### 33.6.1 Border unanimity states (#2874 item 45)

The style panel's border controls (side toggles, weight, dashed) reflect
one of three states per side, computed independently for `top`/`bottom`/
`left`/`right` (#2874 item 29 — sides are independent toggles, not a
single-select):

- **Unanimous-bordered** — every selected cell has an identical border on
  that side (same weight/color/dashed). The side button lights, and the
  weight/dashed controls show the shared value.
- **Unanimous-none** — no selected cell has a border on that side. The
  side button stays dark; if this holds for every side, "None" lights.
  The weight/dashed controls seed to sane defaults (1px/solid, #2874
  item 54) as their underlying working value — so that a subsequent
  side click on this still-borderless selection applies immediately at
  a sane default rather than requiring a weight click first — but that
  default is held, not shown: the weight/dashed buttons do **not**
  light while every side is unanimous-none. Only "None" lights; the
  default is real underneath but invisible until a side is actually
  active.
- **Mixed** — the selected cells disagree on that side: some have it and
  some don't (partial presence), or every cell has it but weight/color/
  dashed differ. Matt's ruling (2026-09-24): partial presence counts as
  mixed, not absent — "one cell bordered, one not" is a disagreement, not
  a unanimous "no border". Rendering (Matt's ruling): **the side button
  stays lit**; **"None" stays dark**; **every weight/dashed button stays
  dark** (indeterminate, matching the existing option-B convention
  already used for cross-side weight/dashed disagreement — see below).

Cross-side disagreement (two *different* sides are each individually
unanimous but disagree with each other, e.g. Top unanimously 2px and Left
unanimously 1px) is a separate, already-shipped case (#2874 item 29,
"Matt's Rulings" #1, option B): both side buttons stay lit, and the
weight/dashed controls go indeterminate the same way mixed same-side
disagreement does. Item 45 is the same-side case one layer below this —
disagreement within a single side's own cells, which used to be swallowed
(collapsed into "no cell has this side") before the cross-side logic ever
saw it.

A click on a side toggle while the weight/dashed controls are still
indeterminate does not silently apply a fallback weight of 1px/solid —
that would clobber real, already-set per-cell weights the user never
chose to change. The apply is deferred until the user explicitly resolves
the ambiguity via an actual weight or dashed click (or via "All"/"None",
which are absolute actions per Matt's ruling #2, not toggles, and always
apply immediately). Side-toggle clicks keep this wait guard unchanged.

**Border panel clicks write only the clicked attribute (Matt's ruling,
2026-09-24, closing Reed's delta-audit FAIL on PR #2963 — also fixes
#2874 item 29's cross-side version of the same clobber).** The wait
guard above only ever covered the side-toggle path; it did not cover a
bare weight or dashed click, which is a *different* button and therefore
not deferred by that guard at all. A weight or dashed click never waits
— it always applies immediately ("no dead clicks") — but it now writes
**only the one attribute actually clicked**, per cell, per side:

- **A dashed click** on a mixed (or cross-side-disagreeing) selection
  makes every affected cell dashed on the active side(s), while each
  cell **keeps its own weight and color** for that side — never a shared
  panel-wide weight.
- **A weight click** works the same way for the other axis: every
  affected cell takes the clicked weight on the active side(s), while
  each cell **keeps its own dashed state and color**.
- **No overwrites** — a real, already-set per-cell value for the
  attribute *not* clicked is never replaced with a shared or default
  value. (A side that was just toggled on and has no prior data at all
  on a given cell still gets a sane 1px/solid default for the
  not-clicked attribute — there is nothing real to preserve there, so
  this is not an overwrite.)
- **No dead clicks** — the click always takes effect; mixed state is
  never "fixed" by making a weight/dashed click a no-op.

This is the same clobber in both its item-45 form (same-side mix: e.g.
two cells with a top border at weights 2 and 1 — clicking dashed used to
collapse both to weight 1) and its older item-29 form (cross-side mix:
Top unanimously 2px, Left unanimously 1px — clicking dashed used to
collapse both sides to weight 1 too, since the underlying apply path was
shared and was never guarded on this axis for either shape of
disagreement).

**New borders created by a weight/dashed click follow the outer-edge
rule; existing borders keep their own values regardless of position**
(Matt's ruling, 2026-09-24, closing Reed's second delta-audit FAIL on PR
#2963). The "sane default for a not-clicked attribute" carve-out above
only applies when the cell is on the selection's outer edge for that
side — the same `resolvedBorderSides` geometry check the full side-apply
path already uses. A cell that already has a real entry for the side is
updated per-attribute no matter where it sits in the selection (that's
the clobber fix above, unchanged); a cell with no entry only gets a
fresh one when it's on the outer edge, so a weight/dashed click on a
partial-presence mix ("one cell bordered, one plain") fills the border
in along the outer edge — the same as clicking the side would — and
never creates a border on an inner cell.

**"All" vs. individual sides are two distinct modes, not an equivalence
(#2874 item 53, Matt's ruling, 2026-10-04 — reversing item 29's original
fix, commit `889b88d2`).** Item 29 originally treated "All" and "all
four sides toggled on individually" as the same desired state, since
both produce the identical side list. That equivalence is overturned:

- **"All" mode** — every line, inner lines included, is painted on
  every cell in the selection. "All" lights in the panel. Reached ONLY
  by an explicit click on the "All" button.
- **Individual-sides mode** — outline only, using the existing
  outer-edge rule (§33.6.1 above) exactly as any other 1–3-side
  selection. "All" is **never** lit, even when all four directional
  buttons happen to be toggled on. Reached by toggling any single
  side — including from "All" mode, which switches to individual-sides
  mode on that click. "None" clears either mode back to individual-sides
  with no sides active.

These two modes can produce the identical `sides` array
(`top`/`bottom`/`left`/`right`, all four) but are NOT the same desired
state — the distinguishing signal is a tagged mode value threaded from
the panel's own state through the apply call, never a count of how many
sides are set. A directional side count can never be the gate: that was
precisely item 29's (now-reversed) mistake.

**Opening the panel on an existing selection** seeds which mode is shown:
"All" lights only if **every** cell in the selection individually has
all four sides present; otherwise the sides are shown individually (per
§33.6.1's existing per-side unanimity/mixed rules, unaffected). For a
single-cell selection the two checks ("every cell has all four" vs. the
per-side unanimity loop) always agree, so lighting "All" there is
correct either way.

#### 33.6.2 A side click changes only that side (#2874 item 59)

> Note: the brief commissioning this clause referred to it as §33.6.1;
> that number is already in use above ("Border unanimity states", #2874
> item 45). Filed here as §33.6.2, the next free subsection under §33.6,
> per Build Standards §6 (flag spec numbering conflicts rather than
> silently overwrite or renumber an existing, unrelated, already-shipped
> section).

> **Revision, 2026-10-07:** this clause originally described a REBUILD
> (`bd` regenerated fresh from the panel's complete `sides` set plus the
> outer-edge check, on every apply). That shipped, passed review, and
> was then found to delete borders the user never touched: a cell's own
> stored side that happened not to be lit in the panel at the moment of
> an UNRELATED side click — on that cell or any other cell in the same
> selection — was silently wiped, because a rebuild answers "what does
> the panel say is lit" instead of "what did the user just click." Matt's
> ruling below supersedes the rebuild description; the toggled-side rule
> is now the authoritative behavior.

**A side click changes only that side** (Matt's ruling, 2026-10-07,
quoted in full): "A side click changes only that side. Turning a side on
adds it to cells on the selection's outer edge. Turning it off removes
it from every selected cell. Every other side on every cell stays
exactly as stored, whether or not it's lit in the panel."

- **Turning a side on** adds it to every cell on the selection's outer
  edge for that side (§33.6.1's existing outer-edge rule, unchanged) —
  cells not on that edge are untouched entirely, not even re-written with
  their own unchanged value.
- **Turning a side off** removes that one side's entry from **every**
  selected cell, regardless of outer-edge position.
- **Every other side, on every cell, is preserved exactly as stored** —
  whether or not that other side happens to be lit in the panel's
  current toggle set. A cell's own real border data is never discarded
  just because the panel's `sides` set doesn't currently include it;
  only the ONE side named in this click is ever read or rewritten.
- Clicking a side while in "All" mode is the one exception that is NOT a
  delta: it redraws the whole selection as an outline (the mode switch
  to individual-sides, §33.6.1 above), discarding "All"'s blanket
  inner-line fill — a genuine full-snapshot moment, since "All" was
  itself a blanket action and not per-cell preserved data.

**Mechanism — how the apply learns which side was toggled, and in which
direction:** the border panel already computes both at the moment of the
click (it diffs the clicked side against its own pre-click toggle set
before updating it), so the click handler sends `{side, on}` directly to
the apply instead of collapsing that information back into a full
`sides` array. The apply then touches only the named side on each
selected cell — adding it (on the outer edge only) when turning on,
removing it unconditionally when turning off — and never reads or
rewrites any other side's stored data. This replaces the rebuild this
clause previously described, which discarded a cell's entire border set
and regenerated it from the panel's complete toggle set on every
individual-sides apply (correct only for the "leaving All mode" case
above, where a full redraw genuinely is the desired behavior).

### 33.7 Sort / filter

View-state only: sorting or filtering never rewrites `sort_order` or any
stored data. Per-sheet view state persists locally (not synced) — mechanism
mirrors existing per-view UI state; design phase finalizes.

Toolbar sort and filter act on the selected column through the existing
header-menu engines. When no whole column is header-selected, the anchor
cell's column is used. Successive toolbar-sort presses cycle ascending,
descending, cleared; switching columns resets the cycle to ascending.

### 33.8 Deletion and integrity

Deletion path: §1.12.2. The **orphan sweep** deletes side-table rows whose
`note_id` has no `notes` row, and cells whose `row_id`/`column_id` no
longer exists — closing the concurrent delete-vs-edit orphan class (CRR,
no FK). One implementation, two triggers, one owner (the integrity module):
1. **At launch**, on the existing startup integrity path.
2. **After each `apply_changesets` batch** that touched any sheet table,
   scoped to the affected `note_id`s, before the post-sync recompute
   (§33.5.4) so formulas never evaluate against orphaned cells.

### 33.9 Versioning — full-grid snapshots

> **Status:** BUILT (#2339 PR 8) — backend only; the version-history UI
> panel that lists/triggers sheet restores is design-gated, not in scope
> for this PR. Three additional rulings from Matt (2026-09-04), layered
> on the 2026-09-03 ruling below: reorders (`reorder_sheet_columns`/
> `reorder_sheet_rows`) are STRUCTURAL — snapshot-before + throttle
> bypass, resolving §33.9.2's "borderline" flag (range membership depends
> on row order, so a reorder can silently change SUM/AVERAGE results);
> the pre-restore capture (§33.9.3 step 1) reuses the existing
> `pre_restore` source tag, no sheet-specific variant; the manual-edit
> throttle constant is extracted once (`notes::MANUAL_EDIT_THROTTLE_SECS`)
> and shared by both the regular-note gate and the new sheet throttle.

> Matt ruling 2026-09-03, reversing this spec's initial "no v1 versioning"
> position (push-back recorded in session notes). Sheets get
> moment-in-time full-grid snapshots via a **dedicated path**. Sheets never
> route through `maybe_save_manual_version`; the notes.rs:1370 gate stays
> byte-identical.

#### 33.9.1 Storage

Snapshots are `note_versions` rows (§1.5) with `source = 'sheet_snapshot'`
(new source value; column unconstrained — no migration). `content` holds a
versioned JSON envelope:

```json
{ "v": 1,
  "columns": [{ "id", "name", "col_type", "options_json", "width", "sort_order" }],
  "rows": [{ "id", "sort_order", "height" }],
  "cells": [{ "row_id", "column_id", "value", "style_json" }] }
```

Raw values and formula strings only — **computed values are never stored**
(ADR-0066 D4 applies to snapshots too). IDs are stored verbatim; they are
load-bearing for restore (§33.9.3). The existing 50-row all-source prune
(versions.rs:36–43) applies unchanged and is the retention policy.

The preview renders a display-only recompute and never writes it back.

#### 33.9.2 Capture triggers and throttling

Snapshot capture mirrors the §1.5 manual-edit path's shape: capture on
grid-mutation commit, throttled to at most one snapshot per sheet per
throttle window (mirror the manual_edit window constant in versions.rs at
build time — same value, one source of truth), so a burst of cell edits
produces one snapshot, not fifty. Structural mutations (add/delete row or
column, type change) capture immediately, bypassing the throttle — they are
the changes users most want to undo. Capture serializes the **pre-mutation**
state, matching manual-edit semantics (a version is what you can go back
to).

#### 33.9.3 Restore

Restore is a diff-apply against the snapshot, one transaction:
1. Capture a pre-restore snapshot first (restore is itself undoable).
2. Upsert every snapshot column/row/cell **by its stored ID**; delete live
   rows/columns/cells absent from the snapshot (cascade rules §1.12.2 —
   same hard-DELETE mechanism, same exemption).
3. Recompute (§33.5.4), regenerate `body` (§33.4), update FTS.

Preserving IDs keeps every formula reference valid across restore and
minimizes CRR churn (unchanged cells produce no changefeed entries). The
version-history UI renders sheet snapshots sheet-natively (design phase);
the markdown diff view never renders the JSON envelope.

### 33.10 Multi-device warnings block — deferred conflict UX

Deferred to the multi-device arc. That arc inherits this checklist:

- **Insert-order drift:** concurrent row/column inserts converge with
  LWW-resolved `sort_order`; order may differ across devices until
  convergence. Required UX: none in v1; multi-device arc must decide
  visible ordering-settled indicator vs. fractional-index scheme.
- **Delete-vs-edit resurrection:** cr-sqlite deletes are LWW — a concurrent
  edit resurrects the row (sync.md:100). The orphan sweep (§33.8) removes
  strays; multi-device arc decides whether a recovery indicator replaces
  silent sweep.
- **Same-cell race:** LWW per column — last causal write wins, silently.
  Acceptable (same semantics as note title races).
- **`body` skew:** transient cross-device disagreement until local
  recompute; self-healing, no UX required.
- **Layout sizes sync** (column `width`, row `height` — Matt ruling
  2026-09-03): revisit whether per-device layout is wanted at the
  multi-device arc.

### 33.11 AI tools

Five new tools (ai-integration §4.1; ADR-0066). AI writes formulas as cell
values — "tally this column" works in v1 by writing `=SUM(...)`. Sheet
content reaches AI context on-demand via tools/context markers only; never
eagerly serialized into system prompts. AI-driven mutations flow through
the same write chokepoint as user edits: body regeneration, FTS, snapshot
throttling all apply identically.

The agent has six sheet tools: create_sheet, get_sheet_data, set_sheet_cells, add_sheet_row, add_sheet_column, delete_sheet_column. create_sheet accepts an optional columns array of {name, type, options}; when absent it seeds five Text columns named Column 1–5. delete_sheet_column takes a column id and validates it belongs to the sheet; it runs through the same chokepoint as the UI, so it snapshots before deleting. Neither tool can act on a sheet in an agent system folder.

Status: PENDING — items 35, 40 / #2874. I merge, then Stia.

### 33.12 CSV export (v1 — ADR-0066 D8)

Per-sheet, user-initiated, one sheet → one CSV via OS save dialog.

- **Computed values, not formula strings.** A formula cell exports its
  evaluated display value; error cells export their error string
  (`#DIV/0!` etc.).
- RFC 4180: header row = column names in `sort_order`; data rows in
  `sort_order`; UTF-8; quoting/escaping per the RFC. Checkbox exports as
  `TRUE`/`FALSE`; date as ISO 8601; currency as the decimal value (code is
  column metadata, not repeated per cell).
- Lossy by design: types, formulas, styling, and column config do not
  round-trip (ADR-0066 consequence; import is out of v1).
- Default filename: sanitized sheet title + `.csv`.
- Export reads the in-memory computed grid — it never triggers writes,
  snapshots, or recompute.

### 33.13 Out of scope (v1)

Functions beyond SUM, AVERAGE, and IF; cross-sheet references; computed/rollup
columns; cell comments; row grouping; **CSV import** (export is IN —
§33.12); bulk/workspace export; cell-level version history (full-grid
snapshots are IN — §33.9); multi-device conflict UX (§33.10); mobile grid
editing; `analyze_sheet` aggregate tool (revisit with bookkeeping North
Star).
