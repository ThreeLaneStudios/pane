# AI Memory System

**Last Updated:** 2026-04-19  
**Source:** Extracted from pane-architecture-spec.md  
**Sections:** §5

---

## 5. AI Memory System
> **Status:** PARTIAL (v11.28.24) — extraction ✅, synthesis ✅, daily logs ✅, dedup ✅, async synthesis ✅, compaction guard ✅, error logging ✅ (#161), E2E test ✅ (#162), unit tests ✅ (#155). Missing: version history backend wiring, memory management page UI, save button incomplete

### 5.1 Core Principle
> **Status:** N/A — design principle, not code

**User's machine is the source of truth.** Pane owns memory extraction, storage, and retrieval. Cross-device sync stores encrypted copies remotely — zero-knowledge, user holds the key. Sync is opt-in. Without sync enabled, no data leaves the device. This applies to all tiers — Pane Cloud (Tier 1), BYOK (Tier 2), and OpenClaw (Tier 3). Pane Cloud processes AI requests but does not retain user data server-side.

**Pane is the single interface.** Users interact with their AI through Pane. Conversations on other surfaces (Telegram, Discord, etc.) are not supported and not expected. This simplifies the memory architecture: one input surface, one memory system, one source of truth.


### 5.2 Memory Layers
> **Status:** BUILT (v11.28.24) — personal, work, and daily layers operational

```
┌──────────────────────────────────────────────────────────────┐
│  PERSONAL MEMORY (user-private)                               │
│  Identity, working style, preferences, cross-project facts    │
│  Stored: Memory note in main agent folder + flat file         │
│  ~/Pane/.pane/system/{agent}/MEMORY.md                        │
│  Visibility: Only the user + their AI.                        │
│  Survives compaction. Always in context.                      │
├──────────────────────────────────────────────────────────────┤
│  WORK MEMORY (per-project)                                    │
│  Project decisions, task context, architecture choices         │
│  Stored: Memory note per project + flat file                  │
│  ~/Pane/Projects/{project_name}/Memory.md                     │
│  Visibility: Project members can see (project tree).          │
│  Survives compaction. Loaded in project views.                │
├──────────────────────────────────────────────────────────────┤
│  DAILY MEMORY (chronological log)                             │
│  What happened today: decisions, outcomes, key context         │
│  Stored: Daily note + flat file                               │
│  ~/Pane/.pane/system/{agent}/logs/YYYY-MM-DD.md               │
│  Written by the AI throughout the day.                        │
│  Read on startup + after compaction (today + yesterday).      │
├──────────────────────────────────────────────────────────────┤
│  CHAT TRANSCRIPTS (raw conversation history)                  │
│  Every message, tool call, and response — verbatim            │
│  Stored: Pane local DB (pane.db)                              │
│  Pane stores full transcript from gateway responses.          │
│  Available for scroll-back and memory search.                 │
│  NOT curated — raw data for reference and search.             │
├──────────────────────────────────────────────────────────────┤
│  TEAM MEMORY (org-level, FUTURE — post-launch)                │
│  Shared standards, org decisions, team preferences             │
│  Stored: Sync layer (shared across team members' devices)     │
│  Managed by admins. AIs can propose additions.                │
└──────────────────────────────────────────────────────────────┘
```

**Cross-project memory is a killer feature.** Personal memory is visible in ALL project contexts. The AI can reference any project's memory when relevant. A decision in Project A that affects Project B is surfaced automatically.

### 5.3 Memory Storage
> **Status:** BUILT (v11.28.24) — DB + flat file dual storage working

Memory is stored in two places:

1. **Markdown flat files** (human-readable, portable, source of truth): MEMORY.md, logs/*.md, Projects/{name}/Memory.md
2. **SQLite table** (queryable, structured, mirrors flat files): `ai_memory` table

**Runtime source:** The DB is the runtime source for performance — the app reads from the DB, not flat files. **Portable source of truth:** Flat files are the authoritative format for portability, export, and external access (OpenClaw, user editing). On startup, Pane checks if flat files have been modified externally (by OpenClaw, sync, or the user) and syncs changes INTO the DB. If there's a conflict, flat files win. This preserves both performance (DB) and portability (files) — similar to how Git uses files as source of truth but indexes for speed.

```sql
CREATE TABLE ai_memory (
    id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    layer           TEXT NOT NULL,          -- 'personal' | 'work' | 'daily'
    project_id      TEXT,                   -- NULL for personal/daily, set for work
    user_id         TEXT,                   -- owner (for personal memory)
    fact            TEXT NOT NULL,
    category        TEXT,                   -- 'decision' | 'preference' | 'fact' | 'deadline' | 'style' | 'identity' | 'reflection'
    source_message_id TEXT,                 -- chat message it was extracted from
    is_active       INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT,
    last_referenced TEXT,                   -- for decay policy
    routed_at       TEXT                    -- when fact was last routed to LLM
);

CREATE INDEX idx_memory_layer ON ai_memory(layer);
CREATE INDEX idx_memory_project ON ai_memory(project_id) WHERE project_id IS NOT NULL;
CREATE INDEX idx_memory_user ON ai_memory(user_id) WHERE user_id IS NOT NULL;
```

### 5.3A File Locations
> **Status:** BUILT (v0.65.2) — agent folders operational. Project Memory auto-created at project root per ADR-0020 (#800). Startup backfill for existing projects (#803, #808 crash fix). AI subfolder retired in #470.

**Pane System project** has a unique structure: named agent folders (not an "AI" folder). Regular projects get an AI folder (see below).

**System note file paths (MANDATORY, added 2026-03-21):**

System notes (`is_system = 1` or `note_type` set) MUST have their `file_path` pointing directly to `~/Pane/.pane/system/`. NOT to `Projects/Pane System/`. There is ONE copy of each system file, not two. The editor reads from `.pane/system/` directly. The integrity check creates notes with `file_path` set to the `.pane/system/` location. No copy to `Projects/` is needed or allowed.

**Why:** Two copies of system files (one in `.pane/system/`, one in `Projects/`) violates the single-owner principle and creates drift. OpenClaw writes to `.pane/system/` via symlink. If Pane's editor reads from a `Projects/` copy, the two versions can diverge silently — causing data loss (#121) and stale content.

**Workspace-level memory** lives on the user's machine under `~/Pane/.pane/system/`:

```
~/Pane/.pane/system/
├── {main-agent-name}/           # e.g., "Brit Freed"
│   ├── MEMORY.md                # Personal memory (synthesized profile)
│   ├── SOUL.md                  # AI personality
│   ├── IDENTITY.md              # AI identity
│   ├── AGENTS.md                # Operating instructions
│   ├── USER.md                  # User profile
│   ├── TOOLS.md                 # Tool notes
│   └── logs/                    # Daily logs (chronological records)
│       ├── 2026-03-21.md        # Today's log
│       └── 2026-03-20.md        # Yesterday's log
├── {agent-2}/                   # e.g., "Stia"
│   ├── Soul.md
│   └── Memory.md
└── {agent-3}/                   # e.g., "Lane"
    ├── Soul.md
    └── Memory.md
```

**Per-project memory** lives at the project's root:

```
{Any Project}/
├── Memory.md                    # What the AI knows about THIS project
├── {user's folders and notes}
└── ...
```

**Project Memory rules (ADR-0020, 2026-04-30; supersedes Mar 7 / Mar 16 AI-folder decisions):**

- **Path:** `~/Pane/Projects/{project_name}/Memory.md`. Project-root, user-visible. The AI folder used between Mar 16 and Apr 16 was retired in #470.
- **Identification:** by `note_type = 'memory'` flag on the `notes` row, not by title or path. Title is "Memory" by convention only.
- **One Memory note per project**, deduplicated at startup (`database.rs:462-467`).
- **Auto-created** at project-creation time (#800) at this path.
- **Read-only in Pane editor** per ADR-0011. Memory Settings is the write surface.
- **Synthesis routing:** project-scoped facts (`project_id = Some(project)`) route to that project's Memory.md. Personal facts (`project_id = NULL`) route to agent MEMORY.md per §5.5.
- **Project rename:** when the project directory is renamed, Memory.md moves with it via the file watcher, which updates the DB row's `file_path`. Same pattern as any other file in a project.
- **Visibility asymmetry from agent MEMORY.md is intentional.** Agent memory is personal and lives in `.pane/system/`. Project memory is project-scoped and lives in the visible Projects/ tree. See ADR-0020.
- System prompt injection order: Soul → Identity → User → Tools → Rules → Memory. Per-project merges with workspace-level (workspace Soul + project Soul override). (Corrected per ADR-0039 — previous spec listed 4 elements; actual code in `build_context_prompt` (`ai::chat`) injects 6 files in this order since §24 was implemented.)

> **📌 Why This Exists — project memory at project root (2026-04-30, ADR-0020):** The §5.2 spec accumulated three eras of project-memory paths without the older forms being removed: a pre-Mar 16 hidden form under `.pane/system/`, a Mar 16 AI-subfolder form, and the Apr 16 project-root form (#470). The §11 audit on 2026-04-30 surfaced all three coexisting in the spec while the code uses only the third. This update removes the two stale forms and locks the visibility-posture asymmetry (agent memory hidden, project memory visible) as a deliberate decision rather than an inference from the layer diagram.

### 5.4 Auto-Extraction
> **Status:** PARTIAL — async extraction pipeline, `source_message_id` dedup, proper error logging BUILT (v11.28.24). Trigger threshold update (≥5 words) and dual-write to daily log PENDING per ADR-0010.

After each AI response, the backend runs an asynchronous extraction pass:

1. Check the trigger. Extraction runs if **either**:
   - The user message is ≥ 5 words, OR
   - The AI used a structural tool in its response (`create_note`, `create_folder`, `create_project`, `edit_note`, `set_status`, `move_note`, `delete_note`, `delete_folder`).
   
   If neither condition is met, skip. Short exchanges with no state change ("ok", "thanks") don't produce extractable facts. See §5.4A for the reasoning behind the hybrid trigger.

2. Send the conversation exchange (both user message AND assistant response) to the configured extraction model (see §5.4C) with the prompt: *"Extract decisions, preferences, or important facts. Return JSON array with category, or empty array."* The prompt also instructs the model to tag each fact with its **attribution** (see step 3). The assistant response is sent for context — it is required to interpret terse user turns (e.g. "yeah, B" only means something against the AI's prior message) — but assistant-origin facts must not be saved; step 3 enforces that mechanically.
   
   **The frontend must pass the database-persisted message ID** (not a temp/placeholder ID) so the extraction worker can fetch the user message content. If the user message fetch fails, the extraction worker logs the error and falls back to the most recent user message in the same thread.

3. **Output filter — drop assistant-origin facts (the extraction red line).** Before any fact is written, the extraction worker runs a mechanical filter over the returned facts. A fact is dropped if it is *about the assistant's own actions or statements* rather than about the user, the user's decisions, or the world. Origin is determined by **attribution**, not by which message the words came from — a decision the user reached but the AI articulated ("yeah, B" → the AI's reply "we'll go with the Postgres migration") is a user-attributed fact and is KEPT; a fact describing what the AI did ("created note X," "you asked me to set the status") is assistant-attributed and is DROPPED. Attribution is determined by two mechanical signals, applied in order:

   - **(a) Attribution tag.** The extraction prompt (step 2) requires each returned fact to carry an `attribution` field with value `user` (the fact is about the user, their intent, decisions, preferences, or the world) or `assistant` (the fact is about what the AI did, said, or was asked to do). This tags the fact's *subject* — who the fact is about — not which message supplied the wording. Facts tagged `assistant` are dropped. Tagging the fact's subject is a structural classification against the labeled `User:` / `Assistant:` turns the model already sees, not a semantic suppress-judgment; it is more reliable than the prior free-form "do not extract AI actions" instruction, but it is still model-produced, so signal (b) backstops it.
   - **(b) Mechanical backstop — subject/pattern guard.** Independent of the model's tag, the filter drops any fact whose surface form marks it as assistant-origin by construction: a fact in the `fact` or `reflection` category whose grammatical subject is the assistant or whose phrasing addresses the AI's action — first-person-AI ("I created…," "I set…," "I moved…") or second-person-addressing-AI ("you asked me to…," "you created…," "the assistant/AI created/set/moved/deleted…"), and facts that restate a tool result (matching the structural-tool verbs already enumerated in §5.4A's ❌ IGNORE list: created/edited/moved/deleted note/folder/project, set status). This backstop is a pattern check in the same spirit as the §5.4A/§5.6.3A `is_compaction_summary()` and audit-marker filters — system-recognizable surface forms that are never legitimate user facts. It catches assistant-origin facts that a weak model mis-tagged as `user` in (a).

   A fact survives the filter only if it is neither tagged `assistant` (a) nor matched by the subject/pattern guard (b). This is purely subtractive: the filter removes assistant-origin facts and changes nothing about which user-origin facts are extracted (the extraction red line — see §5.4A). Token-overlap between the fact and the assistant message is explicitly NOT a signal: a terse user turn whose decision the AI articulates produces a legitimate user-attributed fact whose words match the assistant message, and dropping it on overlap would eat a real fact.

   > **📌 Why attribution, not word-overlap (2026-06-21, #812):** The leak and the legitimate decision-fact differ by *who the fact is about*, not by *whose words articulated it*. A token-overlap filter (drop if the fact's words match the assistant message above a threshold) was rejected: when the user is terse and the AI articulates the decision, the legitimate fact's words come from the assistant message, so overlap is high for both the leak and the keep — the signal cannot separate them, and the threshold was an untuned guess with no labeled corpus to set it. Attribution (who the fact is about) is the axis that separates the classes. This filter is the hook that becomes #1711's attribution-routing: #1711 routes facts by origin/attribution rather than dropping assistant-attributed ones, and the `attribution` tag introduced here is the field it routes on. Until #1711, assistant-attributed facts are dropped (not routed).

4. Each surviving fact is categorized (see §5.4A category table) and **dual-written**:
   - **Authoritative:** Insert into `ai_memory` DB row with `category`, `project_id`, `source_message_id`, `confidence`, `is_active = 1`, `routed_at = NULL`.
   - **Best-effort:** Append a `[saved]` entry to today's daily log flat file (`.pane/system/{agent}/logs/YYYY-MM-DD.md`) in the format `- HH:MM [saved] category: "fact text"`. If this append fails, log a warning; do not fail the extraction. The DB row is the source of truth.

5. User sees a subtle toast: `💾 Remembered: Prefers dark mode` with dismiss button (see §5.4B).

6. The extraction worker checks the unrouted fact count. When `COUNT(*) WHERE routed_at IS NULL AND is_active = 1 >= memory_synthesis_threshold` (default 1, configurable in Memory Settings), it emits a `trigger-router` event. The router (§5.5) handles category-based routing to agent and project memory files. **Extraction does NOT write to memory files.** Extraction writes only to `ai_memory` and the daily log.

Extraction runs async — it does NOT block the UI response.

**Main agent folder:** During onboarding, `main_agent_folder_id` is stored in AI settings. All extraction routes to this folder's Memory note when in the Pane System project. This prevents writes going to the wrong agent's folder.

**Why dual-write to the daily log:** The daily log is the complete record of the day (§5.6.1). Every extracted fact appears there as an audit trail — "at 14:03, the system learned this about you." This makes the daily log searchable as a journal and preserves what the AI extracted even if a later edit or delete removes the fact from memory. Daily log entries are append-only; they are never rewritten in place. See ADR-0010 for the broader "`ai_memory` is source of truth, files are rendered views" decision.

> **📌 Why This Exists — extraction model is a dedicated user-configurable setting (2026-05-18, ADR-0038, supersedes 2026-02-28 Brit/Matt decision):** The 2026-02-28 decision pinned extraction to "Sonnet-tier" on reliability and cost grounds — a correct structural constraint expressed by naming a model family. Hardware testing on 2026-05-18 (issue #1182) surfaced the cost of the model-family naming: extraction silently routed through whatever the primary chat model was (Opus via OpenClaw in the test), produced 30-second timeouts, and dropped daily-log entries downstream. The corrective is to make extraction's model selection a user-configurable `ai_settings` key (`extraction_model`) with a fallback chain (§5.4C) that gives sensible defaults without binding the spec to model identifiers that age fast. The 2026-02-28 structural rationale — extraction needs a model reliable at structured JSON output and cheap enough to run on every qualifying message — survives in ADR-0038's Rationale. The dismiss toast referenced in the original note is unaffected and remains spec'd at §5.4B. See ADR-0038.

> **📌 Why This Exists — trigger threshold ≥5 words (2026-04-23):** The original threshold was ≥8 words, chosen to avoid extraction noise on short exchanges. Pre-launch testing showed the threshold was leaving real preferences on the floor — "I prefer that one" (4 words) and "let's go with option B" (5 words) are both legitimate facts that were being skipped. Lowering to ≥5 catches these while still filtering the "ok" / "thanks" / "yes" noise. Cost is trivial: Sonnet extraction is ~$0.001 per call. Manual save (§5.12) remains the catch for anything the trigger still misses. A user-configurable threshold is deferred to post-launch — ship consistent data first, tune from evidence later.

### 5.4A Memory Extraction Discipline (Brit/Matt decision, 2026-03-02; updated 2026-04-23)
> **Status:** BUILT — hybrid triggers, compaction exclusion, quality tuning BUILT (v11.28.24). `goal` category added to extraction prompt (#760).

**What to extract vs. ignore:**

| ✅ EXTRACT | ❌ IGNORE |
|-----------|----------|
| User name, role, identity | Greetings, small talk |
| Explicit preferences ("I prefer dark mode") | App context (note titles, task counts) |
| Decisions ("we're going with Option A") | Obvious conversation facts ("you asked me to create a note") |
| Deadlines, commitments | Trivial details ("I said hello") |
| **Goals ("I want to ship Pane v1 by June")** | Tool results ("created note X") |
| Project facts, architecture choices | Anything the AI can re-derive from the data |
| Recurring patterns (user always does X) | Compaction summaries — never extract from these |
| Reflections ("that approach didn't work because...") | |

**Extraction format: Each extracted fact must be one atomic sentence.** Never create multi-sentence summaries, paragraph-length entries, or conversation recaps. "Matt owns a 2021 Jeep Willys" — not a paragraph about the conversation where it came up. If the extraction model produces a summary blob, discard it.

**Categories (8 total):**

| Category | Meaning | Examples |
|----------|---------|----------|
| `identity` | Core facts about the user | Name, role, background |
| `preference` | Stated likes/dislikes, working style preferences | "Prefers dark mode," "likes concise responses" |
| `style` | How the user communicates and works | "Uses em-dashes heavily," "thinks in checklists" |
| `goal` | Aspirational direction, not yet decided | "Wants to ship Pane v1 by June," "aiming to launch with 10 beta users" |
| `fact` | Neutral information about the user or world | "Lives in Atlanta," "uses macOS" |
| `decision` | Concrete choice made | "Chose SQLite over Postgres," "going with Option A" |
| `deadline` | Time-bound commitment | "Demo is Friday at 3pm," "report due end of Q2" |
| `reflection` | Mid- or post-action thought | "That approach didn't work because of X" |

**Compaction exclusion (implemented v11.28.21, #165):** The extraction pipeline filters out compaction summary messages before processing using a backend `is_compaction_summary()` check that detects 2+ structural markers (e.g., `## Conversation Summary`, bullet-list recaps, system role messages). On startup, existing compaction-derived facts in `ai_memory` are deactivated (`is_active = 0`). The extraction prompt is also reinforced to reject summaries, and frontend filters align with backend detection. These are system-generated recaps, not user statements — extracting from them produces duplicate, low-quality summary blobs.

**Personal vs. project tagging:** A fact **about the user** is ALWAYS personal (`project_id = NULL`), regardless of which project the conversation happens in. "Matt's Jeep has red shocks" said in the Jeeps project is still a personal fact. Only facts specifically about the project's work, goals, or decisions get a `project_id`. Personal memory is visible in all project contexts anyway — cross-project visibility is built in.

**Extraction triggers (hybrid approach, threshold updated 2026-04-23):**
- **Always extract if:** AI used structural tools in its response (`create_note`, `create_folder`, `create_project`, `edit_note`, `set_status`, `move_note`, `delete_note`, `delete_folder`). Tool usage = something happened = worth remembering. "Yes please" + AI creates folders = extract it.
- **Also extract if:** user message ≥ 5 words. Catches reflective statements, preferences, and decisions that don't involve tools.
- **Skip if:** neither condition met. "ok" (1 word) + AI responds with info only = no extraction needed.

This hybrid approach is more semantic than pure word count — it uses tool usage as a signal that something consequential occurred.

**Extraction philosophy: Extract aggressively, let the user prune.** Missing a real memory is worse than saving one extra fact. The dismiss button and memory management page handle over-extraction; nothing handles under-extraction.

**Confidence threshold:** Extraction model rates each fact 1-5. Only save ≥ 4. This cuts noise without risking lost memories.

**Contradiction handling:** When a new fact contradicts an existing one in the same category, replace it. "User likes tea" → later "User prefers coffee" → second one wins. This handles 80% of deduplication without requiring an embedding model.

**Decay policy:**
- `identity` / `preference` / `style` / `goal` → **never decay**
- `fact` / `decision` / `deadline` → decay after 90 days unused (set `is_active = 0`)
- `reflection` → decay after 30 days unused
- Decay checks run weekly. Decayed facts are soft-deleted (recoverable).
- Decayed facts are excluded from consolidator input (filtered by `is_active = 1`), so they disappear from memory files on next consolidation. See §5.5.

> **📌 Why This Exists — `goal` category added (2026-04-23, ADR-0010):** The original 7 categories treated goals awkwardly. "Matt wants to ship Pane v1 by June" would either be miscategorized as a `decision` (it's not decided) or a `deadline` (the date is aspirational, not committed). Aspirational direction is distinct from both and is core to the "AI life partner" product vision — the AI should know what the user is building toward, not just what they've committed to. Routes to agent MEMORY.md when extracted with no project context, project Memory when extracted in a project. Treated as "never decay" because goals anchor identity even when dormant; if a goal is abandoned, the user can delete it explicitly.

> **fix(memory-pipeline) (#3120 PR 4, pre-existing):** extracted facts carry a `scope` field (`"durable"` or `"episodic"`, Extraction-guard PR A, #2745/#2750 — not otherwise documented in this section) that the extraction worker uses to drop episodic-scoped facts (PR/issue/build/test-run/thread-scoped) before they ever reach `ai_memory`. A schedule request itself ("remind me to X at time T", "schedule Y for tomorrow") is now explicitly tagged `"episodic"` by the extraction prompt's SCOPE REQUIREMENT section — previously untagged by any rule, it read like a plausible `"durable"` preference to the classifying model and was saved as a standing fact (observed: a two-minute "tick" test schedule request stored durably). The schedule-request steering is prompt-level only; the drop mechanism itself was already correct and unchanged.

### 5.4B Memory Toast Toggle (Brit/Matt decision, 2026-03-02)
> **Status:** BUILT (v0.67.48, PR #1716, #641) — toast restored after #335 async regression. Event-driven: extraction worker emits `memory-extracted`, frontend listens. Dismiss (✕ wrong) soft-deletes via `ai_dismiss_memory`. Settings toggle (`pane_memory_toasts`). Single-fact display; multi-fact batches show last fact (#1715 tracks count-string spec amendment).

**Settings → AI → Memory:**

| Setting | Default | Description |
|---------|---------|-------------|
| **Show memory toasts** | ON (new users) | Display "💾 Remembered: {fact}" toast with dismiss button |
| **Memory extraction** | ON | Enable/disable auto-extraction entirely |

- **Toast ON**: User sees what the AI learned, can dismiss wrong extractions. Builds trust.
- **Toast OFF**: Silent extraction. Power users who trust the system.
- **Memory management page** (Settings → AI → Memory) always available. Browse, edit, delete, search all stored memories.

Toast behavior:
- Appears above chat input, subtle (dark theme, small font)
- Auto-fades after 5 seconds if not interacted with
- "✕ wrong" button → soft-deletes the memory (`is_active = 0`)
- Non-blocking — never delays chat response

### 5.4C Extraction Model Resolution
> **Status:** SPECIFIED — landing via #1201 (extraction routing implementation).
> **Amended 2026-05-19** per ADR-0038 amendment: per-feature lookup, no cross-feature fallback, no fallback to primary. The original three-step chain (extraction_model → simple_tier_model → primary `model`) is superseded.

The extraction worker reads `extraction_model` from `ai_settings`. That is the entire resolution:

- If `extraction_model` is set, extraction dispatches via the §29 internal-provider path (smart-routing.md §27.6) using the configured model.
- If `extraction_model` is null, **extraction is silently disabled**. No memory write, no daily-log entry, no toast on the user side, no log noise beyond a single info-level startup line per worker. The customer sees the absence of memory growth in the Memories tab — that is the surface.

There is no fallback to `simple_tier_model`. There is no fallback to primary `model`. Per-feature lookup is the rule across all background AI features (smart-routing.md §27.6.1).

**Setting shape.** `extraction_model` is a TEXT value in the key-value `ai_settings` table. The value is a provider-native model identifier (e.g., `anthropic/claude-3-5-haiku`, `claude-3-5-haiku-20241022`, `llama3.2:latest`). The customer picks the model from their configured internal provider's model list — at first run via the Smart Features onboarding step (onboarding.md §6.4), or post-onboarding via Settings → AI → Smart Features (ADR-0037 §1 + §3a).

**On failure of the configured model.** When extraction is enabled (`extraction_model` set) and the call fails (timeout, auth, unknown model, provider 5xx, etc.), the worker logs the failure via existing extraction error logging (§5.4 step 6 / `extraction_queue` retry semantics) and does NOT retry through a different model. Configured-model failures surface to the customer for correction — silently routing to a different model would hide a configuration error the customer needs to fix.

**Dispatch path.** Extraction dispatches via the internal provider resolved per smart-routing.md §27.6.2 (OR / Anthropic / Ollama). It does NOT dispatch via the primary provider, including for OpenClaw users. The pre-2026-05-19 path (primary-provider dispatch with gateway model-field rewrite) is what caused issue #1186 / Reed audit Addendum 2; #1201 closes the gap.

**Why no fallback.** Phase 1 product decision (2026-05-19): extraction is core but its cost profile is sensitive (12K-token system prompt; Opus ~$0.18/call vs Haiku ~$0.01/call). Silently falling back from a misconfigured `extraction_model` to `simple_tier_model` or primary `model` masks the configuration intent and risks running extraction on whatever the primary happens to be — exactly the failure mode the amendment is closing. Customers who want extraction must opt in explicitly during onboarding or Settings; customers who don't opt in pay $0 for extraction.

### 5.5 Router, End-of-Day Agent, and Consolidator
> **Status:** PARTIAL — Three-writer split per ADR-0010: Router (§5.5.1) BUILT v0.65.1 (#796 fix: project routing was broken since first commit, now fixed), End-of-Day Agent (§5.5.2) BUILT (narrative header; dedup pending #799), Consolidator (§5.5.3) BUILT v0.63.0 (#795 path fix in v0.65.0).

> **📌 Why This Exists — three-writer split (2026-04-23, ADR-0010):** The old §5.5 described a single "synthesis" step that both routed extracted facts to destination files and rewrote those files as clean prose. Three concrete failures earned this rewrite: (1) **#646** — 90-second synthesis timeouts on ~1400-word MEMORY.md rewrites, because every routing event paid the full rewrite cost; (2) **agent-memory bloat** — general `fact` and `decision` entries without project context were routing to agent MEMORY.md, and Matt recently had to manually trim the Brit agent's MEMORY.md from over 20,000 characters back under OpenClaw's 12K cap; (3) **dishonest delete semantics** — Memory Settings let a user "delete" a fact that still lived as a bullet in a memory file and eventually as a sentence in a prose paragraph, so deletions silently failed to propagate. Splitting writes into three distinct writers (router, end-of-day agent, consolidator) and making `ai_memory` the source of truth addresses all three.

ADR-0010 splits memory writes into three distinct writers, each owning a specific target with a specific trigger:

1. **Router** — deterministic, no LLM. Appends fact bullets to memory files (and removes bullets on delete). Triggered when N unrouted facts accumulate.
2. **End-of-day agent** — LLM-based, daily. Deduplicates yesterday's daily log and prepends a 3–6 sentence narrative header. See §5.6.5.
3. **Consolidator** — LLM-based, rare. Rewrites memory files to clean prose, reading from `ai_memory` (the source of truth) and the existing file's prose style. Triggered by word-count threshold plus a 14-day staleness backstop.

Each owns its target. No writer overlaps. Preserves the "one writer per file" invariant (§5.7) with a cleaner split.

#### 5.5.1 Router

**Trigger:** Extraction worker emits `trigger-router` when unrouted fact count ≥ `memory_synthesis_threshold` (from `ai_settings`, default 1, configurable in Memory Settings).

**What it does:**

1. Acquire the router mutex (serializes router runs; see §5.7).
2. Query `SELECT * FROM ai_memory WHERE routed_at IS NULL AND is_active = 1`.
3. For each fact, determine its destination from category + `project_id` (see routing table below).
4. If destination = none (daily-log-only categories): mark `routed_at`, skip file write.
5. If destination = memory file: acquire per-file mutex, read file, check if exact bullet text is already present (idempotent append). If not present, append the bullet. Mark `routed_at`. Release per-file mutex.
6. After the batch: for each memory file touched, check its word count. If past threshold (default 2000 words), emit `trigger-consolidator` for that file.
7. Release router mutex.

**The router never invokes an LLM and cannot time out.** Appending a bullet is a file read + write, measured in milliseconds.

**The router also handles bullet removal on user delete** — when a fact is marked `is_active = 0` via Memory Settings, the router is invoked with a `remove-bullet` operation on the destination file. Same mutex, same atomic write, no LLM. Details in §5.7.

**Routing table:**

| Category | `project_id` context | Appends to `ai_memory`? | Appends to memory file? |
|----------|---------------------|-------------------------|-------------------------|
| `identity` | any | yes | agent MEMORY.md |
| `preference` | any | yes | agent MEMORY.md |
| `style` | any | yes | agent MEMORY.md |
| `goal` | `NULL` | yes | agent MEMORY.md |
| `goal` | `Some(project)` | yes | that project's Memory note |
| `fact` | `Some(project)` | yes | that project's Memory note |
| `fact` | `NULL` or `"workspace"` | yes | agent MEMORY.md |
| `decision` | `Some(project)` | yes | that project's Memory note |
| `decision` | `NULL` or `"workspace"` | yes | agent MEMORY.md |

> **Project Memory note location:** `~/Pane/Projects/{project_name}/Memory.md`. See §5.2 and ADR-0020. ADR-0011's read-only guard applies via `note_type='memory'` regardless of path.
| `deadline` | any | yes | **none** (daily log only) |
| `reflection` | any | yes | **none** (daily log only) |

> **`project_id = "workspace"` normalization (#1238):** The home-screen conversation assigns `project_id = "workspace"` as a built-in default. The router normalizes `"workspace"` (and empty string `""`) to `None` before routing, so home-screen facts are never routed to a non-existent project. They follow the `NULL` routing path and land in agent MEMORY.md.

**Agent MEMORY.md routing** covers `identity` / `preference` / `style` / `goal` (any context) plus `fact` / `decision` with no real project context (NULL or workspace-scoped).

#### 5.5.2 End-of-Day Agent

See §5.6.5 for the full spec. Summary:

- Triggered on the first AI response of a new day (date change vs. last daily log filename).
- Reads yesterday's daily log, deduplicates exact-match entries, prepends a 3–6 sentence narrative header.
- **Daily log only.** Never touches memory files.
- Falls back to a mechanical header (`## {date} — {count} entries logged`) if the LLM call fails.

#### 5.5.3 Consolidator
> **Status:** BUILT (v0.63.0, #671/#646) — consolidator module, startup staleness check, content-loss guard, mtime check, atomic write, ai_settings tracking. v0.67.47: startup polls for provider readiness (2s × 15 = 30s ceiling) instead of blind 15s sleep (#697, PR #1707). Consolidation prompt templates user name from IDENTITY.md instead of hardcoded "Matt" — falls back to "the user" (#695, PR #1708).

**Trigger:** Any of the following:

1. Router touches a memory file past the word-count threshold (default 2000 words) → emits `trigger-consolidator` for that file.
2. App startup: for each memory file, if `last_consolidated_at > 14 days ago` → emit `trigger-consolidator`. Staleness backstop.
3. Manual Settings trigger: user clicks "Consolidate now" on a memory file in Memory Settings.

No cron is used. Event-driven triggers match the pattern used for the end-of-day agent (§5.6.5) and don't rely on the app being open at scheduled times.

**What it does:**

1. Acquire the per-file mutex for the target memory file (shared with router).
2. **Capture `mtime`** of the target file at read time (`T1`). Used for optimistic concurrency check in step 9.
3. Read the existing memory file content (used for prose style preservation).
4. Save a pre-consolidation backup to `.pane/backups/synthesis/{file}-{timestamp}.md`.
5. Query the canonical fact list: `SELECT * FROM ai_memory WHERE is_active = 1 AND {project_id filter}` — for agent MEMORY.md, `project_id IS NULL AND category IN ('identity', 'preference', 'style', 'goal')`; for project Memory, `project_id = ? AND category IN ('fact', 'decision', 'goal')`.
6. Build the consolidation prompt using `synthesis_size_guard()` to set mode (`gentle` / `consolidate` / `aggressive`) based on existing word count. Prompt: *"Here is the current memory prose (for style). Here is the canonical list of active facts (for content). Rewrite the memory preserving prose style. Content must match the fact list. Remove anything not backed by a fact."*
7. Call the LLM. This is the slow, expensive step. Under the old model, this ran on every routing event and caused #646. Under the new model, it runs rarely.
8. **Content-loss guard (§2C.2):** If new content is >50% shorter than existing content AND existing content > 100 chars, block the write with `[CONSOLIDATOR-BLOCKED]` log. Pre-backup is preserved; the original file is untouched.
9. **mtime check (optimistic concurrency):** re-`stat` the target file. If `mtime` has changed since `T1`, abort consolidation. The file was edited during the LLM call (by the Pane editor, OpenClaw, or an external tool). Log `[CONSOLIDATOR-ABORTED-MTIME]`, leave the file unchanged, rely on the staleness backstop to re-trigger later.
10. **Case-collision check (§2C.3):** ensure the target filename is the exact expected case (`MEMORY.md`, not `Memory.md`) to prevent macOS / Windows case-insensitivity bugs.
11. Atomic write via `safe_write_system_file` (temp file + rename).
12. Update `notes` table metadata: `word_count`, `content_preview`, `updated_at`.
13. Update FTS5 index (per INVARIANTS §5).
14. Update `last_consolidated_at` for this file.
15. Emit `pane-reload-editor` event if the file is currently open.
16. Release the per-file mutex.

**Why the consolidator reads from `ai_memory` instead of just the existing file:** See ADR-0010. This is the architectural shift that makes `ai_memory` the source of truth for memory content. User deletes (`is_active = 0`) and decayed facts are filtered out of step 5 and therefore absent from the rewritten file. This makes "forget this" honest.

**Tradeoff — manual edits to memory files via "Open in Editor" get overwritten on next consolidation** if they add content not backed by an `ai_memory` row. Memory Settings is the supported edit surface; the Pane editor opens system notes read-only. See §5.7 and §23.3.

#### 5.5.4 Writer ownership summary

| Writer | Target | LLM? | Frequency | Timeout risk |
|--------|--------|------|-----------|--------------|
| Extraction | `ai_memory` DB + daily log | yes (Sonnet) | per message that trips trigger | low (bounded per-message call) |
| Router | memory files (append or remove bullet) | no | per N unrouted facts | none |
| End-of-day agent | daily log (dedup + header) | yes | once per day | low (fallback header) |
| Consolidator | memory files (full rewrite) | yes | per word-count threshold + 14d staleness | moderate (slow but rare) |

See ADR-0010 for the full design rationale and the considered alternatives.

### 5.6 Daily Logs
> **Status:** PARTIAL (v11.28.24 built basic, #223 regression, needs rebuild per Mar 23 redesign). Audit-entry markers and end-of-day agent PENDING per ADR-0010.

#### 5.6.1 Philosophy

**Daily logs are the complete record of the day.** They capture everything significant — whether it also lives in agent memory, project memory, or both. Overlap between daily logs and other memory layers is expected and good.

- **Memory note** = synthesized profile (who you are, timeless facts)
- **Project memory** = project-scoped knowledge (decisions, architecture, context)
- **Daily log** = chronological record (what happened today, when, why)

**Daily logs are append-only.** Entries are never rewritten in place, never deleted, never reordered. The only non-append mutation is the end-of-day agent (§5.6.5), which prepends a narrative header and deduplicates exact-match entries — both are additive or lossy-safe operations that never modify the semantic content of existing entries. See INVARIANTS §5.

#### 5.6.2 Scope & Ownership

- **One daily log per user** (not per agent). The user had one day — one log captures it.
- **Cross-project.** Facts from ALL projects appear in a single chronological stream.
- **Agent name tagged on entries** when multiple agents contribute: `- 15:18 [Brit] — Code signing working`
- **Location:** `.pane/system/{main-agent}/logs/YYYY-MM-DD.md`
- **Retention:** Indefinite. Daily logs persist forever.

#### 5.6.2A Storage Model (#280, v0.17.16)

Daily log notes are stored in the `notes` table with:
- `note_type = 'daily'`
- `agent_id` — references the main agent in `ai_agents` table (§24). This is the primary lookup key.
- `file_path` — `.pane/system/{agent}/logs/YYYY-MM-DD.md`
- `project_id` — NULL. Agent-owned notes have no project.

**`notes.project_id` is nullable** (migration 013). Agent-owned notes (daily logs, system notes) set `project_id = NULL` and use `agent_id` exclusively. User-created notes always have a `project_id`. This separation is enforced by:
- INNER JOINs on `projects` naturally exclude agent notes from project listings
- `WHERE project_id = ?` queries never match NULL — agent notes are invisible to project queries
- Search uses LEFT JOIN so daily logs remain discoverable via FTS
- Note counts filter on `project_id IS NOT NULL`

**`logs_folder_id` ai_setting is deprecated** and removed by migration 012. `folder_id` is unused for daily logs. The `__system` project is cleaned up if no notes reference it.

#### 5.6.3 Format

Timestamped facts as the backbone, with brief narrative connectors when the "why" matters.

```markdown
## Mar 22, 2026
Focused on code signing and memory architecture. Fixed #154, decided daily logs capture everything, filed 4 new issues.

- 14:04 — Code signing: .env exists but Tauri not loading it. Builds still ad-hoc signed.
- 15:18 — Code signing WORKING. v11.28.88 tested, no network popup. #154 closed.
  Why: build.sh now sources .env before tauri build. Also signs crsqlite.dylib.
- 19:50 — Daily logs folder renamed memory/ → logs/.
  Why: Memory.md at agent level made the naming confusing.
- 21:10 — Note editor redesign filed (#219). Details panel too bulky, subtasks too heavy.
- 21:28 [saved] decision: "Daily logs capture everything, overlap with memory is fine"
- 22:03 [saved] preference: "Prefers concise commit messages"
- 22:15 [memory edited] preference: "Prefers dark mode" → "Prefers dark mode with high contrast"
- 23:01 [memory deleted] fact: "Uses VS Code" (user removed via Memory Settings)
```

**Rules:**
- Timestamps in user's local timezone (HH:MM format)
- One fact per line, prefixed with `- HH:MM —` (auto-extracted) or `- HH:MM [marker]` (audit entries)
- Optional `Why:` line indented below for context
- Narrative header: 3–6 sentence summary at top (generated end-of-day, see §5.6.5)
- **Format is mechanical, not AI-discretionary.** Backend constructs each entry from structured data.

#### 5.6.3A Audit-Entry Markers
> **Status:** BUILT (v0.63.0, #647/#672/#673) — extraction dual-write, Memory Settings audit markers, and extraction filter all implemented.

Every mutation to the memory system — extraction, edit, delete — leaves a timestamped marker in the daily log. These markers are the audit trail: "at this time, the system did this to your memory."

| Marker | Trigger | Format | Example |
|--------|---------|--------|---------|
| `[saved]` | Auto-extraction (§5.4) or manual save (§5.12) | `- HH:MM [saved] {category}: "{fact_text}"` | `- 14:03 [saved] preference: "Prefers dark mode"` |
| `[memory edited]` | User edits a fact in Memory Settings | `- HH:MM [memory edited] {category}: "{old_text}" → "{new_text}"` | `- 15:30 [memory edited] preference: "Prefers dark mode" → "Prefers dark mode with high contrast"` |
| `[memory deleted]` | User deletes a fact in Memory Settings | `- HH:MM [memory deleted] {category}: "{fact_text}" (user removed via Memory Settings)` | `- 16:45 [memory deleted] fact: "Uses VS Code" (user removed via Memory Settings)` |

**Audit markers are NOT re-extracted by the extraction pipeline.** The extraction pipeline (§5.4) must filter out lines matching the `[saved]`, `[memory edited]`, and `[memory deleted]` marker patterns before processing. This is a pattern-based filter matching the existing compaction exclusion approach (§5.4A): system-generated content is never treated as user input. Extracting from audit markers would create circular duplication (fact → log entry → re-extracted fact → another log entry).

**Audit markers are append-only.** They follow the daily log's append-only invariant — once written, never modified. Even if the underlying fact is later edited or deleted again, the original audit entry stays. The log is a complete history, not a current-state view.

#### 5.6.4 Writers (Four Sources)

**The chat agent is not a fifth writer (#3036 PR 3, R1).** Until this fix, two
separate pieces of per-message prompt text told the dispatched chat agent it
SHOULD call `append_to_note` on the daily log itself (`build_context_prompt`'s (`ai::chat`)
"## Daily Logging (IMPORTANT)" system-prompt block, injected whenever a
`daily_note_id` resolved, plus `execute_list_daily_logs`'s own tool-result
text, "Use append_to_note with the note ID to add entries") - directly
contradicting this section's "four writers" model and the backend guard
below. The guard already refused every one of those calls unconditionally
(`note_type="daily"` is on the `append_to_note` rejection list, same row in
the table above as `note_type='memory'`) - the instruction told the agent to
do something the backend had always refused, every time, for the life of the
feature. Both prompt-text locations are removed; the backend refusal is
unchanged (it was never the thing that was wrong). `execute_list_daily_logs`
now points the agent at `read_note` instead, matching §5.6.8's read-only
"what happened recently" framing, and ABSOLUTE RULE 16 (`build_context_prompt`, `ai::chat`,
formerly numbered 15-SYS; see #3036 PR 3's rule-numbering fix) still states
the prohibition explicitly ("Daily log: Do NOT write to it directly -
extraction pipeline handles this").

**1. Auto-extraction (primary input path):**
After the extraction pipeline (§5.4) saves a fact to `ai_memory`, it also performs a **best-effort append** to today's daily log. This dual-write (§5.4 step 4) is the primary way entries arrive in the daily log.

- Backend formats the entry mechanically: `- {HH:MM} [saved] {category}: "{fact_text}"`
- No AI judgment needed for formatting
- Best-effort: if the daily log append fails, the extraction still succeeds (DB row is authoritative). Log a warning; do not fail the extraction.
- Uses `append_to_note` (Tier 1/2) or flat file append (Tier 3)

**2. Manual Save (§5.12):**
When a user clicks Save to Memory on a chat message, the fact routes to the chosen destination (agent/project memory via router) AND appends to the daily log:
`- {HH:MM} [saved] {category}: "{fact_text}"`

Same format as auto-extraction. The daily log doesn't distinguish between auto and manual saves — both are `[saved]`.

**3. Memory mutations (audit trail):**
When a user edits or deletes a fact in Memory Settings:
- **Edit:** `- {HH:MM} [memory edited] {category}: "{old_text}" → "{new_text}"`
- **Delete:** `- {HH:MM} [memory deleted] {category}: "{fact_text}" (user removed via Memory Settings)`

These are written by the Memory Settings mutation handlers, not by the extraction pipeline.

**4. Narrative header (end-of-day):**
Generated once per day by the end-of-day agent, summarizing the day's entries (see §5.6.5).

#### 5.6.5 End-of-Day Agent

**Trigger:** Date-change check on the first AI response of a new day. When the current date differs from the last daily log filename:
1. Finalize yesterday's log (dedup + narrative header)
2. Create today's empty log file
3. Continue processing the message

```
on_ai_response():
  if today != last_log_date:
    finalize_yesterday()  // dedup + narrative header
    create_today_log()    // new empty file
    last_log_date = today
```

This is **event-driven, not cron-driven.** Works whether the app was just launched, has been open for a week, or was offline for 3 days (retroactively finalizes all missed days).

**Finalization steps (yesterday's log):**

1. **Dedup:** Scan for exact-match duplicate entries (same timestamp + same fact text). Remove duplicates, keeping the first occurrence. This catches error loops and legitimate double-extractions from retries. Dedup compares the full line text — partial matches are left alone.

2. **Narrative header:** Send yesterday's (now-deduped) entries to a Sonnet-tier model with a fixed prompt: *"Summarize these daily log entries in 3–6 sentences. Focus on what was accomplished, what decisions were made, and what's still in progress. Write in past tense, concise."*

3. **Prepend** the narrative header to the file, above all timestamped entries.

4. **No pruning.** The end-of-day agent never removes entries except exact-match duplicates. Every `[saved]`, `[memory edited]`, and `[memory deleted]` entry survives finalization. The narrative header is additive context, not a replacement for the entries below it.

**Failure handling:**
- If the LLM call fails: retry once after 30 seconds.
- If retry also fails: write a mechanical fallback header: `## {date} — {count} entries logged`
- **Verify:** Read file back, confirm header exists. If not, write mechanical fallback.
- Failure mode is cosmetic (no narrative summary), never data loss. The timestamped entries are always preserved.

**The end-of-day agent is Writer #2 in the three-writer model (§5.5).** It owns daily log files only. It never touches memory files. See §5.5.2 for the summary and §5.5.4 for the writer ownership table.

> **📌 Why This Exists — end-of-day agent (2026-04-23, ADR-0010):** The original §5.6.5 spec described a "narrative generation" step that was underspecified — it wasn't clear whether it could prune entries, how it handled duplicates, or what happened on failure. The three-writer model (ADR-0010) promoted this to a named writer with explicit constraints: dedup exact-match only, 3–6 sentence header, mechanical fallback, no pruning. The daily log's value is completeness; the narrative header adds readability without sacrificing it.

#### 5.6.6 Safety Guards

**Size cap: 50KB per daily log.** Normal heavy days produce 5-10KB. If the file hits 50KB, stop appending and surface a health warning (§16.3). This catches error loops.

**Dedup within same day.** Before appending, check if the exact same fact text was already logged today. If yes, skip. Catches error loops AND legitimate duplicate extractions.

**Zero-entry alert.** If the user chatted today AND the daily log has zero entries, surface a health warning. The daily log writer is broken.

**Timezone.** "New day" = midnight in user's local system timezone. Not UTC. A user in EST should not get their day split at 7 PM.

#### 5.6.7 Tier Behavior

| Tier | Auto-write | Narrative | Storage |
|------|-----------|-----------|---------|
| **Tier 1 (Pane Cloud)** | Backend → `append_to_note` | AI via Pane Cloud proxy | `.pane/system/{agent}/logs/` |
| **Tier 2 (BYOK)** | Backend → `append_to_note` | AI via user's API key | `.pane/system/{agent}/logs/` |
| **Tier 3 (OpenClaw)** | Backend → `append_to_note` (primary). OpenClaw can also write via `exec` (backup). | AI via OpenClaw | `.pane/system/{agent}/logs/` + OpenClaw cron as belt-and-suspenders |

All three tiers use the same code path. No tier-specific logic needed.

#### 5.6.8 AI Context Loading

- **On startup/compaction:** AI reads today's daily log + yesterday's daily log to recover context.
- **Search:** Daily logs are indexed by FTS5 (§5.9) and searchable via `search_memory` tool.
- **The daily log is the AI's "what happened recently" — Memory notes are "what do I know."**

### 5.7 AI Tools for Memory
> **Status:** PARTIAL — `append_to_note` server-side append and `edit_note` guards on system notes BUILT (v11.28.24). Writer-ownership clarification per ADR-0010 PENDING.

> **📌 Why This Exists — writer ownership clarified (2026-04-23, ADR-0010):** The old §5.7 said "Synthesis is the sole writer to Memory notes." That was true under the single-synthesis model but became imprecise when ADR-0010 split synthesis into three writers (router, consolidator, end-of-day agent). Two concrete failures earned this update: (1) the **2026-03-25 MEMORY.md overwrite incident** — multiple writers hit the same file with no coordination, wiping accumulated memory (documented in §2C.8); (2) the **"forget this" delete was dishonest** — a user's delete marked `is_active = 0` in the DB but the bullet stayed in the memory file because the delete path had no file-write permission. Both failures trace to the same root: "one writer per file" was a principle without a mechanical ownership table. This section now codifies which writer owns which target file and what operations each is allowed to perform.

The AI has specific tools for memory operations. All AI-facing tools are intentionally narrow — they cannot write to memory files or daily logs directly.

| Tool | Purpose | Safety |
|------|---------|--------|
| `append_to_note` | Add content to end of any user note | Server-side append. AI sends only new text. Cannot truncate. **Rejected on notes with `note_type='memory'` or `note_type='daily'`** (see §5.4 mechanical guard). |
| `edit_note` | Replace full note content | **Blocked on system notes** (note_type set). Content loss guard rejects >50% reduction on any note. |
| `create_note` | Create new notes | No restriction |
| `search_notes` | Find notes by **content** (full-text over `notes_fts`), returning note IDs + content snippets — see §5.7.3 | No restriction |

**System notes** (Memory, Soul, Rules, Identity, User, Tools) are **append-only** via AI tools. The AI cannot rewrite them through `edit_note`. Memory notes and daily logs are further restricted: `append_to_note` is also blocked on them. Only the memory pipeline (§5.5) can write to these files.

#### 5.7.1 Writer ownership table

Under ADR-0010's three-writer architecture, each target file has exactly one owning writer for each operation:

| Target file | Writer | Operations | Never writes |
|-------------|--------|-----------|--------------|
| Agent MEMORY.md | **Router** | append bullet, remove bullet | LLM rewrite |
| Agent MEMORY.md | **Consolidator** | full rewrite (from `ai_memory`) | append, remove bullet |
| Project Memory note | **Router** | append bullet, remove bullet | LLM rewrite |
| Project Memory note | **Consolidator** | full rewrite (from `ai_memory`) | append, remove bullet |
| Daily log | **Extraction pipeline** | append timestamped fact, append audit marker (`[saved]` / `[memory edited]` / `[memory deleted]`) | dedup, narrative header |
| Daily log | **End-of-day agent** | dedup existing entries, prepend narrative header | append new facts |
| Any user note | User (editor) and AI tools (`create_note`, `edit_note`, `append_to_note`) | all CRUD | none — user notes are not pipeline-managed |

**Router has exactly three write paths:** append bullet (for routing new facts), remove bullet (for propagating deletes), and nothing else. No other operation on memory files is allowed from the router.

**Consolidator has exactly one write path:** full file rewrite, sourced from `ai_memory WHERE is_active = 1`. No partial edits.

**End-of-day agent has exactly one write path:** prepend narrative header to yesterday's daily log + dedup its body. No writes to memory files.

#### 5.7.2 Pane editor is read-only for all system notes

Any note with a non-null `note_type` opens **read-only** in the Pane editor. This applies uniformly to all seven system note types:

- `note_type = 'memory'` — agent MEMORY.md, project Memory notes
- `note_type = 'daily'` — daily logs
- `note_type = 'soul'` — agent SOUL.md
- `note_type = 'rules'` — agent RULES.md
- `note_type = 'identity'` — agent IDENTITY.md
- `note_type = 'user'` — USER.md
- `note_type = 'tools'` — TOOLS.md

The "Open in Editor" button on any of these notes exists as a **read-only view affordance** — users can open a file to inspect its current rendered state, but the editor does not accept edits on these note types. User notes (any note with `note_type IS NULL`) remain fully editable.

**Write surfaces by note type:**

| Note type | Supported write surface |
|-----------|------------------------|
| `memory` | Memory Settings (§5.12) — inline edit/delete on facts; consolidator and router render the file from `ai_memory` |
| `daily` | Memory pipeline only — extraction (§5.4) appends, end-of-day agent (§5.5.2) dedups and headers |
| `soul` / `rules` / `user` / `tools` | Onboarding wizard creates these at first run (§6.2). Post-onboarding UI editing is deferred per ADR-0011 |
| `identity` | Onboarding agent creation writes initial content (§6.2). Settings rename updates on agent rename. Integrity check writes stub if missing. See ADR-0039. |

**View surface for system notes:** The Agents settings page is the canonical surface for inspecting all system notes alongside per-file token cost (see #682). Until that page ships, "Open in Editor" remains the sole inspection path; the read-only rule applies regardless.

**Escape valve:** Direct filesystem edits at `~/Pane/.pane/system/{agent}/` are possible but unsupported. For memory files specifically, filesystem edits are overwritten by the next consolidation. For other system notes (soul, rules, identity, user, tools), filesystem edits persist but bypass any UI validation, formatting checks, or token-cost feedback. Power users who choose this path own its consequences.

See ADR-0011 for the decision rationale.

> **📌 Why This Exists — uniform read-only on all system notes (2026-04-24, ADR-0011):** The earlier version of this section (Cluster C, 2026-04-23) made memory and daily files read-only while saying soul/rules/identity/user/tools "remain user-editable in the Pane editor." Investigation surfaced two failures behind that asymmetry. First, the code had been enforcing uniform read-only on all seven types since commit 48f5939 (March 14, v11.21.0) — a behavior that drifted into existence as part of an unrelated synthesis revert with no decision doc, no linked issue, and no rationale recorded. Second, the asymmetry had no architectural justification: memory and daily files needed the read-only rule to prevent silent overwrite during consolidation, but extending the rule to soul/rules/identity/user/tools cost nothing (they had no in-UI edit surface anyway, since the onboarding wizard is onboarding-only) and bought a uniform mental model — system notes are AI-managed, viewed in Agents settings, written through their respective specialized surfaces. ADR-0011 records the call to keep code and spec aligned at uniform read-only.

#### 5.7.3 search_notes Content-Search Contract
> **Status:** FTS5 content search BUILT (#638). Hybrid (FTS5 + semantic) **NOT BUILT** — amended 2026-06-24. *Rationale: live test (v0.67.41) confirmed the AI uses `search_notes` for note queries, so an FTS-only `search_notes` never reaches the indexed note embeddings — a conceptual query ("early american leaders") returns nothing. Making `search_notes` hybrid puts semantic retrieval on the path the AI actually uses; chosen over re-routing the AI to `search_memory`.*

`search_notes` is the AI's note search path. It returns notes matched by **keyword or by meaning** — FTS5 keyword match and semantic (embedding) search, merged into one result set, the same hybrid pattern §5.9 defines for `search_memory`.

**Retrieval.** `search_notes` performs hybrid retrieval over user notes:

- **FTS5 keyword match** against `notes_fts`, kept current by the §5.4 live triggers (`notes_fts_insert` / `notes_fts_update`) that index note `body` on every create/edit/append. This is the always-on baseline. It MUST NOT use a `title LIKE` query *(the original #638 bug: `WHERE title LIKE ?1` is title-only and never touches the index, so a content query such as "Washington" returns nothing even when the body contains it)*.
- **Semantic (embedding) search** over the note embeddings, gated **identically to `search_memory`**: the semantic pass runs only when `embeddings_enabled == "true"` AND `embeddings_status` is `"ready"` or `"partial"`. Otherwise `search_notes` falls back to FTS-only.
- Keyword and semantic hits are merged into one result set with combined ranking, as §5.9 / §5.15 define for `search_memory`.

**Return shape.** Unchanged from #638. Each result MUST include, at minimum:

- `note_id` — the stable note identifier, so the AI can navigate to or read the matched note (`read_note` / `get_note`). Bare title strings are insufficient for navigation.
- `title` — for display.
- `content_preview` — a snippet of the note body around the match (FTS5 `snippet()`, or the stored `content_preview` column).
- existing metadata as today (`project_id`, `status`).

Semantic results merge into this same shape. Returning a list of title strings remains non-conformant.

**Ordering & limit.** Combined keyword + semantic ranking; retain the existing result limit. When the semantic pass is gated off, ordering is FTS5 relevance (`fts.rank`) as before.

**Baseline & degradation.** FTS5 is always the baseline — `search_notes` works whenever `notes_fts` is populated, which is always (live triggers plus the startup rebuild). The semantic pass is additive and gated as above; when embeddings are disabled, not ready, or unavailable, `search_notes` runs FTS-only with no loss of availability.

**Surface boundary.** `search_notes` searches user notes only (`notes` / `notes_fts` and their embeddings). `ai_memory`, chat history, and the archive remain `search_memory`'s surfaces (§5.9, §5.14). The precise `search_notes`-vs-`search_memory` boundary and any de-duplication of overlapping coverage is tracked under **#765** and is out of scope here.

### 5.8 Memory Protection
> **Status:** PARTIAL (v11.28.24) — content loss guard ✅, version history UI exists (#70 closed) but backend not fully wired

**1. Content loss guard on system notes:**
- `edit_note` on any note with `note_type` set: if new content < 50% of old content, reject with error message
- Prevents AI from accidentally truncating accumulated memory
- Does NOT block: `append_to_note`, user edits, synthesis pipeline

**2. Confirmation on destructive user edits:**
- If user clears or bulk-deletes Memory/Soul content: show confirmation dialog
- "Are you sure? This will erase what your AI knows about you. Restore from version history."
- Does NOT block: normal edits, appending content

**3. Silent version history:**
- Every Memory/Soul modification saves the previous version automatically
- Stored in `ai_memory_versions` table
- Accessible from right-click → "Version History"
- User can restore any previous version with one click
- Retention: last 50 versions

```sql
CREATE TABLE ai_memory_versions (
    id          TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    note_id     TEXT NOT NULL,
    content     TEXT NOT NULL,
    modified_by TEXT NOT NULL,    -- 'user' | 'ai_extraction' | 'ai_synthesis' | 'ai_append' | 'onboarding'
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### 5.9 Memory Search
> **Status:** FTS5 keyword search BUILT (v11.28.24, #66) — always-on default. Semantic search redesigned to **opt-in** (ADR-0048, supersedes the #66 auto-enable / auto-download model) — **NOT BUILT**.

**All tiers have memory search.** This is essential for the AI to find relevant memories as they accumulate beyond what fits in the context window. **Keyword search (FTS5) is the always-on default for every user; semantic search is opt-in and OFF until the user explicitly enables it.**

**Full-text search (FTS5)** — over the `ai_memory` table, notes (`notes_fts`, §5.7.3), and chat history — keyword matching, fast, no external dependencies, no downloads. **Always active on all hardware, for every user.** It is the default and the only search path until the user opts into semantic search. When semantic search is off, disabled, still indexing, or errored, search runs FTS-only — this is the normal default path, not a degraded fallback.

#### 5.9.1 Semantic search is opt-in (ADR-0048)

Semantic search is **OFF by default**. Nothing about it is automatic: no model download, no indexing, and no RAM-based auto-enable. The previous behavior — auto-enabling local embeddings when ≥16 GB RAM was detected at startup, and auto-downloading the model — is removed. ADR-0048 supersedes the #66 decision.

Enabling is an explicit user action in **Settings → AI → Semantic Search**:
- A toggle, default OFF.
- Turning it on prompts the user to choose an engine (§5.9.2).
- Only after the user enables it and selects an engine does any indexing occur (Pane connects to the chosen provider's embeddings endpoint; it downloads no model).
- Until then, embeddings are never fetched, the pipeline never runs, and search is FTS-only.

#### 5.9.2 Engine options

When the user enables semantic search, they choose one embedding engine. All three are HTTP calls to the provider's embeddings endpoint — Pane runs no model in-process and downloads nothing.

| Engine | Model(s) | Notes |
|---|---|---|
| **Ollama** | e.g. `nomic-embed-text` (dimensions vary, ~384–1024) | The user's local Ollama via `/api/embed`; needs the Ollama endpoint. The private option — embeddings run on the user's own Ollama, nothing leaves their machine. |
| **OpenRouter** | `text-embedding-3-*` (1536–3072-dim) | Hosted; requires an OpenRouter key. Embeds note/fact/chat content via a third-party API. |
| **Venice** | `text-embedding-bge-m3` and other Venice `/embeddings`-capable models (#2988) | Hosted; POSTs to Venice's OpenAI-shaped `/embeddings` endpoint. Reuses the existing Venice key from Provider Connections; there is no Venice field in the Semantic Search card and no separate key entry. |

Embedding engine availability is gated on embeddings API support (ADR-0065 S1), not a fixed provider set.

**Credential source (C50 / #3171 PR 1 — supersedes the pre-C50 "card field is the source of truth" rule below).** Semantic Search has **no key-entry field for any provider**. Both key-based engines read their key from **Provider Connections** at call time, exactly like Venice already did and exactly like §39.2's image-generation picker does for the same two engines: OpenRouter via `credential_migration::read_provider_key(conn, "openrouter")`, Venice via `credential_migration::read_venice_key` (unchanged — Venice was already the target shape; see below). The provider dropdown only ever offers openrouter/venice when Provider Connections already has a key for them (`ssProviderOptions`, filtered to `connected`); Ollama is unaffected (endpoint, not a key) and remains always offered. A missing connection key errors before any network call — it never falls back to an empty key.

**What this replaces.** Before C50, the Semantic Search card had its own `embeddings_api_key` field (plaintext in `ai_settings` — a credential-handling defect against ADR-0044, not merely a UI inconsistency) that was the authoritative source for the OpenRouter engine, pre-filled from a Smart Features credential for convenience (Bug #1657's fix). That field, its plaintext storage, and the pre-fill convenience are all retired. `credential_migration::run_feature_key_consolidation` performs a one-time backfill-if-absent-else-discard, copy-confirm-clear migration out of `embeddings_api_key` into the OpenRouter Provider Connection (see that function's doc comment).

**Venice was already the target shape (#2988) — now the norm, not the exception.** Venice's `/embeddings` endpoint shares the same single Bearer key as `/chat/completions` and `/models` — there is no separate embeddings scope on Venice's side, which is why its arm has read `credential_migration::read_venice_key` (keychain-only, ADR-0065 D4) directly since #2988, never the old `embeddings_api_key` slot. C50 made OpenRouter's arm do what Venice's arm already did, closing the gap `runtime.md` §39.1 named forward ("C50 says the AI tab already asks for the OpenRouter key three times").

**Anthropic has no embeddings.** Anthropic offers no embeddings API (it redirects to Voyage AI — separate billing, not integrated). A user whose Smart Features provider is Anthropic therefore must use **Ollama**, enter an **OpenRouter key**, or configure **Venice** as their chat provider, for semantic search. There is no Anthropic-embeddings path.

#### 5.9.3 Re-index on engine switch

Embedding dimensions differ by engine (Ollama ~384–1024; OpenRouter 1536–3072). Vectors from one engine are not comparable with another, so **switching engines invalidates the existing index**. On an engine switch the app MUST:
1. Clear the existing `embeddings` rows (the prior engine's vectors).
2. Re-run the indexing pipeline under the new engine (reconnect to the new provider's endpoint).
3. Hold `embeddings_status` at a non-`ready` value until the re-index completes, so the `search_memory` semantic gate falls back to FTS-only meanwhile (no stale cross-engine matches).

Disabling semantic search returns the user to the FTS-only default; existing vectors MAY be cleared or retained per implementation, but the semantic gate is off regardless.

> **fix(memory-pipeline) (#3120 PR 4, #3101):** the re-index-on-switch predicate above now clears on an engine change OR a MODEL change within the same engine (`embeddings.model IS NULL OR model != ?` added alongside the existing `engine` check) — switching models within one engine (e.g. OpenRouter `text-embedding-3-small` → `qwen3-8b`) previously cleared nothing, since the engine string was unchanged; the old model's vectors survived to be scored against the new model's query vector with no dimension guard, producing plausible-looking but meaningless similarity scores. `semantic_search` (§5.15) now also skips (logs, never scores) any stored vector whose dimension doesn't match the query vector's — a correctness backstop for any vector that reaches scoring despite the clear (e.g. a race during re-index), since a dimension mismatch is a bug, not a low score. The `embeddings` table gains a `model TEXT` column (plain `ALTER TABLE ADD COLUMN`, not CRR-tracked — same precedent as the pre-existing `engine` column; `SCHEMA_VERSION` unchanged). `search_notes`'s semantic-search similarity cutoff drops from a fixed `0.5` to `0.35` (`embeddings::SEMANTIC_SEARCH_SIMILARITY_THRESHOLD`) — `0.5` was dropping conceptually correct hits (reproduced: the query "shipping announcement workflow" against a Release Playbook note scored below 0.5 and was excluded even though it was the right answer); `semantic_search` additionally logs its top-3 scored candidates on every call, regardless of threshold, so a near-miss is diagnosable from terminal output instead of silently vanishing.

**The re-index MUST NOT block the UI thread (#3082).** The main-thread rule, stated precisely so the next person doesn't delete the test that enforces it (#3084's lesson): a `#[tauri::command]` declared as a plain `pub fn` (not `async fn`) runs **in-line on Tauri's main/event-loop thread**, the same thread every other IPC call dispatches on. There is no implicit blocking thread pool that a synchronous command is secretly routed through — `async fn` plus an explicit `tokio::task::spawn_blocking(...)` around the blocking work is the only mechanism that moves it off that thread (the same shape R7/#1921 established for backups, and #1296 for initial sync). `switch_embedding_engine` follows this shape: validation stays synchronous (pure, no I/O), then the entire settings-write / stale-row-clear / credential-build / pipeline-run / terminal-status-write sequence runs inside one `spawn_blocking` closure, with the `JoinError` (thread panic) surfaced back through `.await.map_err(...)?` rather than swallowed.

**Moving the re-index off the main thread makes a second, concurrent switch reachable for the first time** — the previous main-thread block serialized every call by construction; it no longer does. A second `switch_embedding_engine` call while one is still running **MUST be rejected outright**, not queued and not silently interleaved: both calls would otherwise race the stale-row `DELETE`, orphan rows tagged with the first call's abandoned engine, and the terminal `embeddings_status`/`embeddings_failed_count` write would become last-writer-wins non-deterministic. The guard (`EmbeddingSwitchGuard`, an `AtomicBool` + RAII drop, the same shape as backup's `BackupGuard`) is acquired as the first statement inside the command, before `spawn_blocking` starts, and returns `Err("a re-index is already running")` if already held. This is a reject, not a cancel-and-restart — true cancellation (stopping a re-index already in flight) is explicitly out of scope here and depends on the harness Stop plumbing; it is not built by this fix.

#### 5.9.4 State & persistence

- `embeddings_enabled` (`ai_settings`): default **false**, no longer RAM-derived. Set true only by the explicit user toggle.
- `embeddings_engine` (`ai_settings`): durable record of the chosen engine (`local` | `ollama` | `openrouter` | `venice`); openrouter and venice both resolve their key from Provider Connections at call time (C50 / #3171 PR 1, §5.9.2) — no engine-specific credential is persisted in `ai_settings` anymore.
- `embeddings_status` (`ai_settings`): durable terminal state — `ready` / `partial` / `error: …` / building. `partial` means the index completed but some items had transient embed failures; the index is still usable for search. The downstream `search_memory` semantic gate accepts both `ready` and `partial`. `embeddings_failed_count` (`ai_settings`) records the number of failed items when status is `partial`.
- Index counts (notes, facts, chats) are derived at display time from a live `SELECT source, COUNT(*) FROM embeddings GROUP BY source` query, not persisted per-run. This ensures counts reflect the actual index state and survive Settings reopens without stale-zero display.
- `embeddings_max_tokens` (`ai_settings`): the embedding model's context-length ceiling in tokens. Sourced in priority order: (1) OpenRouter's live `context_length` field from the model list API, threaded through at engine-switch time; (2) a static seed table for Ollama models whose API does not report context length (entries verified against upstream model cards — e.g. bge-base → 512, nomic-embed-text → 8192); (3) a conservative floor of **512** with a logged warning ("unknown model, using conservative floor"). No bare magic number is authoritative — callers always read the persisted value.
- Default install state: `embeddings_enabled = false`, no `embeddings_status`, FTS-only.

**Content truncation before embedding.** Every embedding sub-pass (notes, facts, chats) truncates content to fit the model's token limit before sending. The truncation target is `floor(embeddings_max_tokens × chars_per_token × 0.9)` characters, where `chars_per_token` is the `TokenEstimationRatio` heuristic from `context_budget` and the 0.9 factor is a safety margin absorbing estimation error. The result is clamped to a UTF-8 code-point boundary (`floor_char_boundary`). All three sub-passes — notes, facts, and chats — use the same formula. The facts sub-pass must not skip truncation; content of any length can appear in `ai_memory` rows.

#### 5.9.5 Progress reporting

Index progress is surfaced via the `embeddings://status` event contract (`docs/spec/637-embedding-progress-events.md`, ADR-0047). Under opt-in there is no model download, so the relevant stages are **off → indexing → ready / error**. The #637 `downloading` and `loading` stages do not apply to this flow (there is no Pane-side model download or in-process engine load); those variants remain defined in the #637 contract but are not emitted on this no-local-model path. Progress renders **inside the Settings → AI → Semantic Search section** the user just interacted with — not as a silent background indicator. This spec does not redefine the event contract; see #637.

**`/embeddings` calls are batched (#3082).** Rather than one HTTP call per note/fact/chat item, the re-index pipeline sends up to `embeddings_batch_size` items (`ai_settings`, default **25** — chosen to match the existing per-25-item progress-throttle cadence above, so one full batch lines up with one progress tick with no new cadence logic) in a single `/embeddings` request with an array `input`. The batch size is read from settings on every run, never a bare literal, and is not currently user-editable in the UI (a future Settings control, or a direct `ai_settings` write, can override it without a code change).

**Batch failures degrade to per-item attribution, never whole-batch attribution.** A provider's `/embeddings` endpoint returns one HTTP response for the whole array — there is no partial success within a single request, so a batch cannot by itself identify which item was the problem. On any non-auth batch-level failure (HTTP error, JSON error, or a returned vector count that doesn't match the request — the latter is treated as a failure outright, never positionally zipped against a response shape that wasn't expected), the pipeline falls back to the pre-#3082 one-call-per-item path **for that one batch only**, reusing the existing `embed_with_token_retry` retry logic unchanged. This means a single bad item inside a batch of 25 is attributed to that item alone — the other 24 succeed normally — and a batch that fails as a unit never marks an unrelated item as failed or succeeded based on another item's outcome. An auth failure (401/403) is never retried per-item; it aborts the whole pipeline immediately, exactly as the unbatched path always has. The `embeddings://status` event contract itself is unchanged by batching — only how many HTTP calls produce the counts it reports.

**Hybrid ranking.** When semantic search is enabled and `embeddings_status` is `"ready"` or `"partial"`, search combines FTS + semantic results (combined ranking, §5.15). Otherwise search is FTS-only. There is no automatic system detection — the mode follows the user's explicit choice and the index state.

**What's searchable:**
- All memory notes (personal, work, daily)
- Chat transcripts stored in Pane's local DB
- Note content across all projects

**Context loading:**
- Personal memory: always in context, every conversation
- Work memory: loaded when user is in that project
- Daily memory: today + yesterday loaded on startup and after compaction
- Everything else: retrieved via search when relevant

### 5.10 Chat Transcript Storage
> **Status:** BUILT (v11.28.24) — (#67 closed)

Pane stores the full conversation transcript locally:

- After each AI exchange, Pane saves the complete message (user message, AI response, tool calls, tool results) to the local SQLite DB
- Transcripts are available for scroll-back in the chat UI
- Transcripts are indexed for memory search
- Transcripts are NOT curated memory — they're raw data

**Tier-specific behavior:**
- **Tier 1/2:** Pane stores all transcripts locally. This is the only record.
- **Tier 3 (OpenClaw same machine):** OpenClaw already stores session transcripts locally. Pane detects local OpenClaw and skips duplicate transcript storage. OpenClaw's transcripts are the record.
- **Tier 3 (OpenClaw different machine):** Pane stores transcripts locally from gateway responses, ensuring the user's machine has the complete record even though OpenClaw runs elsewhere.

### 5.11 Tier Integration
> **Status:** PARTIAL — Tier 3 (OpenClaw) working, Tier 1/2 not built

#### Tier 1 (Pane Cloud) & Tier 2 (BYOK)

> **Status:** N/A (tier design doc)

- Pane handles everything: extraction, storage, search, synthesis
- All data in local DB + flat files on user's machine
- Pane Cloud processes AI requests but does not retain user data
- Memory search via FTS5 + local embeddings

#### Tier 3 (OpenClaw)

> **Status:** N/A (tier design doc)

- **Pane owns memory.** OpenClaw does not perform memory operations.
- Pane is the brain. OpenClaw is the hands (tool execution only).
- Flat files on user's machine are the bridge:
  - Pane writes memory → flat file write-back to `~/Pane/.pane/system/{agent}/`
  - OpenClaw's workspace config points to that folder (symlink or direct path)
  - OpenClaw reads flat files on startup and for `memory_search`
- Pane stores transcripts locally from gateway responses
- **When Pane is connected, OpenClaw should NOT inject its own system prompt** — only send the user's message with thread_id
- The AI talks to the user through Pane only. Other surfaces (Discord, Telegram) are not used when Pane is the interface.

### 5.12 Manual Memory Management
> **Status:** PARTIAL (v11.28.24) — save to memory built (#69 closed), memory management page commands built (#68 closed), "Open in Editor" info note BUILT (#676). `goal` category added to save-to-memory dropdown (#760). Health dashboard not built (#164 open). Inline edit/delete on routed/consolidated facts PENDING per ADR-0010.

#### 5.12.1 Save to Memory (chat message action)
> **Status:** BUILT — save button exists; category picker includes all 8 categories including `goal` (#760).

Every chat message has a **save icon** (bookmark/floppy). Clicking it:

1. Opens a category picker dropdown with all 8 categories: `identity`, `preference`, `style`, `goal`, `fact`, `decision`, `deadline`, `reflection`. See §5.4A for category definitions.
2. **Smart default based on category:**
   - `identity` / `preference` / `style` / `goal` (no project context) → routes to agent MEMORY.md
   - `goal` / `fact` / `decision` (in project context) → routes to that project's Memory note
   - `fact` / `decision` (no project context) → daily log only
   - `deadline` / `reflection` → daily log only

   (Matches the routing table in §5.5.)
3. User can override the destination.
4. The full message content is extracted as a structured fact and saved to `ai_memory` DB table with `is_active = 1` and `routed_at = NULL`.
5. **Also appended to daily log (§5.6):** `- {HH:MM} [saved] {category}: "{fact_text}"`. Daily log gets every manual save regardless of primary destination — same dual-write pattern as auto-extraction (§5.4).
6. Router (§5.5.1) picks up the fact on its next run and routes it to the chosen memory file.
7. Toast confirmation: "Saved to memory" with dismiss.

This replaces the deprecated pin feature (§4). The save icon does everything pinning did, plus categorization and proper routing.

#### 5.12.2 Memory Management Page (Settings → AI → Memory)
> **Status:** PARTIAL — list/edit/delete commands (`get_all_memories`, `update_memory`, `delete_memory`) built; "Open in Editor" info note BUILT (#676). `goal` category in UI and post-routing/post-consolidation edit propagation PENDING per ADR-0010.

The Memory Management page is the supported write surface for viewing, editing, and deleting memory facts. It operates on the `ai_memory` table, which ADR-0010 established as the source of truth for memory content.

**What the page shows:**

- All active memories (`is_active = 1`), grouped by category
- All 8 categories displayed including `goal` (with its own color pill, consistent with the other 7)
- Filter by: layer (personal / work / daily), category, project
- Search by fact text (client-side substring match; extends to FTS5 post-launch per §5.15)
- Per-entry: fact text, category pill, created date, last referenced date, project label (if `layer = 'work'`)

**What the user can do:**

- **Edit** — click a fact, edit in place, save. Updates `ai_memory.fact`. See §5.12.3 for how edits propagate to memory files and daily logs.
- **Forget** — soft-delete a fact. Sets `is_active = 0`; the fact is hidden from the page but recoverable. Confirm copy should reflect that this is reversible (e.g., "Forget this memory? You can restore it from the deleted list."). Router removes the corresponding bullet from the memory file immediately; consolidator excludes the fact on next pass.
- **Toggle** — show memory toasts ON/OFF, memory extraction ON/OFF (existing settings).
- **Health badges** — green/yellow/red on memory file sizes (§16.3), surfaces when a file is approaching the consolidation threshold.

**Open in Editor (read-only view):**

Memory files and daily logs open **read-only in the Pane editor** per §5.7.2. The Memory Management page includes an "Open in Editor" button next to each memory file (agent MEMORY.md, project Memory notes), alongside an info note:

> ℹ️ Memory files are auto-regenerated from extracted facts and open read-only. To add or change a fact, use Memory Settings or "Save to Memory" in chat.

This lets users inspect the current rendered state of their memory files without introducing an editable surface that would silently lose data on the next consolidation.

#### 5.12.3 Edit and delete propagation across stages

> **Status:** BUILT (v0.63.0, #674) — delete propagation wired (router remove-bullet on routed facts); edit drift is by design per §5.12.3.

A fact passes through three stages: staged (pre-router, `routed_at IS NULL`), routed (post-router, bullet exists in memory file), consolidated (absorbed into prose). The Memory Management page provides the same edit/forget actions at every stage; what differs is how changes propagate to the rendered files and daily log.

| Stage | Edit action | Delete action |
|-------|-------------|---------------|
| **Staged** (`routed_at IS NULL`) | Update `ai_memory.fact`. Append `[memory edited]` audit marker to today's daily log (§5.6). | Set `is_active = 0`. Append `[memory deleted]` audit marker to today's daily log. Router ignores the fact on next run. |
| **Routed** (bullet in memory file) | Update `ai_memory.fact`. Append audit marker. **Memory file bullet drifts** — the old bullet text stays until next consolidation. The AI still sees the correction via the daily log audit marker injected in context. | Set `is_active = 0`. Append audit marker. **Router removes the exact-text bullet from the memory file immediately** (dedicated remove-bullet path, §5.5.1). No LLM call, no wait. |
| **Consolidated** (absorbed into prose) | Update `ai_memory.fact`. Append audit marker. Next consolidation re-renders the file from the updated fact list. | Set `is_active = 0`. Append audit marker. Next consolidation re-renders the file without the fact. |

**Key asymmetry:** edits can drift up to the consolidator's staleness window (~14 days); deletes cannot. Router removes bullets immediately, and `is_active = 0` filters out of consolidation input on the next run either way.

All audit markers are append-only entries to today's daily log — never yesterday's, never rewrites in place. See §5.6 for audit marker format.

> **📌 Why This Exists — inline edit/delete across all stages (2026-04-23, ADR-0010):** Memory Settings in the earlier model could only edit staged facts. After routing, the fact also lived as a bullet in a memory file; after consolidation, as a sentence in prose. The edit UI only touched the DB row, so "edits" and "deletes" silently failed for anything that had already been routed or consolidated. Making `ai_memory` the source of truth and giving the router a remove-bullet path closes the loop — users can now honestly edit or forget a fact at any stage. The daily log audit marker makes every change traceable, and the consolidator's read-from-`ai_memory` ensures rendered files catch up within the staleness window.

### 5.14 Long-Term Transcript Storage & Search
> **Status:** NOT BUILT

*Added 2026-03-18 (Matt/Brit decision). Store everything, load only what matters.*

#### 5.14.1 Core Principle

Transcripts grow forever. AI context stays fixed. The memory pipeline bridges the gap — it distills conversations into structured knowledge so the AI gets smarter without getting slower. Raw transcripts are the permanent archive. Extracted facts are the working memory.

**Three layers:**

| Layer | Purpose | Grows? | In AI Context? | Searchable? |
|-------|---------|--------|----------------|-------------|
| **Raw transcripts** | Complete record of every message | Yes, forever | ❌ Never | ✅ Always (FTS5) |
| **Extracted facts** | Structured knowledge from conversations | Yes, managed by synthesis + decay | ✅ Selectively (by relevance) | ✅ Always |
| **Synthesized memory** | Prose summaries per project/agent | Bounded by health guard (§16.3) | ✅ Always (current context) | ✅ Via notes search |

#### 5.14.2 Transcript Retention

**Rule: Never delete transcripts.** Every message the user or AI sends is permanently stored. Users own their data — we don't decide what's worth keeping.

**Active storage** (0–30 days): Full messages in `chat_messages_local` table (local-only, not the legacy `ai_chat_messages` synced table). Indexed by FTS5. Fast to query. These are the conversations the AI might reference naturally ("we talked about this yesterday").

**Archive storage** (30+ days): Messages moved to `chat_archive` table. Same core schema plus `archived_at` column, separate FTS5 index, separate table for performance. The active table stays small and fast. Archive is queryable but not scanned on every search unless the user explicitly asks for deep search.

**Migration:** A startup-check background job moves messages older than 30 days from `chat_messages_local` to `chat_archive`. No data loss — just table migration.

**Trigger mechanism:** On app startup, check `last_archived_at` timestamp (stored in `ai_settings`). If ≥7 days since last run (or never run), execute the migration. Follows the startup-check pattern from the consolidator (§5.5.3), 7-day interval here vs consolidator's 14. No external cron, no interval timer — the check runs once at startup and the migration executes async after UI is ready (non-blocking). If the app stays open for weeks without restart, the archive runs on the next startup. This is acceptable because the 30-day boundary is a performance optimization, not a correctness requirement — a few extra days in the active table costs nothing at these volumes (§5.14.6).

```sql
CREATE TABLE chat_archive (
    -- Matches chat_messages_local core columns plus archived_at
    id          TEXT PRIMARY KEY,
    thread_id   TEXT NOT NULL,
    project_id  TEXT,
    role        TEXT NOT NULL,        -- 'user' | 'assistant' | 'system'
    content     TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    metadata    TEXT,                 -- JSON: tool calls, model, tokens used
    archived_at TEXT NOT NULL         -- when it was moved to archive
);

CREATE VIRTUAL TABLE chat_archive_fts USING fts5(
    content, 
    content_rowid='rowid',
    tokenize='porter unicode61'
);
```

#### 5.14.3 Search Architecture

> **Status:** BUILT (v0.63.0, #678) — `deep` parameter added to `search_memory` tool. Archive FTS search wired. Context snippets PENDING.

> **Note (#1679):** `search_memory` note results include the note UUID as `[{uuid}] {title}` (matching `search_notes` format) so the AI can chain directly to `read_note` without a separate lookup.

**Two-tier search:**

1. **Quick search** (default): Searches `chat_messages_local` (last 30 days) + `ai_memory` (all facts) + notes FTS. Fast, covers most "what did we discuss recently" queries.

2. **Deep search** (explicit): Searches everything — active messages, archived messages, all facts, all notes. Triggered by: user clicking "Search all history" toggle, or AI detecting a time reference older than 30 days ("what did we talk about in February?").

**AI search tool update:** The existing `search_memory` tool gets an optional `deep: boolean` parameter. Default `false` = quick search. `true` = includes archive. AI decides based on the question — recent topics use quick, historical questions use deep.

```
search_memory({ query: "float therapy business", deep: true })
→ Returns: matching facts + matching transcript snippets (with timestamps and project context)
```

**Transcript snippet format:** When search returns a transcript match, it includes surrounding context (2 messages before and after the match) so the AI can understand the conversation flow, not just an isolated message.

#### 5.14.4 AI Access to History

When the user asks "what did we discuss about X last month?" the AI:

1. Calls `search_memory({ query: "X", deep: true })`
2. Gets back: matching extracted facts + matching transcript snippets (supporting context)
3. Responds with a summary, citing dates and project context
4. Can offer to navigate to the original conversation thread

The AI never loads full transcripts into context. It gets search results — snippets with context. This keeps token usage bounded regardless of how many months of history exist.

#### 5.14.5 Per-Tier Behavior

| Tier | Transcript Storage | Search | Archive |
|------|-------------------|--------|---------|
| **Tier 1 (Pane Cloud)** | Local SQLite on user's machine | Local FTS5 | Local migration job |
| **Tier 2 (BYOK)** | Local SQLite on user's machine | Local FTS5 | Local migration job |
| **Tier 3 (OpenClaw)** | Local SQLite + OpenClaw session files (redundant) | Local FTS5 + OpenClaw memory search | Local migration job |

**All tiers:** Transcripts are on the user's machine. No server-side storage. Sync (§9) encrypts and syncs the archive table like everything else — so cross-device search works.

**Tier 3 note:** OpenClaw also stores its own session transcripts (`.jsonl` files). These are redundant with Pane's local storage. Pane's copy is authoritative. OpenClaw sessions can be cleaned up without data loss as long as Pane's DB is intact.

#### 5.14.6 Storage Projections

Estimated storage for a power user (50 messages/day):
- **Year 1:** ~18,000 messages → ~50MB uncompressed SQLite
- **Year 3:** ~54,000 messages → ~150MB
- **Year 5:** ~90,000 messages → ~250MB

This is negligible on any modern machine. No compression needed at these scales. FTS5 indexing adds ~30% overhead. Total: ~325MB for 5 years of complete history.

#### 5.14.7 Data Export

Users can export their complete history at any time:
- **Settings → Data → Export Chat History** → generates a `.json` or `.md` file with all messages
- Includes: timestamps, project context, AI model used, tool calls
- This is a regulatory/trust feature — users must be able to take their data and leave

### 5.13 Known Limitations (Launch)
> **Status:** N/A — documentation of known gaps

1. **Team memory layer** — not built. Post-launch feature for multi-user organizations.
2. **OpenClaw session transcripts** — stored on the OpenClaw machine, not the user's machine. Pane's local transcript copy is the user's record. Feature request filed for configurable session storage path in OpenClaw.
3. **Cross-device memory** — memory lives on one machine. Pane sync (Supabase) handles cross-device via encrypted sync. Not yet tested for memory files specifically.

---

## 5.15 Memory Search Scoring Pipeline

> **Status:** NOT BUILT — Spec only (MemPalace research, GitHub issues #368-372)
> **Created:** 2026-04-07
> **Author:** Reed (subagent)
> **Context:** Based on MemPalace findings, adding four improvements to Pane's memory search: raw conversation search, hybrid scoring, temporal boost, and optional LLM rerank

### Overview

Pane's memory system extracts facts from conversations and stores them in `ai_memory`. The current search implementation (§5.9) uses FTS5 keyword matching + optional local embeddings for semantic search. This works well for simple recall ("what did we decide about auth?") but has limitations:

**Current limitations:**
1. **Extracted facts only** — search misses nuance in the original conversation. If extraction misses something, it's unfindable.
2. **Pure embedding ranking** — semantic search returns top-K by cosine similarity alone. Doesn't boost for keyword overlap or recency.
3. **No temporal awareness** — "what did we discuss this week?" gets mixed with 6-month-old results.
4. **No secondary rerank** — the AI sees results in raw embedding order, missing opportunities for context-aware reordering.

**MemPalace research (issue #368)** validated four improvements that all major RAG systems use. This section adds them to Pane's architecture.

### 5.15.1 Dual-Source Search Architecture

> **Status:** PARTIAL (v11.25.0) — search_memory queries notes_fts, ai_memory_fts, and chat_fts. Missing: source-type boost (§5.15.2.2), result fusion scoring.

**Current:** Search queries `ai_memory` table only (extracted facts).

**New:** Search TWO sources in parallel and fuse results:

| Source | What it contains | Why search it |
|--------|------------------|---------------|
| **`ai_memory`** | Extracted facts (atomic sentences) | High-signal, curated knowledge. What the AI explicitly "knows." |
| **`chat_messages`** | Raw conversation transcripts | Full context, nuance, edge cases. What was actually said. |

#### Implementation

```rust
async fn search_memory(query: &str, project_id: Option<&str>) -> SearchResults {
    // 1. Search extracted facts
    let fact_results = search_ai_memory(query, project_id).await?;
    
    // 2. Search raw transcripts
    let transcript_results = search_chat_messages(query, project_id).await?;
    
    // 3. Fuse and rank (§5.15.2)
    let fused = fuse_results(fact_results, transcript_results, query).await?;
    
    // 4. Optional LLM rerank (§5.15.4)
    if config.llm_rerank_enabled {
        rerank_with_llm(fused, query).await
    } else {
        Ok(fused)
    }
}
```

#### Why Two Sources?

**Extracted facts have priority** — they've been explicitly marked as important by the memory extraction pipeline (§5.4). They get a source-type boost (see §5.15.2.2).

**Raw conversations are backup** — if extraction missed something, the conversation search finds it. Also captures context ("we discussed X in the context of Y") that atomic facts lose.

#### No Decay on Raw Conversations

The `chat_messages` table already stores conversations permanently. Compaction (§16.4) only affects the context window sent to the AI, NOT the database. No new tables needed. No decay on raw data — long-term searchability is the point.

**Compaction summaries** (§16.4) should reference source message IDs. When the AI sees a compaction summary in search results, it can trace back to the original messages for full context.

### 5.15.2 Hybrid Scoring Formula

> **Status:** BUILT v0.18.4 (issues #369, #370)

**Current:** Pure embedding similarity (`cosine_similarity(query_embedding, result_embedding)`).

**New:** Hybrid score combines semantic similarity with keyword overlap boost.

#### Formula

```
fused_score = embedding_score × (1 + keyword_weight × keyword_overlap)

Where:
- embedding_score: cosine similarity (0.0 to 1.0)
- keyword_weight: configuration parameter (default 0.3)
- keyword_overlap: ratio of query keywords found in result (0.0 to 1.0)
```

#### Example

**Query:** "code signing certificate expiration"
**Result A (extracted fact):** "Code signing cert expires March 2027"
**Result B (conversation):** "We discussed the build process and dependency management"

**Embedding scores:**
- A: 0.85 (high semantic match)
- B: 0.65 (moderate semantic match)

**Keyword overlap:**
- A: 3/4 keywords matched (code, signing, certificate) → 0.75
- B: 0/4 keywords matched → 0.0

**Fused scores:**
- A: `0.85 × (1 + 0.3 × 0.75) = 0.85 × 1.225 = 1.04`
- B: `0.65 × (1 + 0.3 × 0.0) = 0.65 × 1.0 = 0.65`

**Result:** A ranks higher (as expected). The keyword boost reinforces the semantic match.

#### Why This Works

- **Pure embeddings miss exact matches** — "auth flow" and "authentication workflow" have high semantic similarity, but if the user typed "auth flow" verbatim, exact keyword match should boost that result.
- **Keyword-only search misses synonyms** — "automobile" won't find "car" in pure keyword search. Embeddings handle that.
- **Hybrid captures both** — semantic understanding + lexical precision.

#### 5.15.2.1 Keyword Extraction

Use a simple tokenizer + stopword filter:

```rust
fn extract_keywords(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split_whitespace()
        .filter(|word| !STOPWORDS.contains(word))
        .map(|s| s.to_string())
        .collect()
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for",
    "is", "was", "are", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "should", "could", "may", "might"
];
```

More sophisticated: use a stemmer (Porter algorithm) to match "running" with "run". Optional enhancement.

#### 5.15.2.2 Source-Type Boost

Extracted facts are higher signal than raw conversations. Add a multiplier:

```rust
let source_boost = match result.source {
    Source::ExtractedFact => 1.2,    // 20% boost
    Source::Conversation => 1.0,     // No boost
};

fused_score = embedding_score × (1 + keyword_weight × keyword_overlap) × source_boost;
```

**Configurable** via `ai_settings`:

```sql
INSERT INTO ai_settings (key, value) VALUES ('memory_search_fact_boost', '1.2');
INSERT INTO ai_settings (key, value) VALUES ('memory_search_keyword_weight', '0.3');
```

### 5.15.3 Temporal Boosting

> **Status:** BUILT (v0.18.5) — issue #371 closed

**Current:** Results sorted by score alone. No awareness of when the conversation happened.

**New:** Boost recent results for time-anchored queries.

#### When to Apply

Detect temporal queries using keywords:

```rust
const TEMPORAL_KEYWORDS: &[&str] = &[
    "today", "yesterday", "this week", "last week", "recent", "recently",
    "latest", "current", "now", "just", "earlier"
];

fn is_temporal_query(query: &str) -> bool {
    let lower = query.to_lowercase();
    TEMPORAL_KEYWORDS.iter().any(|kw| lower.contains(kw))
}
```

**Examples:**
- ✅ "what did we decide this week?"
- ✅ "recent changes to the auth flow"
- ✅ "show me yesterday's conversation"
- ❌ "how does code signing work?" (timeless)

#### Temporal Decay Function

Boost recent results using an exponential decay:

```rust
fn temporal_boost(result_timestamp: DateTime, query_time: DateTime, half_life_days: f64) -> f64 {
    let age_days = (query_time - result_timestamp).num_days() as f64;
    let decay = (-age_days / half_life_days).exp();
    1.0 + decay  // Boost factor: 2.0 for age=0, approaches 1.0 as age increases
}
```

**Half-life configuration** (default 7 days):

```sql
INSERT INTO ai_settings (key, value) VALUES ('memory_search_temporal_half_life_days', '7');
```

**Applied score:**

```rust
let mut score = fused_score;  // from §5.15.2

if is_temporal_query(&query) {
    let boost = temporal_boost(result.timestamp, now(), config.temporal_half_life_days);
    score *= boost;
}
```

#### Why Exponential Decay?

Linear decay (`1.0 - age_days / max_days`) drops to zero at a cutoff. Exponential decay asymptotically approaches zero — old results are never completely excluded, just progressively de-prioritized.

**Half-life = 7 days** means a result from 7 days ago gets 50% of the recency boost. A result from 14 days ago gets 25%. A result from 30 days ago gets ~6%.

### 5.15.4 Optional LLM Rerank

> **Status:** BUILT — v0.18.6 (async fix: v1.0.9, #2603)

> **#2603 (C9) fix, v1.0.9:** The rerank LLM call (`call_llm_for_rerank`, an `async fn`) is invoked from `execute_search_memory`, which is itself a *sync* fn reached from inside the tokio async runtime during chat dispatch (`ai_chat_stream` → `execute_single_tool` → `execute_search_memory`). It previously called `tokio::runtime::Handle::current().block_on(...)` directly, which panics with "Cannot start a runtime from within a runtime" when the calling thread is already a tokio worker — any chat message that triggered memory search with rerank enabled crashed mid-response. The call is now wrapped in `tokio::task::block_in_place(|| Handle::current().block_on(...))`, which hands the worker thread off to the runtime's blocking-pool bookkeeping first, making the nested `block_on` legal (requires a multi-threaded runtime, which both Tauri and this crate's `tokio` dependency use). Rerank failures (including any future `block_in_place` panic on a current-thread runtime) already degraded gracefully to the original fused-score order via the existing `Err(e)` arm — that graceful-degradation contract is unchanged, only the panic path is fixed. See `execute_search_memory` (`ai::tools`).

**Current:** AI sees search results in the order returned by the scoring pipeline.

**New:** Optionally send top-K results to a lightweight LLM for context-aware reranking.

#### When to Use

LLM rerank is expensive (additional API call) but powerful. Enable it:
- **Per-agent setting** — some agents get rerank, others don't
- **User toggle** — Settings → AI → Memory → Enable LLM rerank
- **Automatic for complex queries** — query length >20 words OR contains question words ("why", "how", "explain")

#### Implementation

```rust
async fn rerank_with_llm(results: Vec<SearchResult>, query: &str) -> Vec<SearchResult> {
    // 1. Take top 20 results from hybrid scoring
    let candidates = results.into_iter().take(20).collect::<Vec<_>>();
    
    // 2. Build rerank prompt
    let prompt = format!(
        "User query: \"{}\"\n\nRank these {} results by relevance (return JSON array of indices):\n{}",
        query,
        candidates.len(),
        format_results_for_rerank(&candidates)
    );
    
    // 3. Call lightweight model (Haiku-tier, <100 tokens response)
    let reranked_indices: Vec<usize> = call_rerank_model(&prompt).await?;
    
    // 4. Reorder results
    reranked_indices.iter()
        .filter_map(|&i| candidates.get(i).cloned())
        .collect()
}

fn format_results_for_rerank(results: &[SearchResult]) -> String {
    results.iter()
        .enumerate()
        .map(|(i, r)| format!("[{}] {} (source: {}, score: {:.2})", 
            i, r.text_preview, r.source, r.score))
        .collect::<Vec<_>>()
        .join("\n")
}
```

#### Why LLM Rerank?

The hybrid scoring formula (§5.15.2) is deterministic and fast but context-blind. An LLM can:
- Understand query intent ("why did we choose X?" prioritizes decision-type facts)
- Detect contradictions (if two facts conflict, boost the newer one)
- Spot implicit connections (query mentions "auth" → boost results about "login" even if "auth" isn't in the text)

#### Cost Control

- **Model:** Use Haiku-tier (~$0.001 per rerank call). NOT Sonnet/Opus.
- **Candidate limit:** Only rerank top 20 (not all results).
- **Cache the prompt prefix** — the result list changes, but the instruction template is cacheable.
- **Skip for simple queries** — "John's phone number" doesn't need reranking.

#### Configuration

```sql
-- Per-agent setting
INSERT INTO ai_settings (key, value) VALUES ('memory_search_llm_rerank_enabled', 'true');
INSERT INTO ai_settings (key, value) VALUES ('memory_search_llm_rerank_model', 'claude-haiku-3-20240307');
INSERT INTO ai_settings (key, value) VALUES ('memory_search_llm_rerank_threshold', '20'); -- query word count
```

**Default:** OFF. Enable per-agent or per-user. This is a power-user feature.

### 5.15.5 Compaction Summary Back-References

> **Status:** NOT BUILT (depends on compaction implementation, §16.4)

**Current:** Compaction summaries replace old messages. Original messages are marked as compacted. No link back to source.

**New:** Compaction summaries include source message IDs. When a summary appears in search results, the AI can trace back to originals.

#### Schema Change

```sql
ALTER TABLE ai_conversation_summaries ADD COLUMN source_message_ids TEXT; -- JSON array

-- Example:
-- source_message_ids: ["msg-abc123", "msg-def456", "msg-ghi789"]
```

#### Search Integration

When a search result is a compaction summary:

```rust
if result.is_compaction_summary {
    // Fetch original messages for full context
    let source_ids: Vec<String> = serde_json::from_str(&result.source_message_ids)?;
    let original_messages = fetch_messages_by_ids(&source_ids).await?;
    
    // Include both summary and originals in search result
    result.expanded_context = Some(original_messages);
}
```

The AI sees: "Summary: [2-sentence recap]. Original conversation: [full transcript]."

#### Why This Matters

Compaction summaries are lossy. They capture decisions but lose the reasoning. If the user asks "why did we choose X?", the summary might say "We chose X" but not explain the tradeoffs discussed. Having the source IDs lets the AI retrieve the full conversation on demand.

### 5.15.6 Configuration Options

> **Status:** BUILT v0.18.9 — All search settings UI controls implemented in Settings > Memories.

All search pipeline parameters are configurable via `ai_settings`:

| Setting | Default | Description |
|---------|---------|-------------|
| `memory_search_keyword_weight` | 0.3 | Keyword overlap multiplier (§5.15.2) |
| `memory_search_fact_boost` | 1.2 | Extracted facts boost multiplier (§5.15.2.2) |
| `memory_search_temporal_half_life_days` | 7 | Temporal decay half-life (§5.15.3) |
| `memory_search_llm_rerank_enabled` | false | Enable LLM reranking (§5.15.4) |
| `memory_search_llm_rerank_model` | claude-haiku-3 | Model for reranking |
| `memory_search_llm_rerank_threshold` | 20 | Min query words to trigger rerank |
| `memory_search_max_results` | 20 | Max results before rerank |
| `memory_search_fuse_both_sources` | true | Search both facts and conversations (§5.15.1) |

**User-facing controls** (per ADR-0037 — model selection in Settings → AI, feature behavior in Settings → Memories):

**Settings → AI → Smart Features:**
- Rerank Model picker — writes the canonical key `memory_search_llm_rerank_model`. The dead `rerank_model` key written by the v0.33.0 AI tab picker is retired per ADR-0037 §3.

**Settings → Memories → Search Settings:**
- Toggle: "Enable AI-powered result reranking (costs extra)" — `memory_search_llm_rerank_enabled`
- Greyed-out read-only display: "Rerank model: {configured model name} — Set in Settings → AI" — reads `memory_search_llm_rerank_model` (per ADR-0037 §4 cross-reference pattern)
- Toggle: "Search raw conversations alongside extracted facts" — `memory_search_fuse_both_sources`
- Slider: "Keyword match importance" (0% to 50%) — `memory_search_keyword_weight`
- Slider: "Recency boost strength" (0 to 14 days half-life) — `memory_search_temporal_half_life_days`

> **📌 Why This Exists — Rerank Model picker moved to AI tab (2026-05-18, ADR-0037):** The v0.33.0 redesign introduced a Rerank Model picker in the AI tab that wrote a key (`rerank_model`) the backend did not read; the Memories tab's existing picker continued to write the canonical key (`memory_search_llm_rerank_model`). The AI tab picker was decorative for the entire v0.33.0+ window; memory reranking was silently configured only by the Memories tab. ADR-0037 codifies that model pickers live in the AI tab and the canonical key is the only key; cross-references in feature tabs use the greyed-out + pointer pattern. The Memories tab picker becomes a greyed display; the AI tab picker is retargeted to write the canonical key. No data migration — pre-launch, customer re-picks if needed.

**Defaults are conservative:** Rerank OFF (saves cost), temporal boost moderate (7 days), keyword weight low (0.3).

**Design note (Apr 7, Matt decision):** LLM rerank (§5.15.4) acts as the safety net for source-type boost (§5.15.2.2). If extraction produced a slightly wrong fact, the reranker catches it because it reads the raw conversation alongside the fact. This means the four improvements work as a system — source boost improves ranking, rerank prevents boost from burying correct answers. All four phases should ship, with rerank configurable (default OFF, user toggles ON in settings).

### 5.15.7 Implementation Notes

> **Status:** NOT BUILT — guidance for Stia

#### Search Flow (Complete Pipeline)

```rust
pub async fn search_memory_enhanced(
    query: &str,
    project_id: Option<&str>,
    config: &SearchConfig
) -> Result<Vec<SearchResult>> {
    
    // Step 1: Parallel search of both sources (§5.15.1)
    let (fact_results, transcript_results) = tokio::join!(
        search_ai_memory(query, project_id),
        if config.fuse_both_sources {
            search_chat_messages(query, project_id).await
        } else {
            Ok(vec![])
        }
    );
    
    // Step 2: Hybrid scoring (§5.15.2)
    let mut fused = fuse_and_score(
        fact_results?,
        transcript_results?,
        query,
        config.keyword_weight,
        config.fact_boost
    );
    
    // Step 3: Temporal boost if query is time-anchored (§5.15.3)
    if is_temporal_query(query) {
        apply_temporal_boost(&mut fused, config.temporal_half_life_days);
    }
    
    // Step 4: Sort by score
    fused.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    
    // Step 5: Truncate to max results
    fused.truncate(config.max_results);
    
    // Step 6: Optional LLM rerank (§5.15.4)
    if config.llm_rerank_enabled && should_rerank(query, &config) {
        fused = rerank_with_llm(fused, query, &config.rerank_model).await?;
    }
    
    // Step 7: Expand compaction summaries with source messages (§5.15.5)
    expand_compaction_contexts(&mut fused).await?;
    
    Ok(fused)
}
```

#### Performance Considerations

**Parallel search:** `tokio::join!` runs fact and transcript search concurrently. Total latency = max(fact_search_time, transcript_search_time), not sum.

**FTS5 indexes:** Both `ai_memory` and `chat_messages` need FTS5 virtual tables:

```sql
CREATE VIRTUAL TABLE ai_memory_fts USING fts5(
    fact,
    category,
    content='ai_memory',
    content_rowid='rowid'
);

CREATE VIRTUAL TABLE chat_messages_fts USING fts5(
    content,
    content='chat_messages',
    content_rowid='rowid'
);
```

**Embedding search:** If local embeddings are enabled (§5.9), use `bge-base-en-v1.5` (already in spec). Compute query embedding once, search both fact and message embeddings in parallel.

**LLM rerank latency:** Haiku responds in ~500ms for 20-result rerank. Only triggered for complex queries. User sees initial results immediately (streaming UI), reranked results replace them after LLM call completes.

#### Testing Strategy

**Unit tests:**
- `test_keyword_overlap_calculation()` — verify keyword extraction and overlap ratio
- `test_hybrid_scoring()` — verify formula with known inputs
- `test_temporal_boost()` — verify exponential decay math
- `test_source_type_boost()` — verify fact results get 1.2x multiplier

**Integration tests:**
- Create test data: 5 extracted facts, 10 raw messages, spanning 30 days
- Run queries with known expected results ("this week", "code signing", "why did we choose X")
- Assert top result matches expectation
- Test with rerank ON and OFF, verify results differ appropriately

**Benchmark:**
- Measure search latency with 1K facts + 10K messages
- Target: <100ms for hybrid search (no rerank), <600ms with rerank
- If slower, add caching layer for query embeddings

### 5.15.8 Migration Plan

> **Status:** NOT BUILT

**Phase 1 — Backend infrastructure:**
1. Add `source_message_ids` column to `ai_conversation_summaries`
2. Create `chat_messages_fts` virtual table if not already present
3. Implement `search_chat_messages()` function (FTS5 + optional embeddings)
4. Add `ai_settings` entries for all configuration parameters

**Phase 2 — Scoring pipeline:**
1. Implement `fuse_and_score()` with hybrid formula (§5.15.2)
2. Implement `apply_temporal_boost()` with decay function (§5.15.3)
3. Add source-type boost for extracted facts (§5.15.2.2)
4. Wire into existing `search_memory` tool

**Phase 3 — LLM rerank (optional):**
1. Implement `rerank_with_llm()` function (§5.15.4)
2. Add model configuration and cost tracking
3. Make toggleable per-agent and per-user
4. Test with Haiku model, validate costs are acceptable

**Phase 4 — Compaction integration:**
1. Update compaction pipeline (§16.4) to store source message IDs
2. Implement `expand_compaction_contexts()` to fetch originals
3. Update search result UI to show expanded context

**Phase 5 — UI settings:**
1. Add Advanced section to Settings → AI → Memory
2. Add toggles and sliders for all configuration options
3. Show cost estimate for LLM rerank ("~$0.001 per complex search")
4. Add explainer tooltips for each setting

**Rollout:** Default to current behavior (extracted facts only, pure embedding scoring). Users opt into enhanced search via Settings. After validation period (2-4 weeks), make dual-source + hybrid scoring the default.

### 5.15.9 Cross-References

- **§5.4** — Memory extraction (populates `ai_memory` table)
- **§5.5** — Memory synthesis (consolidates facts into prose)
- **§5.6** — Daily logs (chronological memory)
- **§5.9** — Current memory search implementation (FTS5 + embeddings)
- **§5.10** — Chat transcript storage (`chat_messages` table)
- **§16.4** — Compaction (summarizes old messages)
- **Issue #368** — MemPalace research findings
- **Issue #369** — Raw conversation search
- **Issue #370** — Hybrid scoring with keyword overlap
- **Issue #371** — Temporal boost for time-anchored queries
- **Issue #372** — Optional LLM rerank pass

### 5.15.10 Design Decisions

> **Status:** RESOLVED — 2026-04-07

1. **Embedding model for conversations:** Same model (bge-base-en-v1.5). One model, one embedding space. Messages of any length are handled the same way notes are — chunked if needed. The UX already allows saving full messages which can exceed 500 chars. No special handling needed.

2. **Rerank model provider:** Configurable in agent settings. Default is Haiku. User can change to any available provider/model (Anthropic, OpenAI, Ollama local). Models change — never hard-code a specific model.

3. **Conversation context window:** Yes, ±3 messages around the match. Isolated messages are often meaningless without surrounding conversation flow.

4. **Cost visibility:** Cumulative monthly only, shown in AI Health dashboard. No per-search cost display — too noisy, makes the product feel expensive.

5. **Project-scoped temporal boost:** No different behavior. "This week" means this week regardless of context. Project scope already filters results; temporal boost reranks within that filtered set.

6. **Temporal boost + decay interaction:** No interaction. They solve different problems independently. Decay removes stale facts; temporal boost ranks by recency. Keep them separate.

### 5.15.11 Status Summary

| Component | Status | GitHub Issue |
|-----------|--------|--------------|
| Dual-source search | NOT BUILT | #369 |
| Hybrid scoring formula | NOT BUILT | #370 |
| Temporal boosting | NOT BUILT | #371 |
| LLM rerank | NOT BUILT | #372 |
| Compaction back-refs | NOT BUILT | (depends on #166) |
| Configuration settings | NOT BUILT | (UI work) |
| FTS5 indexes | PARTIAL | `ai_memory_fts` exists, need `chat_messages_fts` |
| Local embeddings | BUILT | §5.9 (bge-base-en-v1.5) |

**Next steps:** File implementation issues for each component. Stia builds Phase 1-2 (backend + scoring). Phase 3-5 (rerank + UI) are post-launch enhancements.

---

