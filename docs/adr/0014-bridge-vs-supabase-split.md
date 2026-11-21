# ADR-0014: Bridge vs Supabase — Two Sync Systems by Design

**Status:** Accepted
**Date:** 2026-04-27
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None

## Context

Pane runs two distinct sync systems in production. They have been live since the bridge plugin shipped (early 2026), they are not interchangeable, and no ADR has explained why. This ADR formalizes the already-shipped decision so future engineers don't try to merge them.

**System 1: Supabase CRDT sync (ADR-0002).** User data — notes, projects, tasks, AI chat history. Twenty CRR-enabled tables. Changesets encrypted client-side (ADR-0004), pushed to Supabase as the relay, pulled and merged on other devices via cr-sqlite. Conflict-free by construction. The local DB is always the source of truth; Supabase is a relay, not an authority.

**System 2: Bridge plugin (HTTP, soon Pane Gateway per ADR-0012).** Agent identity files — SOUL.md, MEMORY.md, identity files in `.pane/system/{agent_uuid}/`. Plain markdown files on disk that OpenClaw reads to understand each agent's identity, behavior, and accumulated memory. Synced via push/pull HTTP endpoints with timestamp-based last-write-wins conflict resolution and a retry queue.

The instinct is to ask why. Two sync systems is more code, more to maintain, more to test, more failure modes than one. The reasoning that justifies the split has been informal — captured in audit footnotes and dev-session notes but not in an ADR. That makes it ripe for a future engineer (human or agent) to "consolidate" the systems and discover during the resulting outage why they were split in the first place.

Three architectural reasons explain the split. Each is a hard constraint on its own:

**Reason 1: Data shape mismatch.** Supabase CRDT works on rows in CRR-enabled tables. Each row is a discrete unit; cr-sqlite tracks per-cell history and merges deterministically. Agent identity files are not rows — they are markdown text written and read by OpenClaw as files. Forcing them into CRR-enabled tables would require either modeling each markdown file as a row (loses file-level semantics OC depends on) or modeling each line of markdown as a row (CRDT merge becomes line-by-line, which is exactly LWW dressed up as CRR with worse failure modes). Neither shape works.

**Reason 2: Zero-knowledge violation if SOUL/MEMORY went through Supabase.** ADR-0004 established that all synced data through Supabase is encrypted client-side; Supabase sees ciphertext only. If agent identity files synced through Supabase, they would be encrypted before upload. But OpenClaw reads them as plaintext from `.pane/system/{agent_uuid}/SOUL.md`. The decryption key would have to live on the OC machine — and OC is not architecturally entitled to it. ADR-0004's threat model is "user keys live on user devices, never elsewhere"; OC running on a separate machine is "elsewhere" in this model unless we redefine what zero-knowledge means. The bridge plugin avoids this entirely by transporting plaintext markdown directly between machines (over TLS once §22C lands; over plaintext-on-Tailscale today per audit gap #11).

A fourth, narrower consideration: latency profile. Agent identity files are read by OC on every chat invocation. Routing them through CRDT-encrypted-Supabase round-trips would add measurable latency to chat startup, especially on the first message of a session when OC reads SOUL/MEMORY/Identity. The bridge architecture (and gateway) keeps these reads local — sync is async and write-only-on-change.

### What if we didn't decide

The split has shipped. Doing nothing leaves the code working but undocumented. Concrete consequences:

- **Future "consolidation" attempt.** A future engineer or AI agent reading the codebase sees two sync systems, doesn't find an ADR explaining the split, and proposes unifying them. Most of the work to discover the three reasons above happens during PR review or — worse — during the migration that ships the consolidation. The cost is a multi-week retraction and possibly a data path that lost agent identity files in flight.
- **Tier 1 hosted inherits the question without context.** When Tier 1 lands, the question of "should we just sync agent files through the CRDT system on hosted?" comes up again. Without ADR-0014, the answer has to be re-derived from first principles every time it surfaces. With ADR-0014, the answer is "no, here's why, and here's what would have to change in the threat model and the data shape to make it viable."
- **§22C cross-references this ADR.** Several §22C decisions (the split-machine-only scope of the gateway, the same-machine symlink preservation, the zero-knowledge boundary) reference the bridge-vs-Supabase logic. Without this ADR, those references hang on informal reasoning.

This ADR is independently sequenceable from ADR-0012 / 0013 / 0015. It formalizes a shipped decision, not a new one. It can be filed before or alongside the gateway ADRs without affecting any of them.

## Decision

Pane runs two sync systems by deliberate design, with the boundary between them defined by data shape, encryption requirements, and same-machine optimization:

**Supabase CRDT sync** is the sync system for **user data**: notes, projects, tasks, AI chat history, settings, and any other data shape that fits the row-based model. Encrypted client-side per ADR-0004. Authoritative for "anything the user authored or that represents the user's state across devices."

**Pane Gateway sync** (formerly bridge plugin, per ADR-0012) is the sync system for **agent identity files**: SOUL.md, MEMORY.md, identity files, and any other on-disk content OpenClaw reads as files for agent operation. Plaintext over TLS (post-§22C cutover). Authoritative for "anything that exists on disk because OpenClaw reads it from disk."

The two systems do not share infrastructure, conflict-resolution mechanisms, encryption models, or transport. They do not need to. The boundary is stable and the systems serve different needs.

The boundary is enforced by which storage system content lives in:

- Content lives in the SQLite database → Supabase CRDT path. Local DB is the source of truth; Supabase is the relay.
- Content lives as a file in `.pane/system/{agent_uuid}/` → Gateway path. Pane's filesystem is the source of truth; gateway transports to OC's filesystem on split-machine, symlink handles it on same-machine.

