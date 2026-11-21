# ADR-0058: Templates Are Named Snapshots in a Dedicated Table

**Status:** Accepted
**Date:** 2026-07-20
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

Note Templates (#193) is the last launch feature: save a note as a reusable template; apply a template into a note. Two facts force a real storage decision.

First, §31 made note bodies incomplete as content snapshots: checklist items live in the `subtasks` table (ADR-0057, §31.3.3), and both §31 components serialize as sentinel `<div data-component>` blocks — a checklist sentinel is empty by design, and a Due Today sentinel carries an absolute `data-date`. A body-copy template produces a blank checklist and a stale date.

Second, ADR-0003 looms over any "template" row: the obvious cheap implementation is a `notes` row with a flag, and this ADR exists partly to close that door with reasons on record.

Existing partial infrastructure (source-read 2026-07-20): a frontend `Template` interface (`types.ts` — id/name/content/created_at), stubbed `listTemplates()`/`createNoteFromTemplate()` in `api.ts`, a wired `Cmd+Shift+T` → `Templates.svelte` grid, and a Settings Templates tab. No backend table or commands exist. The `onboarding_template` column on `user_dashboard_config` is the onboarding wizard's layout choice — unrelated; not reused.

## Decision

**Templates live in a dedicated `templates` CRR table (core-data-model §1.10), not in `notes`.** A template is a **named snapshot** — a factory input for note content — not a note and not a task.

1. A template stores `name`, `content` (body markdown, due-today sentinels stripped of `data-date`), and `checklist_json` (item text + sort_order only). Done-state and due dates are never stored — applied subtasks are born undated and undone, so **no template operation can ever fire a notification**; the safety is structural.
2. Applying a template writes fresh content into an existing note and inserts fresh `subtasks` rows on it (§32.3.2). No FK, no linkage, no cascade between templates and their products.
3. Uniqueness of `name` is app-enforced (CRR: no UNIQUE constraints beyond PK — #493). Save-path collisions prompt Overwrite/Rename; sync-merged duplicates coexist.

### Reconciliation with ADR-0003

ADR-0003 governs the note/task entity; a template is neither — it is the mold, not the cast. Notes and tasks remain one entity with one meaning; templates stay entirely outside that model, exactly as ADR-0057 kept subtasks outside it. No third status, no type column, no flag on `notes`.

The rejected notes-with-flag alternative is what would strain ADR-0003: template rows would be notes that must be invisible everywhere notes appear.

## Consequences

### Positive

- **Zero surface leakage, by construction.** Templates cannot appear in note lists, FTS, counts, kanban, Due Today, the file watcher, flat-file sync, or backups-of-notes, because they are not notes. No guard exists to forget (ADR-0008 lineage; ADR-0057 precedent).
- **Notification safety is structural.** No stored dates → nothing for the §19 watcher to see until a user sets a date on an applied item.
- **Device sync for free** via CRR membership.
- **Reuses the existing frontend contract**: `name`/`content` match the live `Template` interface; the grid, shortcut, and Settings tab are the launch surfaces.

### Negative

- A new table and CRR member: `CRR_TABLES` 22 → 23, `SCHEMA_VERSION` 73 → 74, new readers/writers to build.
- `checklist_json` is not queryable per-field and template content is not FTS-searchable. Accepted: neither is ever queried; templates are picked from a named list.
- No in-place template editor at v1 — revision is apply → edit → re-save-over-name (§32.4).

### Risks

- **Apply atomicity.** The apply writes note body (editor path) + subtasks rows (DB). If these can land partially, a note gets a checklist sentinel with missing items or vice versa. §32.3.2 requires all-or-nothing from the user's perspective; this is the primary thing the code audit must verify.
- **Date-stamp ownership.** Apply bypasses `insertDueToday()`, so stamping `data-date` at apply time is the apply path's job (§32.3.2 step 2). A missed stamp yields a dateless node — §31.2.3's error state, caught visibly, but still a bug class the audit should check.

## Alternatives Considered

**Notes-with-flag** (`notes` row + `note_type='template'` or similar) — *Rejected.* Every notes consumer must learn the flag: FTS triggers fire on every insert, the browser lists it, the file writer materializes a `.md` on disk, flat-file sync and the watcher pick it up, word counts include it, and any future notes query silently inherits the leak. One forgotten guard puts templates on a user surface or on disk. The dedicated table removes the class (ADR-0057's argument, verbatim shape).

**A `template_subtasks` child table** mirroring §1.9 — *Rejected.* Template items are inert payload: never watched, never queried by field, never individually addressed. A JSON column carries text+order with one fewer CRR table and no cascade rules.

**Serialize checklist items into `content`** (e.g. GFM list the apply path parses) — *Rejected.* Re-introduces a markdown↔items parse step and drifts against §31.3.3's DB-is-truth stance; the sentinel-plus-payload split keeps one canonical shape.

## References

- Spec: §32 (`note-templates.md`), core-data-model §1.10, §1.9, §31.
- Reconciled: ADR-0003 (notes/tasks one entity — untouched). Pattern: ADR-0057, ADR-0008.
- Constraint discipline: #493 (CRR app-enforced uniqueness), `test_crr_tables_not_null_columns_have_defaults`.
- Issue: #193. Frontend precursors: `types.ts` `Template`, `api.ts` stubs, `Templates.svelte`, `Cmd+Shift+T` handler in `Editor.svelte` — source-read 2026-07-20.
