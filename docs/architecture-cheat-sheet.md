# Architecture Cheat Sheet
*How Pane works, in plain language*  
*Rewritten 2026-05-20 against current spec. Supersedes the 2026-04-14 version.*

---

## What Pane is

Pane is a notes-and-tasks desktop app where the AI has long-term memory of you. Unlike chat tools that forget the moment you close the tab, Pane's agent remembers across every conversation — your projects, your preferences, what you decided last Tuesday. It's a SvelteKit + Tauri app with a local SQLite database. You can hook it up to a plain API key, a local model on your machine, or a full agent infrastructure that can run shell commands on your behalf.

---

## The three tiers

Pane is the same app for everyone. The tier just decides who provides the AI brain.

**Tier 2 — Bring Your Own Key (BYOK).** You paste in an API key from Anthropic, OpenRouter, or Ollama. Pane talks to that provider directly. You pay the provider for usage; Pane itself is a flat monthly subscription on top. Best for: people who already have a key and want full control over which models they use.

**Tier 3 — OpenClaw.** You run an OpenClaw instance on a second machine — the kind of Mac Studio that sits in a closet running 24/7. Pane talks to a small service called the **Pane Gateway** sitting next to OpenClaw. OpenClaw can do things plain API providers can't: execute shell commands, browse the web, spawn sub-agents, run cron jobs overnight while you sleep. Pane is the interface; OpenClaw is the engine doing the work. Best for: people who want the AI to *do things*, not just chat.

**Tier 1 — Pane Cloud (post-launch, coming next).** Same architecture as Tier 3, except the OpenClaw machine and the Gateway are ours, not yours. You sign in, we route your traffic to a hosted OpenClaw instance. No infrastructure for you to manage. The reason we can build it next instead of years from now: the new Pane Gateway shipped this cycle was designed with multi-tenancy from day one. Every row of gateway state already carries a `tenant_id`; every endpoint scopes by tenant (ADR-0012, ADR-0013). At launch we run with one tenant ("default"). Tier 1 is a config flip plus auth, not a rewrite. The new gateway also replaces the old OpenClaw-side gateway — same protocol on the Pane side whether you're self-hosting (Tier 3) or we're hosting (Tier 1).

Pane is paid. No free tier.

---

## Data model

Pane has two storage layers and they cooperate by following clear rules.

**The database (`pane.db`, SQLite).** A single file on disk. Holds all metadata (titles, timestamps, task fields), all chat transcripts, and all extracted memory facts. Since v0.59.0, for **user notes** the database also holds the body content. That's what makes Pane fast — searching or opening a note doesn't touch the filesystem.

**Flat files (Markdown on disk).** Two kinds, with different rules:

