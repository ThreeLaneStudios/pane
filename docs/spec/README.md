# Pane Architecture Specification

**Last Updated:** 2026-04-19  
**Source:** Extracted from pane-architecture-spec.md

This directory contains the Pane architecture specification, split into domain-specific files for maintainability.

---

## Specification Files

| File | Sections | Description |
|------|----------|-------------|
| [core-data-model.md](core-data-model.md) | §1 | Unified entity system: notes, tasks, tags, projects, folders |
| [file-system.md](file-system.md) | §2, §2A-2D | File structure, lifecycle, deletion, DB safety, write safety, audit |
| [ui-layout.md](ui-layout.md) | §3 | AI context system, sidebar, cards, editor, chat panel |
| [ai-integration.md](ai-integration.md) | §4, §4A, §26, Appendix A | Chat, tools, streaming, tool calling, prompt caching, system prompt |
| [memory-pipeline.md](memory-pipeline.md) | §5 | Memory extraction, synthesis, decay, search scoring |
| [sync.md](sync.md) | §9, §22B | cr-sqlite, CRDTs, Supabase, Pane Bridge, encryption |
| [onboarding.md](onboarding.md) | §6 | Wizard flow, agent setup, first-run experience |
| [agents.md](agents.md) | §24 | First-class agent entities, lifecycle, teams, migration |
| [smart-routing.md](smart-routing.md) | §27 | Classifier, model resolution, dual-provider |
| [testing.md](testing.md) | §25 | CI pipeline, pre-build checks, test layers, invariants |
| [future.md](future.md) | §23 | Roadmap: custom panes, voice, multi-model, mobile, agent economy |
| [note-components.md](note-components.md) | §31 | Due Today + Advanced Checklist note components, subtask data model |
| [note-templates.md](note-templates.md) | §32 | Note templates: save + apply |
| [sheets.md](sheets.md) | §33 | Sheets: typed columns, formulas, grid side tables, CSV export, AI tools |
| [data-refresh.md](data-refresh.md) | §34 | Single data-change refresh point: entity-identified event, echo suppression, per-screen patch-not-reload |
| [startup.md](startup.md) | §35 | Startup sequencing: window-first launch on a migrating install, StartupStatus/get_startup_status race-safe gating, migration progress events, ordering guarantee, failure UX |
| [runtime.md](runtime.md) | §36, §39 | Pane runtime for API agents: loop extraction (#3059), auto-continue with user ceiling + Stop, single-tick scheduler (`agent_jobs`), hidden spawn sessions, schedule sessions listed only in Home's Scheduled panel, spawn to any Pane agent, wake-on-complete chaining, proactive-turn tool withholding, background OC pickup, `agent_message`/`job_failed` notifications, chat attachments + `read_attachment`. OC boundary table in §36.0.1. §39: `generate_image` tool — OpenRouter/Venice adapters, Settings provider+model picker, note-embed placement, per-turn spend cap on job turns. Companion ADR-0069. DRAFT, not yet signed. |

---


---

## Changelog — Architecture Decision Timeline
> **Status:** N/A (meta — decision log)

| Date | Decision | Section | Impact |
|------|----------|---------|--------|
| 2026-10-25 | Runtime sprint spec: Pane becomes its own runtime for API agents — one 60 s scheduler tick as the sole starter of untyped turns; spawns as hidden `origin='spawn'` sessions, schedules as visible `origin='schedule'` sessions; `spawn_job` across providers; report-back via ingest + scheduler wake; proactive turns lose delete/move by withholding; OC gets trigger-only sends and read-side pickup; v92→v93 migration (`agent_jobs` + 4 columns). Supersedes the March §17 spawning outline; retires §19.2's planned `agent_done` in favour of `agent_message`. | §36 (new), §19.2, §30.1.1, §30.11.4 | #3050, #3059, ADR-0068 |
| 2026-02-08 | Revenue vehicle = notes app with AI (dogfood first, then productize) | §0 | Product direction established |
| 2026-02-13 | Sync prototype → AI chat → website → payments → mobile → teams | §9 | Build order set |
| 2026-02-14 | Conversational AI-first UI — AI is the operator, not just navigator | §4 | Natural language creates tasks, writes notes, navigates |
| 2026-02-14 | Dual-brain architecture: Haiku (fast/routing) + Opus (conversation/memory) | §4 | Two-tier AI processing |
| 2026-02-16 | Unified note/task entity — notes and tasks are the same thing | §1 | Single entity model, status/priority are metadata |
| 2026-02-18 | CRR companion tables pattern — mutable data in separate non-CRR tables | §9 | Permanent sync architecture constraint |
| 2026-02-21 | Pane North Star: OS-level interface between human and AI advocate | §0 | Core product vision |
| 2026-02-21 | Cabinet status replaces Idea — Cabinet (default) → Todo → In Progress → Done | §1 | Status workflow simplified |
| 2026-03-20 | Review status added between In Progress and Done. Status flow: Cabinet → To Do → In Progress → Review → Done. Done column collapsed by default in both Kanbans (All Tasks + Project), no count badge, paginated when expanded (last 30 days, "show older" loads more). Done tasks stay in place — never auto-archived, never moved. | §1 | Matches real workflow, handles scale |
| 2026-02-22 | Pipeline rebuild: creativity at write time, mechanics at publish time | — | Content pipeline architecture |
| 2026-02-23 | Architecture spec v1.0 published | All | Foundation document |
| 2026-02-27 | Sync: cr-sqlite CRDTs + Supabase relay + E2E encryption (XChaCha20-Poly1305) | §9 | Zero-knowledge cross-device sync |
| 2026-03-01 | AI is a resident partner, not a feature. Shared workspace paradigm. | §0 | Philosophy locked |
| 2026-03-01 | Native tool calling mandatory — never parse tool calls from text | §4.3 | Reliable tool execution |
| 2026-03-09 | Pins are notes, not chat chrome — pin appends to "Pinned" note | §4 | No banner UI, AI sees pinned context |
| 2026-03-17 | Pin feature DEPRECATED — redundant with memory save system (auto-extraction + manual save button + daily logs). Remove pin icon from chat UI. | §4 | Simplifies UI, reduces feature overlap |
| 2026-03-10 | Card design locked: 120px wide, 3px status border, CSS grid, priority pills | §3 | UI design finalized |
| 2026-03-10 | Layout locked: sidebar + card list (340px) + editor + chat, always split | §3 | Layout finalized |
| 2026-04-16 | CSS Grid folder view: 3-column layout with shared row heights (folder headers, toolbars, content). Cabinet/Tasks split in left panel. Continuous horizontal borders align across all panels. | §3, §4 | Aligned headers + two-column folder content (#424) |
| 2026-03-10 | OpenClaw tool bridge: text injection for tool descriptions | §4 | Tier 3 integration pattern |
| 2026-03-11 | Three-tier business model: Pane Cloud / BYOK / OpenClaw. No free tier. | §12 | Revenue architecture |
| 2026-03-11 | Home dashboard: 8 modules, collapsible 320px panel + chat | §3 | Home experience locked |
| 2026-03-12 | Provider list: OpenRouter, Anthropic, Ollama, OpenClaw. No direct OpenAI/Google. | §4 | Supported providers set |
| 2026-03-13 | Pane = source of truth for all AI identity docs (Soul, Memory, Rules) | §5 | File ownership established |
| 2026-03-13 | ~/Pane/ replaces ~/Shared/ — single shared folder (symlink same-machine, HTTP push/pull split-machine) | §5.3A | File structure simplified |
| 2026-03-14 | Read-only agent docs in Pane editor — note_type set = non-editable by user | §5.8 | System note protection |
| 2026-03-14 | Memory synthesis outputs prose paragraphs, no markdown headers/bullets | §5.5 | Synthesis format locked |
| 2026-03-14 | append_to_note tool — server-side safe append, AI cannot truncate | §5.7 | Memory write safety |
| 2026-03-14 | Content loss guard — edit_note rejects >50% reduction on system notes | §5.8 | Memory protection |
| 2026-03-14 | main_agent_folder_id stored in AI settings during onboarding | §5.4 | Memory routing fix |
| 2026-03-15 | Memory architecture v2 — full rewrite of §5 (13 subsections) | §5 | Memory system spec complete |
| 2026-03-15 | DB is runtime source, flat files are portable source of truth. Flat files win on conflict. | §5.3 | Storage model clarified |
| 2026-03-15 | Adaptive embeddings: auto-enabled ≥16GB RAM, FTS5 always active | §5.9 | Hardware-aware search |
| 2026-03-15 | Transcript storage smart per-tier: skip duplicate if OpenClaw local | §5.10 | Storage efficiency |
| 2026-03-15 | Two search tools: search_notes (navigation) + search_memory (recall) | §5.9 | Tool separation |
| 2026-03-16 | Integrity check = single owner of Pane System structure. Onboarding discovers, never creates. | §6 | Eliminates duplicate note bugs |
| 2026-03-21 | System notes must read directly from .pane/system/ — no duplicate in Projects/. Editor file_path for system notes points to .pane/system/{agent}/FILE.md. Eliminates dual-copy drift. | §5.3 | Single-owner principle enforced at file level |
| 2026-03-23 | Daily logs redesign: capture everything (overlap with memory allowed), timestamped facts + narrative header, event-driven trigger, 50KB cap, dedup, zero-entry alert. One log per user (not per agent). Manual saves also write to daily log. | §5.6 | Complete daily record, simpler routing, robust safety |
| 2026-04-16 | Sidebar collapsible folders: Each project shows chevron that expands to reveal child folders. Folders display at 32px height with 48px left padding, connect to parent with subtle vertical line. Supports opacity transitions on hover. | §4 | Improved project navigation UX |
| 2026-03-27 | #280: `notes.project_id` nullable (migration 013). Agent-owned notes have `agent_id` + NULL `project_id`. No fake `__system` container. `agent_id` column (migration 012) is the primary lookup. All project queries naturally exclude agent notes. | §5.6.2A | Clean separation: user notes → projects, agent notes → agents. No workarounds. |
| 2026-03-23 | Task workspaces: scoped working directories for agent spawns (.pane/tasks/{id}/workspace/). Agent identity folders stay clean (Soul, Memory only). Deliverables saved to project via create_note. Workspace auto-cleaned after 30 days. Works all tiers. | §17.6 | Clean separation of identity vs work artifacts. CI/CD-inspired pattern. |
| 2026-03-16 | Supabase sync confirmed: zero-knowledge encrypted sync stays for mobile + teams | §9, §5.1 | Sync architecture validated |
| 2026-03-16 | §5.1 privacy language: "User's machine is source of truth. Sync is opt-in, zero-knowledge." | §5.1 | Privacy statement finalized |
| 2026-04-18 | #486: User notes migrate to DB-primary storage (body column). System notes stay file-only. FTS5 rebuilt as external content table. | §0.3.2, §1.3, §2A.1, §2A.5 | Migration 026 shipped v0.59.0 |
| 2026-03-16 | Synthesis routes extracted facts to flat files by category (identity→MEMORY.md, decisions→project memory, deadlines→daily log) | §5.5 | Facts survive DB wipe, visible to OpenClaw |
| 2026-03-16 | Extraction writes to DB only, NOT flat files. Synthesis is sole writer to Memory notes. One writer per file. | §5.4, §5.5, §5.7 | Eliminates mixed bullet/prose and cross-purpose writes |
| 2026-03-16 | Per-project AI folder: auto-created with project, Memory note auto-created, Soul/Rules optional. Replaces memory/projects/ under agent folder. | §5.3A | Project memory visible inside the project |
| 2026-03-17 | Tool errors never shown in chat UI — logged to console, returned to AI for graceful recovery | §4.4 | Polished UX, no raw JSON in chat |
| 2026-03-17 | Extraction quality rules: atomic facts only, no compaction summaries, personal facts always personal regardless of project context | §5.4A | Fixes summary blobs and wrong project tagging |
| 2026-03-18 | Compaction safety guard: abort compaction if summary is empty or <50 chars, preserving original messages (#166) | §16.4 | Prevents data loss from failed summary generation |
| 2026-03-18 | Compaction exclusion filter documented: backend `is_compaction_summary()` with 2+ structural markers, startup cleanup (#165) | §5.4A | Backend implementation detail added to spec |
| 2026-03-17 | Agent spawning across all tiers — Pane-native background tasks for Tier 1/2, OpenClaw for Tier 3 | §17 | Any model can spawn agents via tool call |
| 2026-03-17 | Task assignment to humans AND agents — same field, same UI, same tracking | §18 | Linear-style agent-as-assignee pattern |
| 2026-03-17 | Notification system outline — 3 channels (in-app, push, email), AI-contextual reminders, restraint philosophy. **Note: outline only; full spec landed 2026-05-02 per ADR-0021/ADR-0022.** | §19 | Outline of intent; body never written until 2026-05-02 |
| 2026-05-02 | §19 Notification & Reminder System: full spec body in `docs/spec/notifications.md`, type registry, event-driven triggers, coordinator-only AI nudges, per-device read state, schema additions. | §19 | ADR-0021 + ADR-0022 |
| 2026-05-02 | §19 spec moved to its own file (`docs/spec/notifications.md`); README.md §19 becomes a stub. AI Team System content moved to §25 (`docs/spec/ai-teams.md`) — §19 was always notifications per §0 Philosophy; AI Team had been mis-placed. TOC drift fixed: §24 Tag System added (was in body, missing from TOC). | §19, §24, §25 | Resolves §12/§19 TOC collision |
| 2026-03-17 | Multi-step orchestration — decompose→delegate→collect→scaffold with adaptive re-planning and cost-tiered execution | §20 | Novel: mid-execution re-planning + cost-aware model routing |
| 2026-03-17 | Hybrid extraction triggers: tool usage OR ≥8 words. Semantic signal beats pure word count. | §5.4A | Catches "Yes please" → structural changes |
| 2026-03-17 | Vision statement added to §0: one sentence → fully orchestrated project plan. Complex task orchestration is the destination. | §0 | North star for all architecture decisions |
| 2026-03-17 | Memory health system updated: synthesis-time guard (word count thresholds), UI badges, scoped to all note_type="memory" notes | §16.3 | Prevents memory bloat from eating context window |
| 2026-04-26 | §5.14.2 table names corrected: `chat_messages_local` / `chat_archive` (was `ai_chat_messages` / `ai_chat_archive`). `chat_messages_local` is the production active table; `ai_chat_messages` is legacy synced table from pre-migration-18. | §5.14 | Aligns spec with production schema |
| 2026-03-18 | Long-term transcript storage & search: three-layer architecture (raw transcripts, extracted facts, synthesized memory). Never delete. 30-day active/archive split. Two-tier search (quick + deep). | §5.14 | Store everything, load only what matters. Permanent searchable history. |
| 2026-04-08 | Synthesis content loss guard — >50% reduction blocks write, matching edit_note behavior | §5.5 | Prevents synthesis errors from destroying Memory content (#321) |
| 2026-04-08 | Onboarding check uses `should_show_onboarding()` smart function with self-healing flag | §6 | Frontend calls smart check instead of raw DB lookup. Auto-sets `onboarding_completed=true` when content exists (#398) |
| 2026-04-23 | Three-writer memory pipeline + `ai_memory` as source of truth. Router (no LLM, appends bullets), End-of-day agent (LLM, daily log), Consolidator (LLM, rare, reads from `ai_memory`). `goal` category added. Extraction trigger threshold ≥8 → ≥5 words. Extraction dual-writes to daily log. Agent MEMORY.md bounded to identity/preference/style/goal. | §5.4, §5.4A, §5.5 | ADR-0010 — resolves #646 timeout, kills agent-memory bloat, makes delete semantics honest |
| 2026-04-24 | Daily logs: append-only invariant codified, audit-entry markers (`[saved]`, `[memory edited]`, `[memory deleted]`), pattern-based extraction filter for markers, end-of-day agent spec locked (3–6 sentence header, exact-match dedup, mechanical fallback, no pruning), four writer sources. | §5.6 | ADR-0010 — daily log as complete audit trail for memory mutations |
| 2026-04-24 | §5.7.2 amended for ADR-0011 uniform scope: all 7 `note_type`-tagged notes open read-only (memory, daily, soul, rules, identity, user, tools — was memory + daily only). Agents settings (#682) named as view surface. HD filesystem `~/Pane/.pane/system/{agent}/` documented as unsupported escape valve. | §5.7.2 | ADR-0011 follow-up; closes loop on Cluster C inconsistency |
| 2026-04-24 | Writer ownership table (Cluster C): router/consolidator/end-of-day agent own distinct write paths to memory files and daily logs. Pane editor opens memory files and daily logs read-only; Memory Settings is the write surface. Memory Management page gains inline edit/delete at all stages (staged/routed/consolidated) with propagation via router remove-bullet path and consolidator re-render. `goal` added to manual save category picker. "Open in Editor" info note warns about auto-regeneration. | §5.7, §5.12 | Cluster C of ADR-0010 rollout |
| 2026-07-05 | Cabinet is a first-class `status` value (not the NULL sentinel). Vocabulary: cabinet\|todo\|in_progress\|review\|done. Task = status set and not 'cabinet'. `status NOT NULL DEFAULT 'cabinet'`. Amends ADR-0003. | §1 | Kills null-sentinel special-casing (fresh-note kanban leaks, orphaned status='task'); single create path defaults to cabinet; 'task' value + createTask wrapper removed |
| 2026-07-23 | SUPERSEDES 2026-04-16 folder-view layout decision. #2180 replaces the 3-column CSS grid (fluid folderPanelWidth + editor-in-col-3) with: fixed 300px Cabinet + Tasks columns that never resize/reflow; note opens as a resizable position:absolute overlay panel (not a squeeze); each column collapses independently to a 46px rail; state persists globally. | §3 (new §3.2) | #2180 |
| 2026-10-24 | Window-first startup on a migrating launch: cheap pre-check decides fast (unchanged, synchronous) vs. window-first (background thread, generalizes #353) path; `StartupStatus`/`get_startup_status` command gates the frontend race-safely; `migration://progress` added alongside existing `backup://progress`; structural ordering guarantee (`mount_db_state_then`); explicit failure screen (Quit, no in-place retry). | §35 (new) | #2723 PR B |
| 2026-11-03 | §39 Image generation spec draft + ADR-0069: `generate_image` tool over the user's existing OpenRouter/Venice provider connection, output is an ordinary attachment embedded in notes at the brief's chosen position, spend capped on job turns (`max_images_per_turn`, counter on turn state). Ollama excluded this sprint (CLI-only, no HTTP API). Reed's 2026-11-03 review applied (embed format, Venice per-model params, first-match `after:`, cost-omitted-not-zero). DRAFT — lands as a docs PR for Reed's audit; build issues filed only after sign-off. | §39 (new, in `runtime.md`) | ADR-0069 |
| 2026-11-09 | Local-first launch (#3195, OSS prep #3106): Pane opens to Home (chat + Jobs panel) with no sign-in and no entitlement check. `AuthScreen` gate, S7, S8, `auth_check_entitlement`, launch + on-focus cadence and grace-period toast removed. Account sign-in, §6.1.5 session storage, Supabase CRDT sync and Settings → Account stay compiled and CI-tested behind `ai_settings.account_enabled` (default off, no UI). Flag-on sign-in targets Supabase GoTrue directly; the ecom wrapper is retired. Migration is not a sign-out: master key, keychain tokens, `auth_session.json` and `cleanup_pending.json` untouched. ADR-0025/0027/0036 retired; 0026/0029/0035 entitlement halves superseded. | §6.0 (new), §6.1 (retitled, flag-gated; 6.1.2 / 6.1.3 retired), §6.1.5, §6.2, §6.7 (`onboarding.md`) | ADR-0070 |
| 2026-11-09 | ADR-0064 D4 amended (Reed #3106 security review, C2): a turn tainted by external content cannot create, alter or spawn a job. `schedule_job`, `spawn_job`, `cancel_job`, `pause_job`, `resume_job` join `MUTATE_EXISTING_TOOLS` (18 → 23 = every `Write` tool except `create_note`/`create_sheet`). Closes the deferred-execution hole: a tainted turn scheduled a job whose later dispatch started untainted with full write tools. New D4 criterion — a created thing that acts later is executable, not inert; `create_note`/`create_sheet` exemption unchanged. Taint propagation into `agent_jobs` recorded, not built. Residual rewritten honestly (model-paraphrase paths remain). §4.6.3 / INVARIANTS member lists scrubbed to live code (`generate_image`, flag rename, refusal string). | §4.6.3 (`ai-integration.md`), §36.5.2 (`runtime.md`), INVARIANTS §4.6 | ADR-0064 Amendment 2026-11-09 — pinned test `…differ_exactly_by_three` flips red-first |
| 2026-11-14 | Housekeeping, not a behavior change: the three git-tracked `database.rs.before_*` snapshot files (`before_composite_pk`, `before_tags_fix`, `before_v9.1.2`) in `src-tauri/src/` are deleted; nothing in source, tests, scripts, workflows, or docs referenced them. No schema change, no migration change, `database.rs` itself untouched. Added here only because `check-spec-first.sh`'s `CORE_BEHAVIORAL_PATTERN` matches `database\.rs` unanchored, so it flags these backup filenames as core-behavioral the same way `CORE_PATHS` matched them before this PR's `$`-anchor fix; fixing that script's pattern is out of scope for this PR. | n/a (no spec section; these files predate and are unrelated to the current schema) | #3226 |

---

## 0. Philosophy — The Shared Workspace
> **Status:** N/A — design philosophy, not code

Pane is not a notes app with AI bolted on. It is the shared workspace between human and AI — the place where both meet to manage work, life, and creative projects together.

The AI is a resident partner, not a feature. It lives in Pane. It understands the workspace as its home — projects, notes, tasks, and folders are shared territory that both human and AI maintain. The human interacts with the AI through Pane. The AI interacts with the human world through Pane. Neither is the "tool" of the other.

This has architectural consequences:

- **The AI has memory and identity.** It knows who it is (SOUL.md), who the user is (USER.md), what it knows (MEMORY.md), and how to operate (AGENTS.md). It persists across sessions. It recovers from crashes. It is not stateless.
- **The AI has agency within boundaries.** It can create, organize, and manage notes and tasks — not just when asked, but proactively when it sees the need. Destructive actions require confirmation. Everything else is fair game.
- **Context is spatial.** Where the user is in Pane determines what the AI sees, just like walking into a room. The Home page is the strategy room. A project is a workbench. A note is a focused conversation. The AI adapts its scope and role accordingly.
- **The workspace is the interface in both directions.** When the user wants to tell the AI something, they work in Pane. When the AI wants to tell the user something, it writes to Pane — a note, a task, a nudge, a status update. No separate "AI inbox." The workspace IS the communication layer.

**Build for the decade, not the afternoon.** Every line of code in Pane must justify its existence long-term. No migration hacks, no temporary workarounds, no "we'll clean this up later." If it doesn't serve the product Pane is becoming, it doesn't ship. Before writing code, ask: "Will this matter in 2 years?" If the answer is no, find the approach that will.

**Where this is going.** Today, Pane handles notes, tasks, and conversation. Tomorrow, the user says "I want to launch a float therapy studio" and the AI researches the market, drafts a business plan, creates a marketing strategy, builds a project with tasks and deadlines, assigns work to agents and the user, and sends reminders as deadlines approach. One sentence from the human → a fully orchestrated project plan with accountability. That's not a feature request — it's the natural endpoint of an AI partner with memory, tools, agency, and the ability to delegate. Every section of this spec builds toward that capability: memory (§5) so the AI knows context, tools (§4) so it can act, spawning (§17) so it can delegate, assignment (§18) so work has ownership, notifications (§19) so nothing falls through cracks, and orchestration (§20) so complex work flows from a single human intent.

Every technical decision in this document serves this philosophy. The tool layer (§4) gives the AI hands. The memory system (§5) gives it continuity. The agent architecture (§16) gives it the ability to delegate and scale. The sync layer (§9) lets it follow the user across devices. None of these are features — they are what makes Pane a partnership, not a product.

---

## 0.1 User Philosophy & Tiers (Matt/Brit decision, 2026-03-03)
> **Status:** N/A — design philosophy, not code

**We build for our own use cases because we believe everyone will want them.** We span the spectrum — from API user to OpenClaw power user — so we're good first testers. But the product serves everyone from "I just use ChatGPT" to "I run a full agent infrastructure." The use cases are universal: project management, AI memory, persistent chat, proactive assistance. What we need, everyone needs.

### User Tiers
> **Status:** N/A (design doc)

| Tier | Description | AI Capabilities |
|------|-------------|-----------------|
| **Sandbox Mode** (default) | Users who want AI partnership without system access. No OpenClaw, no separate machine. Safe for personal computers. | Notes, tasks, projects, memory, proactive nudges. AI sandboxed to ~/Pane only. |
| **Operator Mode** | Power users with OpenClaw integration. Dedicated AI machine or comfortable with full system access. | Everything in Sandbox PLUS: shell execution, file system access beyond ~/Pane, browser automation, sub-agent orchestration. |

Sandbox Mode is the default experience. Operator Mode is enabled via Settings when a local OpenClaw gateway is detected.

**Positioning:** Pane serves the full spectrum:
- **Novice:** "I use ChatGPT/Claude but wish it remembered things and helped me manage projects" → Pane gives them memory, structure, and a real AI partner.
- **Intermediate:** "I have an API key, I want a proper workflow tool" → Pane gives them project-scoped AI, persistent chat, proactive assistance.
- **Advanced:** "I run OpenClaw, I want a beautiful interface for my agent" → Pane gives them Operator Mode — full system access through a polished UI.

All the same product. The tiers are about capability access, not audience segmentation.

## 0.2 OpenClaw Integration (Operator Mode)
> **Status:** PARTIAL — OC gateway detection built, tool bridge working; full memory reconciliation and extended tool set not fully wired

When Operator Mode is enabled, Pane connects to the local OpenClaw gateway for extended capabilities:

```
┌─────────────────────────────────────────────────────────┐
│                        PANE                              │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐      │
│  │   Memory    │  │   Context   │  │    Chat     │      │
│  │   System    │  │   System    │  │   Interface │      │
│  └─────────────┘  └─────────────┘  └─────────────┘      │
│                         │                                │
│              ┌──────────┴──────────┐                    │
│              │   Tool Dispatcher    │                    │
│              └──────────┬──────────┘                    │
│                         │                                │
│         ┌───────────────┼───────────────┐               │
│         ▼               ▼               ▼               │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐       │
│  │ Pane Tools  │ │   OpenClaw  │ │  Provider   │       │
│  │ (notes,     │ │   Gateway   │ │   Adapter   │       │
│  │  tasks)     │ │ (exec, fs,  │ │ (Anthropic, │       │
│  │             │ │  browser)   │ │  OpenAI)    │       │
│  └─────────────┘ └─────────────┘ └─────────────┘       │
└─────────────────────────────────────────────────────────┘
```

### Memory Reconciliation
> **Status:** PARTIAL — OC gateway detection built; full memory reconciliation not wired

When transitioning from standalone OpenClaw to Pane:

1. **Migration:** "Import from OpenClaw" during onboarding reads `~/.openclaw/workspace/MEMORY.md` and `memory/*.md`, converts to Pane's memory format.
2. **Going forward:** Pane owns memory extraction and storage. OpenClaw no longer performs memory operations.
3. **OpenClaw runs headless:** When Pane calls OpenClaw tools, OpenClaw acts as pure execution — no memory, no context building. Pane is the brain, OpenClaw is the hands.

### Extended Tool Set (Operator Mode Only)
> **Status:** PARTIAL — tool bridge working; extended set not fully wired

| Tool | Description | OpenClaw Command |
|------|-------------|------------------|
| `exec` | Run shell commands | `exec` |
| `read_file` | Read files outside ~/Pane | `Read` |
| `write_file` | Write files outside ~/Pane | `Write` |
| `browser` | Browser automation | `browser` |
| `spawn_agent` | Spawn sub-agents for parallel work | `sessions_spawn` |

These tools are only available when:
- Operator Mode is enabled in Settings
- OpenClaw gateway is running and reachable
- User has confirmed they understand the capabilities

---

## 0.3 Data Flow Overview
> **Status:** N/A — conceptual overview, not code

This section shows the complete data flow picture in one place. The details are scattered across §2 (file structure), §2A (lifecycle), §2.3A (atomic writes), §5.3 (storage model), §0.2 (OpenClaw integration), and §22B (bridge plugin). **Start here** to understand how data moves through Pane.

### 0.3.1 Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│  USER'S MACHINE (MacBook or same machine as OpenClaw)               │
│  PANE = PRIMARY (source of truth)                                   │
│                                                                       │
│  ┌────────────────────────────────────────────────────────────┐     │
│  │                    PANE APP                                 │     │
│  │                                                             │     │
│  │  ┌─────────────┐         ┌──────────────────────┐          │     │
│  │  │   UI/Chat   │ ◄─────► │   SQLite Database    │          │     │
│  │  │  Interface  │         │     (pane.db)        │          │     │
│  │  └─────────────┘         │  - Runtime source    │          │     │
│  │         │                │  - Performance layer │          │     │
│  │         │                └──────────┬───────────┘          │     │
│  │         │                           │                      │     │
│  │         │                           ▼                      │     │
│  │         │                ┌──────────────────────┐          │     │
│  │         └───────────────►│  Flat Files (PRIMARY)│          │     │
│  │                          │  (.pane/system/)     │          │     │
│  │                          │  - Source of truth   │          │     │
│  │                          └──────────┬───────────┘          │     │
│  │                                     │                      │     │
│  │                                     │ HTTP push/pull       │     │
│  │                                     │ (split-machine)      │     │
│  │                                     │ or symlink (same-    │     │
│  │                                     │ machine)             │     │
│  └─────────────────────────────────────┼──────────────────────┘     │
└─────────────────────────────────────────┼──────────────────────────┘
                                          │
                        ┌─────────────────┼─────────────────┐
                        │ Push when Pane  │  Pull on        │
                        │ saves (POST     │  reconnect      │
                        │ /bridge/push)   │  (GET           │
                        │                 │  /bridge/pull)  │
                        └─────────────────┼─────────────────┘
                                          ▼
┌──────────────────────────────────────────────────────────────────────┐
│  OPENCLAW MACHINE (Mac Studio or same machine)                       │
│  OPENCLAW = REPLICA (receives pushes from Pane)                      │
│                                                                       │
│  ┌────────────────────────────────────────────────────────────┐     │
│  │                  OpenClaw Gateway                           │     │
│  │                                                             │     │
│  │  ┌──────────────────────┐       ┌────────────────────┐    │     │
│  │  │  AI Agents (Brit,    │       │  Gateway Plugin    │    │     │
│  │  │  Stia, Reed, etc.)   │◄──────┤  (pane-bridge)     │    │     │
│  │  │                      │       │                    │    │     │
│  │  │  Read: workspace/    │       │  Receives pushes   │    │     │
│  │  │    {agent}/          │       │  Writes to         │    │     │
│  │  │    ├─ SOUL.md        │       │  workspace/        │    │     │
│  │  │    ├─ MEMORY.md      │       │  (REPLICA)         │    │     │
│  │  │    └─ logs/*.md      │       │                    │    │     │
│  │  │  (REPLICA)           │       │  Queues pending    │    │     │
│  │  │                      │       │  when Pane closed  │    │     │
│  │  │  Write when Pane     │       │                    │    │     │
│  │  │  closed: queues to   │       │  Exposes /bridge/  │    │     │
│  │  │  sync_pending        │       │  pull for recon-   │    │     │
│  │  │                      │       │  ciliation         │    │     │
│  │  └──────────────────────┘       └────────────────────┘    │     │
│  └────────────────────────────────────────────────────────────┘     │
└──────────────────────────────────────────────────────────────────────┘

TWO MODES:
  1. Pane open → Push sync (Pane saves → POST /bridge/push → gateway writes)
  2. Pane closed → OC writes locally, queues to sync_pending. On reconnect:
     GET /bridge/pull → Pane ingests → reconciles → pushes canonical back.
```

### 0.3.2 Three Representations of the Same Data

Every piece of data in Pane exists in up to three forms:

1. **SQLite Database** (`pane.db`) — **Runtime source**
   - What the app reads from for performance
   - Structured queries, fast lookups, indexes
   - Note metadata, task fields, tags, timestamps
   - Extracted memory facts (before synthesis)
   - Chat transcripts

2. **Flat Markdown Files** (`.pane/system/` and `Projects/`)
   
   **System notes** (`.pane/system/`) — **FILE-ONLY (source of truth)**
   - Examples: `SOUL.md`, `MEMORY.md`, `RULES.md`, `logs/YYYY-MM-DD.md`
   - DB body column is NULL for these notes
   - Human-editable, survives Pane uninstall
   - Wins on conflict (§0.3.4)
   
   **User notes** (`Projects/`) — **BACKUP (DB is primary at runtime)** [#486, v0.59.0]
   - Notes, tasks, project documentation
   - DB `body` column stores full content (primary source)
   - Flat files written on every save (dual-write for portability)
   - File wins on conflict only if newer timestamp

3. **OpenClaw Workspace** (Tier 3 only) — **REPLICA (receives pushes from Pane)**
   - Maintained by push sync from Pane → Gateway (§22B.3)
   - Same-machine: symlink to `.pane/system/` (no network needed)
   - Split-machine: HTTP push/pull via gateway plugin
   - AI agents read from replica
   - When Pane closed: agents write locally, queue to sync_pending for reconciliation

**Key insight:** Pane is primary, OpenClaw is replica. Push sync flows Pane→OC. Pull reconciliation flows OC→Pane for conflict resolution. The database is for speed, flat files are for truth, OpenClaw workspace is a maintained copy.

### 0.3.3 Primary-Replica Principle

**Pane is PRIMARY. OpenClaw is REPLICA.**

This is the foundational architectural constraint for Tier 3 sync:

- **Pane owns the database** (pane.db) — runtime source of truth for:
  - All note metadata (title, timestamps, task fields)
  - User note content (body column, #486 v0.59.0)
  - Extracted memory facts
  - Chat transcripts
- **Pane owns flat files** (`.pane/system/` and `Projects/`):
  - System notes (SOUL.md, MEMORY.md, logs, etc.) — FILE-ONLY, body=NULL in DB
  - User notes (Projects/) — BACKUP, written on every save for portability
- **Push direction: Pane → OpenClaw** — always. Every write flows this direction when Pane is open.
- **OpenClaw workspace is read-only** when Pane is open — agents consume, don't modify
- **When Pane is closed** — OpenClaw agents CAN write locally (for autonomous operation), but these writes queue to `sync_pending` for reconciliation when Pane reconnects
- **Pull reconciliation: OpenClaw → Pane** — only for conflict resolution. Pane ingests pending changes, resolves conflicts (timestamp-based, newer wins), and pushes canonical state back.

**Why:** Pane is the user-facing interface. It must be the authoritative source. OpenClaw is infrastructure — powerful, but subordinate. This prevents split-brain scenarios and ensures the user's edits in Pane always win.

**One writer per file** (§5.7): Synthesis is the sole writer to Memory notes. Extraction writes to DB only. This prevents mixed bullet/prose formats and eliminates race conditions.

### 0.3.3A Who Writes What

| Writer | Target | Then... |
|--------|--------|---------|
| **Pane UI** (user edits note) | **User notes:** DB body + file (dual-write) [#486]<br>**System notes:** File only (body=NULL) | POST /bridge/push (if Tier 3 system note) |
| **AI chat/tools** (create note, extract memory) | **User notes:** DB body + file (dual-write)<br>**System notes:** File only | POST /bridge/push |
| **Memory synthesis** (§5.5) | Flat files only (`.pane/system/{agent}/MEMORY.md`, `logs/*.md`)<br>These are system notes | DB mirrors metadata on next startup → POST /bridge/push |
| **OpenClaw agents** (Tier 3, Pane closed) | Local workspace files (system notes only) | Writes queued to sync_pending → pulled via GET /bridge/pull on reconnect |
| **Gateway plugin** (§22B) | OpenClaw workspace files (REPLICA) | Receives pushes from Pane, writes locally, queues pending when Pane closed |

### 0.3.4 Conflict Resolution

**Timestamp-based, newer wins.** All conflict resolution is automatic, deterministic, no user dialogs.

#### Local Conflicts (Pane DB vs Flat Files)

If the DB and flat file disagree:

1. On startup, Pane checks file `modified_at` timestamps
2. If flat file is newer than DB's `updated_at`, file content is read and DB is updated
3. Newer timestamp wins (implemented v11.28.65)
4. User never sees a conflict dialog — the system resolves automatically

**Why newer timestamp wins [#486]:** 
- **System notes** (SOUL.md, MEMORY.md, logs): File is source of truth, DB mirrors metadata only
- **User notes** (Projects/): DB body is runtime primary, file is backup
- On conflict: newer timestamp wins regardless of source (prevents data loss on external edits)
- DB is optimized for runtime performance, files provide portability and backup

#### Remote Conflicts (Pane vs OpenClaw, Tier 3 Split-Machine)

**Two operating modes:**

**Mode 1: Pane open (normal operation)**
- Pane pushes all edits to OpenClaw via `POST /bridge/push`
- OpenClaw workspace is read-only replica
- No conflicts possible — push is one-directional

**Mode 2: Pane closed (autonomous OpenClaw operation)**
- OpenClaw agents can write to local workspace files
- Gateway plugin queues these writes to `sync_pending` table with timestamps
- When Pane reconnects:
  1. Pane calls `GET /bridge/pull` → retrieves `sync_pending` entries
  2. For each pending change:
     - Compare timestamps: Pane's `file_modified_at` vs OC's `queued_at`
     - **Newer wins** — if OC timestamp > Pane timestamp, ingest the change
     - If Pane timestamp > OC timestamp, discard (Pane's version is fresher)
  3. After ingestion, Pane reconciles and pushes canonical state back via `POST /bridge/push`
  4. Gateway clears `sync_pending` queue

**Why timestamp-based:** Simple, deterministic, no merge algorithms needed for identity files. These are small text files, not collaborative documents. Last-edit wins is the right model.

**Example reconciliation flow:**

```
1. Pane closes at 14:00, MEMORY.md modified_at = 14:00
2. OpenClaw agent updates MEMORY.md at 15:30 → queued to sync_pending
3. Pane reconnects at 16:00
4. GET /bridge/pull → sees pending change with timestamp 15:30
5. Pane's file still shows 14:00 → OC version is newer
6. Pane ingests OC changes, merges into MEMORY.md, sets modified_at = 16:00
7. POST /bridge/push → sends canonical version back
8. sync_pending cleared
```

**No `.conflict` files needed.** The queue + timestamp approach eliminates silent overwrites. Everything is tracked.

### 0.3.5 Deployment Variants

#### Single Machine (Pane + OpenClaw on same box)
- Flat files are local: `~/Pane/.pane/system/`
- OpenClaw workspace symlinks to flat files: `~/.openclaw/workspace/{agent}/ → ~/Pane/.pane/system/{agent}/`
- No network involved, no HTTP push/pull needed
- Symlink keeps both sides in sync instantly
- Most common setup for power users (one Mac Studio, one user)

#### Split Machine (current Matt/Brit setup)
- **User's MacBook:** Runs Pane app, SQLite DB, flat files stored locally
- **Mac Studio:** Runs OpenClaw gateway + agents
- **Network:** Tailscale (recommended), LAN, or internet. Tailscale provides the encrypted network tunnel between machines. Any network that can reach the gateway URL works.
- **Sync method (over that network):** 
  - Push: Pane saves → `POST /bridge/push` → gateway writes to local workspace
  - Pull: Pane reconnects → `GET /bridge/pull` → reconciles pending changes
- **OpenClaw workspace:** Local files on Mac Studio (REPLICA), maintained by gateway plugin
- **Tailscale is the network, not the sync mechanism.** No Tailscale file sharing. No cross-machine symlinks. Sync is HTTP push/pull over the Tailscale tunnel (or any network).
- **Why:** Separates UI machine (portable) from AI compute (powerful, always-on)

#### Onboarding from Existing OpenClaw Installation
- **Scenario:** User has OpenClaw running with agents (Brit, Stia, etc.)
- **Setup:** Fresh Pane install on same machine OR different machine
- **Flow:**
  1. Pane detects no local `.pane/system/` agents
  2. User enters OpenClaw gateway URL + auth token
  3. Pane calls `GET /v1/agents` → discovers existing agents (Brit, Stia, etc.)
  4. Pane imports agent list to `ai_agents` table
  5. **Pane becomes PRIMARY** — OpenClaw workspace becomes REPLICA
  6. All future edits flow Pane → OpenClaw via push sync
- **Result:** Existing OpenClaw setup preserved, Pane takes over as UI/primary
- **No manual file copying.** Discovery via gateway API, sync via push/pull.

#### No OpenClaw (Tier 1/2 — Sandbox Mode)
- Pane app only, no external AI infrastructure
- DB ↔ flat file sync still happens internally
- No gateway, no symlink, no network
- AI runs via OpenRouter/Anthropic API, sandboxed to `~/Pane/`
- Memory extraction, synthesis, daily logs all work — they're Pane features, not OpenClaw features

### 0.3.6 Cross-References for Deep Dives

- **§2** — File system structure (directory layout, markdown files)
- **§2A** — File lifecycle (create, move, delete, trash)
- **§2.3A** — Atomic write safety (unified write function, verification, bridge trigger)
- **§5.3** — Storage model (DB vs flat files, why flat files win)
- **§5.4** — Memory extraction (DB writes, no direct file writes)
- **§5.5** — Memory synthesis (sole writer to Memory notes, prose output)
- **§0.2** — OpenClaw integration (Operator Mode, extended tools)
- **§22B** — Bridge plugin (file sync, conflict resolution, event-driven pushes)

**When in doubt:** DB for queries, flat files for truth, bridge for sync.

---

## Table of Contents

1. [Data Model — Unified Entity System](#1-data-model--unified-entity-system)
2. [File System Structure](#2-file-system-structure)
   - [2A. File Lifecycle & Deletion Architecture](#2a-file-lifecycle--deletion-architecture)
   - [2B. Database Safety & Migration Architecture](#2b-database-safety--migration-architecture)
   - [2C. File Write Safety](#2c-file-write-safety)
   - [2D. Pre-Ship Audit Requirements](#2d-pre-ship-audit-requirements)
3. [AI Context System](#3-ai-context-system)
4. [AI Tool Layer](#4-ai-tool-layer)
5. [AI Memory System — Three Layers](#5-ai-memory-syste## Appendix C: Related Documents
> **Status:** N/A (reference doc)

| Document | Purpose | Location |
|----------|---------|----------|
| **Master Roadmap** | Features, phases, pricing, marketing, timeline | `pane-master-roadmap.md` |
| **Execution Plan** | Exact build sequence and key decisions | `pane-execution-plan.md` |
| **Home Dashboard Spec** | Onboarding wizard flow, module types, UX details | `home-dashboard-spec.md` |
| **AI Chat Panel Concepts** | Lane's visual designs for chat UI | `ai-chat-panel-concepts.md` |
| **Home Screen Mockups** | Lane's visual mockups (HTML) | `home-screen-mockups.html`, `home-daily-dashboard-mockup.html` |
| **Sync Implementation Guide** | Reed's deep-dive with code examples (1,500 lines) | `~/Shared/overnight/2026-02-24/pane-sync-implementation-guide.md` |

---

*This is the canonical architecture specification for Pane. It supersedes `ai-architecture-spec.md` and should be referenced by all future specs and implementation work. When in doubt, this document wins.*

## 25. AI Team System
> **Status:** Spec lives in `docs/spec/ai-teams.md`. NOT BUILT (spec'd 2026-03-09; section number assigned 2026-05-02 after §19 collision resolved per ADR-0021).

Users create, manage, and assign AI teammates through conversation. Teams visible on the Home screen with name, role, soul note, memory note, model. See `docs/spec/ai-teams.md` for full spec.

## 34. Data Change Refresh Point
> **Status:** Spec lives in `docs/spec/data-refresh.md`. NOT BUILT (#2972, tracked on #2874).

One entity-identified event (`entity_id`, `entity_type`, `reason`) for every note/sheet/folder/project change, replacing `pane-data-changed`, `pane-note-created`, `pane-note-changed`, `pane-project-changed`, and `pane-task-meta-changed`. Two senders (backend, app), identical payload. Origin-tag echo suppression. Every screen patches/inserts/removes in place — never reloads. See `docs/spec/data-refresh.md` for the full spec, stale scope-doc corrections, migration table, and four-stage build-out.

## 35. Startup Sequencing & Migration UX
> **Status:** Spec lives in `docs/spec/startup.md`. BUILT (#2723 PR B).

On a launch that needs a schema migration, the window opens first ("Updating Pane… don't quit" with real progress) while the backup + migration run on a background thread; every other launch takes the exact same synchronous path it always has. Generalizes issue #353's daily-backup-to-background-thread pattern to the one case that can actually block the window. `StartupStatus`/`get_startup_status` gates the frontend race-safely (command, not event, for the initial fast/slow decision). See `docs/spec/startup.md` for the ordering guarantee, progress events, and failure-path design.
