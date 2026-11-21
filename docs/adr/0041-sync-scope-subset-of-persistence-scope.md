# ADR-0041: Sync Scope Is a Filtered Subset of Persistence Scope

**Status:** Accepted
**Date:** 2026-05-28
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** ADR-0012 (Pane Gateway as primary sync transport), ADR-0014 (Bridge vs Supabase split — gateway owns sync), ADR-0040 (Gateway workspace resolution — the resolution mechanism this allowlist filters the output of)
**Related:** `docs/spec/agents.md` §24.3 (canonical workspace structure), `docs/spec/sync.md` §22C (sync allowlist), `docs/spec/README.md` §0.3.2/§0.3.3A (three representations, who-writes-what)

## Context

A fresh OC onboard smoke test surfaced that the gateway syncs every file in the OC workspace, not just the canonical agent identity set. `read_workspace_files()` (`src-gateway/src/routes/sync.rs:282-336`) performs a full recursive directory walk, filtering only dotfiles. The result: `archive/`, `scratchpad-*.md`, temp files, backup `.tar.gz`, and deep `memory/` subdirectories all crossed the wire and landed in Pane's system directory — 166 files for the main agent alone, likely compounding the initial-sync hang (#1296).

The root cause is not a missing filter in one place. A filter already exists on the push side — `IDENTITY_WHITELIST` (`sync_client.rs:75-83`), enforced in `trigger_sync` via `extract_agent_and_filename` (`sync_client.rs:117-155`), filtering to the canonical identity set plus `logs/*.md` and `memory/*.md`. The bug is that the filter exists on **one of three surfaces** the sync system has, and the other two (gateway read, Pane ingest) walk everything.

The deeper issue the bug exposes is a spec gap: **the spec never stated that the set of files which sync is a filtered subset of the set of files which persist.** §0.3.2 defines three representations (DB, flat files, OC workspace replica). §0.3.3A defines who writes what. §22C/§22B define sync transport. But no section states that sync scope ⊊ persistence scope — that a file can legitimately exist in Pane's local persistence and never cross the wire to OC, and that this is correct rather than a desync. The spec treated "system files" as a monolithic category and implicitly assumed sync scope equals persistence scope for that category. The recursive walk is the direct consequence: nothing told the gateway the wire scope is narrower than the disk scope.

Reed's confirmation audit established the load-bearing fact that makes filtering safe: **ingest is flat-only.** `pull_pending_changes` writes received files to disk; system notes are `body=NULL` on creation and the DB row is populated on first edit ("flat-canonical, DB mirrors on edit"). There is no DB row written at ingest time. Filtering a file at ingest therefore desyncs nothing — there is no DB state to leave dangling, and since Pane is PRIMARY and OC is REPLICA (§0.3.3), a file Pane chooses not to ingest is correctly absent (Pane's not-having-it is authoritative).

What if we didn't decide:

- **The recursive walk gets patched ad-hoc.** Someone adds a filter to `read_workspace_files()` that doesn't match the push-side `IDENTITY_WHITELIST`, and the two surfaces drift. The next file class that should sync (or shouldn't) gets handled differently on each surface.
- **The principle stays implicit.** The next engineer who touches sync assumes sync scope == persistence scope (the same assumption that produced the recursive walk) and reintroduces the bug in a new code path.
- **The `memory/` depth boundary gets "improved."** A future engineer sees `memory/*.md` flat-only and makes it recursive to "support nested memory organization," silently breaking alignment with OC's dreaming reader (which reads `memory/*.md` flat via `entry.isFile()`, no recursion). Nested memory files would sync but OC would never read them — dead weight plus a false user expectation.

## Decision

**The set of files that sync between Pane and OC is a filtered subset of the set of files that Pane persists locally. Sync scope and persistence scope are governed by different mechanisms and must not be conflated.**

Concretely:

1. **The allowlist governs the wire, not storage.** The sync allowlist (defined in §22C, structured per §24.3) determines which files cross the Pane↔OC wire in either direction. It does not govern Pane's local DB + flat persistence. A file can exist in Pane's local storage and be excluded from sync; that exclusion is correct, not a desync.

2. **No file's local persistence is affected by sync exclusion.** Excluding a file from the allowlist changes only whether it crosses the wire. Its DB row (if any) and flat file on the Pane side are untouched by the sync layer. The persistence layer (§2.3A `atomic_write`, §0.3.3A who-writes-what) and the sync layer (§22C) have independent scopes.

3. **Exclusion causes no desync because Pane is PRIMARY.** Per §0.3.3, Pane is PRIMARY and OC is REPLICA. A file Pane does not sync to OC is correctly absent from OC's replica. A file Pane does not ingest from OC is correctly absent from Pane (Pane's authoritative state does not include it). The replica converging to "Pane's filtered subset" is the correct steady state, not a divergence.

