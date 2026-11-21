# 31. Note Components — Due Today + Advanced Checklist

**Section:** §31
**Status:** TARGET — NOT BUILT
**ADRs:** ADR-0057 (subtasks as component line items). Extends §19 (notifications) and core-data-model §1.9 (subtasks).

---

## §31.1 What a note component is

A **note component** is an interactive block embedded in a note body. It is not free text and not an attachment — it is a first-class TipTap node with a Svelte NodeView, following the exact pattern the Attachment node established (#401):

- `Node.create(...)` with `group: 'block'` registers the node type.
- `addNodeView()` mounts a Svelte host component into a `contenteditable="false"` wrapper; the Svelte component owns the interior and its own styles.
- An `insert…` command inserts the node at the cursor.
- Persisted node attributes live on the wrapper as `data-*`; the serializer emits a raw-HTML block so `parseHTML` round-trips losslessly.

> **📌 Why This Exists — reuse the NodeView pattern (2026-07-18, ADR-0057):** "Add a component to a note" is not new note infrastructure. The Attachment node (#401) already proved the mount-point pattern: a block node whose visual layer is a Svelte NodeView and whose persisted state is `data-*` on a wrapper div. Both components here ride that pattern. The only structural addition either component needs is the `subtasks` table (§31.3, core-data-model §1.9) — the embed mechanism is settled.

This section specs two components:

- **§31.2 Due Today** — an interactive, date-anchored daily snapshot of everything due on the node's stored date; rows can be checked off or clicked through to their source.
- **§31.3 Advanced Checklist** — an authoring component whose items are subtasks (dedicated `subtasks` table) carrying their own due dates that fire through the existing notification system (§19).

---

## §31.2 Due Today

> **📌 Amended 2026-07-18 (Design review):** Due Today is no longer a read-only live view. It is an **interactive, date-anchored daily snapshot**: items can be checked off from the list, done items stay visible (marked done), and the node shows the list for the date it was inserted — it never rolls over to the current day. This matches Matt's one-todo-note-per-day workflow (a note per day, each holding that day's snapshot).
>
> **📌 Why This Exists — anchored, not live (2026-07-18):** The stored insertion date is deliberate and must not be "fixed" into tracking the current day. A node inserted on the 18th always shows the 18th's items — that is the point: each daily note is a durable record of that day, not a window that empties as the date advances. A future change that makes Due Today roll over to `now()` breaks the workflow and re-introduces the cross-device timezone ambiguity that a stored local date removes. Treat "shows an old date" as correct behavior, not a bug.

### §31.2.1 Behavior

The Due Today node renders an **interactive** list of every item due on the node's **stored date** (§31.2.3). Items can be checked off from the list (§31.2.4). The node stores one piece of state — its date (`data-date`) — and queries item data at render time; it does not store item content.

> **Amended 2026-07-18:** Supersedes: "The Due Today node renders a live, read-only list of every item due today across the whole workspace. It stores no data of its own — it queries at render time."

**Scope (locked):**

- **All projects.** Not scoped to the current project or context.
- The node's date defaults to the **local calendar date at the moment of insertion** (local midnight boundary), stamped into `data-date` as the default and thereafter **user-changeable via the block date picker** (§31.2.3). It never rolls over on its own. All devices show the same stored date (the attribute lives in the synced note body), so there is no viewing-device drift.

  > **Amended 2026-07-18:** Supersedes: "Day boundary is local midnight. \"Today\" is the local calendar day of the viewing device."

- **What counts as due on the stored date:** both — the snapshot includes items due on the stored date **regardless of done state**; done rows render distinctly (§31.2.5). Trashed/deleted items still drop out. Let `:D` = the node's `data-date`:
  - **Tasks** — `SELECT … FROM notes WHERE due_date = :D AND deleted_at IS NULL` *(no `status != 'done'` filter)*. (Notes and tasks are the same entity per ADR-0003; a task is a note with a workflow status. "Any task" here means any such note.)
  - **Subtasks** — `SELECT … FROM subtasks s JOIN notes n ON n.id = s.note_id WHERE s.due_date = :D AND n.deleted_at IS NULL` *(no `s.done = 0` filter; keep the parent-not-trashed join)*.

  > **Amended 2026-07-18:** Supersedes: Tasks filter `status != 'done'` and Subtasks filter `done = 0`.

  > **⚠️ Display-only change — the watcher is NOT affected.** This done-inclusive query is Due Today's **render** query only. The §19.5.1b due-date watcher scan keeps its `done = 0` / undone filters unchanged — done items must not fire notifications. Do not align the two queries; the asymmetry is intentional.

**Differentiation (required — two axes):** task rows and subtask rows are visually distinguished so the user can tell them apart — a subtask row is labeled as a subtask and shows its parent note's title for context. This mirrors the notification-drawer differentiation (§19). Rendering **additionally** distinguishes done vs not-done rows (§31.2.5).

### §31.2.2 Routing

Each row has a **checkbox** and a **body**.

- **Checkbox tap** → check-off (§31.2.4). Does not navigate.
- **Body tap** → route to source, unchanged: task row → open the note; subtask row → open the parent note and highlight the item.

> **Amended 2026-07-18:** Supersedes: "Each row routes to its source on click:" — single affordance; the colon introduced the task/subtask routing sub-bullets. Rows now have two affordances: checkbox (check-off) and body (routing).

Routing reuses the note-open path already used by notification click-through (§19 `target_kind`/`target_id` routing).

### §31.2.3 Node + serialization

- Node name: `dueToday`, `group: 'block'`, `draggable: true`, `contenteditable="false"` wrapper.
- The node persists exactly one attribute — its **date** — alongside the component sentinel. It stores no *item* content (items are still queried from `notes`/`subtasks` at render).

  > **Amended 2026-07-18:** Supersedes: "No persisted content attributes (the list is always queried live). A single `data-component="due-today"` sentinel on the wrapper identifies the block."

- Node attribute `date` (serialized `data-date`, ISO 8601 local date, e.g. `2026-07-18`), following the Attachment attr pattern:
  ```ts
  date: {
    default: null,
    parseHTML: el => el.getAttribute('data-date'),
    renderHTML: attrs => attrs.date ? { 'data-date': attrs.date } : {},
  }
  ```
- Insert command: `insertDueToday()` stamps the **current local date** into `date` as the default at insert time.
- **The date is user-selectable.** A date picker on the block lets the user change the block's date; picking a date writes the new value to `date` (`data-date`) and the snapshot re-queries for that date. This is a **block-level** control, distinct from the per-row checkbox (§31.2.4). The anchored-not-live rule is unchanged: the block shows whatever date is set and never rolls over to the current day.
- Serialization: `<div data-component="due-today" data-date="2026-07-18"></div>`. There is no plain-markdown equivalent; the block renders in-app and is inert in external markdown viewers.

  > **Amended 2026-07-18:** Supersedes: `<div data-component="due-today"></div>` (no date attribute).

- `date` is required for a functioning node. Since §31.2 is NOT BUILT, no `dueToday` node exists without it — no legacy/null-date path to handle. (Defensive: a null `data-date` should render an empty/error state, never silently fall back to `now()`, which would violate the anchored-not-live rule above.)

### §31.2.4 Check-off behavior

The **checkbox is the only per-row control** (tick and un-tick). It writes through the **existing** paths — no new write path, no §19 change. Any status change other than done/undone happens in the note itself, not from Due Today.

- **Task row**
  - **Tick** → set the note's `status = 'done'` via `update_note`. Same path that fires §19.5.1 step-5 auto-dismiss of that task's due notification + §19.5.1a companion behavior. The row then renders done, still visible in the snapshot.
  - **Un-tick** → set the note's `status = 'todo'` via `update_note` (a task has no stored prior status; `todo` is the fixed revert target). Fires the §19.5.1a companion re-fire path normally.
- **Subtask row**
  - **Tick** → flip `subtasks.done` → 1 via the **same write path as the checklist component** (§31.3); triggers §19.5.1c auto-dismiss.
  - **Un-tick** → flip `subtasks.done` → 0 via the same path; triggers the §19.5.1c companion re-emit.

Because both routes reuse existing writers and reconciliation, **no §19 amendment is required** and the subtasks table is unchanged.

### §31.2.5 Done rows and empty state

- **The block displays its stored date in its normal (non-empty) state**, so the snapshot is self-identifying — the user can see which day's list they are looking at without opening the picker.
- **Done rows stay visible**, rendered distinctly (checkmark / strikethrough — Claude Design owns exact visuals). A fully-completed day still shows its rows; it is **not** the empty state.
- **Empty state** applies only when nothing (done or not) was due on `:D`. Wording: **"Nothing was due on {date}."** (not "nothing due today").
- **Deleted ≠ done.** A trashed note or hard-deleted subtask drops out of the snapshot (the `deleted_at`/join filters); only *done* items remain visible.

---

## §31.3 Advanced Checklist (authoring + data)

### §31.3.1 Behavior

The Advanced Checklist node renders the parent note's subtasks as an editable checklist. Each item is a `subtasks` row (core-data-model §1.9): a line of text, a done toggle, an optional due date, and an ordering. When an item's due date crosses a threshold, it fires a notification through the **existing** pipeline (§19) — no new notification mechanism.

**Item operations:** add item, edit text, toggle done, set/clear a per-item due date, reorder, delete item. Each maps to a subtask command (writer side; Stia builds after this spec lands).

### §31.3.2 Ownership model

Subtasks are keyed by `note_id` (the parent note). The checklist node renders that note's subtask list. The node reads its parent note id from the `activeNote` store (`$activeNote?.meta?.id`) — the same source used by `AttachmentNodeView.svelte`. This was corrected from the original `canvasState.context.noteId` reference, which is null in new-note creation and All Tasks panel flows.

<!-- Amendment: 2026-07-19 — corrected note ID source from canvasState.context.noteId to activeNote store per diagnosis (PR #2104 live-test finding) -->

**One checklist per note (launch model).** Because subtasks are keyed by `note_id`, a note's subtasks are a single logical list. A second checklist node in the same note would render the same list. This is supported but not meaningful; per-component lists are out of scope at launch. *(If per-component lists are wanted later, add `subtasks.component_id` and key on it — flagged in session notes, not built here.)*

### §31.3.3 Data source of truth

The `subtasks` table is the authority for item text, due date, done state, and order. The checklist node is a **mount point**, not the payload — item data is **not** written into the note's flat markdown. In external markdown viewers the block renders inert.

> **📌 Why This Exists — DB is the source of truth, not the markdown (2026-07-18, ADR-0057):** Subtasks carry structured state (per-item due dates, done flags, ordering) that does not round-trip cleanly to portable markdown, and that state must be queryable by the due-date watcher independent of note-body parsing. Keeping items in the `subtasks` table — not serialized into the `.md` — makes the watcher scan (§19.5.1b) a simple indexed query and keeps a single writer per item. Portability of the structured list is out of scope at launch; the checklist is app data, the way a task list is app data in any task tool.

### §31.3.4 Node + serialization

- Node name: `checklist`, `group: 'block'`, `draggable: true`, `contenteditable="false"` wrapper.
- Persisted attribute: `data-component="checklist"` sentinel only. No item data on the node.
- Insert command: `insertChecklist()`.
- Serialization: raw-HTML block `<div data-component="checklist"></div>`. Items render from the `subtasks` table at mount, keyed by the parent note id.

### §31.3.5 Notification integration

Fully specified as an extension of §19 — see §19.5.1b (subtask due-date scan), the `target_kind='subtask'` routing arm, and the drawer label. In summary:

- A subtask's due date fires **`task_due` / `task_overdue`** (reuse the existing types — channels and rendering unchanged), distinguished by **`target_kind='subtask'`**, `target_id` = subtask id.
- idem_key = `subtask_due:{subtask_id}:{threshold}` (distinct prefix from the note key `due:` — no collision).
- The drawer marks subtask-attached rows distinctly and clicking routes to the parent note + highlights the item.

### §31.3.6 Deletion path

See core-data-model §1.9.4 and §19.5.1c for the authoritative rules. Summary of what the user sees:

- **Delete an item** → the item disappears and any pending/overdue notification for it clears.
- **Trash the parent note** → its subtasks stop surfacing (Due Today, watcher) and their pending notifications clear; the items are preserved and return if the note is restored.
- **Restore the parent note** → subtasks reappear; due ones re-fire on the next watcher pass.
- **Permanently delete the parent note** → its subtasks and their notifications are gone for good.

---

## §31.4 Relationship between the two components

Two separate components sharing one due-date data path:

- Due Today (§31.2) is an **interactive dated snapshot** of due items (tasks + subtasks) — it both displays them and lets the user check them off; check-offs route through the same writers as the Advanced Checklist and the task status path. It is a per-date snapshot, not a live view.

  > **Amended 2026-07-18:** Supersedes: "Due Today (§31.2) is a read-only view over due items (tasks + subtasks)."

- Advanced Checklist (§31.3) is the **authoring surface** that produces dated subtasks, which then show up in Due Today and fire notifications.

They are not one merged spec. Due Today also surfaces plain task due dates that predate any checklist.

---

## §31.5 Out of scope (launch)

- Per-component subtask lists (`subtasks.component_id`).
- Serializing checklist items into portable markdown.
- Claude Design's visual layout of either component (Matt drives that directly).
- Fixing the AI-set-due-date-doesn't-light-the-bell bug (2026-07-15) — a prerequisite/coordination point (session notes), not fixed here.
