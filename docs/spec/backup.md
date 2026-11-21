# Backup System Spec

**Status:** Implemented  
**Relates to:** §25 (Automated Database Backup System), #325, #1910, #1915, #2723  
**Last updated:** 2026-10-24 (#2723 PR B — `create_migration_backup` threads an `Option<&AppHandle>` through to `backup://progress`; see §35 `docs/spec/startup.md` for the window-first launch sequencing and ordering guarantee this feeds into)

---

## Overview

Pane maintains a rolling backup system that protects user data across four scenarios:

| Trigger | Type | Retention |
|---|---|---|
| App launch (first of the day) | `Daily` | 7 most-recent daily backups |
| Sunday launch | `Weekly` | 4 most-recent weekly backups |
| Before any schema migration | `Migration` | Kept until manually deleted; not rotated |
| User-initiated via Settings | `Manual` | Kept until manually deleted; not rotated |

---

## Artifacts

Every backup produces a set of sidecar files alongside the main `.db` copy. The canonical list of sidecar suffixes is defined in `SIDECAR_SUFFIXES` in `backup.rs` — this is the single source of truth used by `delete_backup_and_sidecars()`.

| Artifact | Suffix | Content | Produced for |
|---|---|---|---|
| DB copy | `.db` | Full SQLite backup via backup API; WAL checkpointed (TRUNCATE) before copy | All types |
| System files | `_system.tar.gz` | `~/Pane/.pane/system/` — agent workspace files (SOUL.md, MEMORY.md, AGENTS.md, IDENTITY.md, etc.) | All types **except Migration** (skipped — #2723, see below) |
| Install UUID | `_install-uuid.json` | Copy of install UUID file | All types |
| Keychain export | `_keychain.json` | Keychain items for known accounts | All types |
| Metadata | `_metadata.json` | App version, timestamp, backup type, schema version, account email | All types |
| Media archive | `_assets.tar.gz` | `~/Pane/Projects/*/Assets/` + `~/Pane/.pane/attachments/` — user attachment blobs | **Never** (see #1915) |

### All backup types skip the media archive

**As of #1915 (2026-07-04):** No backup type produces `_assets.tar.gz`. The media/Assets tree is intentionally excluded from all backups.

**Rationale:**
- The Assets tree grows to hundreds of GB / TB on real installs. Archiving it synchronously adds unacceptable latency and disk churn for a backup that users trigger or that runs automatically.
- Pane's backup system is designed to protect structured data (notes, settings, schema). Media blobs are large, binary, and replaceable from original sources or re-downloadable from attachment sync.
- Users are expected to maintain their own media backup via Time Machine, iCloud, rsync, or equivalent. Pane should not attempt to duplicate that responsibility.

**Historical note:** #1910 first shipped this skip for `BackupType::Migration` only (to unblock the migration hang on Matt's 5.4 GB library). #1915 extends the skip to all four variants (`Daily`, `Weekly`, `Migration`, `Manual`) as the correct general policy.

**If a future migration mutates media:** It must carry a `// MIGRATION-DISK-OPS:` marker in `migrations.rs` (see the module-level convention comment there) and arrange its own disk-safe operation. The assets archive no longer provides that safety net. A CI enforcement gate for this convention is tracked in #1912.

### Migration backups also skip the system-files archive (#2723)

**As of #2723 (2026-10-23):** `BackupType::Migration` additionally skips `_system.tar.gz` — **only** this type; Daily/Weekly/Manual/PreRestore are unaffected and still produce it unconditionally.

**Rationale:** no migration through the current schema version touches `~/Pane/.pane/system/` (the `MIGRATION-DISK-OPS` convention in `migrations.rs` exists precisely to flag any migration that would need this coordination, and none currently carry that marker for `.pane/system/`). Restore already treats a missing `_system.tar.gz` as a soft-fail ("older backup format", log-only, no `RestoreWarning`) — confirmed by execution via `test_restore_migration_backup_without_system_archive` in `backup.rs`, not just by reading the restore branch.

**Same forward-safety caveat as the media-archive skip:** if a future migration adds a `// MIGRATION-DISK-OPS:` marker for `.pane/system/`, the skip must be revisited for that migration's pre-backup. This is convention-enforced today, not CI-enforced (tracked under the same #1912 follow-up).

---

## Copy Pacing (#2723)

The SQLite backup-API copy step (`rusqlite::backup::Backup`) runs at one of two explicit, named paces — every `create_backup()` caller must supply a `BackupPacing` value; there is no default.

| Pacing | Behavior | Callers |
|---|---|---|
| `FullSpeed` | Copies every remaining page in a single step, no pause (SQLite's own `nPage < 0` convention for "all remaining pages now") | Pre-migration (`create_migration_backup`), Daily/Weekly (`create_daily_backup`), pre-restore safety backup (inside `restore_backup_internal`) |
| `ThrottledForLiveApp` | 5 pages / 250ms pause between steps (~80 KB/s) — the exact pre-#2723 constant, preserved, not re-derived | Manual (`create_manual_backup`, Settings UI) |

**Why FullSpeed is safe for three of four callers:** none of them have a live reader/writer to be polite to. Pre-migration backups run on the main/setup thread before any window exists and before any other code has opened the DB. Daily/Weekly backups run on a background thread *after* the window has already rendered (#353) — the pacing was never protecting the UI thread, since nothing else holds `source_conn`. The pre-restore safety backup runs after the live `DbState` connection has already been closed (ADR-0056 R2) — quiescent by construction.

**Why Manual keeps the original pacing:** a user may be actively using the app (typing, saving, syncing) while a manual backup runs. The constant itself (5 pages / 250ms) predates any documented rationale — it was introduced in the original #325 implementation (b06459df, 2026-04-04) with no comment, commit message, or doc reference, and has never been revisited. #2723 does not invent a new number for this caller; it names and documents the existing one.

**Measured effect (15 MB DB, pre-launch/migration backup):** ~190s before (`ThrottledForLiveApp`-equivalent pacing, cross-validated against a real production log in #2583) → well under 1s after (`FullSpeed`). See #2723's PR description for the full before/after table at both 15 MB and Matt's real 50 MB size.

---

## Output Staging (#2723)

Both of `create_backup()`'s large outputs — the `.db` copy and (when produced) the `_system.tar.gz` archive — are written to a temporary `*-staging` path on the same filesystem and atomic-`rename()`d into their final, listed filename only after the write (and, for the `.db`, the integrity check) succeeds. This reuses the pattern already shipped for restore (`restore_backup_internal`, `db_path.with_extension("db-restoring")`).

- A stale staging file left behind by a prior crashed/killed run is removed before a new attempt starts (same cleanup `restore_backup_internal` already does for its own staging path).
- A kill mid-copy or mid-archive can therefore never leave a half-written file visible at the filename a complete backup would use — the next launch's retry starts clean.
- A failed `rename()` is propagated as an error, not swallowed (#3049 lesson): the staged write succeeding does not mean the backup succeeded.

---

## Implementation

### `create_backup(db_path, backup_type, version, app_handle, pacing)` — `backup.rs`

Central implementation. All backup entry points delegate here.

1. Validates DB exists and is non-empty (≥ 4 KB).
2. Generates filename based on type:
   - Migration: `pane_pre-migration-<version>.db`
   - Manual: `pane_manual-<timestamp>.db`
   - Daily/Weekly: `pane_<timestamp>.db`
3. Runs `PRAGMA wal_checkpoint(TRUNCATE)` on source DB and inspects the
   result row (per R6) — a blocked/partial checkpoint does not raise an
   error and MUST be surfaced, not assumed successful.
4. Uses SQLite's backup API (handles WAL correctly) to copy to backup path.
5. Runs `PRAGMA integrity_check` on backup; retries once on failure.
5a. Runs FTS5 `'integrity-check'` on the staged copy's `notes_fts` with
   `rank=1`; retries once on failure. Skipped when `notes_fts` is absent.
   `rank=1` is required: for an external-content table a rank-less check
   validates only the index's own structure and never detects index/content
   drift (SQLite FTS5 §6.5).

> Status: BUILT (PR #3079) — FTS5 `integrity-check` added to `verify_backup_integrity`;
> `notes_fts` is no longer rebuilt on every launch. The obsolete startup
> `DELETE` + per-row disk-read rebuild (`305afa11`, Mar 2026) was left behind when
> `9df638a4` (#486) restored `content='notes'` with full-body triggers; its
> non-transactional window is what surfaced as `notes_fts malformed` (#3072,
> #2352, #2566, #2610). A rebuild now runs only at the migration boundary or
> when the drift gate fails, via the native FTS5 `'rebuild'` inside a
> transaction. `BackupPacing::FullSpeed` is unchanged (#2723/#3064).
6. Backs up `~/Pane/.pane/system/` → `_system.tar.gz` (non-fatal) — **skipped entirely for `BackupType::Migration`** (#2723, see Artifacts above).
7. ~~Archives `Projects/*/Assets/` + `.pane/attachments/` → `_assets.tar.gz`~~ — **removed in #1915.** All backup types skip the media archive. Users rely on Time Machine / iCloud for media.
8. Copies install-uuid.json.
9. Exports keychain items → `_keychain.json`.
10. Writes `_metadata.json`.

### Entry point delegates

```rust
create_migration_backup(db_path, version)  → create_backup(_, Migration, Some(version), None, FullSpeed)
create_manual_backup(db_path, app_handle)  → create_backup(_, Manual, None, app_handle, ThrottledForLiveApp)
create_daily_backup(db_path)               → create_backup(_, Daily or Weekly, None, None, FullSpeed)
```

`create_migration_backup` is a one-liner delegate. **Do not add per-type logic there** — it will be bypassed by any path that calls `create_backup` directly.

---

## Note storage: why DB-only backup is sufficient

Notes are stored with the DB as the source of truth (`notes.body` column). Flat `.md` files at `Projects/{project}/{folder?}/{slug}.md` are a write-through projection — they are updated on every save but derived from the DB body on read (with flat-file fallback for NULL body rows from migration edge cases).

This means: a DB-only restore recovers all note content. Flat `.md` files are not archived in any backup type (neither `_system.tar.gz` nor `_assets.tar.gz` captures them).

---

## Rotation

Daily backups rotate at 7 most-recent. Weekly backups rotate at 4 most-recent. `rotate_backups()` in `backup.rs` deletes the oldest beyond the retention threshold.

When a backup `.db` is deleted, **all sidecars are deleted atomically** by iterating `SIDECAR_SUFFIXES`. Adding a new sidecar type requires adding its suffix to `SIDECAR_SUFFIXES` — no other code changes needed for deletion to work correctly.

---

## Restore

`restore_backup(backup_path, db_path)` restores:
1. DB (with schema version check per §6.1.4.1 and R4 — reads the real
   `ai_settings` schema version, not `PRAGMA user_version`; blocks a
   backup whose schema is newer than the current binary).
2. System files from `_system.tar.gz` if present.
3. Keychain items from `_keychain.json` if present (partial failures produce non-fatal `RestoreWarning`).
4. Install UUID from `_install-uuid.json` if present.
5. Attachment files from `_assets.tar.gz` if present (absent sidecar is non-fatal — new backups post-#1915 never produce this file; older backups that carry it are still restored gracefully).

Restore is exposed via Settings UI (Manual backup section). Migration backups are visible in the backup list but are not intended for user-initiated restore under normal operation.

## Restore & backup integrity contract

> **Status:** NOT BUILT — spec only. Pins the contract for #1922 (restore
> corruption, P0), #1923 (mid-write snapshot), #1921 (UI-thread block, P0).
> Restore-path connection lifecycle and WAL/SHM handling are the subject of
> ADR-0056. Build order: #1922 → #1923 → #1921.

The database is not quiescent when the current backup/restore path touches
it. Every contract below exists to make it quiescent before a copy or swap
so no stale or in-flight state survives the operation.

### R1. Restore quiescence (#1922 — WAL/SHM)

A restore replaces the live database file. It MUST guarantee that no state
from the pre-restore session can survive the swap. The required behavior,
in order:

1. **Close/invalidate the live DB connection** before touching any file.
2. **Swap the DB** — the restored `.db` replaces the live `.db`.
3. **Clear the sidecar journal files** — `.db-wal` and `.db-shm` for the
   live DB MUST be removed (or otherwise made non-replayable) as part of
   the swap.
4. **Reopen the DB connection** against the restored file.

Rationale (behavioral, not implementation): a WAL file left in place after
a file-level `.db` swap shares the pre-restore session's salt. On the next
open, SQLite recognizes the WAL as valid for the restored file and replays
pre-restore frames on top of it — resurrecting operations the restore was
meant to discard (e.g. a trash/`deleted_at` write), producing rows whose
columns are split between backup state and replayed state. Restoring even
a perfect backup triggers this if the WAL is present and the salts match.
Clearing WAL/SHM is therefore mandatory on **every** restore, not only on
restores from suspect backups.

### R2. Restore connection lifecycle

The restore path MUST hold and manage the app's live database connection
(`DbState`), not operate on a bare path while the live connection stays
open. Concretely:

- The restore commands MUST take the live `DbState` handle.
- The live connection MUST be closed/invalidated before the swap (R1 step
  1) and a fresh connection opened after (R1 step 4).
- No file-level copy or swap of the live DB may occur while a connection
  to that file is open. An open connection both (a) holds the journal that
  R1 must clear and (b) blocks checkpoint/truncate on the source.

This mirrors the connection-lifecycle discipline ADR-0051 established for
migrations, applied to the restore path.

### R3. Safe safety-backup

The restore-time safety backup (the pre-restore snapshot of the user's
current DB) MUST be taken through a quiescent-DB path — the same
checkpoint + SQLite-backup-API path `create_backup()` uses — **not** a raw
file copy of a live, WAL-open database. A raw copy of a live DB can itself
capture torn WAL-mixed state, which defeats the purpose of a safety net.
The safety backup is only trustworthy if it is created after the live
connection is quiesced (or via the backup API against a checkpointed
source).

### R4. Schema-compatibility guard (reader fix)

Restore performs a schema-compatibility check before mutating state (the
fatal/confirm/silent contract in `onboarding.md` §6.1.4.1). That contract
is correct and unchanged. What is broken is the **version source it reads
from** — it currently compares `PRAGMA user_version`, which is always `0`
under cr-sqlite, so the guard is a silent no-op (`0 < 0` always “matches”).

The guard MUST read the schema version from the same source the migration
system authoritatively uses — the version value stored in the `ai_settings`
table — and MUST NOT read `PRAGMA user_version`. This applies on **both**
sides of the comparison:

- **At backup time:** the schema version written into `_metadata.json` MUST
  come from the real `ai_settings` version accessor, not `user_version`.
- **At restore time:** the current binary/DB version compared against the
  backup's metadata MUST come from the same accessor.

Reuse the existing `ai_settings` version accessor; do not introduce a
second reader. Once both sides read the real version, the §6.1.4.1
block-newer / confirm-older / silent-equal behavior functions as specified.

> **Pre-launch:** existing on-disk backups carry `schema=0` in their
> metadata (written by the broken accessor). Zero users → these are nuked,
> not migrated. The guard is not required to special-case legacy `schema=0`
> metadata.

### R5. Backup completeness (#1923 — the debounce gap)

**Guarantee:** *a backup reflects every note durably saved before the
backup was initiated.*

The current gap is not a WAL bug — `create_backup()` checkpoints the source
and uses the SQLite backup API, both correct. The gap is upstream: a
just-saved note is written to its `.md` file immediately but only reaches
SQLite after the file-watcher debounce (`watcher.rs`, `DEBOUNCE_SECS = 2`).
A backup triggered inside that window snapshots a DB that does not yet
contain the note.

Before snapshotting, backup MUST close the debounce gap: pending
file-watcher writes for saves that completed before the backup was
initiated MUST be flushed to SQLite before the checkpoint + snapshot runs.
(The flush mechanism — draining/awaiting the debounce, or force-syncing
pending file events — is the implementing issue's call, verified against
the live save/watcher path. The spec fixes the guarantee, not the call.)

### R6. Checkpoint result must be inspected (backup hardening)

`PRAGMA wal_checkpoint(TRUNCATE)` does not error when a live reader blocks
truncation — it returns a `(busy, log, checkpointed)` result row and leaves
the WAL on disk. Backup MUST inspect that result rather than assume success
from the absence of an error, and surface a partial/blocked checkpoint
(log/diagnostic) rather than silently proceeding. With R2 in force for
restore and the backup API already handling WAL, this is defense-in-depth
against a future path re-introducing a live reader during backup.

### R7. Backup must not block the UI thread (#1921)

Backup MUST run without blocking the application/event-loop thread, and
MUST surface progress to the UI for its duration (the current path blocks
~37s on a synchronous command thread — checkpoint, `integrity_check`,
keychain export, and the synchronous chain). The contract is behavioral:
non-blocking execution + progress surfaced. The specific async mechanism
(e.g. `spawn_blocking`) is the implementing issue's call.

### Build sequencing

The fixes land in dependency order, each citing its sub-section:

1. **#1922** (R1–R3, R4) — restore quiescence, connection lifecycle, safe
   safety-backup, schema-guard reader fix. Highest severity (silent
   corruption).
2. **#1923** (R5, R6) — backup completeness + checkpoint hardening.
3. **#1921** (R7) — async + progress.

R2 (connection lifecycle) and R3 (safe safety-backup) are coupled and land
together in #1922.
