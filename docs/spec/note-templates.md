# 32. Note Templates

**Section:** §32
**Status:** TARGET — NOT BUILT
**ADRs:** ADR-0058 (templates are named snapshots in a dedicated table). Extends §31 (note components), core-data-model §1.10 (templates table), §1.9 (subtasks).
**Issue:** #193

---

## §32.0 What a template is

A **template** is a named, reusable snapshot of note content: the body markdown (including any §31 component sentinels) plus the checklist item payload that the markdown alone cannot carry (§31.3.3 — checklist items live in the `subtasks` table, not the note body). Applying a template stamps that snapshot into a note.

A template is **not a note and not a task**. It has no status, no due date, no project, no file on disk, and appears on no note or task surface. It lives in the dedicated `templates` table (core-data-model §1.10, ADR-0058) — the same structural-exclusion move as ADR-0057.

> **📌 Why This Exists — a naive body-copy is broken (2026-07-20, ADR-0058):** After §31, a note's body is not the whole note. Checklist items are `subtasks` rows; the serialized checklist sentinel is an empty `<div data-component="checklist">`. Copying only the markdown produces a checklist that instantiates blank, and copying a Due Today sentinel verbatim would carry a stale absolute date. Templates must therefore capture the checklist payload explicitly and store the Due Today block dateless — this section defines exactly what is captured and what is regenerated at apply time.

## §32.1 What a template stores

Per template (schema in §1.10):

- **`name`** — user-facing, unique-by-convention (app-enforced, §32.2.2).
- **`content`** — the note's body markdown at save time, as produced by the normal note serializer (component sentinels ride through as raw-HTML blocks), with one normalization: any `data-component="due-today"` sentinel has its `data-date` attribute **stripped**. Templates are dateless by construction.
- **`checklist_json`** — the note's checklist items as `[{"text": "...", "sort_order": N}, …]`, read from the `subtasks` table at save time. **Text and order only.** `done`, `completed_at`, and `due_date` are never stored (locked, Matt 2026-07-20 — stale dates would fire immediate overdue notifications on apply; done-state is instance state, not template content).

Explicitly **not** stored: note title, status, priority, due date, assignee, project/folder placement, tags, attachments beyond whatever refs happen to sit in the markdown. A template snapshots content, not note metadata.

Because no date and no done-state can exist in a template, **no template operation can ever emit a notification** — this is structural, not a rule (§32.5).

## §32.2 Save-as-template

### §32.2.1 Entry point and capture

Saving is an editor-level action on the currently open note ("Save as template" — placement of the control is a design-phase call; Claude Design owns visuals). On invoke:

