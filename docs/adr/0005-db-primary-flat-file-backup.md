# ADR-0005: DB as Primary, Flat Files as Backup

**Status:** Accepted  
**Date:** 2026-04-18  
**Author:** Matt Carano, Brit Freed

## Context

Pane stores notes as both SQLite rows (with a `body` TEXT column) and `.md` flat files on disk. For most of Pane's history, flat files were the source of truth and the DB read from them on launch. This created race conditions, stale reads, and a complex "who wrote last?" problem.

The DB-primary migration (#486, v0.59.0) moved the source of truth to the database.

## Decision

The SQLite database is the runtime source of truth for note content. Flat `.md` files exist as human-readable backups and for OpenClaw workspace sync, but are never read to populate the DB during normal operation.

- **Reads:** Always from DB (`body` column in `notes` table)
- **Writes:** Always to DB first, then async write-back to flat file
- **System notes** (SOUL.md, MEMORY.md, etc.): Remain file-primary for bootstrap compatibility. DB mirrors them.
- **Recovery:** If the DB is lost, flat files can reconstruct content via integrity check on launch.

## Consequences

### Positive
- **One writer** — DB writes are atomic (SQLite transactions). No race conditions.
- **Faster reads** — No disk I/O for note content during normal use.
- **Simpler sync** — CRDTs operate on DB rows. Flat files are a side effect, not a sync participant.
- **Search works** — FTS5 indexes the `body` column directly.

### Negative
- **File watcher regression** — External edits to `.md` files no longer auto-reflect in the DB. Needs a file watcher that updates the `body` column (#488).
- **DB corruption is catastrophic** — If the DB is lost without backup, flat files are the only recovery path. Automated backups (§25) mitigate this.
- **Two representations** — DB and flat files can drift if write-back fails. Need monitoring.

### Risks
- OpenClaw workspace sync writes to flat files. If Pane doesn't detect those changes (#488), OC edits are invisible to Pane.
- Large notes may have write-back latency.

## Alternatives Considered

**Flat file primary** — Original approach. Rejected: race conditions between multiple writers (AI chat, user edits, sync, memory pipeline). No transactional guarantees.

**DB only, no flat files** — Simpler but loses human-readable backups and OC compatibility. Rejected for now; may revisit post-launch.

**Dual-write (both authoritative)** — Attempted informally. Created "who wrote last?" bugs. Rejected.

## References

- Migration: #486 (v0.59.0 + v0.59.1)
- File watcher regression: #488
- Architecture spec: §2 (core data model)
- Canonical schema: `~/Shared/ExampleApp/docs/canonical-schema.md`
