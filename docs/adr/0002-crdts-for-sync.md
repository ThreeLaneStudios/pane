# ADR-0002: CRDTs for Multi-Device Sync

**Status:** Accepted  
**Date:** 2026-02-27  
**Author:** Matt Carano

## Context

Pane needs multi-device sync (MacBook ↔ Mac Studio, future mobile). Users expect changes made on one device to appear on another. The sync must work with our local-first architecture — the user's machine is always the source of truth, and data should never leave the device without explicit opt-in.

Traditional sync approaches (server-authoritative, last-write-wins) create conflicts and risk data loss. We needed a sync strategy that handles concurrent edits without a central server making decisions.

## Decision

Use cr-sqlite (Conflict-free Replicated Data Types built into SQLite) for sync. Specifically:
- 20 tables are CRR-enabled (CRDT-backed replicated relations)
- Changes are tracked as changesets in a queue
- Sync happens via Supabase (encrypted) as the relay
- Each device generates changesets → pushes to Supabase → other devices pull and merge
- CRDTs guarantee eventual consistency without conflicts

## Consequences

### Positive
- **No conflict resolution logic** — CRDTs merge deterministically. No "pick a winner" code.
- **Offline-first** — Devices work independently, sync when connected.
- **No server authority** — Supabase is a relay, not a source of truth. The local DB is always authoritative.
- **Encryption at rest** — Changesets are encrypted (Argon2id → BLAKE3 → XChaCha20-Poly1305) before leaving the device.

### Negative
- **cr-sqlite is young** — Limited community, sparse documentation, edge cases in CRR registration.
- **Schema changes are painful** — Adding/removing CRR columns requires careful migration. Pre-launch we adopted a canonical schema approach (#492) to avoid migration chains.
- **Database size grows** — CRDT metadata adds overhead per row.
- **Debugging is harder** — Merge behavior is deterministic but not always intuitive. Understanding why a value resolved to X requires understanding the CRDT clock.

### Risks
- cr-sqlite may have bugs in edge cases (concurrent deletes, schema migrations).
- If Supabase goes down, sync pauses (but local operation continues).
- CRDT tombstones accumulate — need a garbage collection strategy eventually.

## Alternatives Considered

**Last-write-wins (LWW)** — Simple but loses data. If two devices edit the same note, one edit disappears. Unacceptable for a notes app.

**Operational Transform (OT)** — Used by Google Docs. Requires a central server to resolve transforms. Violates our local-first principle.

**Custom sync engine** — Too much engineering for a pre-launch product. cr-sqlite gives us CRDTs "for free" inside SQLite.

**No sync (single device)** — Matt uses multiple devices. Not an option.

## References

- Sync infrastructure: §5 in architecture spec
- cr-sqlite: https://github.com/vlcn-io/cr-sqlite
- Supabase project: jngkiljhmnrygdnfvrgp.supabase.co
- Related issues: #425 (sync hardening), #331 (agent config sync)
