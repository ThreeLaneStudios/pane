# ADR-0060: System-File Canon per Provider Class; RULES.md Retired as a Generated Concept

**Status:** Proposed
**Date:** 2026-07-28
**Author:** Matt Carano, Claude (spec session)
**Supersedes:** None
**Supplements:** ADR-0039 (IDENTITY.md as identity SOT; the six-file read order), ADR-0059 (local agent creation),
 agents.md §24.10 / §24.10.1, onboarding.md §6.6.4, ai-integration.md §27.3
**Related:** #2030, #2277 (read-path fix — consumes this canon, not respec'd here), #2213/#2216 (workshop scaffolding),
 ADR-0011 (system notes read-only / flat-file layer)

## Context

#2030 shipped local agent creation to a "parity" sentence that named the three onboarding seeding
buttons but enumerated nothing else. The build was faithful to the sentence: it wrote a file set
that did not match what agents use, listed filenames instead of synthesizing uploads, and skipped
the review step. Separately, RULES.md is dead at runtime for the main agent — the system-prompt
read block reads AGENTS.md in the slot labeled "Rules" (ai-integration.md §27.3: SOUL, IDENTITY,
USER, TOOLS, **AGENTS**, memory), and RULES.md diverged from it long ago. This ADR fixes the canon:
which files a local agent has, which are OC-only, the authority direction, and RULES.md's status.

## Decision

### D1 — Required system-file set is provider-class-specific

A **locally-created (non-OC) agent** has exactly three system files written at creation:
**IDENTITY.md, SOUL.md, MEMORY.md.** All three are required and created at creation time (not
"populated over time"). IDENTITY.md carries the name (ADR-0039); SOUL.md and MEMORY.md are seeded
per the chosen path (§24.10.1c).

Every other file in the identity sync whitelist (onboarding.md §6.6.4 — USER.md, TOOLS.md,
AGENTS.md, HEARTBEAT.md, RULES.md, INVARIANTS.md) is **OC-only**: it arrives via OC import/sync and
is **never generated locally**. For a local agent these files are absent, and their absence is
correct. Two OC-only sub-cases, distinct because they behave differently:

- **Read into the system prompt if present:** USER.md, TOOLS.md, AGENTS.md (the §27.3 read set).
- **Synced but outside the read set:** HEARTBEAT.md, RULES.md, INVARIANTS.md — whitelisted for
 Pane↔OC sync (§6.6.4) but not injected into the system prompt.

The read block's include-if-present behavior for missing files is the stated contract. The read
path itself is owned by #2277; this ADR defines the canon it reads, not the read mechanism.

### D2 — Flat file is canonical; the DB note record is derived

For both provider classes, the flat file on disk is the source of truth for system-file content;
the DB note record is a rebuildable projection. On divergence the file wins for system-prompt
content. Consistent with the flat-file architecture (ADR-0011) and local-first (ADR-0039).

### D3 — RULES.md is retired as a *generated* concept; the read/sync side survives

Onboarding stops **authoring** RULES.md. Evidence it is already dead for local agents: nothing on
the main-agent path reads it (the "Rules" read slot reads AGENTS.md); it has **no DB-side setter**
(no `ai_set_*` companion, unlike the files that are actively maintained); and its content diverged
from the live AGENTS.md long ago.

- **Survives untouched:** the `rules` note type, `get_system_note`, the §24 AGENTS.md read (labeled
 "Rules"), sync restore, existing DB `rules` records, and the §6.6.4 whitelist entry for RULES.md.
 OC can still sync a RULES.md over; Pane simply never generates one.
- **Removed (onboarding authoring only):** RULES.md generation, the Rules workshop screen, the
 Rules workshop note (`ensureNote`) and its purge entry, and the "Moving to Rules Document"
 transition marker.

RULES.md is redundant for local agents: the constraints that matter are generated into SOUL.md, and
AGENTS.md (OC-authored team structure) is meaningless for API agents, which cannot spawn subagents.

## Consequences

- **Positive:** a local agent's file set is defined and matches the read block; "parity" is
 enumerated; the dead RULES.md authoring path is removed without touching the live read/sync side;
 OC users lose nothing.
- **Negative:** the OC-only set is larger than ruling 1's shorthand implied (six files, not three).
 This ADR is the authoritative enumeration and supersedes any three-file reading of that ruling.
- **Risk:** over-deletion. D3's survives-boundary is explicit precisely so implementation removes
 authoring only. Removing the `rules` note type or the AGENTS read would break OC agents.

## References
- ADR-0039 (identity SOT, read order), ADR-0059 (local creation), ADR-0011 (flat-file layer)
- agents.md §24.10 / §24.10.1; onboarding.md §6.6.4; ai-integration.md §27.3
- #2030, #2277, #2213/#2216
