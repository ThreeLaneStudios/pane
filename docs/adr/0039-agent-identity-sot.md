# ADR-0039: Agent Identity — IDENTITY.md as Single Source of Truth

*Line citations predate the #3097 module split.*

**Status:** Accepted
**Date:** 2026-05-27
**Author:** Matt Carano, Brit Freed (spec session)
**Supplements:** ADR-0011 (Agent Files spec), ADR-0037 (Settings UI Architecture), ADR-0038 (Extraction Model Routing — shape precedent for multi-decision ADRs)
**Related:** agents.md §24.2/§24.3, memory-pipeline.md §5.3 (write surfaces), onboarding.md §6.5 (greeting)

## Context

The agent has no self-identity surface. Three disconnected stores hold name-adjacent data, none of which reaches the system prompt correctly:

1. **`lib.rs:506`** — Hardcoded `"You are Pane, an AI partner..."` in `AiConfig.system_prompt` at app startup. Never updated when the agent is renamed. Every chat message carries this regardless of the agent's actual name.

2. **`user_dashboard_config.greeting_name`** — Written during onboarding, read only by the greeting path (`commands.rs:4295-4320`). The greeting replaces the entire system message with `"[System] You are {greeting_name}..."`, which works for the first message but is not used for any subsequent chat.

3. **`ai_agents.name`** — Written during onboarding and Settings rename. Drives UI labels (chat header, sidebar, settings display). Never injected into the system prompt.

The result: the agent introduces itself correctly in the greeting (reads `greeting_name`), then immediately forgets its name (subsequent messages read the hardcoded "You are Pane" from `lib.rs:506`). Renaming in Settings updates the UI but the AI continues to call itself "Pane."

`IDENTITY.md` already exists in the file load order (`commands.rs:1116`, position 2 of 6: Soul → Identity → User → Tools → Rules → Memory) and is loaded into the system prompt when present. It is never written.

### What if we didn't decide

- The agent introduces itself as "Pane" regardless of what the user named it. The onboarding name-setting step is decorative beyond the greeting.
- `greeting_name` continues to exist as a parallel identity store, diverging from `ai_agents.name` over time.
- The hardcoded "You are Pane" persists as the only identity the AI sees, contradicting both the user's choice and the agent's own files.

## Decision

### 1. IDENTITY.md is SOT for agent self-identity in the system prompt

The agent's name and identity description live in `IDENTITY.md` at `~/.pane/system/{workspace_path}/IDENTITY.md`. This file is loaded at position 2 of 6 in the system prompt injection order (Soul → **Identity** → User → Tools → Rules → Memory). The AI reads it on every chat message.

`ai_agents.name` remains the SOT for UI display (chat header, sidebar, settings). Both are updated atomically on rename.

### 2. Strip hardcoded identity claim from lib.rs:506

Remove "You are Pane, an AI partner embedded in a notes and project management app..." from the initial `AiConfig.system_prompt`. Replace with a role description that carries no name:

> "You are an AI partner embedded in a notes and project management app called Pane. You help the user organize their work, create and manage notes and tasks, and think through problems. Be concise, helpful, and proactive. You have access to the user's workspace context shown below."

The agent's name comes from IDENTITY.md at assembly time. The role description ("AI partner in a notes app") is product-level, not identity-level — it stays hardcoded because it doesn't change per-agent.

### 3. Write IDENTITY.md on agent creation and Settings rename

Two write surfaces:

- **Onboarding agent creation:** When the user names the agent during onboarding, write `IDENTITY.md` with the chosen name. Minimal format: `# {name}\n\nYour name is {name}.\n`
- **Settings rename:** When the user renames the agent in Settings, update `IDENTITY.md` with the new name (atomic with `ai_agents.name` update).

The format is intentionally minimal — the user can expand it with role, personality, backstory. The write surface only guarantees the name is present.

### 4. Unify greeting through standard prompt assembly

The greeting path (`commands.rs:4295-4320`) currently reads `greeting_name` from `user_dashboard_config` and replaces the entire system message with a special-case greeting prompt. After this change:

- The greeting still injects a `[System] You are {name}, meeting this user...` instruction.
- The name is read from `ai_agents.name` (already in the DB at greeting time), not from `greeting_name`.
- The rest of the system prompt (including IDENTITY.md content) is preserved — the greeting instruction is prepended, not a replacement.

### 5. Startup migration for existing installs