- **User notes** at `~/Pane/Projects/{Project}/...md`. The DB is runtime primary; the flat file is a backup, dual-written on every save. You can crack open the folder in Finder and export everything — it's all just Markdown.
- **System notes** (the agent's identity files) at `~/Pane/.pane/system/{Agent Name}/SOUL.md`, `MEMORY.md`, daily logs, etc. **Flat files are the sole source here — the DB body stays NULL.** This is so OpenClaw can read the files directly when it's working autonomously.

```
~/Pane/
├── .pane/                          ← App-managed (hidden by default)
│   ├── system/
│   │   └── {Agent Name}/
│   │       ├── SOUL.md, MEMORY.md, IDENTITY.md,
│   │       ├── USER.md, TOOLS.md, AGENTS.md,
│   │       ├── RULES.md, HEARTBEAT.md
│   │       └── logs/YYYY-MM-DD.md      ← Daily logs
│   └── pane.db                     ← SQLite database
│
└── Projects/                       ← Your data (portable Markdown)
    └── {Project Name}/
        ├── Memory.md               ← Per-project AI memory
        └── {your notes}.md
```

**Pane is PRIMARY. OpenClaw is REPLICA.** When Pane is open, only Pane writes. When Pane is closed and OpenClaw is doing autonomous work, OC writes to its local copy of the system files and queues the changes. On Pane's next launch they reconcile.

---

## How the AI accesses your data

The AI provider itself has no memory — it forgets everything between messages. Pane works around this by re-sending the entire context every single message. Picture it as a 200-page notebook that travels with every question.

For each message, Pane assembles the package: the agent's SOUL.md (personality), the agent's MEMORY.md (long-term memory), the current project's Memory.md if you're in a project, today's daily log plus yesterday's, your recent chat history, and the list of tools the AI can call (search notes, read notes, save memory, and so on). That whole pile goes to the AI provider with your new message. The AI replies. Next message, repeat.

This sounds expensive — and would be — except providers cache repeated prefixes. The first ~20 pages of each request are byte-identical between consecutive messages, so the provider charges full price the first time and roughly 90% off thereafter. Pane keeps the order stable so caching keeps working. The cache lasts about 5 minutes; walking away resets it.

**Tools work the same way.** If the AI wants to search your notes, it sends a tool call back to Pane. Pane runs the SQLite query, sends the results to the AI, and the AI continues. The AI never touches the database directly — Pane is always the middleman.

**Compaction — when the notebook gets too thick.** Every model has a context window, and the budget manager Pane uses for each request fits as much recent history as it can. When history outgrows the window — or when responses start getting noticeably slow — Pane runs **compaction**: an LLM summarizes the older messages into a compact recap that takes the place of the raw originals in subsequent requests. Two triggers, both adaptive:

- **Budget-aware (primary).** If the budget manager is already dropping more than ~12 messages to fit the window, compaction fires to summarize what got dropped. This is **self-tuning based on the model's context size** — there's no fixed message count. A 200K-context provider (OpenRouter, Anthropic) barely triggers it; a 32K local Ollama model triggers it sooner; an 8K local model triggers it within a dozen messages or so.
- **Latency (secondary).** If three consecutive requests all take longer than 30 seconds *and* you have more than 100 messages in the session, compaction fires even when the context still technically fits. Slow context isn't useful context.

**Originals stay searchable.** Compaction doesn't delete anything. The raw messages stick around in the database, retrievable via memory search — the live context just carries the summary instead.

---

## The memory system

This is what makes Pane different from a vanilla chat tool. Three writers and an end-of-day agent each own a specific job, with no overlap (ADR-0010, INVARIANTS §5):

**1. Extraction — after every conversation.** A background worker reads the conversation and pulls out atomic facts: your name, preferences, decisions, deadlines, project choices. Each fact gets a confidence score from 1 to 5; only 4+ saves. Extraction does a **dual-write**: the fact goes into the `ai_memory` table AND a timestamped `[saved]` entry gets appended to today's daily log immediately. The DB row is the source of truth; the daily log is the audit trail (memory-pipeline.md §5.4).

**2. Router — when ≥1 unrouted fact accumulates (default, configurable).** Takes new facts from `ai_memory` and routes them to the right memory file as a bullet. Identity, preference, style, and project-scoped goals → agent's MEMORY.md. Project facts and decisions → that project's `Memory.md`. General facts and decisions with no project context → daily log only (they're already there from extraction). The router is mechanical, no LLM, runs in milliseconds (memory-pipeline.md §5.5.1).

**3. Consolidator — rare.** When a memory file crosses ~2000 words or hits a 14-day staleness backstop, the consolidator does an LLM rewrite — turns the accumulated bullets into clean prose. It reads the active fact list from `ai_memory` (the source of truth) plus the existing file's prose style, then rewrites the file (memory-pipeline.md §5.5.3).

**4. End-of-day agent — once a day.** Deduplicates yesterday's daily log and prepends a 3–6 sentence narrative header. Only touches daily logs, never memory files (memory-pipeline.md §5.5.2).

**Search.** When you (or the AI) search memory, Pane queries two sources: extracted facts (`ai_memory`) and raw chat transcripts. Results are scored by embedding similarity, boosted by keyword overlap (weight 0.3), with extracted facts getting a 1.2× boost over raw conversations, and a temporal boost if your query mentions time ("this week", "yesterday"). Nothing is ever deleted — old conversations stay searchable. Compaction archives originals but doesn't drop them (memory-pipeline.md §5.15).

**Decay.** Facts not referenced in 90 days are soft-deleted automatically — except identity and preferences, which never expire.

---

## Smart routing and the dual-provider model

Not every question needs the smartest model. "What's 2+2" doesn't need Opus. Smart routing handles this with a tiny classifier call: a cheap fast model reads your message, decides simple-or-complex, routes accordingly. Simple → Haiku-class. Complex → your full model.

The architecturally interesting part is **where each kind of call goes** (smart-routing.md §27.6):

```
                                  User chat
                                      │
                                      ▼
                          ┌─── Primary provider ───┐
                          │ (your Tier 2 API key, or  │
                          │ your Tier 3 gateway)      │
                          └────────────────────────┘

   Classifier / Rerank / Extraction  ──► Internal provider
                                          (always direct, never
                                          through the gateway)
```

**Two providers, by design.** Chat goes through whatever you set as primary. Background calls — classifier, memory rerank, fact extraction — go direct to a separate **internal provider** you configure during onboarding. The internal provider can be **OpenRouter, Anthropic, or Ollama**.

Why the split? When your primary is a gateway (Tier 3), that gateway only reliably routes your main agent's model. Trying to push a Haiku classifier call through the same gateway fails because the gateway is shaped for chat, not arbitrary side calls. So background calls bypass the gateway and call their provider direct. (Memory features — extraction, smart routing, rerank — won't work without an internal provider configured. This is enforced at onboarding, no internal provider means no memory.)

---

## AI backend module map

`src-tauri/src/ai/` is split by responsibility, not grown as one file (#3097):

| File | Responsibility | Spec section |
|---|---|---|
| `chat.rs` | Chat streaming, sessions, retry/backoff, compaction | runtime.md §36.1–§36.2, sync.md §22C.9/§22C.12 |
| `ingest.rs` | OpenClaw transcript pull, external-message ingestion, tombstoning, agent sync/import | sync.md §22C.8 |
| `scheduler.rs` | Background jobs nobody typed: memory decay/extraction/consolidation, embeddings | runtime.md §36.3 (seeded now; populated by #3097's follow-up PR 2) |
| `tools.rs` | Tool schema + dispatch/execution for every agent-facing tool | ai-integration.md §4 |
| `connections.rs` | Provider connection management, API-key storage, model probing, settings passthrough | ai-integration.md §28 |
| `provider.rs` | Shared LLM-call primitives used by chat, scheduler, and tools | — |

---

## Two-machine sync (Tier 3)

For Tier 3, you have two computers that need to stay in sync.

```
User Machine (laptop)             Compute Machine (Mac Studio)
┌────────────────────┐            ┌──────────────────────┐
│  Pane App          │   HTTPS    │  Pane Gateway        │
│                    │ ──push──►  │   (port 8443)        │
│  pane.db           │            │                      │
│  .pane/system/     │ ◄──pull──  │  ┌────────────────┐  │
│    Brit/           │            │  │  OpenClaw      │  │
│      SOUL.md       │            │  │  workspace     │  │
│      MEMORY.md     │            │  └────────────────┘  │
│      logs/         │            │                      │
└────────────────────┘            └──────────────────────┘
```

**The Pane Gateway** is a small service running alongside OpenClaw on the compute machine. It speaks HTTPS to Pane and handles all sync. Two endpoints carry the load (sync.md §22C.6.2):

- `POST /v1/sync/push` — Pane sends a file. Triggered every time you save a SOUL.md or MEMORY.md edit.
- `GET /v1/sync/pull` — Pane fetches changes that piled up while it was closed. Runs at startup and reconnect.

(The old bridge plugin the previous cheat sheet described is retired. The new gateway replaces it end-to-end — ADR-0012.)

**Sync whitelist.** Only these files cross the wire: SOUL.md, MEMORY.md, IDENTITY.md, USER.md, TOOLS.md, AGENTS.md, HEARTBEAT.md, RULES.md, and daily logs (`logs/YYYY-MM-DD.md`). Work artifacts — scripts, builds, drafts — stay on the machine that made them.

**Conflict resolution.** Newer timestamp wins. If both machines edited the same file while disconnected, the later edit is kept. A content-identity check avoids gratuitous file rewrites (sync.md §22C.8.2).

**Networking.** The two machines need to reach each other. Tailscale is the recommended path — zero-config mesh VPN that works across networks and behind NATs. WireGuard, ZeroTier, or plain LAN IP all work. The network only provides reachability; all data movement is HTTPS through the gateway.

**Why the gateway is its own service.** It's not just sync. It also abstracts OpenClaw's WebSocket-only API into a clean HTTP/2 surface for Pane, handles session creation and message streaming over SSE, and carries the multi-tenancy seam for Tier 1 hosted. One service, one protocol, one cutover.

---

## Multi-device sync (Pane ↔ Pane)

Separate system, separate problem. This is how your laptop and your phone (or two laptops) stay in sync — your notes, tasks, projects.

- **CRDTs via `cr-sqlite`** — a SQLite extension that turns tables into conflict-free replicated relations. Two devices editing different fields of the same note both win; conflicts on the same field resolve last-writer-wins per column (sync.md §9.2).
- **Supabase as the relay.** Devices push encrypted changesets up; other devices pull them down. Supabase only sees ciphertext.
- **End-to-end encryption.** XChaCha20-Poly1305 for content, Argon2id derives the master key from your password, X25519 for new-device pairing. The server holds ciphertext only — zero-knowledge (sync.md §9.6).

This system is independent of Tier 3 gateway sync. They solve different problems and you can use one without the other.

---

## Onboarding

First launch sets up your AI. The flow depends on which tier you pick.

**Tier 2 (BYOK):** Pick a provider (Anthropic / OpenRouter / Ollama), paste your API key, done.

**Tier 3 (OpenClaw):** A three-trust-boundary pairing flow — network reach, TLS identity, app-level pairing code (onboarding.md §6.5.3, sync.md §22C.5):

1. **URL entry.** Paste the gateway URL (e.g., `https://your-machine.ts.net:8443`). Pane probes it.
2. **Verify & connect.** Pane fetches the gateway's self-signed TLS fingerprint and shows it. You compare against what the gateway printed to its terminal when it started, then accept. Then you paste a **6-digit pairing code** (5-minute expiry) that the gateway also printed at startup. If the code expires, you run `openclaw gateway pairing-code` on the compute machine to get a fresh one.
3. **Initial sync.** The gateway returns all your existing agents and their files. Pane imports them. A "Your Team" screen shows what landed.

Then a **Smart Features** step picks the internal provider (OpenRouter / Anthropic / Ollama) that backs extraction, classifier, and rerank. This step is required — no internal provider, no memory features.

**Tier 1 (post-launch):** Sign in, done. No gateway pairing because the gateway is ours.

---

## Load-bearing principles

A short list of rules that everything else respects:

- **Pane is PRIMARY, OpenClaw is REPLICA.** One owner at a time. Pane writes when open; OC writes when Pane is closed and reconciles on reconnect.
- **One writer per memory file.** Extraction writes only to `ai_memory` and the daily log. The router appends or removes bullets. The consolidator rewrites. The end-of-day agent finalizes daily logs. Nobody else.
- **`ai_memory` is the source of truth for memory content.** Memory files are rendered views of it. Deletes propagate cleanly because the consolidator reads the active fact list.
- **Internal background calls bypass the gateway.** Classifier, rerank, and extraction call the internal provider direct. Chat goes through the gateway (OC owns chat-model routing). By design (smart-routing.md §27.6).
- **Sync whitelist is fixed.** Identity files and daily logs only. Work artifacts never cross the wire.
- **Compaction summarizes, never deletes.** Originals stay in the DB and remain searchable. Triggers are adaptive to the model's context size — no fixed message-count threshold.
- **Pre-launch: no migration chain.** Single canonical schema, delete-and-recreate on version mismatch. Production gets real migrations.
- **CRR tables (multi-device sync) cannot use UNIQUE or FOREIGN KEY constraints.** cr-sqlite rejects both; enforce in application code.
- **Build for the decade, not the afternoon.** No temporary workarounds. If something doesn't earn its keep long-term, it doesn't ship.
