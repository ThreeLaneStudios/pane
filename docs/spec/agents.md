# First-Class Agent Entities

**Last Updated:** 2026-05-07
**Source:** Extracted from pane-architecture-spec.md
**Sections:** §24

> **Cross-references:** See [memory-pipeline.md](memory-pipeline.md) §5 for memory system, [onboarding.md](onboarding.md) §6 for agent setup.

---

## 24. First-Class Agent Entities
> **Status:** PARTIAL (startup sequence BUILT v0.17.39, rest NOT BUILT)
> **Created:** 2026-03-27
> **Replaces:** Current approach of agents-as-folders under "Pane System" project
> **Cross-references:** §16 (Agent Architecture), §5 (Memory System), §6.5 (OpenClaw Import)

*Agents are first-class entities with their own table (`ai_agents`), separate from projects/folders entirely. This eliminates sidebar clutter, feature collision, and hierarchy bugs caused by shoehorning agents into the project model.*

**Scope note:** This section defines ONLY how agents are stored and managed. It does NOT change:
- The memory system (§5) - extraction, synthesis, memory notes all remain unchanged
- The strategy agent model (§16) - one agent, multiple contexts, task agents are ephemeral
- Daily logs - still written to `.pane/system/{main-agent}/logs/` by the strategy agent only
- How conversations work - per-project threads, strategy agent in all of them

**Agent file protection:** Agent identity files (SOUL.md, MEMORY.md, etc.) are **AI-managed and protected**. Users can VIEW them in Settings → Agents, but editing is done by the AI (via memory synthesis, identity updates, etc.). This matches the existing read-only agent docs behavior (v11.21.0) - system notes are non-editable in the UI, the AI manages them via its tools. Direct filesystem edits are possible for power users but not encouraged; the AI is the intended author.

### 24.0 Startup Sequence
> **Status:** BUILT (v0.17.39)
> **GitHub Issue:** #288

The startup sequence is ordered to prevent race conditions:

1. **Database initialization** - Create DB if missing, run migrations, restore ai_settings from config.json
2. **Agent integrity check (SYNCHRONOUS)** - Scan `.pane/system/` for agent folders, create/verify DB records in `ai_agents` table. Must complete before UI shows.
3. **System notes integrity check (SYNCHRONOUS)** - Verify system notes exist in DB, recreate from flat files if missing. Must complete before UI shows.
4. **UI launch** - Frontend renders. If `onboarding_completed = 0`, onboarding shows. `get_main_agent` now reliably returns the discovered agent.
5. **FTS rebuild (ASYNC)** - Full-text search index rebuilt in background thread. Signals `INTEGRITY_DONE` when complete (watcher waits for this).

**Why synchronous for agents:** The integrity check is fast (reads ~5 folders). Running it async created a race condition where onboarding would call `get_main_agent` before agents were discovered, creating phantom "My Agent" folders. The fix (v0.17.39) splits the agent scan (synchronous, fast) from the FTS rebuild (async, slow). This ensures the UI sees existing agents before rendering onboarding.

**Implementation:** `src-tauri/src/lib.rs` runs `verify_agents_integrity()` and `verify_system_notes_integrity()` synchronously in the setup block, then spawns a background thread for FTS rebuild only.

### 24.1 Core Principle - Agents Are Not Projects
> **Status:** NOT BUILT

| Concept | Project | Folder | Agent |
|---------|---------|--------|-------|
| Purpose | Container for user's work | Organizes notes within project | AI identity and memory |
| Has tasks? | ✅ | ✅ | ❌ |
| Has chat thread? | ✅ (per-project AI session) | ❌ | ❌ (agent IS the chat partner) |
| Has AI folder? | ✅ (auto-created) | ❌ | ❌ |
| User creates? | ✅ | ✅ | ❌ (system creates via import; future: AI-assisted creation) |
| Visible in sidebar? | ✅ | ✅ (under project) | ❌ |
| Managed via? | Sidebar | Sidebar | Settings → Agents |