On first launch after update: if `ai_agents.name` has a non-empty value but `IDENTITY.md` is empty, contains only the integrity-check stub (`"auto-created by integrity check"`), or doesn't exist — write `IDENTITY.md` from `ai_agents.name`. One-time, idempotent. Runs inside `verify_agents_integrity()`.

### 6. Remove greeting_name from greeting path; drop column immediately

The greeting path stops reading from `user_dashboard_config.greeting_name`. The column is dropped in a separate migration PR (not deferred to post-launch). `greeting_name` has no other consumers after Decision 4.

### 7. ai_agents.name remains UI label SOT

`ai_agents.name` continues to drive all UI surfaces: chat header, sidebar, settings display, onboarding agent-names screen. On rename, both `ai_agents.name` and `IDENTITY.md` are written in the same operation. If they diverge (bug), `ai_agents.name` wins for UI and IDENTITY.md wins for system prompt — but divergence should be treated as a bug, not a designed state.

## Rationale

### Why IDENTITY.md as SOT, not a DB column

The flat-file system is the architectural pattern for agent context. SOUL.md, MEMORY.md, USER.md, TOOLS.md, and RULES.md all live on the filesystem and are loaded into the system prompt at assembly time. A DB column for agent identity would be the only piece of agent context that bypasses the file layer — readable only through code, not inspectable by the user, not editable with a text editor, and not part of the sync payload. IDENTITY.md follows the existing pattern and inherits its properties: user-editable, sync-compatible, inspectable, and extensible without schema changes.

### Why strip the hardcoded identity, not override it

Overriding "You are Pane" at runtime (e.g., string-replacing "Pane" with the agent name) is fragile — it assumes the hardcoded string won't change shape and creates a hidden dependency between the startup default and the override logic. Stripping the name from the hardcoded prompt and letting IDENTITY.md supply it is cleaner: the startup prompt describes the product role ("AI partner in a notes app"), the file supplies the identity ("Your name is Birdy"). Two concerns, two sources, no coupling.

### Why unify the greeting path, not maintain the bypass

The greeting path today replaces the *entire* system message with a one-line greeting instruction, discarding Soul, Rules, Memory, and all other context. This means the greeting is the one message where the agent has no personality, no memory, and no rules — only its name. Unifying the greeting through the standard assembly path means the first message carries the same context as every subsequent message. The agent greets with its full personality, not a stripped-down version of itself.

## Consequences

### Positive
- Agent knows its own name on every message, not just the greeting
- Single source of truth for system prompt identity (IDENTITY.md) — inspectable, user-editable, part of the flat-file architecture
- `greeting_name` column eliminated — one fewer parallel store
- Existing file load order already handles IDENTITY.md — no system prompt assembly changes needed
- Forward-compatible: IDENTITY.md can grow to include role, pronouns, backstory without schema changes

### Negative
- Two stores must be kept in sync (`ai_agents.name` for UI, `IDENTITY.md` for system prompt) — divergence is a new bug class
- Rename now writes to filesystem in addition to DB — slightly more I/O per rename operation
- Migration path for existing installs adds startup complexity

### Risks
- **Rename race condition:** If rename writes `ai_agents.name` but fails to write IDENTITY.md (disk full, permissions), they diverge. Mitigation: write IDENTITY.md first, then update DB; if IDENTITY.md write fails, abort the rename entirely.
- **Integrity check overwrites user edits:** If user manually edits IDENTITY.md and the integrity check runs, it should NOT overwrite with `ai_agents.name`. Mitigation: integrity check only writes IDENTITY.md if it's empty or contains the stub marker, never if it has user content.

## Alternatives Considered

### A. Inject ai_agents.name at assembly time (no file)

Read `ai_agents.name` from the DB during system prompt assembly and prepend "Your name is {name}" dynamically.

- **Pro:** Single source of truth (DB only), no file to manage, instant on rename
- **Pro:** Works retroactively for all existing installs without migration
- **Con:** Not inspectable or user-editable — agent identity becomes invisible infrastructure
- **Con:** Doesn't scale — if identity grows beyond a name (role, backstory, pronouns), you need a file anyway
- **Con:** Inconsistent with the flat-file architecture that already loads SOUL.md, MEMORY.md, etc.

Rejected: the flat-file system is the architectural pattern. Identity should live in a file like everything else.

### B. Write identity into SOUL.md instead of IDENTITY.md

