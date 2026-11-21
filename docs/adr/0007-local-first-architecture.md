# ADR-0007: Local-First Architecture

**Status:** Accepted  
**Date:** 2026-02-08  
**Author:** Matt Carano

## Context

Most modern productivity apps (Notion, Roam, Linear) are cloud-first — data lives on a server, the app is a view layer, and offline access is an afterthought. This creates dependencies on internet connectivity, vendor lock-in, and trust that the provider won't read, lose, or monetize your data.

Matt's philosophy: the user's machine is the source of truth. Always. Data should never leave the device without explicit opt-in.

## Decision

Pane is local-first:
- **All data stored locally** in SQLite on the user's machine
- **App works fully offline** — no internet required for any core functionality
- **Sync is opt-in** — users can choose to sync between devices, but it's not required
- **AI runs locally when possible** (Ollama) with cloud AI as an opt-in alternative
- **No analytics, no telemetry, no phone-home** unless the user explicitly enables it
- **User owns their data** — exportable, deletable, portable

## Consequences

### Positive
- **Works everywhere** — No internet dependency. Airplane, cabin, submarine.
- **Fast** — No network latency for reads/writes. SQLite is sub-millisecond.
- **Private** — Data doesn't leave the device unless the user says so.
- **Trustworthy** — Users can verify what the app does by inspecting local files.
- **Resilient** — Server goes down? App keeps working. Company goes away? Data survives.
- **Alignment** — Matches Matt's philosophical position on sovereignty, privacy, and decentralization.

### Negative
- **No web version** — Can't run Pane in a browser (see ADR-0001).
- **Sync is complex** — Without a central server as authority, we need CRDTs (see ADR-0002).
- **No collaboration** — Real-time multi-user editing requires a server. Out of scope for v1.
- **Local AI is limited** — Ollama models are smaller/slower than cloud models. Users who want Claude/GPT quality must opt in to cloud AI.
- **Backup is the user's responsibility** — We provide automated local backups, but if the machine dies and there's no sync, data is lost.

### Risks
- Users accustomed to cloud apps may find "sync is opt-in" confusing.
- Local-first is a marketing differentiator today but may become table stakes, reducing competitive advantage.
- If Pane Cloud (Tier 1) takes off, we'll be running cloud infrastructure despite the local-first philosophy. Need clear messaging about what "local-first" means when we also offer a cloud tier.

## Alternatives Considered

**Cloud-first with offline cache** — Standard SaaS approach. Rejected: violates privacy principles, creates vendor lock-in, requires internet for core functionality.

**Hybrid (cloud primary, local secondary)** — Data on server, cached locally. Rejected: "local-first" means the local copy is authoritative, not a cache.

**Pure local (no sync ever)** — Simplest. Rejected: Matt uses multiple devices and wants his data everywhere.

## References

- Architecture spec: §1 (Principles)
- Pane business model: local-first is the pitch
- Related: ADR-0002 (CRDTs), ADR-0004 (encryption), ADR-0005 (DB primary)
- Inspiration: https://www.inkandswitch.com/local-first/