1. Prompt for a name, defaulting to the note's title.
2. Capture `content` per §32.1 (serialize body → strip due-today dates).
3. Capture `checklist_json` per §32.1 (query the note's `subtasks`, project to text + sort_order).
4. Write the `templates` row.

A note with no checklist saves `checklist_json = '[]'`. A blank checklist sentinel in the body therefore applies blank; a filled one applies filled — the template's own contents decide (locked).

### §32.2.2 Name collision (app-enforced — no DB constraint)

`templates` is a CRR table; uniqueness cannot be a schema constraint (#493 pattern). At save, a **case-insensitive** name match against existing templates governs the flow:

- **Exactly one match** → prompt the user: **Overwrite** (UPDATE the matched row's `content`/`checklist_json`/`updated_at` in place — id stable) or **Rename** (new row).
- **Two or more matches** (possible only via sync-merged duplicates, below) → Overwrite requires an **explicit target**: the prompt lists every matching row (name + `updated_at`) and the user picks the one row to overwrite, or chooses Rename. Overwrite never selects a target implicitly — silence is not a default.

No silent overwrite, no silent duplicate from the save path.

Sync merge can still produce same-named templates created independently on two devices: they **coexist** as distinct rows (distinct ids; LWW applies per-row, never across rows). The list surface displays both; the user resolves by rename/delete (or by the multi-match Overwrite pick above). This is accepted CRR behavior, not a bug.

## §32.3 Applying a template

### §32.3.1 Entry point — the existing surface

Apply targets the **currently open note** via the existing template surface: `Cmd+Shift+T` → `pane-show-templates` → the templates grid (already wired frontend, currently fed by a stub). Selecting a template applies it into the open note. Matt's anchor flow: new note → rename → `Cmd+Shift+T` → pick template → today's daily note is ready.

**Apply-to-current-note is the only instantiation path at launch.** New-note-from-template (the dormant `createNoteFromTemplate` stub) is post-launch (§32.6).

### §32.3.2 Apply semantics

Applying template T to note N:

1. **Content:** insert T's `content` into N at the cursor (empty note ⇒ becomes the body), through the normal editor content path so the sentinels reconstruct as live nodes on the next parse.
2. **Due Today date stamp:** every due-today sentinel in the inserted content receives `data-date` = the **current local calendar date at apply time**, before insertion. This reproduces §31.2.3's insert-time default mechanically — apply does not route through `insertDueToday()`, so the stamp is the apply path's responsibility. A dateless sentinel must never reach the rendered note (§31.2.3's null-date rule: error state, never silent-now() — apply prevents the case from arising).
3. **Checklist items:** for each entry in `checklist_json`, create a **fresh `subtasks` row** keyed to N: new UUID, `text` copied, `done = 0`, `due_date = NULL`, `completed_at = NULL`, `sort_order` **continuing after N's current maximum** (append semantics). The checklist sentinel in the content then renders these at mount per §31.3.
4. Steps 2–3 and the body write land atomically from the user's perspective: an apply either fully takes or leaves N unchanged.

### §32.3.3 Interaction with existing components in the target note

**Checklist:** subtasks are keyed by `note_id` and a note has one logical list (§31.3.2). If N already has subtasks, applied items **append** to that list. If N already contains a checklist sentinel and T contributes another, both nodes render the same merged list — supported, not meaningful, unchanged from §31.3.2. Apply never deletes or reorders pre-existing items.

**Due Today:** §31.2 imposes no one-per-note constraint — each due-today node is self-contained (it stores only its own `data-date` and queries item data at render, §31.2.3). A template-inserted Due Today block therefore coexists with any existing block as an **independent node stamped with the apply date**; existing blocks keep their own stored dates unchanged (anchored-not-live, §31.2.3). Two blocks carrying the same date render the same snapshot twice — supported, not meaningful, mirroring §31.3.2's stance on duplicate checklist nodes.

## §32.4 Template lifecycle

- **List:** the templates grid (`Cmd+Shift+T`) and the Settings → Templates tab, both existing surfaces, both fed from the `templates` table.
- **Rename:** in-place UPDATE of `name` (same collision flow as §32.2.2).
- **Edit content:** v1 has no in-place template editor. To revise a template: apply it to a note, edit, save-as-template over the same name (Overwrite). Flagged as a deliberate simplification.
- **Delete:** hard DELETE of the row, with confirmation. No soft-delete — templates are not notes; nothing references them.
- **Independence:** applied notes are copies. Editing or deleting a template never touches any note or subtask it previously produced. There is no linkage, by construction (§1.10.4).

## §32.5 Notifications

No template path can emit, dismiss, or clear a notification. Templates store no dates and no done-state; applied subtasks are born `due_date = NULL, done = 0`, which the §19.5.1b watcher ignores until the user sets a date. No §19 amendment required or permitted from this section.

## §32.6 Out of scope (launch)

- **New-note-from-template.** `api.ts` `createNoteFromTemplate()` stays a dormant stub; post-launch candidate (session notes).
- Recurring/scheduled instantiation (pairs with future recurring tasks — door open).
- Date tokens in titles or content.
- Template sharing beyond normal CRR device sync.
- In-place template content editor (§32.4).
- Visual design of the grid, save control, and prompts (Matt drives Claude Design after merge).
