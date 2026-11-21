# ADR-0066: Sheets Are Typed Notes with Sparse Grid Side Tables

**Status:** Proposed
**Date:** 2026-09-03
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

Sheets (#2339) add structured tables to Pane: typed columns, cell styling,
sort/filter, basic formulas, and an AI tool set. Reed's scoping audit
(2026-08-01) sized this as a subsystem, not a note type. Matt ratified the
strategy decisions on 2026-09-02 and amended v1 scope by rulings on
2026-09-03 (CSV export in; full-grid snapshot versioning in, reversing the
session's initial no-versioning position; `create_sheet` joins the AI tool
set, five tools); two Reed audits (2026-09-03) enumerated every `note_type`
predicate the change touches — 22 null-predicate sites and 15 equality/list
sites across database.rs, embeddings.rs, lib.rs, notes.rs, notifications.rs.

The data-model question is the fork ADR-0057 and ADR-0058 faced from the
opposite side. Subtasks and templates were kept OUT of `notes` because they
must appear on no note/task surface — structural exclusion. Sheets are the
inverse: user-created content that must appear on EVERY task surface (Kanban,
project view, folder view, All Tasks, calendar) and wants the full notes
affordance set — project/folder placement, tags, pinning, trash, status,
priority, due dates. Excluding sheets from `notes` would mean rebuilding all
of that bespoke, plus a UNION branch in every task-surface query.

One semantic is load-bearing: today `note_type` is a system-note discriminator
(memory, soul, rules, daily, ...) and `note_type IS NULL` means "real user
content" at 13 production predicate sites. Sheets are the first non-system
`note_type` value. This ADR makes that redefinition explicit rather than
letting it happen by accident.

## Decision

**A sheet is a `notes` row with `note_type = 'sheet'`. Grid data lives in
three dedicated CRR side tables — `sheet_columns`, `sheet_rows`,
`sheet_cells` — keyed by `note_id`, mirroring the `subtasks` pattern
(core-data-model §1.12). Cell storage is sparse: empty cells are never
stored.**

1. **`note_type` is redefined from "system-note discriminator" to "content-kind
   discriminator."** `NULL` = markdown note/task. System values = AI-managed,
   surface-hidden. `'sheet'` = user content with a non-markdown editor.
   Task-surface queries widen from `note_type IS NULL` to
   `(note_type IS NULL OR note_type = 'sheet')`; system-scope filters stay
   untouched. The two Reed audits (2026-09-03, on #2339) are the site-by-site
   map; the sheets spec (§33) carries the per-site dispositions.
2. **The notes row is plumbing, not presentation.** Opening a sheet mounts a
   full grid editor in the note-editor overlay position; the board card gets
   its own face. No markdown editor ever renders a sheet.
3. **Sheets have no disk file; per-sheet CSV export is the v1 data-access
   mechanism.** `file_path = ''`; canonical data is the side tables; `body`
   holds an auto-generated flat-text copy for FTS/embeddings/AI context,
   written atomically in the same transaction as every grid write. Local-first
   requires the user's data be reachable without Pane — for markdown notes the
   `.md` on disk satisfies that; for sheets, CSV export does (Decision 8).
   A generated disk file as the always-on representation is rejected: it
   would put sheets in the file-watcher path where a stale flat file can
   overwrite `body`. `'sheet'` joins the lib.rs §486 integrity-sweep
   allowlist (repair-from-file is meaningless for a fileless note).
4. **Computed values never sync — and are never persisted.** Formula strings
   and raw values sync; every device recomputes locally after
   `apply_changesets`. No CRR column, and no snapshot (Decision 9), ever
   holds a computed value. Spec invariant.
5. **Per-cell LWW is the sync model.** Each cell is a CRR row; concurrent
   edits to different cells both win, same-cell races resolve LWW —
   identical semantics to two devices editing different fields of one note.
   Structural conflict UX (row insert-order drift, delete-vs-edit
   resurrection) is deferred to the multi-device arc; §33 carries an explicit
   warnings block so that arc inherits a checklist.
6. **Custom grid component.** No licensed libraries (Handsontable, AG Grid
   ruled out); headless TanStack Table at most if its Svelte 5 story matures.
   Cells keep set sizes; overflow scrolls both axes with sticky headers.
7. **Deletion follows the notes lifecycle with an app-code cascade.** Soft
   delete rides `notes.deleted_at` (side tables untouched, hidden via parent).
   Hard delete cascades DELETE across all three side tables (CRR = no FK;
   §1.9.4 precedent; the tombstone-exemption mechanism is recorded in
   §1.12.2). Sheets are NOT added to the system-note delete protection
   lists — they delete like any note.
8. **Per-sheet CSV export ships in v1** (Matt ruling, 2026-09-03, amending
   the ratified 2026-09-02 v1 scope). One sheet → one CSV; formulas export
   as their computed values, not formula strings. Bulk/workspace export is
   deferred. Import (CSV or otherwise) stays out of v1.
9. **Full-grid snapshot versioning ships in v1** (Matt ruling, 2026-09-03,
   reversing the session's initial no-versioning position — push-back
   recorded in session notes). Snapshots are moment-in-time full-grid
   captures via a **dedicated path**: `note_versions` rows with
   `source = 'sheet_snapshot'` (new value on an unconstrained column — no
   migration), a versioned JSON envelope holding columns/rows/cells with
   raw values and formula strings only, IDs stored verbatim. Capture is
   pre-mutation, throttled per the §1.5 manual-edit precedent (structural
   mutations bypass the throttle); the existing 50-row all-source prune is
   the retention policy. Restore is an ID-preserving diff-apply preceded by
   a pre-restore snapshot, so formula references survive and restore is
   undoable. Sheets never route through `maybe_save_manual_version`; the
   notes.rs:1370 gate stays byte-identical. (§33.9.)

### Reconciliation with ADR-0003 / ADR-0057 / ADR-0058

ADR-0003 says notes and tasks are one entity. A sheet **is** that entity — it
takes a status, holds a due date, sits on the board. It differs only in what
its content is and which editor renders it. Putting sheets in `notes` is the
ADR-0003-faithful move, not a strain on it.

ADR-0057/0058's structural exclusion is not contradicted; its test is
inverted. Subtasks and templates were excluded because every surface
appearance would be a bug. For sheets every surface appearance is a
requirement. Exclusion-by-construction and inclusion-by-membership are the
same principle — let the data model, not remembered guards, decide surface
visibility. The cost this lineage warned about (auditing the shared query
surface) has been paid up front: 37 sites enumerated and dispositioned before
a line of schema ships.

## Consequences

### Positive

- **Full notes affordance set for free** — placement, tags, pinning, trash,
  task metadata, sort_order — no parallel implementations.
- **Task-surface inclusion is one predicate widening**, applied at sites the
  audits already name, each locked by a regression test (sheet variant of
  `test_kanban_excludes_notes_with_note_type`, inverted to assert inclusion).
- **Search and embeddings work day one** via the `body` flat-text contract;
  the embeddings.rs COALESCE query and notes.rs:637 search exclusion need no
  changes (equality audit #9: confirmed leave-as-is).
- **Sync reuses the entire existing engine.** Three new CRR members, zero new
  sync architecture.
- **Versioning reuses `note_versions` wholesale** — retention, pruning, and
  storage machinery come free; the only new pieces are the envelope format,
  the capture triggers, and the restore path (Decision 9).
- **Local-first data access holds** without a watcher-visible disk file:
  per-sheet CSV export covers the "my data without Pane" guarantee that the
  `.md` file covers for markdown notes.

### Negative

- **Three new CRR tables.** `CRR_TABLES` 24 → 27; `SCHEMA_VERSION` "89" →
  "90" with a real v89→v90 migration arm (post-launch policy — per-pair arm,
  upgrade fixture v89, `test_fresh_install` floor 42 → 45). Every NOT NULL
  column carries a DEFAULT per `test_crr_tables_not_null_columns_have_defaults`.
- **`note_type IS NULL` no longer means "all user content."** Every future
  content-kind addition must re-walk the widened predicate list. Mitigation:
  §33 carries the canonical predicate map; INVARIANTS.md gains the widening
  rule.
- **`body` is derived state in a synced column.** LWW on `body` can
  transiently disagree with cell-level truth mid-sync; local recompute
  (Decision 4) rewrites it. Accepted: `body` is a search/AI surface, never
  read back into the grid.
- **Sheet data is not continuously on disk.** No always-current `.md`/file
  representation; between exports, sheet content lives only in the DB (and
  its backups). CSV export is user-initiated, not automatic. Accepted for
  v1; if continuous on-disk representation is later wanted, it must be
  designed outside the file-watcher path.
- **CSV is lossy by design.** Computed values only — formulas, types,
  styling, and column config do not round-trip. Acceptable because import is
  out of v1; revisit fidelity if/when import lands.

### Risks

- **Orphaned side-table rows.** No FK enforcement (CRR); a concurrent
  row-delete vs. cell-edit can orphan cells. App-code cascade on hard delete
  plus an orphan-sweep on the integrity path (launch + post-sync); the
  delete path is the primary code-audit target (equality audit #11).
- **Body-write atomicity.** If any path writes cells without regenerating
  `body` in the same transaction, FTS/AI context drifts from grid truth. The
  spec names one write chokepoint; the allowlist entry (Decision 3) prevents
  the integrity sweep from ever "repairing" a sheet body to garbage.
- **A sheet row created with `note_type` unset** would leak into every
  plain-note surface as an uneditable ghost. Creation sets `note_type`
  atomically at INSERT; no infer-later path (null audit #1–3 caveat). The
  AI `create_sheet` tool calls the same creation chokepoint — no side door.
- **Snapshot envelope size.** A full-grid JSON snapshot of a large sheet is
  a heavier `note_versions.content` payload than any text version today;
  the 50-row prune bounds total footprint, and sparse serialization (only
  stored cells) bounds each. If real-world sheets outgrow this, move to a
  dedicated snapshot table in a follow-up — the envelope's `v` field exists
  so that migration is non-breaking.

## Alternatives Considered

**Option B — parallel `sheets` entity table, surfaced via UNION.** Rejected.
Every task surface, search, tags, pinning, trash, and AI tool would need a
second query path; wider blast radius than widening enumerated predicates,
and it re-implements the notes affordances one by one. (Reed scoping audit
§1.2.)

**JSON blob in `notes.body`.** Rejected. LWW on one TEXT column loses
concurrent cell edits wholesale, formulas/styling don't round-trip, and every
cell write rewrites the whole blob.

**Serialize the grid to a file on disk as the always-on representation.**
Rejected for the canonical store (lossy for types/formulas/styling; watcher
would parse or overwrite instead of indexed reads; stale-flat-file class).
Survives in two bounded forms: the generated `body` flat-text copy, and
user-initiated per-sheet CSV export (Decision 8).

**Dedicated snapshot table for versioning.** Deferred, not rejected.
`note_versions` reuse (Decision 9) ships v1 with zero schema change and
inherited retention; the versioned envelope keeps the door open.

## References

- Decision record: Matt, 2026-09-02, on #2339; amended by Matt rulings
  2026-09-03 (integrity allowlist accepted as flagged deviation;
  fileless-with-CSV-export; CSV export in v1 scope; layout sizes sync;
  full-grid snapshot versioning in v1; `create_sheet` in v1 tool set).
- Predicate audits: docs/audits/2026-09-03-note-type-audit.md,
  docs/audits/2026-09-03-note-type-equality-audit.md.
- Amends/relates: ADR-0003 (reconciled, not superseded), ADR-0057/0058
  (pattern lineage), ADR-0002 (CRR membership), ADR-0017 (migration arm),
  ADR-0008 (mechanization over discipline).
- Spec: sheets.md §33 (new), core-data-model §1.12 (new), §1.5
  (`note_versions` — reused, `source='sheet_snapshot'`), ai-integration §4
  (five new tools: `create_sheet`, `get_sheet_data`, `set_sheet_cells`,
  `add_sheet_row`, `add_sheet_column`), INVARIANTS.md (new Sheets section).
- Schema source: migrations.rs @ SCHEMA_VERSION "89", CRR_TABLES [24],
  `note_tags` composite-PK precedent (migrations.rs:1165–1173), engine.rs
  extract/apply_changesets — verified via Brit pulls, 2026-09-03.
- Grammar ratification: Matt, 2026-09-03, recorded in §33.5.1, shipped
  in #2851.