There is no third path. New content categories pick one based on whether they match the user-data shape (DB-backed, row-based, encrypted-via-CRDT) or the agent-identity-file shape (filesystem-backed, OC reads as file, plaintext at rest).

## Consequences

### Positive

- **Two clear, narrow sync paths instead of one paradoxical one.** Each system is good at its data shape. Supabase CRDT for rows, gateway for files. No system is asked to do what it's bad at.
- **Zero-knowledge property preserved without compromise.** SOUL/MEMORY plaintext stays on user-controlled machines (Pane device + user's OC instance). Supabase never sees agent identity content; encrypted blobs in Supabase remain user data only.
- **Shipped decision now traceable.** Future engineers / agents reading the codebase find this ADR linked from §22C, ADR-0002, and ADR-0004. The split is no longer "two systems exist for reasons nobody documented."
- **Closes audit gap #8 from the 2026-04-27 bridge audit.** That gap noted "architectural decisions undocumented — bridge vs Supabase, HTTP vs WS, ID-routing." This ADR addresses the first; ADR-0015 addresses the second; the ID-routing decision is documented in #364 and referenced in §22C.10.

### Negative

- **Two systems to maintain, test, and reason about.** Twice the surface area for sync-related bugs. Twice the spec sections (§9 for Supabase CRDT, §22C for gateway). Twice the "is this synced yet?" mental model for both engineers and (sometimes) users. The cost is real and ongoing.
- **No consolidation pressure-valve.** This ADR explicitly forecloses the "merge them later when we have time" path. Anyone proposing to merge them now has to overturn this ADR with a new ADR that either (a) fundamentally changes the data shape of agent identity files, (b) redefines zero-knowledge to permit OC reading user keys, or (c) abandons the same-machine optimization. Each of those is a bigger decision than the consolidation itself.
- **Onboarding teaches two sync systems.** Anyone joining the team needs to understand both. Mitigated by this ADR + §9 + §22C being readable in 30 minutes total.

### Risks

- **Future feature lands in the wrong system.** A new content category gets misclassified and goes into the wrong path. Mitigation: the boundary rule above is sharp ("does it live in the DB or as a file?"), and the §11 spec audit catches misclassifications during PR review.
- **A third sync need emerges that fits neither system cleanly.** Real-time collaboration, multi-agent coordination state, hosted-Tier-1-specific data — any of these could pressure a new path. Mitigation: this ADR scopes itself to "the two systems that exist today." A new system is a new ADR, not a consolidation of these two.
- **Tier 1 hosted introduces pressure to route agent files through Supabase to avoid running gateways.** This pressure is real — running per-user gateway instances in a hosted environment is operationally heavier than reusing the existing CRDT pipeline. Mitigation: Tier 1 hosting design is post-launch and gets its own ADR. This ADR documents why the current split exists; it does not pre-commit Tier 1's choice. If Tier 1 finds a way to honor the three reasons above (data shape, zero-knowledge, same-machine) while consolidating the systems, that's a legitimate future ADR. Today's data says it can't.

## Alternatives Considered

**One unified sync system through Supabase CRDT.** Route agent identity files through the same CRDT pipeline as user data. Rejected for the two reasons in Context: data shape mismatch (markdown files aren't rows) and zero-knowledge violation (OC would need decryption keys). Each reason is independently sufficient.

**One unified sync system through the bridge / gateway.** Route user data (notes, tasks, etc.) through the same HTTP path as agent identity files. Rejected: the bridge / gateway is LWW-only and cannot handle the conflict density of multi-device note editing. CRDTs exist in System 1 specifically because LWW loses data on concurrent edits — a notes app cannot ship with that property. Additionally, the bridge / gateway has no relay infrastructure; it requires direct point-to-point connectivity, which doesn't model "phone offline, edits on laptop, both sync to a relay later."

**Hybrid: agent identity files in Supabase but unencrypted.** Bypass zero-knowledge for agent identity files only. Rejected: the value of zero-knowledge is the architectural property that no plaintext leaves user-controlled machines. Carving an exception for one data category creates a category-by-category encryption policy that is hard to reason about and easy to violate accidentally. ADR-0004's invariant is binary: synced data is encrypted, full stop.

**Filesystem-only sync (drop Supabase entirely).** Move all sync — including notes and tasks — onto the bridge / gateway model. Rejected: the bridge / gateway has no relay, requires direct connectivity, and is LWW. None of those properties work for multi-device user-data sync. This alternative is "delete System 1 and replace with System 2," and System 2 cannot do System 1's job.

**Defer this ADR (keep the split undocumented).** Rejected per "What if we didn't decide" above. The decision shipped; the documentation is the only outstanding work, and it's cheap.

## References

- Spec sections: `docs/spec/sync.md` §9 (Supabase CRDT — user data); `docs/spec/sync.md` §22C (Pane Gateway — agent identity files); `docs/spec/sync.md` §0.3 (source-of-truth model)
- Related ADRs: ADR-0002 (CRDTs for Multi-Device Sync — System 1's foundation); ADR-0004 (Zero-Knowledge Encryption for Sync — the constraint that prevents merging the systems); ADR-0005 (DB as Primary, Flat Files as Backup — clarifies which sync system handles which storage); ADR-0012 (Pane Gateway as primary sync transport — supersedes the bridge plugin in System 2)
- Audits: 2026-04-27 bridge audit (#689) gap #8 (architectural decisions undocumented — this ADR closes it)
- Migration history: bridge plugin migration #364 (ID-routing) — agent identity files keyed by `oc_agent_id` UUID, not by name
- INVARIANT (existing, in §0.3): "User data is canonical in the DB; agent identity files are canonical on disk." This ADR explains why those two storage models drive two different sync paths.
