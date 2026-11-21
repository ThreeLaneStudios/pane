# ADR-0056: Restore Quiesces the Live Database Before Swapping It

**Status:** Proposed
**Date:** 2026-07-14
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** ADR-0024 (local-only decryption / backup format — restore-integrity contract added on top; backup format unchanged), ADR-0026 (install-UUID active-machine id — restore ordering already fatal on UUID; unchanged), ADR-0051 (migrations on a separate short-lived connection — established the connection-lifecycle discipline this ADR applies to the restore path)
**Related:** `docs/spec/backup.md` → "Restore & backup integrity contract" (R1–R7), `docs/spec/onboarding.md` §6.1.4 / §6.1.4.1 (restore offer + error contract — decision table unchanged), `docs/spec/file-system.md` §2B.6 (`init_database` / `open_connection` / `DbState` lifecycle), Reed triage `docs/audits/2026-07-14-backup-restore-triage.md`, issues #1922 (P0), #1921 (P0), #1923

## Context

Live testing surfaced silent data corruption on restore (#1922, P0). Reed's
triage established the mechanism and two adjacent defects on the same path.

The restore path replaces the database at the file level while the app's
long-lived connection stays open and the journal sidecars are left in
place. Specifically (triage, verified against `~/Projects/example-app` @ e9005868):

- Restore does `fs::copy(backup_path, db_path)` (`backup.rs:775`) and never
  deletes or clears `.db-wal` / `.db-shm`. On the next open, SQLite finds a
  WAL whose salt matches the restored file and replays pre-restore frames on
  top of it. Trash/`deleted_at` writes from the pre-restore session
  reappear, producing rows split between backup state and replayed state —
  the observed "phantom Trash note, Invalid Date, un-deletable" symptom. The
  cr-sqlite `__crsql_clock` entries from the replay leave a partial CRDT
  causality record, so later delete/restore attempts on that row no-op.
- The restore commands (`lib.rs:231–265`) do not take `DbState`; they act on
  a bare path while the live `Mutex<Connection>` remains open against the
  same file. That open connection both holds the stale WAL and blocks
  checkpoint/truncate on the source.
- The restore-time "safety backup" (`backup.rs:765–773`) is a raw
  `fs::copy` of the live, WAL-open DB — the safety net can itself capture
  torn state.

This is the same class of failure ADR-0051 addressed for migrations: an
operation that mutates the database on disk while a long-lived connection —
with its own view of the WAL and its own cached state — survives across the
mutation. ADR-0051 fixed it for the migration connection by scoping that
connection's lifetime. The restore path needs the analogous discipline.

The backup format (ADR-0024) and the install-UUID restore ordering
(ADR-0026, §6.1.4.1) are correct and are not reopened here. This ADR governs
only *how the live DB is made quiescent around the restore swap*.

## Decision

**A restore quiesces the live database — closes the app connection and
clears the WAL/SHM sidecars — before and around the file swap, then reopens
against the restored file.**

Concretely:

1. **Restore takes the live connection.** The restore commands take the
   app's `DbState` handle rather than operating on a bare path. Restore owns
   the connection's lifecycle for the duration of the operation.

2. **Close before swap.** The live connection is closed/invalidated before
   any file is touched. No file-level copy or swap of the live DB occurs
   while a connection to that file is open.

3. **Clear WAL/SHM as part of the swap.** After the restored `.db` replaces
   the live `.db`, the live DB's `.db-wal` and `.db-shm` are removed (or
   otherwise made non-replayable) so no pre-restore frames can replay. This
   is unconditional — it applies to every restore, including restores from
   an intact backup, because the replay hazard is a property of the WAL's
   presence and salt, not of the backup's quality.

4. **Reopen after.** A fresh connection is opened against the restored file
   and becomes the app's live `DbState`.

5. **Safety backup via the quiescent path.** The pre-restore safety backup
   is taken through the checkpoint + SQLite-backup-API path `create_backup()`
   uses (against a quiesced/checkpointed source), not a raw copy of a
   live-WAL DB.

6. **Backup format unchanged (ADR-0024).** Sidecar set, keychain export,
   install-UUID handling, metadata schema — all unchanged. This ADR adds
   restore-time quiescence, not a format change.