Put "Your name is {name}" into the Soul document.

- **Pro:** One fewer file to manage
- **Con:** SOUL.md is user-curated personality and tone — mixing in a machine-written name line creates a maintenance conflict
- **Con:** Onboarding workshop already writes to SOUL.md (AI-authored personality); adding a name line would be a second write surface with different ownership semantics
- **Con:** IDENTITY.md already exists in the load order — no reason not to use it

Rejected: SOUL.md is personality, not identity. They're different concerns.

## Follow-ups

- **CI/audit check for ai_agents.name vs IDENTITY.md divergence** (#1281) — mechanizes the invariant from Decision 7.
- **Spec: memory-pipeline.md §5.3 injection order correction** — current spec says "Soul → Rules → Memory → Context" (4 elements); code does Soul → Identity → User → Tools → Rules → Memory (6 elements). Amended in the same PR as this ADR.
- **Spec: onboarding.md §6.5 greeting subsection update** — update to reflect Decision 4 (greeting reads `ai_agents.name`, not `greeting_name`). Amended in the same PR as this ADR.

## References

- Related issues: #1279 (implementation), #1280 (greeting_name drop), #1281 (divergence check), #1264 (display fallback — disposition TBD)
- Spec sections: agents.md §24.2/§24.3, memory-pipeline.md §5.3, onboarding.md §6.5
- Code references: `lib.rs:506` (hardcoded identity), `commands.rs:1116` (injection order), `commands.rs:4295-4320` (greeting path), `commands.rs:7559` (integrity check)

## Supplement — 2026-07-15 (#1541): Fixed-template greeting supersedes Decision 4

**Status:** Accepted (supplements ADR-0039; supersedes Decision 4 only)
**Author:** Matt Carano, Claude (spec session)
**Related:** #1541 (launch-blocking); onboarding.md "Post-onboarding greeting (#1223)", §6.5.3, "Project greeting (#1541)"; smart-routing.md §27.6.5 (retired)

### What this changes

Decision 4 ("Unify greeting through standard prompt assembly") is **superseded.** D4 routed the post-onboarding greeting through standard system-prompt assembly as a **model call** — reading `ai_agents.name`, prepending a `"(System) You are {name}, meeting this user... Introduce yourself briefly..."` instruction, and letting the model generate a warm introduction "with its full personality."

Under #1541 the greeting is no longer generated by a model. It is a **fixed template**:

> Hey there, I'm [agent]. Very nice to meet you. What would you like to do first?

`[agent]` = `ai_agents.name WHERE is_main = 1` (empty-name fallback `Pane`, unchanged from ADR-0039). No system message, no assembly, no dispatch. The project greeting (onboarding.md "Project greeting (#1541)") follows the same fixed-template, non-generated form.

### Why

D4's "greet with full personality" goal produced the failure #1541 was filed for: the onboarding agent **confabulated persona claims about itself** ("a curious and easygoing guy 😊") present in no identity file. The greeting is a first-impression, source-of-truth surface for who the agent is; letting a model improvise identity text there is a trust/accuracy defect on the core chat experience. A fixed template removes the generative surface entirely — the agent cannot invent persona text it doesn't have, because it is not asked to write anything.

### Scope — what does NOT change

This does not reverse the rest of ADR-0039. IDENTITY.md remains SOT for agent self-identity (D1); `ai_agents.name` remains UI-label SOT (D7); the hardcoded-identity strip (D2), IDENTITY.md write surfaces (D3), startup migration (D5), and `greeting_name` removal (D6) all stand. Only the greeting's *generation mechanism* (D4) changes — from a model call to a static insert — for all provider paths (BYOK and OC). Persona files still drive every subsequent (real) message exactly as before.

### Consequences

- The greeting is no longer an AI call and does not dispatch through either provider path (§29). smart-routing.md §27.6.5 (greeting-bypass) is thereby obsolete — nothing is routed — and is retired in the same spec PR.
- D4's "greeting carries the same context as every subsequent message" property no longer holds for the greeting, by design: the greeting carries no model context because it is not a model output.
- The `greeting: true` context flag has no remaining routing consumer; residual handling may be removed in the code PR (non-normative).

### Decision 4 body unchanged

Per ADR immutability, the Decision 4 text is not edited. This supplement records that D4's greeting-as-prompt-assembly mechanism is superseded by the fixed template, effective #1541.