4. **Ingest filtering is clean because ingest is flat-only.** Filtering a received file at `pull_pending_changes` ingest writes nothing — no flat file, no DB row. Since system notes are `body=NULL` on creation with the DB row populated on first edit, there is no DB state at ingest time to leave dangling. The filtered file simply never enters Pane's persistence. (Corrected characterization: system notes are "flat-canonical, DB mirrors on edit," not "flat-only" — the DB row exists with `body=NULL` and is populated when the user first edits. At ingest, neither is written for a filtered file.)

5. **The allowlist is enforced on all three sync surfaces.** Push (`trigger_sync` — exists today via `IDENTITY_WHITELIST`), gateway read (`read_workspace_files` — missing), and Pane ingest (`pull_pending_changes` — missing). Defense-in-depth: a file blocked at any one surface is blocked, but all three enforce so that a version-skewed component (old gateway + new Pane during rolling upgrade, or vice versa) cannot leak non-allowlisted files. The contents of the allowlist live in §22C and §24.3; this ADR pins the principle that all three surfaces enforce the same list.

6. **`memory/` and `logs/` are depth-1 (flat) by contract, not by convenience.** `memory/*.md` flat-only matches OC's dreaming/consolidation reader, which reads `memory/*.md` via `entry.isFile()` with no recursion. Syncing `memory/subdir/note.md` (depth 2) would put a file on the wire that OC never reads — dead weight plus a false expectation that user-organized memory subdirectories are visible to OC. The depth-1 rule is alignment with OC's reader contract; it is not a performance heuristic and must not be relaxed to recursive without a corresponding change to OC's reader.

## Rationale

### Why this is an ADR and not just spec text

The allowlist *contents* are spec text (§22C lists the files; §24.3 documents the canonical structure). But the *principle* — sync scope ⊊ persistence scope, governed by different mechanisms — is the thing whose absence produced the bug, and it is the thing a future engineer will re-violate if it lives only in a file list. A file list answers "which files sync." It does not answer "why is it correct for a persisted file to not sync." The ADR carries the why, so the next person who finds a file in Pane's storage that doesn't sync reads "this is by design (ADR-0041)" rather than "this looks like a sync bug, let me make it sync."

### Why ingest filtering is the clean enforcement point

Reed's confirmation that ingest is flat-only is what makes Decision 4 safe. If ingest wrote a DB row before the flat file, filtering at ingest could leave a DB row with no backing file (or vice versa). Because ingest writes only the flat file (DB row populated lazily on first edit), there is no partial-write window for a filtered file — it is simply never written, in either representation. This is why the filter can sit at ingest rather than requiring a more complex two-phase "write then reconcile" approach.

### Why the depth-1 rationale gets pinned explicitly

This is scar tissue against a specific predictable "improvement." `memory/*.md` flat-only looks arbitrary to someone who doesn't know OC's reader contract — the obvious "enhancement" is to support nested memory organization by making the sync recursive. That enhancement would silently break the OC alignment: files would sync, OC would never read them, and the user who organized their memory into subdirectories would have a reasonable expectation (OC sees my organized memory) that silently fails. Pinning the rationale in the ADR means the next engineer who proposes recursive memory sync reads "this breaks OC's reader contract" before they write the code.

### Why we don't build a cleanup migration

Already-synced junk (the 166 files) is not removed by the steady-state filter — the filter stops new junk from crossing but doesn't clean what prior unfiltered syncs already wrote to disk. A cleanup migration (detect-and-remove non-allowlisted files in existing system directories) is deliberately not built: no real users carry the junk (pre-launch), and Matt's test installs are nuked routinely. The cost of building and testing a cleanup migration exceeds its value when the population that needs it is "test installs that get wiped anyway." Documented as a known limitation; filed post-launch in case the calculus changes (e.g., a long-lived dogfood install accumulates enough junk to matter).

## Consequences

### Positive

- **The 166-file mess stops at the source.** Gateway read and Pane ingest filter to the allowlist; the recursive walk no longer puts junk on the wire.
- **Three surfaces converge on one list.** Push, gateway read, and ingest all enforce the same allowlist; no per-surface drift. The principle (one list, three enforcement points) is durable.
- **Sync volume drops, which may help #1296.** Fewer files on the wire means less work per initial sync. This does not *fix* the #1296 main-thread-blocking architecture (separate, #1180), but it reduces the load that architecture has to handle.
- **The persistence-vs-sync boundary is explicit.** Future sync work has a stated principle to build against; the implicit assumption that produced the recursive walk is replaced with a documented invariant.

### Negative

- **Three enforcement points to keep in sync.** The allowlist is now enforced in three places; a change to the list must update all three. Mitigated by Decision 5's framing (one canonical list, referenced by all three surfaces) and by the implementation anchoring on the existing `IDENTITY_WHITELIST` constant rather than three independent literals — Stia extends the one constant's enforcement, not three copies.
- **Already-synced junk persists.** The steady-state filter doesn't clean existing junk; documented as a known limitation. Accepted because the affected population is test installs.
- **Depth-1 rule is a constraint future memory features must respect.** A genuine future need for nested memory organization would require coordinating a change to both Pane's sync and OC's reader. Accepted; the coupling is real and the ADR makes it visible rather than hidden.