7. **Restore error contract unchanged (§6.1.4.1).** The fatal/confirm/silent
   schema decision table and the fatal-on-UUID ordering (ADR-0026) stand.
   The only correction to the schema check is its *version source* (real
   `ai_settings` version, not `PRAGMA user_version`) — a reader fix, not a
   behavior change (see `backup.md` R4).

## Rationale

**Why clearing WAL/SHM is mandatory, not conditional.** A file-level swap
that leaves a matching-salt WAL in place is unsound by construction: SQLite
will treat the WAL as authoritative recovery data for the restored file.
The only safe states are "no WAL present" or "WAL that belongs to the
restored file." Since the restored `.db` arrives without its own coherent
WAL session for the live path, the live WAL/SHM must be cleared. Gating this
on backup quality would be wrong — a perfect backup restored over a present
WAL still corrupts.

**Why restore must own the connection.** WAL is per-database on disk but
each connection carries its own checkpoint/lock state. Clearing the WAL and
swapping the file underneath an open connection is undefined at worst and
blocked at best (the open connection prevents TRUNCATE). Owning the
connection lifecycle — close, swap+clear, reopen — is the only way to make
the sequence deterministic. This is the ADR-0051 discipline (scope the
connection around the schema-changing operation) applied to restore.

**Why safety-backup safety is part of the same decision.** The safety
backup exists to let a user recover if a restore goes wrong. If it is itself
a torn copy of a live-WAL DB, it provides false assurance. It must be
produced by the same quiescent path the primary backup uses; otherwise the
recovery story has a hole exactly where it is needed.

**Why not amend ADR-0024/0026.** Those ADRs govern backup *format* and
*machine identity*. Restore-time connection quiescence is a distinct
architectural concern with its own failure mode and its own invariant
(no live connection or replayable WAL across a swap). A new ADR keeps the
decision auditable and leaves the referenced ADRs' bodies immutable, per the
playbook.

## Consequences

- Restore becomes a close → swap → clear-WAL/SHM → reopen sequence owned by
  the restore commands via `DbState`. The window where the app has no open
  connection is brief and bounded by the swap.
- The safety backup gains the same durability guarantee as the primary
  backup.
- The schema guard becomes live (via the R4 reader fix) rather than a
  no-op — restores of newer-schema backups now block as §6.1.4.1 specifies.
- Pre-launch, zero users: no migration/back-compat for existing `schema=0`
  backups; the dev DB is nuked. No back-compat code is added.
- INVARIANT candidate (for `INVARIANTS.md`, filed with the fix): *no
  file-level copy or swap of the live DB may occur while a connection to
  that file is open, and a restore must clear `.db-wal`/`.db-shm` for the
  swapped DB.* Mechanize if a grep/CI guard is feasible (e.g. flag
  `fs::copy`/`fs::rename` targeting the live DB path outside the quiescent
  restore helper).

## Validation gate

Source-clean review and CI green do not prove the restore path. The gate is
Matt's live test on the built DMG: perform a restore with a WAL present,
restart, and confirm no phantom Trash rows / Invalid Date and that
delete/restore on restored notes behaves. (Render/GUI-adjacent per the
playbook — live DMG is the only proof.)

## References

- Reed triage: `docs/audits/2026-07-14-backup-restore-triage.md` §2 (Finding A/B/E), §3 (#1922 mechanism)
- Source evidence (triage): `backup.rs:775` (DB copy, no WAL/SHM handling), `backup.rs:765–773` (raw safety copy), `lib.rs:231–265` (restore commands omit `DbState`), `backup.rs:116` (`user_version` read), `migrations.rs:361–367` (real `ai_settings` version)
- ADR-0051: `docs/adr/0051-migrations-separate-connection.md` (connection-lifecycle discipline for migrations)
- ADR-0024 / ADR-0026: backup format / install-UUID (unchanged)
- Spec: `backup.md` R1–R7; `onboarding.md` §6.1.4.1; `file-system.md` §2B.6
- Playbook §2 — ADR immutability (0024/0026/0051 bodies not edited; this ADR supplements)