### 24.2 `ai_agents` Table - Schema
> **Status:** BUILT v0.18.3 - UUID-based path routing enforced throughout codebase
> **Updated:** 2026-04-07 - Fixed all name-based path bugs (11 fixes from Reed's UUID audit)
> **Build:** #364 (v0.18.2), v0.18.3 (UUID path fixes)

```sql
CREATE TABLE ai_agents (
    id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    name            TEXT NOT NULL,
    slug            TEXT NOT NULL UNIQUE,
    role            TEXT NOT NULL DEFAULT 'sub',
    description     TEXT,
    model           TEXT,
    is_main         INTEGER NOT NULL DEFAULT 0,
    is_active       INTEGER NOT NULL DEFAULT 1,
    workspace_path  TEXT NOT NULL,
    oc_agent_id     TEXT,
    soul_hash       TEXT,
    memory_hash     TEXT,
    config_json     TEXT DEFAULT '{}',
    owner_id        TEXT,
    source          TEXT DEFAULT 'import',
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX idx_ai_agents_main ON ai_agents(is_main) WHERE is_main = 1;
CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_oc_id_unique ON ai_agents(oc_agent_id);
CREATE INDEX idx_ai_agents_slug ON ai_agents(slug);
CREATE INDEX idx_ai_agents_active ON ai_agents(is_active) WHERE is_active = 1;
CREATE INDEX idx_ai_agents_owner ON ai_agents(owner_id);
```

**Fields:**
- `id` = UUID, immutable. Used for all internal routing, folder names on disk, and sync operations.
- `name` = display name, user-changeable. Cosmetic only - never used for routing or file paths.
- `slug` = slugified name for URLs/references.
- `workspace_path` = directory under `.pane/system/`. **Uses the agent UUID** (e.g., `E170EB0D29556C08E6593D3CC4DC4F58`), not the display name and not the slug. This means renaming an agent never breaks file paths or sync. **This holds for every agent regardless of `source`** - `import`, `onboarding`, and `ai_created` agents all use a UUID workspace folder. There is no carve-out for the onboarding or locally-created path: a non-OC agent created during onboarding uses a UUID folder identical to an imported OC agent. (Enforced v0.18.3; creation-flow ordering in §24.10. #1535.)
- `oc_agent_id` = the agent's ID in OpenClaw config (e.g., `"main"`, `"coder"`, `"researcher"`). Set during onboarding when agents are discovered from the gateway. Used by the bridge plugin to map Pane agents to OC workspace directories. NULL for Tier 1/2 agents (no OC connection).
- `is_main` = strategy agent (one per user).
- `source` = `'import'` | `'onboarding'` | `'ai_created'`.
- `config_json` = tier-specific overrides.

**On `idx_ai_agents_oc_id_unique`.** No two rows may share an OC agent identity. This is load-bearing in two places: it is what makes §24.5.5's overwrite-in-place match on `oc_agent_id` resolve to exactly one row, and it is what makes §24.7C's nullity discriminator unambiguous. Locally-created agents write `oc_agent_id = NULL` and are unaffected - SQLite treats NULLs as distinct under a unique index, so any number of local agents coexist.

**Routing rule:** All sync operations (push, pull, sync-all) use `oc_agent_id` when communicating with the gateway, and `id` (UUID) for local file paths. Display `name` is never used for routing. This eliminates the fragility of name-based routing - renaming an agent in the UI has zero impact on sync or file storage.

**ADR-0063 (runtime-per-agent) schema motion, effective at its build:** `oc_agent_id` → `runtime_ref` (rename; OC rows keep their values, locals stay NULL); new `runtime_type TEXT` (`'openclaw' | 'openrouter' | 'anthropic' | 'ollama' | 'venice'`; `'venice'` added by ADR-0065 D1, 2026-08-29; NULL only as the migration's legacy-unpinned state, pin-on-first-selection); `idx_ai_agents_oc_id_unique` → `idx_ai_agents_runtime_unique UNIQUE(runtime_type, runtime_ref)`; `is_main` → `is_default` with `idx_ai_agents_default`. `model` is required at creation for API-runtime agents (dispatch value) and remains the synced display value for OC agents (#1453, per-agent form — ADR-0063 D9). Full migration order: ADR-0063 D4. The domain is enforced in Rust (`resolve_runtime_type_for_upsert` and the local-create path), not by a CHECK constraint; every site that enumerates the domain must carry all five values (ADR-0065 session notes, entry-point census). Venice agents are local (`runtime_ref` NULL, `model` required) and are never ambiently inferred; `'venice'` is always supplied explicitly at creation.

### 24.3 Filesystem Structure
> **Status:** BUILT v0.18.3 - All code paths now use workspace_path (UUID) for folder operations
> **Updated:** 2026-04-07 - All 11 name-based path bugs fixed
> **Amended:** 2026-05-28 - sync allowlist cross-reference added; sub-agent folder structure corrected to UUID-based paths; memory/ and logs/ depth-1 rule pinned (ADR-0041)

```
~/Pane/.pane/system/
├── E170EB0D29556C08E6593D3CC4DC4F58/  # Strategy agent (main) - "Brit Freed"
│   ├── SOUL.md                 # REQUIRED - synced
│   ├── MEMORY.md               # REQUIRED - synced
│   ├── AGENTS.md               # optional - synced
│   ├── USER.md                 # optional - synced
│   ├── IDENTITY.md             # optional - synced
│   ├── TOOLS.md                # optional - synced
│   ├── HEARTBEAT.md            # optional - synced
│   ├── memory/                 # depth-1 only - synced (flat *.md)
│   │   ├── 2026-05-28.md
│   │   └── 2026-05-27.md
│   └── logs/                   # strategy agent ONLY - synced (flat *.md)
│       ├── 2026-03-27.md
│       └── 2026-03-26.md
├── {uuid}/                     # Sub-agent (e.g. Stia, Reed, Lane) - UUID-based path
│   ├── SOUL.md                 # REQUIRED - synced
│   ├── MEMORY.md               # REQUIRED - synced
│   ├── IDENTITY.md             # optional - synced
│   ├── AGENTS.md               # optional - synced
│   ├── USER.md                 # optional - synced
│   └── TOOLS.md                # optional - synced
└── ...                         # additional sub-agents follow same structure
```

**Sync scope note (ADR-0041):** Only the files marked `synced` above cross the Pane↔OC wire. All other files that may exist in an OC workspace (`archive/`, `scratchpad-*.md`, temp files, backup archives, `memory/` subdirectories at depth ≥ 2, etc.) are **not** synced. Their presence on disk is not a desync - sync scope ⊊ persistence scope by design. See `docs/spec/sync.md` §22C.8.5 for the full allowlist and three-surface enforcement spec.

**`memory/` and `logs/` depth-1 rule:** These directories sync flat (`*.md` at depth 1) only. Nested subdirectories (e.g. `memory/archive/note.md`) do not cross the wire. This is a contract with OC's dreaming reader, not a performance heuristic - see ADR-0041 §Decision 6.

Daily logs: strategy agent only. Sub-agents are ephemeral - output summaries go to the strategy agent's memory.

### 24.4 No "Pane System" in Sidebar
> **Status:** NOT BUILT

No "Pane System" project in `projects` table. No agent-related entries in `projects`. Sidebar shows only user projects. Agent management lives exclusively in Settings → Agents.

### 24.5 Import Behavior
> **Status:** PARTIAL. Discovery + identity-file fetch via gateway BUILT through v0.65.5; redesigned read-only display NOT BUILT (sub-tasks #883, #275 - display-only Your Team).
> **Updated:** 2026-05-07 - rewritten for onboarding redesign (#953) and gateway architecture (§22C cutover, 2026-04-27).

When Pane completes pairing with an OpenClaw gateway during onboarding (`docs/spec/onboarding.md` §6.5, `docs/spec/sync.md` §22C.22), it discovers and imports OC's existing agents into Pane's local model. The import is automatic - there is no manual "Import from OpenClaw" step, and the customer does not choose which agents to import or which agent is the main agent.

#### 24.5.1 Discovery and import - gateway endpoints

The flow uses gateway endpoints (the bridge equivalents are retired per §22B.2 / §22C.16):

1. **Initial sync - `GET /v1/sync/initial`** (per §22C.6.2 / §22C.22). Pulls the whitelisted identity files (per `docs/spec/onboarding.md` §6.5.4) for every agent OC knows about. Returns content grouped by agent. SameMachine: with a symlink in place, the on-disk files are typically already present; initial sync is a state-machine-consistency call (it surfaces missing-file conditions deterministically). SplitMachine: this is where the files actually arrive.

2. **Upsert into `ai_agents`.** For each discovered agent: insert a row in `ai_agents` if not present (keyed on agent identity from `IDENTITY.md` / `openclaw.json`'s agent registry); update if present. The `is_main` flag is set per OC's main-agent designation (see §24.5.2). The `workspace_path` is the agent's directory name under `.pane/system/`, matching OC's workspace layout.

3. **Filesystem materialization.** `.pane/system/{workspace_path}/` directory is ensured to exist. Whitelisted files (per `onboarding.md` §6.5.4) are written if missing or stale. SameMachine: symlink already provides this; the integrity check in §24.0 confirms. SplitMachine: gateway-side push from initial sync writes the files atomically (per §22C.6.2 atomic-write invariant + §2.3A).

The previous bridge-endpoint flow (`GET /agents` + `POST /identity` per §22B.2) is retired. No new code references those endpoints; existing code paths that still reference them are tracked for migration per §22C.16.

#### 24.5.2 Main agent - determined by OC config, not user selection

In the redesign, the main agent is determined by OC's configuration on the OC side, not by the customer's selection during Pane onboarding. The "Your Team" screen in onboarding (§6.5.5) is **display-only** - it shows the customer who their main agent is and what other agents are available, with no picker UI.

Source of truth for "which agent is main":

- OC's `openclaw.json` / agent registry designates the main agent.
- Pane reads this designation through the gateway's agent-list / initial-sync responses.
- Pane writes `is_main = 1` for the matching row in `ai_agents` (and `is_main = 0` for the others).

The pre-redesign UI element - a radio-button picker on the "Your Team" screen letting the customer choose the main agent - retires (sub-task #883). Customers who want to change the main agent are directed to edit OpenClaw's config:

> Help text on the Your Team screen and in Settings: "To change main agent, edit OpenClaw config."

This applies to both onboarding and Settings. Sub-task #275 ensures Settings does not surface a model picker for OC users either (the model is OC's choice; Pane displays it read-only).

#### 24.5.3 Re-discovery on subsequent sessions

On every Pane launch, §24.0's startup-integrity-check rediscovers agents from `.pane/system/{agent_uuid}/`. This is a filesystem-level operation, independent of gateway availability - it works offline (consistent with ADR-0024 local-only decryption). Gateway-side syncing of *changes* is handled by the push/pull endpoints in §22C.6.2; agent *existence* on Pane is a filesystem fact.

If a new agent appears on the OC side mid-session (e.g., the customer adds an agent via OC's config + restart), Pane discovers it on the next gateway sync push or on the next launch's integrity check, whichever comes first. Same-machine watchers (§22C.11.3) catch this faster.

#### 24.5.4 Removal of agents

When an agent is removed on the OC side, the corresponding `.pane/system/{workspace_path}/` directory is **not** auto-deleted by Pane. The integrity check sets `is_active = 0` on the `ai_agents` row, hiding the agent from chat surfaces, but the files remain on disk for the customer to inspect or archive. Manual cleanup is via Settings (post-launch sub-task #11 covers the workspace-archive re-trigger UI).

This aligns with Pane's local-first principle (ADR-0007): the customer's data is the customer's; Pane does not silently delete files that were once theirs. OC and Pane disagree on agent existence is acceptable in the user-content layer; the divergence is surfaced for resolution in Settings, not auto-resolved.

#### 24.5.5 Provider-switch re-import (Settings)
> **Status:** NOT BUILT - #956, v1.0 Launch milestone
> **File:** `src-tauri/src/ai/commands.rs` (`reimport_oc_agents`, `check_oc_agent_collisions`), `src-tauri/src/sync_client.rs` (`sync_re_import`, `fetch_gateway_agents`), `src/lib/components/Settings.svelte` (`handleSaveAi`)

⚠ **Mode framing superseded at ADR-0063's build (2026-08-16).** This section's trigger — a provider *change* to OpenClaw — assumes the global provider mode, which ADR-0063 retires: runtimes connect once and stay live, so there is no switch to fire on. **What survives verbatim as the OC connection's connect/refresh semantics:** the two-phase command split (read-only `check_oc_agent_collisions` → confirmed `reimport_oc_agents`), overwrite-in-place via UPDATE-never-DELETE, coexist + disambiguate on slug collision (numeric-increment-until-free), fresh import, the warn-and-confirm modal, and the `notes.agent_id` never-broken invariant. **What retires with the mode:** the trigger condition, the reverse-switch selection contract below (picker/forced-create on →API — no such transition exists), and the §22C.11A.4 key wipe's switch trigger (re-scoped to explicit OC disconnect). Until ADR-0063's build, this section governs as written, over ADR-0062's bindings: a class switch strands out-of-class bound sessions read-only (sessions.md §30.1.3) — it never re-binds them.

§24.5.1 covers agent discovery and import at onboarding time, via the `sync_initial_import` path. This sub-section covers the **Settings-time counterpart**: when a customer who onboarded on a non-OC provider (OpenRouter, Anthropic, Ollama) later switches the AI provider to OpenClaw in Settings, Pane must discover and import the gateway's agents - the onboarding path never ran for them.

**Trigger.** The import fires only on a provider *change* to OpenClaw - `provider === "openclaw" && savedProvider !== "openclaw"` in `handleSaveAi`, after OC connectivity verification succeeds and before the save is considered complete. Re-saving Settings with OpenClaw already the active provider does NOT re-trigger import.

**Two-phase command split.** Discovery and mutation are separate Tauri commands:

1. **`check_oc_agent_collisions(url, token)` - read-only.** Fetches the gateway agent list and compares each gateway agent's `oc_agent_id` against existing `ai_agents` rows. Returns `{ collisions: [...], new_agents: [...] }`. No DB mutations, no file writes. This is the read half of the warn-then-confirm flow.
2. **`reimport_oc_agents(url, token)` - mutating.** Executes the import (called only after the customer confirms, or directly when there are no collisions). This is the write half.

The split is deliberate: the read phase informs the modal decision; the write phase runs only on confirmed intent. (Matt, 2026-06-15.)

**Re-import semantics - `sync_re_import`, distinct from `sync_initial_import`.** Per discovered gateway agent:

- **Existing agent (`oc_agent_id` matches an existing `ai_agents` row): overwrite in place.**
  - Reuse the existing row's `workspace_path` - do NOT generate a new UUID, and do NOT derive the path from the gateway's `workspace` field. `workspace_path` is the local agent UUID directory under `.pane/system/` (§24.2); it is read from the pre-queried existing row. Gateway files are written INTO that existing directory.
  - **UPDATE the existing row in place** (`UPDATE ai_agents SET name=?, slug=?, updated_at=? WHERE id=?`). **NEVER DELETE+INSERT.** `notes.agent_id` references `ai_agents.id` with no FK cascade - a delete would orphan notes rows. Agent rows are updated, never deleted, on re-import.
  - **All files are overwritten, including `IDENTITY.md`.** This is a **deliberate divergence from `sync_initial_import`**, which preserves `IDENTITY.md` via an `!exists` guard (appropriate at first-time onboarding, where no prior identity exists to protect). `sync_re_import` carries no such guard: on a provider switch *back* to OpenClaw, the gateway is authoritative for identity too. A local `IDENTITY.md` that differs from the gateway's is stale by definition - the agent moved on in another setup, and the gateway copy is current. Preserving the local copy would reintroduce the staleness that the re-sync exists to correct. This divergence is intentional and documented here precisely so it is not flagged as an inconsistency at audit. (Matt, 2026-06-15.)

- **No `oc_agent_id` match, but the incoming slug collides with an existing row: coexist + disambiguate.**
  - This is the **default path for every non-OC provider**. Onboarding always creates one main agent (`ensureMainAgent` → `upsert_agent`) with `oc_agent_id = NULL` and a slug derived from the agent name (§24.2 slugify: lowercase, hyphens). When the customer later switches to OpenClaw, the incoming OC main agent slugs to the same value (its name, or its OC agent id when the name is null, run through the identical slugify), and the slugs collide. This happens for Anthropic, OpenRouter, Ollama, and any future provider - the rule is keyed on **slug equality with any existing `ai_agents` row** (including `oc_agent_id IS NULL` rows), not on which provider the customer came from.

  > **#2629 fix note (D7):** prior to this fix, `ensureMainAgent`'s `upsert_agent` call passed no `runtime_type` and no `model`, so the INSERT branch always fell back to `resolve_runtime_type_for_new_local_agent`'s ambient keychain inference (tie-breaking to `openrouter` whenever both an Anthropic and an OpenRouter key were present in the keychain) instead of the provider the customer actually just connected and validated (`connectProvider`) — and the chosen model was written only to the retired `ai_settings.model` scalar (dead since #2585/D8, never read by dispatch), leaving `ai_agents.model` NULL. `CreateAgentRequest` gained a new `runtime_type: Option<String>` field; `upsert_agent`'s INSERT branch now calls new helper `resolve_runtime_type_for_upsert(conn, &request.runtime_type)`, which uses an explicit `Some(valid_value)` directly (validated against the known runtime types: four at #2629, five with `'venice'` under ADR-0065, erroring on anything else) and skips ambient inference entirely, falling back to the original `resolve_runtime_type_for_new_local_agent` behavior only when `None` — preserving every other existing caller unchanged. `ensureMainAgent` now passes `runtime_type: connectProvider` and `model: connectModel || selectedComplexModel` directly on the `upsert_agent` request, so the main agent's `runtime_type` and `model` are set correctly at creation, in one write, with no dependence on keychain state or a second dead-key write path. (Relates to #2629.)

  > **#2649 guard note (D19 closure, PR 3C):** `upsert_agent` rejects `runtime_type='openclaw'` with an error before any write — before the directory/`IDENTITY.md` creation and before the `ai_agents` INSERT/UPDATE. OpenClaw agent rows are created exclusively by `sync_initial_import` and `sync_re_import` (both INSERT `ai_agents` directly with `runtime_ref` set); `upsert_agent` never sets `runtime_ref`, so a caller reaching it with `runtime_type: 'openclaw'` would otherwise mint a `runtime_ref=NULL` phantom row — the exact shape that caused D19 (#2649). This guard mechanically prevents that shape from ever landing, regardless of whether a future call site (re)introduces the bad call. `ensureMainAgent` is unaffected: it only ever passes `runtime_type: connectProvider`, which is never `'openclaw'` (OpenClaw is not a `connectProvider` choice on the local-agent creation path).
  - **The two agents coexist as separate rows.** The pre-existing row is kept **entirely untouched** - no UPDATE, no overwrite, no file changes, no `oc_agent_id` adoption. The incoming OC agent imports as its **own new row** (new UUID, new `.pane/system/{uuid}/` directory, all files including `IDENTITY.md`, INSERT), exactly like a fresh import - except its slug is disambiguated to avoid the `slug` UNIQUE constraint.
  - **Disambiguation - numeric-increment-until-free.** The incoming OC agent's slug is its base slug. If the base slug is already used in `ai_agents`, append `-N` starting at `2` and increment (`-2`, `-3`, ...) until the slug is unused in `ai_agents`. (Example: base `pane` taken → try `pane-2`; if taken → `pane-3`; ... first unused wins.) This is collision-proof by construction regardless of provider-switch history or agent count - unlike a fixed suffix (e.g. `-oc`), which re-collides if the customer switches providers more than once. Slugs are internal plumbing; the customer sees the agent **name** (unchanged from the gateway), not the slug, so the cosmetic cost of the numeric suffix is acceptable. (Matt, 2026-06-15.)
  - **No rollback on this collision type.** A slug collision against an existing row is a *handled, expected* condition on the default non-OC→OC switch path, not a failure. It does NOT roll back the transaction; the incoming agent imports with its disambiguated slug and the batch continues.

- **New agent (`oc_agent_id` has no existing-row match AND no slug collision): fresh import.**
  - Generate a new UUID, create a new `.pane/system/{uuid}/` directory, write all files (including `IDENTITY.md` - a net-new agent has no prior identity to protect), and INSERT a new `ai_agents` row. Same logic as `sync_initial_import`. (The disambiguation step above is a no-op here - the base slug is already free.)

The whole operation is wrapped in a transaction. Rollback applies **only to genuine unexpected failures** - a DB error other than the now-handled slug collision, a file-write failure, or a network failure mid-batch. A slug collision against an existing row is **not** a failure and does **not** roll back: it is handled by the coexist + disambiguate rule above (the incoming agent imports with a disambiguated slug and the batch continues). On rollback for a genuine failure, DB state reverts cleanly; files already written to earlier agents' directories within the batch are not removed by the DB rollback (SQLite transactions do not span the filesystem) and self-correct on the next successful re-import.

**Warn-and-confirm modal (collision path).** If `check_oc_agent_collisions` returns ≥1 collision, Pane shows a modal before any mutation:

> "Switching to OpenClaw will overwrite your local agent files with the gateway's versions. Local edits will be lost. [Cancel] [Overwrite]"

The modal does NOT enumerate the colliding agents - warning copy only (Matt, 2026-06-15). On **Overwrite**, Pane calls `reimport_oc_agents`. On **Cancel**, Pane aborts the entire switch: the `provider` dropdown reverts to `savedProvider`, `ai_configure_and_save` is NOT called, no DB state is modified, and no file is written. The customer is returned to the exact pre-Save state with no error message - they chose to cancel. There is no partial save and no half-applied config.

**No-collision path.** If `check_oc_agent_collisions` returns zero collisions (all gateway agents are net-new), no modal is shown - Pane imports silently via `reimport_oc_agents` as part of the save.

**Invariant - `notes.agent_id` references are never broken.** Across both overwrite-in-place and fresh-import, no existing `ai_agents.id` is ever deleted or reassigned. The link between a note and its agent survives re-import unconditionally.

**Reverse switch - main-agent selection (→ API provider).** Switching *to* OpenClaw demotes the existing main and promotes the OC main agent (above; #956 - OC is authoritative on `is_main` while active). The reverse switch - provider changed *away* from OpenClaw to an API provider - runs no agent re-import, but it MUST re-target `is_main` onto an agent usable under the new provider (§24.7C).

> **⚠ Superseded (2026-07-24, #2030 / ADR-0059).** This paragraph previously specified *automatic* restore of "the unique non-OC agent (`oc_agent_id IS NULL AND is_active = 1`)." That rule assumed the non-OC agent set was a singleton by construction - an assumption #2030's local agent creation removes. **The automatic single-target restore is retired on this path and MUST NOT run alongside the selection behavior below.** `restore_non_oc_main`'s prior role ends here; the code PR retires the call rather than extending it.

The reverse switch now resolves by explicit selection:

| Local agents present (`oc_agent_id IS NULL AND is_active = 1`) | Behavior |
|---|---|
| **Zero** | Forced create flow (§24.10.1). There is no fallback into OC agents - an OC agent is not usable under an API provider (§24.7C). |
| **One or more** | Picker over local agents, with "create new agent" always offered. Default selection is the most recently created usable agent (`created_at DESC`). |

**Commit ordering.** The provider switch does not commit until the selection resolves - until an agent is picked, or until the create flow reaches name-commit (§24.10.1). Until then no provider value is written and no gateway-key wipe (§22C.11A.4) runs, so a cancelled selection leaves the customer on their previously-saved provider with OC credentials and agents intact. Once resolved, the agent promotion and the provider write commit together, then `pane-agent-switched` dispatches (§24.7B).

Every `is_main` change on this path remains a single demote-then-promote transaction (§24.7A, ADR-0045 D2): the current `is_main = 1` row is demoted and the selected row promoted atomically, never as two independent UPDATEs. If the transaction fails, `is_main` is left unchanged, the error is surfaced, and the provider switch does not commit; there is no partial flip.

The full selection contract is specified in §24.7A, §24.7C, and ADR-0059.

### 24.6 Settings → Agents UI
> **Status:** BUILT v0.68.64 — #2030 PR B (two-group view, create overlay, picker modal, set-main); OC-gate BUILT — #2392 (disabled button + note + OC empty-state)  
> **spec-covered-by:** #2030, #2395

**Data loading discipline.** Settings tab data loaders (agents, templates, logs) use a `loaded` flag guard — not array-length checks — to prevent reactive loops when the backend returns empty results. The `$effect` fires once on initial tab open; subsequent visits short-circuit on the flag. Mutations (create, delete, rename) call the load function directly rather than resetting the flag. **Cross-component refresh:** `ProviderConnectionsPanel` fires an `onAgentsChanged` callback after `handleConnectionSaved` completes (OC import, provider add/disconnect) so Settings can reset `agentsLoaded`, triggering the `$effect` re-fetch — the panel has no direct access to Settings' agent state. `onChange` (the callback passed to every `ProviderConnectionCard`, covering card-level mutations including disconnect) triggers the same refresh via `handleCardChange`, not just `handleConnectionSaved`. (D24 #2649, PR #2263, #2261; D25 #2649.)

**List view (as built, ADR-0063 Stage 6; extended by ADR-0065):** Agents are grouped by `runtime_type` in display order (`RUNTIME_TYPE_ORDER`): **OpenClaw**, **OpenRouter**, **Anthropic**, **Ollama**, **Venice**. The grouping predicate is the `runtime_type` column, not `runtime_ref` nullity and not `source`; `source` is provenance metadata whose `recovered` value can belong to any group. An agent whose runtime is disconnected is shown disabled rather than hidden (§24.7C successor rule); a deactivated agent (`is_active = 0`) is excluded from the tab entirely (`activeAgentsList`, Settings.svelte). *(The pre-Stage-6 text here described two groups keyed on `oc_agent_id` nullity; that grouping shipped out with the global provider mode.)*

**Model is editable post-creation (#3074).** A second inline pencil next to the model text (mirroring the rename pencil) opens a `SearchableModelSelect` picker, gated by `isAgentModelEditable(agent)` (`src/lib/utils/agentDisplay.ts`) — true only for `anthropic` / `openrouter` / `venice` / `ollama`. Picking a model saves immediately via `update_agent({ agent_id, model })` and takes effect on every session bound to that agent starting the next dispatch (ADR-0063 D1 — `resolve_runtime_config_for_agent_id` runs a fresh, uncached `SELECT` per call). `runtime_type = 'openclaw'` rows stay read-only: `ai_agents.model` there is a write-target for `write_back_agent_model_if_changed`'s post-dispatch cache, not a dispatch-time input the OC gateway honors (D9), so a manual edit would be dispatch-inert and then silently reverted on the next clean turn. `runtime_type = NULL` (legacy-unpinned, D4.3) rows also stay read-only — there is no runtime yet to fetch a model list from.

No status indicators. The static "Active" green dot was removed (v0.67.100, PR #1953) because it showed every agent as active regardless of real state; real live-status display deferred to post-launch.

**Creating agents.** Local agent creation ships in v1 (#2030, §24.10.1) - the earlier "import only, AI-assisted creation is future" scope is superseded. Agents are created either from this list or by the forced create flow on a provider switch to an API provider with no local agents (§24.5.5).

**The "+ New agent" control is gated by the active provider (#2392).**

- **Active provider is an API provider** (OpenRouter / Anthropic / Ollama / Venice): the control opens the local create flow directly at the name step - one step, name-first (§24.10.1). Unchanged. *(Verified for Venice at #2795: `AgentCreateOverlay`'s runtime-picker loader admits any `connected && runtime_type !== 'openclaw'` row from `listRuntimeConnections()` — the filter is runtime-agnostic, not a named enum, so a connected Venice runtime reaches Step R exactly like the other three with no code change required.)*
- **Active provider is OpenClaw:** the control is **disabled - visible, not hidden** - and does not open the create flow. A short note accompanies it. The exact copy is Design's; the note must convey two facts: (1) OpenClaw agents are created in OpenClaw and imported automatically (§24.5.5), and (2) to create a Pane-local agent, the customer switches to an API provider in the AI tab.

**Why disabled rather than name-first-then-disabled-agent.** A locally-created agent has `oc_agent_id IS NULL` and is unusable while OpenClaw is active (§24.7C); OpenClaw agents are gateway-minted, not Pane-authored (#956). Opening the create flow on OpenClaw would therefore produce an agent that is disabled the moment it lands - a dead-end. Disabling the control instead, with the note, redirects both intents: a new OpenClaw agent is made in OpenClaw and imported; a new local agent requires switching provider first. The disabled control mirrors §24.7C's disabled-not-hidden grammar for agents outside the active class.

**Boundary.** This gate keys only off the active provider. It does not touch the forced create flow reached via a provider switch (§24.5.5 / §24.10.1), which is unchanged - including the pending-credentials skip that suppresses a second key prompt on that path (#2268). No local agent is created while OpenClaw is the committed active provider outside a provider-switch flow - the forced-create and picker-"create new" paths both require an in-progress switch and apply the D3 / §24.5.5 contract, not this gate.

**No deletion or archiving in v1.** The agent list only grows. System files persist on disk. This follows §24.5.4's existing posture: Pane does not delete files that were once the customer's (ADR-0007). Agents removed OC-side are deactivated (`is_active = 0`), not removed.

**Detail view:** Tabs for Soul, Memory, Logs (main only), Settings. Files are AI-managed (read-only in UI per v11.21.0). Changes save to filesystem, hash updated in DB.

### 24.7 Context Loading
> **Status:** Amended 2026-08-16 (ADR-0062 D7)

No change to §16 model. On a chat path, resolve the **session's bound agent** (`chat_sessions.agent_id` via the dispatching thread — sessions.md §30.1.3/§30.7.1) → read files from `.pane/system/{workspace_path}/` → inject into system prompt per Appendix A. Where no session is in scope (background surfaces), resolve the designated default: phase 1 `is_main = 1 AND is_active = 1`; ADR-0063 `is_default = 1 AND is_active = 1`. **No new ambient default lookup may be added on a session-scoped path** (INVARIANTS, "Agent Identity").

### 24.7A Main-Agent Selection

> **Status:** BUILT v0.68.64 — #2030 PR B (`set_main_agent` command, single-tx demote-then-promote; "Set as Main" button in Settings → Agents)

⚠ **Scope narrowed (2026-08-16, ADR-0062 D6) and renamed at ADR-0063's build (D3).** Changing the flag no longer changes who answers any existing session — bindings are immutable (§30.1.3). Phase 1, the flag governs exactly: (a) the default binding for new sessions and the sessionless background surfaces (ADR-0062 D7), and (b) the ADR-0059 D3 switch-path selection while the global mode lives. At ADR-0063's build: the column renames `is_main` → `is_default` (index → `idx_ai_agents_default`), "Set as Main" becomes **"Set as Default"** offered for any active agent of any runtime (the class gate retires with §24.7C), OC import **never re-targets the default** (ADR-0045 D3 retired — the flag is a Pane-side preference), and path (b) ceases to exist. The demote-then-promote single-transaction shape (ADR-0045 D1/D2) is preserved verbatim under the new name, including the deactivation re-target rule (ADR-0063 D3).

`is_main` is a single global flag (§24.2: `idx_ai_agents_main` permits one `is_main = 1` row). It changes only via **demote-then-promote in a single transaction** - the current main is demoted and the new main promoted atomically, never as two independent UPDATEs. The flag is global; the *selection rule* is provider-aware: `is_main` must always point at an agent usable under the active provider (§24.7C).

Three paths exercise this:

1. **Selection on provider switch to an API provider** - §24.5.5. Resolves by picker (local agents present) or by the forced create flow (none present). Supersedes the automatic single-target restore of ADR-0045 D4 (#2030 / ADR-0059).
2. **OC-authoritative promotion on provider switch to OpenClaw** - §24.5.5. Unchanged (#956, ADR-0045 D3).
3. **Manual "Set as Main"** - below. The durable user-facing mechanism, scoped to agents usable under the active provider.

While `provider = openclaw`, OpenClaw is authoritative on `is_main` (#956): switching to OC re-claims main, and a manual off-OC choice does not survive a switch to OC - auto-restore re-applies it on the next switch back.

#### Switch Mechanism

In Settings → Agents (§24.6), selecting an agent offers:
- **"Set as Main"** - promotes this agent via the demote-then-promote transaction above.

**Candidate set.** The control offers only agents usable under the active provider (§24.7C); agents of the other class are shown disabled and cannot be promoted. Under OpenClaw the gateway remains authoritative and the control is not offered (ADR-0045 D3).

> **⚠ Supersedes ADR-0045 D4 and D5 (2026-07-24, #2030 / ADR-0059).** D4 identified the off-OC main as "the unique non-OC agent," and D5 deferred a persisted selection signal until agent creation could produce ≥2 non-OC agents. #2030 is that trigger. The candidate set is no longer size 1, and selection is now explicit rather than derived - the picker is the mechanism D5 anticipated. No persisted per-provider main signal is introduced: the picker defaults to the most recently created usable agent (`created_at DESC`), which needs no new column. ADR-0045 D1, D2, and D3 are preserved unchanged.

### 24.7B Main-Agent Display Refresh on Provider Switch
> **Status:** BUILT (#1543)

⚠ **Narrowed (2026-08-16, ADR-0062 D6/D9).** Session chat headers display the **bound agent** (sessions.md §30.1.3) and do not listen for `pane-agent-switched` — a session's agent cannot change. The event remains the refresh signal for **default-agent display surfaces** (Settings → Agents, and any surface showing "the default"), firing on every default re-target (manual Set-as-Default; phase 1 also the §24.5.5 switch paths and OC re-import promotion, until ADR-0063 retires those). The ChatPanel listener described below applies only where ChatPanel renders a sessionless/default-following surface; per #2536, Settings itself hosts no chat.

§24.7A governs which agent is main after a provider switch. This subsection governs the display side: how an open chat surface reflects the new main agent in place, without navigation or reload.

**Contract.** When a provider switch completes, Settings dispatches the window event `pane-agent-switched`. It fires on both transitions:
- **→ OpenClaw:** after the OC re-import completes - the import that promotes the OC main agent (§24.5, §24.7A).
- **→ non-OC:** after the prior non-OC main is restored (the auto-restore of §24.7A / #1492).

ChatPanel listens for `pane-agent-switched`. On receipt it re-queries the current main agent (`is_main`) and updates the chat header in place - no navigation, no reload. Because ChatPanel re-reads main-agent state rather than relying on event payload, the header reflects the true main regardless of which direction fired.

**Implementation note (BUILT #1543).** The event is dispatched from every provider-switch completion path in Settings - `confirmCascadeAndSave`, `handleTestConnection`, `handleSaveAi`, and `confirmReimport` - so no switch path can leave the header stale.

### 24.7C Provider-Scoped Agent Usability
> **Status:** SPEC'D - #2030. Supersedes ADR-0045 D4/D5.

⚠ **Superseded at ADR-0063's build (2026-08-16).** Class-scoped usability exists only while the global provider mode does. Under runtime-per-agent there is no active class: an agent is permanently bound to its runtime (`runtime_type`, pinned at creation; `runtime_ref` for registry-bearing runtimes — ADR-0063 D1) and every session dispatches via its own agent's runtime (§30.7.1). "Usable" reduces to: agent `is_active = 1` **and** its runtime connected. The disabled-never-hidden render rule survives for those two conditions. Until ADR-0063's build, this section governs as written and ADR-0062's phase-1 picker offers in-class agents only.

An agent is usable only under the provider class it belongs to. The discriminator is `oc_agent_id` nullity, not `source`:

- **OC-imported agents** (`oc_agent_id IS NOT NULL`) are usable only while `provider = openclaw`.
- **Locally-created agents** (`oc_agent_id IS NULL`) are usable only while the active provider is an API provider (OpenRouter, Anthropic, Ollama).

An agent outside the active provider's class is **rendered disabled - visible but not selectable - never hidden**. The customer can always see their full agent roster; they simply cannot animate an OC-imported agent with an API model, or vice versa.

The nullity test partitions the table cleanly because `oc_agent_id` is constraint-backed: `idx_ai_agents_oc_id_unique` (§24.2) guarantees no two rows share an OC identity, while locals write NULL and coexist freely.

`is_main` therefore always points at an agent usable under the active provider. Every provider switch re-targets it (§24.5.5) via the demote-then-promote transaction of §24.7A. The single global `is_main` flag is unchanged (ADR-0045 D1); it is the selection rule that is provider-aware, not the data model.

**Why this replaces the prior behavior.** Before #2030, an OC-onboarded customer who switched to an API provider kept their OC agents and animated them with the API model. That path is closed: an agent's identity files, memory, and provenance belong to the runtime that created them, and borrowing them across the boundary produced an agent that was neither one thing nor the other, with identity and memory files silently mismatched against a model that never wrote them. The `oc_agent_id IS NULL` restore signal of ADR-0045 D4 assumed such an agent was unique; local creation (§24.10.1) makes it one of many, and D4 is superseded accordingly.

### 24.8 Agent Tracking on Chat Messages
> **Status:** SPEC'D - in v1 scope per #2030 / ADR-0059 D5. Table name corrected: the live messages table is `chat_messages_local`, not `ai_chat_messages`.

Under provider-scoped agent usability (§24.7C) a thread can contain messages written by an agent that is not usable under the active provider. Without per-message attribution those messages render under whichever agent is currently main, which misattributes them - and re-misattributes them on every subsequent provider switch.

```sql
ALTER TABLE chat_messages_local ADD COLUMN agent_id TEXT REFERENCES ai_agents(id);
```

**Semantics.** Nullable. Stamped at write time on **assistant messages only** - user messages carry no agent attribution. **The stamp source is the session binding (ADR-0062 D4):** `save_chat_message` resolves `SELECT agent_id FROM chat_sessions WHERE id = ?thread_id` — never the ambient `is_main`/`is_default` row. Because the binding is immutable (§30.1.3), save-time and dispatch-time agree by construction. Every assistant message written after this column ships carries an explicit `agent_id`; no live write path leaves it NULL. A deactivated bound agent still stamps — correct provenance.

*(NULL-legacy paragraph, Render, Rename-relabels-history, No-backfill, Per-device paragraphs, and §24.8.1: unchanged, preserved verbatim.)*

**`NULL` means "written before attribution shipped." It does not mean "the main agent."** Defining NULL as "main agent" - the prior text - would silently re-attribute every historical message each time the customer switches providers, which is precisely the failure this column exists to prevent.

**Render.** Each assistant message displays its stamped agent. Messages with `agent_id IS NULL` render under the current agent - the accepted limitation for pre-existing history (ADR-0059 D5).

**Rename relabels history — intended.** The stored fact is `agent_id`, not a name; the display name is resolved at render via a join to `ai_agents`. Renaming an agent therefore relabels that agent's entire message history, retroactively, everywhere it appears. This is intended: it is the same agent under a new name, and the alternative — freezing a name string onto each historical message — would make a rename produce a thread narrated by two names for one agent. Name is presentation; `agent_id` is identity. (This is distinct from the NULL case above: a re-stamp on rename changes *which name renders for a known agent*; it never assigns an agent to a NULL row.)

**No backfill.** Pre-existing rows are not populated. Which agent wrote a historical message is unknowable from the data, and inventing an answer would be fabricated provenance in the one field whose purpose is provenance. The model that answered is already persisted per message (`model TEXT`) and remains the durable per-message fact; the agent stamp is added alongside it.

**Per-device, not synced.** `chat_messages_local` is not a CRR table (`migrations.rs:1370`, "Per-device table"), so `agent_id` is per-device state and does not propagate between devices. This matches ADR-0022's per-device precedent and v1's single-machine posture (onboarding §6).

#### 24.8.1 Migration - adding `agent_id` to `chat_messages_local`
> **Status:** SPEC'D - #2030. First schema change under the migration policy adopted 2026-07-24 (external testers exist; no DB nuking).

`SCHEMA_VERSION` **75 → 76**. Plain additive `ALTER` on the numbered migration path - `chat_messages_local` is not a CRR table, so no CRR-aware path and no `CRR_TABLES` change is involved.

**Contract:**

1. **Purely additive.** A single nullable column add. The migration MUST NOT drop, recreate, rename, or copy-and-swap `chat_messages_local`. Existing message rows survive unchanged.
2. **No default, no backfill.** Pre-existing rows take `NULL`, which is the correct migrated state and maps to the documented render limitation (§24.8).
3. **Forward-only.** No down-migration. A NULL column is inert to every reader that does not know about it.
4. **Chat history is customer data.** The discipline INVARIANTS applies to `ai_settings` ("must NEVER be dropped, recreated, or cleared by any migration") applies here: a migration that loses message history is a data-loss incident, not a bug.
5. **Completion tracked by the `schema_version` key in `ai_settings`**, never inferred from table structure (INVARIANTS, "Database Migrations").
6. **No sync surface.** The column adds no CRR membership and no sync payload; it is per-device by design (§24.8).

#### 24.8.2 Attribution accuracy — derived at save, not captured at dispatch
> **Status:** SPEC'D — documents a ratified implementation limitation (#2030, Matt 2026-07-24).

⚠ **Superseded (2026-08-16, ADR-0062 D4).** The derive-at-save mechanism this section ratifies is replaced by structural attribution: the stamp source is the immutable session binding (§24.8 Semantics), so the divergence window described below **ceases to exist** rather than being narrowed. The retrofit this section names as its exit — **#2248, stream-time capture — is closed by elimination**: with an immutable binding, save-time resolution through the session is exact, and no agent identity needs threading through the streaming return path. The text below is retained as the record of the ratified limitation and its reasoning.

`agent_id` is stamped at persistence time by reading the current main agent (`ai_agents WHERE is_main = 1`) inside the save path (`save_chat_message`), **not** by capturing the agent that dispatched the response. The two agree whenever the main agent does not change between a response completing and that response being written.

**The divergence window** is exactly that gap: response completion → message persistence. If the main agent changes inside that window, the message is stamped with the *new* main, not the agent that produced the text.

**Why this ships as-is.** The dispatching agent's id is not in scope at the save callsite, and threading it through would require changes across the streaming return path, the captured-context carrier, the callback layer, and the save command itself. At the time of the decision there was **no trigger**: nothing switched the main agent mid-stream. That is no longer true going forward — customer-facing agent switching (§24.5.5, §24.10.1) can move `is_main` while a response is in flight — so the window becomes reachable, and this limitation is documented rather than left implicit.

**Blast radius is bounded.** Only assistant messages whose response spans an agent switch are affected, and the mis-stamp is a wrong *attribution*, not lost data: the message content and its `model` are correct. A subsequent rename of either agent still relabels via the join (§24.8, "Rename relabels history").

**Retrofit path (intended correction, not scoped here).** Capture the dispatching agent at stream start and thread it to the save path — e.g. `ai_chat_stream` returning `{stream_id, agent_id}` so the save path stamps the captured id instead of re-deriving it. This removes the window entirely. Tracked as #2248; named here so the limitation has a documented exit.

### 24.9 Future: Per-Project Agent Selection
> **Status:** V2/FUTURE - see docs/agent-lifecycle-spec.md §3

**V1 decision:** No project-level agent assignment. Strategist is default for every project. Specialists are spawned by strategist as needed for specific tasks, not assigned to projects permanently.

**Why v1 is strategist-only:**
- Simpler mental model - user talks to one agent (strategist), strategist coordinates everything
- No context confusion - user doesn't wonder "which agent am I talking to right now?"
- Aligns with orchestration vision (§20) - strategist is the brain, specialists are the hands

**V2 consideration:** Optional project affinity. User can set a "primary agent" for a project. That agent handles direct questions, strategist available as fallback. Requires routing logic + UI for "Ask strategist" button.

**DB support already exists:** `agent_id` on `chat_messages_local` (§24.8) tracks which agent spoke. UI: Project settings → "AI Agent" dropdown.

**For full detail:** See `docs/agent-lifecycle-spec.md` §3.

### 24.10 Agent Creation Atomicity
> **Status:** SPEC'D - see docs/agent-lifecycle-spec.md, issue #305
> **GitHub Issue:** #279, #305
> **Amended:** 2026-06-19 - creation-flow folder name corrected from slug to the agent UUID, aligning §24.10 to the §24.2/§24.3 UUID path enforcement (BUILT v0.18.3, Reed UUID audit). The slug-based folder step predated that enforcement pass. (#1535)

**V1 scope (user-initiated only):**
- **User-initiated creation only.** No AI proposals. User creates agents via Settings → Agents button or chat request. Strategist does not propose agents autonomously.
- **Filesystem-first, DB-second.** Files are durable source of truth. DB record created after successful file write.
- **Hard cap on agents.** Default: 5 agents (main + 4 specialists). Configurable 3-10. Hard rejection at limit - no soft warnings.
- **Confirmation card UX.** User reviews name, role, model, cost estimate before creation proceeds.
- **Strategist manages specialist identity.** Creates SOUL.md/MEMORY.md for new specialists. Specialists own daily logs (Tier 3 only).

**Atomicity guarantee:**
1. Validate inputs (name, role, agent cap)
2. Generate slug, check collision. Determine the agent UUID now - the `ai_agents.id` value - before any folder is created. The UUID is generated by the application at this step, not deferred to the DB `DEFAULT`, because the folder name (step 3) and the DB row (step 6) must carry the same UUID.
3. Create `.pane/system/{uuid}/` folder, where `{uuid}` is the agent UUID from step 2. The folder is **never** named by display name or slug (§24.2: `workspace_path` is always the UUID, so renames never break paths or sync).
4. Write IDENTITY.md with the agent name (ADR-0039 — required at creation for every agent regardless of provider class; closes the gap where §24.10 wrote SOUL/MEMORY but not IDENTITY)
5. Write SOUL.md (AI-generated / path-seeded per §24.10.1c)
6. Write MEMORY.md (path-seeded per §24.10.1c; empty on the "just start chatting" path)
7. Insert DB record with `id` and `workspace_path` both set to the UUID from step 2.
8. Tier 3: Pane-bridge plugin detects folder, auto-registers in OpenClaw

**Recovery:** If creation crashes between steps 6 and 7, integrity check (§24.13) discovers the orphan folder on next launch and creates the DB record with `source = 'recovered'`, using the folder-name UUID as the record's `id` and `workspace_path`. Zero data loss.

**For full detail:** See `docs/agent-lifecycle-spec.md` - covers creation flow, tier distinctions (personas vs. sub-agents), strategist HR role, gateway plugin registration, deactivation, deletion, export/import (v2).

#### 24.10.1 Customer-initiated local agent creation
> **Status:** BUILT v0.68.64 — #2030 PR B (`create_local_agent` command, `AgentCreateOverlay.svelte`, `AgentPickerModal.svelte`); seeding paths BUILT (Bug 7 fix, fix/2030-seeding-paths-and-cleanup)  
> **spec-covered-by:** #2030, #2264

Local agents are created either from Settings → Agents (§24.6) or by the forced create flow on a provider switch to an API provider with no local agents (§24.5.5). (The Settings → Agents "+ New agent" control is disabled while OpenClaw is the active provider - §24.6, #2392. Voluntary creation runs only under an API provider.) Both entry points run the same flow and the same atomicity guarantee above.

**Shape — onboarding parity (enumerated).** The customer names the agent, then chooses one of the three seeding paths: **upload documents**, **answer questions**, or **just start chatting**. "Parity" is the enumerated behavior of §24.10.1a–e, shared with onboarding via one component (§24.10.1d) — not merely the same three buttons. This subsection is the canonical contract for both flows; onboarding references it.

**§24.10.1a — Required file set (ADR-0060 D1).** At creation a local agent has exactly three system files: **IDENTITY.md, SOUL.md, MEMORY.md**, all required, all written at creation. IDENTITY.md carries the name (ADR-0039). USER.md, TOOLS.md, AGENTS.md, HEARTBEAT.md, RULES.md, INVARIANTS.md are OC-only (§6.6.4 whitelist) — never generated locally, absent-and-correct for a local agent. The read block includes USER/TOOLS/AGENTS if present (§27.3); HEARTBEAT/RULES/INVARIANTS are synced but outside the read set. Flat file canonical, DB note derived (ADR-0060 D2). An agent whose *seeding* is thin is still valid — the three files exist; content deepens with use.

**§24.10.1b — Upload convention.** Uploaded seeding documents land in **`.pane/system/{UUID}/uploads/`** — per-agent, namespaced by UUID. This is the single convention for both flows. It supersedes onboarding's global `~/Pane/.pane/ai/uploads/`, which is **retired entirely**: onboarding's raw uploads were orphans (written to the global dir, never referenced after extraction), so no vestigial global write survives — every raw upload lands under the per-agent `uploads/`. It also supersedes any flat-in-agent-dir placement (an uploaded `SOUL.md` would collide with the agent's own system file). On filename collision within `uploads/`, **suffix rather than overwrite** (`notes.pdf` → `notes (2).pdf`): uploads are customer content and overwrite is silent data loss (ADR-0007).
>
> **Status update:** BUILT v0.68.70 — #2288 PR 4. `resolveWorkshopProjectId` replaced by `WORKSHOP_SCOPE_ID` sentinel (§24.10.1e). Global upload path (`ai_save_upload`) retired (§24.10.1b). qComm dead-state bug fixed: communication style selector added to questions screen.
>
> **Status update (#2311):** The create-agent "Add context files" upload sub-step (§24.10.1) now has a "What should I upload?" link above the drop zone, opening `https://paneapp.ai/system-files-creation` via the shared `openExternalUrl` helper (Tauri `plugin-shell` `open()`) — identical link text/treatment on onboarding's Upload Documents screen (onboarding.md).
>
> **Status update:** #2304 — reverted the questions-screen qComm dropdown; comm-style input is the path-chooser buttons only (single source, wired directly to `qComm` state), avoiding the duplicate-question UX the prior fix introduced. Questions screen is back to 4 questions.

**§24.10.1c — Generate, never transcribe (ruling 4).** Both seeding paths produce generated documents via a model call, reusing onboarding's existing generation path — not a shallower reimplementation. Upload: the document *contents* are streamed into the model, which synthesizes MEMORY.md and SOUL.md (not a filename listing). Interview: the answers are run through the model and the generated documents are parsed out (not a transcript). Both yield the same output shape as onboarding: generated MEMORY.md + SOUL.md, handed to the shared review step.
>
> **Status:** BUILT v0.68.69 — #2288 PR 3. Seed generation pipeline extracted into shared `src/lib/onboarding/seedGeneration.ts` (streamAndCollect, parseSoulAndMemory, buildUploadExtractionPrompt, buildInterviewExtractionPrompt). Onboarding.svelte and AgentCreateOverlay.svelte both call the shared module. Dead Rust commands `seed_agent_uploads`/`seed_agent_interview` deleted. #2268 folded in: provider credentials threaded through AgentCreateOverlay props. spec-covered-by: #2285.

**§24.10.1d — Review step, shared component (ruling 5).** The flow includes the review step: Memory → Soul, full-screen doc view, step indicator, chat rail for refinements, Next gated on acceptance. The workshop is **extracted into a single component that both onboarding and create-agent import** — copying is prohibited (Svelte scoped CSS does not cross component boundaries; three prior unstyled-reimplementation bugs came from reference-by-name reuse). Abandonment mid-review is valid, not an error (carries the Cancellation semantics below).

**§24.10.1e — Workshop-doc scope (ruling 6 + Q5, sentinel — chosen).** The review docs are editable notes the AI targets by id via `edit_note`. With no default project (default-project.md, retired-creation), they attach to a **dedicated sentinel scope** — a reserved `project_id` value not backed by a `projects` row, distinct from the Home `workspace` scope — lazily ensured when the workshop starts, never on fresh install. `notes.project_id` has no foreign key (CRR table, app-level enforcement only, #493), so a sentinel scope is valid. Workshop-scope notes must remain visible to their editors (the review panel and `edit_note`) for the workshop's duration and are **purged at completion** (both docs, per #2213/#2216). Identical for both flows via the shared component. **Known consumer to make sentinel-aware at scope time:** `execute_list_notes`'s (`ai::tools`) note/task listing INNER JOIN is the one query that would silently drop sentinel-scoped notes (everything else LEFT JOINs or does not join projects) — it must become a LEFT JOIN or sentinel-aware filter, or workshop docs vanish from the listing mid-review.
>
> **Status:** BUILT v0.68.68 — #2288 PR 2. `WORKSHOP_SCOPE_ID = "__workshop__"` defined in `notes.rs` (mirrored as a TS constant in `src/lib/constants.ts`). `execute_list_notes`'s (`ai::tools`) INNER JOIN is now a LEFT JOIN with `COALESCE(p.name, 'Workshop')`. `create_note_internal` has a sentinel branch that skips the project-row lookup and `create_note_on_disk`, inserting a DB-only row (`file_path = ""`). `execute_edit_note` skips `write_note_content`/`note_policy_path` for any note with an empty `file_path`, updating the `body` column directly instead. A startup sweep (`lib.rs` setup hook) deletes orphaned `project_id = WORKSHOP_SCOPE_ID` rows left behind by abandoned/crashed workshop sessions. spec-covered-by: #2285.
>
> **Q5 verification (test-only):** #2288 PR 5 adds three test groups closing gaps not covered by PRs 2–4: (1) `check_cross_project_write_permission` with both session and note at `WORKSHOP_SCOPE_ID` and zero backing `projects` rows — proves the `resolve_project_id` fallback produces the same literal on both sides so the guard passes; (2) `execute_edit_note` in the create-agent context (`onboarding_completed = true`, workshop note with `note_type = NULL`) — proves `is_protected` doesn't fire for plain workshop notes so the edit takes the direct-write path, not the pending-approval path; (3) a fresh-install (zero `projects` rows) end-to-end create-then-edit of a sentinel note. No production code changed. spec-covered-by: #2285.
>
> **Status update:** BUILT v0.68.70 — #2302. PR 2 (#2294) only guarded `execute_edit_note`'s content-write path against empty `file_path`; three sibling call sites still called `write_note_content`/`delete_note_file` unconditionally on sentinel-scope (DB-only) notes, crashing with `"note path must be under Projects/ or .pane/system/: "` on second-save/append/purge. All three now carry the same `file_path.is_empty()` guard: `notes::update_note`'s content-update branch (mirrors its own pre-existing title-rename guard), `ai::commands::execute_append_to_note` (mirrors `execute_edit_note`), and `notes::purge_note` (skips the filesystem delete, proceeds straight to DB row + cascade cleanup; also now explicitly deletes orphaned `note_versions` rows, which had no cascade of their own). `AgentCreateOverlay.svelte` and `Onboarding.svelte`'s `purge_note` callers no longer swallow failures via a bare `.catch()` — both now log at `console.error` so a failed purge is visible in the console instead of vanishing as a silent `warn`.
>
> **Status update:** #2309. The create-agent workshop's Memory-edit permission had been gated on the global `ai_settings.onboarding_completed` flag in `build_messages_with_context` — correct for the onboarding workshop (runs *before* the flag flips), wrong for the create-agent workshop (always runs *after* the flag is already `true`), which meant the AI injected the strict "Memory is append-only via the pipeline" instruction and refused direct edits to the workshop's own sentinel-scoped Memory draft. Fixed with a new backend-derived `is_workshop_session` check in `ai/commands.rs`: resolves `context.noteId` against the `notes` table and tests whether *that note's own* `project_id` is the sentinel `WORKSHOP_SCOPE_ID` (falling back to `context.projectId` when no `noteId` is present yet) — not a frontend-trusted boolean. `build_context_prompt` now returns `(String, bool)`, threading `is_workshop` through to `build_messages_with_context`, which selects the permissive sentinel-scoped-only instruction for workshop sessions and keeps the strict append-only protection (INVARIANTS §5) for every normal chat context, including post-onboarding chat. Same fix also stops injecting the main agent's real SOUL.md/IDENTITY.md/USER.md/TOOLS.md/AGENTS.md/MEMORY.md into workshop context (previously loaded unconditionally off `is_main = 1`, regardless of session type) — workshop context now comes only from the workshop's own sentinel-scoped notes. **Intentional consequence (ruled by Matt):** the workshop assistant loses main-agent knowledge of the user; user context in a workshop comes only from uploaded docs/interview answers, not the real agent's system files. This narrows (does not close) #2310 — persona-quality verification on the DMG build is separate. Previously-dead `context.navigationContext` param (the "ONBOARDING MODE" hint `WorkshopReview.svelte` built and sent but nothing on the backend read) is now wired into the system prompt as an informational hint; it is not a permission gate (`is_workshop_session` alone gates Memory-edit permission).
>
> **Status update:** #2618. The workshop chat rail's `ai_chat_stream` call passed `context.projectId = WORKSHOP_SCOPE_ID` but no `sessionId` — post-ADR-0062's session-first dispatch, the backend's no-`sessionId` fallback path treated that sentinel string itself as a literal `chat_sessions.id`, which never existed as a row, throwing `"Session __workshop__ does not exist"` for every brand-new agent (both onboarding and create-agent workshops). `WorkshopReview.svelte` now takes a required `sessionId` prop threaded straight into `ai_chat_stream`'s top-level `sessionId` argument, bypassing the fallback path entirely. Both callers resolve a real session **before** entering the workshop screen via a new shared `createWorkshopSession(agentId)` helper (`src/lib/onboarding/workshopSession.ts`), which calls `api.createSession(WORKSHOP_SESSION_SCOPE_ID, agentId)` — a new sentinel distinct from the notes-only `WORKSHOP_SCOPE_ID` above — to create a real, disposable `chat_sessions` row explicitly bound to the just-created/just-resolved agent (not `getActiveSession`, which would leave a pre-existing session's `agent_id` binding unchanged, misattributing the refinement chat to the wrong agent in the create-agent-from-Settings flow). No backend changes — `ai_chat_stream` and `resolve_and_selfheal_session_agent` were already correct given a real session id; full detail in onboarding.md's "Workshop chat session" subsection.

> **Seeding contract — successor to onboarding.md's obsolete section.** onboarding.md's seeding section is a tombstone (OBSOLETE as of #1541, no unique normative content). §24.10.1a–e above is the canonical successor contract for seeding behavior, shared by both onboarding and create-agent (§24.10.1d). This closes the drift the prior note flagged (#2030 amendment). The seeding *UI/visual design* remains as-built; this contract governs behavior, not layout.

**Field values.** `oc_agent_id` is NULL - this is what makes the agent local and governs its usability (§24.7C). `source = 'ai_created'`. `workspace_path` is the agent UUID per the §24.2 invariant, never the display name or slug.

**Name-commit is the transaction boundary.** At the moment the customer confirms the name, one transaction performs the step-6 INSERT together with the `is_main` demote-then-promote (§24.7A, ADR-0045 D2). Steps 2-5 (UUID, folder, SOUL.md, MEMORY.md) precede it as specified above; seeding follows it, against a live agent - which is what makes "just start chatting" possible.

There is never a no-`is_main` interim: the previously-selected main holds the flag until the new row is committed and promoted in the same transaction. A zero-`is_main` window would break §24.7 context loading and the greeting path, both of which select on `is_main = 1`.

**Name collision.** The `slug` UNIQUE constraint (§24.2) is validated at name entry and rejected inline - "An agent named X already exists" - checked against **all** agents, including OC-imported ones that are not currently usable. This deliberately differs from the import path's coexist-and-disambiguate rule (#1481, §24.5.5): import is non-interactive and must not fail, whereas here the customer is present and can correct the name in one keystroke.

**Cancellation.** Before name-commit: nothing is written - no agent row, no folder, no provider change. After name-commit: the agent exists and the provider switch has committed; abandoning seeding is not an error.

### 24.11 Teams - Multi-User Agent Considerations
> **Status:** NOT BUILT - Documented for awareness
> **GitHub Issue:** #278

**Known issues for teams:**
1. `UNIQUE(slug)` → needs `UNIQUE(slug, owner_id)`
2. Shared vs personal agents - need `visibility` field
3. Agent file permissions for multi-user `.pane/system/`
4. `is_main` unique index needs `owner_id` scoping

### 24.12 Migration - From Folders to First-Class
> **Status:** NOT BUILT

1. Create `ai_agents` table
2. Extract agent data from `projects` children of "Pane System"
3. Set `is_main` from config
4. Delete "Pane System" and children from `projects`
5. Add `agent_id` column to `chat_messages_local` (shipped ahead of this migration per §24.8.1 / #2030)

Filesystem unchanged. **No impact on memory system** — `ai_memory`, `chat_messages_local`, extraction, synthesis, decay all untouched.

### 24.13 Integrity Check Updates
> **Status:** SPEC'D - see docs/agent-lifecycle-spec.md, issue #305

**Agent creation recovery:**
- Scan `.pane/system/` for folders not in DB → create `ai_agents` records with `source = 'recovered'`
- If DB entry exists but files missing → set `file_missing = 1` flag (never auto-delete)
- If files return (reconnected storage) → clear `file_missing` flag
- Recompute `soul_hash` and `memory_hash` on every integrity check

**What integrity check does:**
1. Discover new agents (orphan folders from partial creates or imports)
2. Detect missing files (flag as `file_missing`, show warning in UI)
3. Validate minimum files exist (SOUL.md + MEMORY.md for all agents)
4. Recompute hashes to detect external edits
5. **Never auto-delete.** Missing files may be on disconnected network shares or unmounted drives. Flag for user review.

**Agent creation guarantee:** Partial creates survive crashes. If folder exists but DB record doesn't, integrity check creates it. User's intended agent becomes usable on next launch. Zero data loss.

**For full detail:** See `docs/agent-lifecycle-spec.md` §11.

### 24.14 Sections Affected

| Section | Change |
|---------|--------|
| §6.5.3 Screen 3 | "Creates Pane System" → "Inserts into `ai_agents`" |
| §16.2 Creation | Reference §24.2 for storage |
| §21 Schema | Add `ai_agents` table |
| §22 API | Add `get_agents`, `get_agent`, `update_agent`, `deactivate_agent` |

### 24.15 Agent Lifecycle - Full Specification Reference
> **Status:** SPEC'D - see docs/agent-lifecycle-spec.md

**This section (§24) covers agent architecture and storage model.** For complete lifecycle operations, see the dedicated Agent Lifecycle Specification:

**`docs/agent-lifecycle-spec.md`** - Consolidated spec for:
- **Creation flows:** User-initiated (Settings + chat), confirmation UX, cost estimation
- **Atomicity guarantees:** Filesystem-first, crash recovery, integrity check reconciliation
- **Agent cap enforcement:** Hard limit (default 5), configurable 3-10, rejection at limit
- **Strategist HR role:** Creates SOUL.md/MEMORY.md for specialists, manages identity
- **Tier distinctions:** Personas (Tier 1/2) vs. real sub-agents (Tier 3), parallelism differences
- **Gateway plugin integration:** Auto-registration in OpenClaw, path-based detection
- **Lifecycle operations:** Editing metadata, deactivation (soft delete), permanent deletion
- **V2 roadmap:** AI proposals, project-level assignment, export/import, marketplace

**GitHub issues:**
- #305 - Agent Lifecycle Specification (canonical)
- #279 - Agent creation atomicity
- #278 - Multi-user slug uniqueness (Teams, v2)
- #289 - Bridge plugin sync all agent folders

**Key v1 decisions:**
- User-initiated only (no AI proposals)
- Hard agent cap (default 5, configurable)
- No project-level agent assignment (strategist is default for all projects)
- Strategist manages specialist identity (SOUL.md/MEMORY.md)
- Tier 1/2 = personas, Tier 3 = real OC sub-agents

Refer to the lifecycle spec for implementation details, UI mockups, API commands, and testing checklists.

---