### Risks

- **A version-skewed component leaks junk during rolling upgrade.** Old gateway (no read filter) + new Pane (ingest filter) → Pane catches it. New gateway (read filter) + old Pane (no ingest filter) → gateway catches it. Defense-in-depth (Decision 5) covers both skew directions; the risk is only if both surfaces are simultaneously old, which is the pre-fix state. Accepted.
- **The depth-1 rule gets relaxed without updating OC's reader.** Mitigated by pinning the rationale in this ADR (Decision 6 + Rationale). Residual risk if a future engineer changes the code without reading the ADR; the §22C spec text cross-references this ADR at the depth rule to reduce that.
- **Someone reads "sync scope ⊊ persistence scope" as license to exclude files arbitrarily.** The principle is about the *direction* of the subset relationship, not a grant to exclude anything. The allowlist contents are Matt-approved and spec'd; this ADR governs the relationship, not the membership. Mitigated by keeping contents in §22C/§24.3 where changes go through spec review.

## Alternatives Considered

**Filter only at the gateway read.** Fix `read_workspace_files()` to apply the allowlist; leave Pane ingest unfiltered on the theory that the gateway is the only source. **Rejected** because it fails the rolling-upgrade skew case (new Pane + old gateway leaks junk) and because defense-in-depth at ingest is cheap (mirror the existing whitelist check). Single-surface enforcement is what produced the asymmetry in the first place.

**Filter only at Pane ingest.** Leave the gateway walking everything; Pane drops non-allowlisted files on receipt. **Rejected** because it wastes wire bandwidth (the 166 files still cross the network, just get dropped) and doesn't help the #1296 load — the gateway still reads and transmits everything. The gateway read filter is where the volume reduction happens.

**Build the allowlist fresh as a new module.** Treat this as greenfield, write a new allowlist filter applied uniformly. **Rejected** because `IDENTITY_WHITELIST` already exists and nearly matches the design; a parallel filter would be a second source of truth that drifts from the first. Extend the existing constant's enforcement to the surfaces that lack it.

**Build a cleanup migration for existing junk.** Detect and remove non-allowlisted files in existing system directories on first launch post-fix. **Rejected** for this launch because the affected population is pre-launch test installs that get nuked routinely; the migration's cost exceeds its value. Filed post-launch.

**Allow `memory/` recursion to support nested organization.** Make the sync recursive under `memory/`. **Rejected** because it breaks alignment with OC's flat dreaming reader — files would sync but OC would never read them. The depth-1 rule is a contract, not a limitation to be lifted.

## References

- Bug source-read: `src-gateway/src/routes/sync.rs:282-336` — `read_workspace_files()` recursive walk, dotfile-only filter
- Existing push allowlist: `sync_client.rs:75-83` (`IDENTITY_WHITELIST`), enforced via `extract_agent_and_filename` (`sync_client.rs:117-155`)
- OC scaffolder: `workspace-hhTlRYqM.js` `ensureAgentWorkspace()` line 254 (8 flat files), `VALID_BOOTSTRAP_NAMES` line 146, `MINIMAL_BOOTSTRAP_ALLOWLIST` line 417 (sub-agent 5-file set)
- Reed confirmation audit (2026-05-28) — ingest flat-only; system notes `body=NULL` on creation, DB mirrors on edit; filtering at ingest causes no desync
- ADR-0012 — Pane Gateway as primary sync transport
- ADR-0014 — Bridge vs Supabase split (gateway owns sync)
- ADR-0040 — Gateway workspace resolution (this allowlist filters the output of `read_workspace_files`, which ADR-0040 made resolve correct paths)
- `docs/spec/README.md` §0.3.2 (three representations), §0.3.3 (Primary-Replica), §0.3.3A (who-writes-what)
- `docs/spec/sync.md` §22C — sync allowlist (contents + three-surface enforcement; lands with this ADR)
- `docs/spec/agents.md` §24.3 — canonical workspace structure (lands with this ADR)
- Related bugs (filed separately, launch-blocking, NOT fixed by this ADR): #1298 ping-pong loop on ingest (`file_ops.rs:38-42`, `sync_client.rs:304-311`); #1299 LWW tie-break inversion (`conflict_resolution.rs:73-76`); #1300 path traversal on pull (`sync_client.rs:748-752` vs `initial_sync:1111`)
- #1296 — initial-sync hang; this ADR reduces sync volume but does not fix the underlying main-thread architecture (#1180)

## Follow-ups filed (not gated by this ADR)

- **Cleanup migration (post-launch).** If a long-lived dogfood install accumulates enough already-synced junk to matter, build a detect-and-remove migration. Not built now; population is test installs.
- **Allowlist-constant single-source enforcement (implementation-time).** Stia extends the one `IDENTITY_WHITELIST` constant's enforcement to `read_workspace_files` and `pull_pending_changes` rather than copying the literal. If the three surfaces can't share the one constant (gateway is a separate crate from `sync_client`), file a follow-up to define the allowlist in a location both crates import, so the list has one source of truth.
