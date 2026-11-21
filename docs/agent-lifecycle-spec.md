# Pane Agent Lifecycle Specification
*Version 2.0 · 2026-03-29*  
*Author: Reed (sub-agent) · Three Lane Studios LLC*  
*Status: Canonical — supersedes v1.0 draft, incorporates Matt's review decisions*

---

## 0. Overview & Purpose

This specification defines the complete lifecycle for agents in Pane — from creation through daily operation to deactivation and deletion. It consolidates design decisions from issues #279, #278, and #289, and extends the architecture defined in `pane-architecture-spec.md` §24.

**Current state:** Pane has import-based agent discovery only. During onboarding, the integrity check scans `.pane/system/` for existing agent folders and creates `ai_agents` DB records. Agents cannot be created through the UI or by the strategist yet.

**This spec enables:** User-initiated agent creation (v1), proper atomicity guarantees, multi-user slug handling (for Teams, v2+), and lifecycle operations beyond creation.

---

## 1. V1 Philosophy — User-Initiated Only

**Decision (Matt review, 2026-03-29):** Agent creation in v1 is **user-initiated only**. The strategist does NOT propose agents. No AI-initiated agent creation flows. This simplifies the UX, avoids unwanted agent proliferation, and keeps costs predictable.

**User creates agents via:**
1. **Settings → Agents button** — "New Agent" opens creation dialog
2. **Chat request** — "Create an agent named Quinn for research" → AI recognizes intent, shows creation card, waits for user confirmation

**Why user-only for v1:**
- **Predictable costs:** Every agent adds token cost (system notes loaded as context). User controls the roster explicitly.
- **No agent sprawl:** AI can't create 10 specialists without asking. User decides when to expand the team.
- **Simpler UX:** One creation path. No "AI proposed an agent" notifications to manage.

**V2 and beyond:** AI proposals may return as an opt-in feature. Spec sections covering AI proposals are preserved but marked as v2/future scope.

---

## 2. Agent Cap — Hard Limit Enforcement

**Decision (Matt review, 2026-03-29):** Pane enforces a **hard cap** on active agents. Default: **5 agents** (main + 4 specialists). Configurable in Settings, but always a hard rejection — not a soft warning.

### 2.1 Why a Hard Cap

Every active agent adds token cost to every conversation:
- Agent's SOUL.md loaded into context (if agent is relevant to current project)
- Agent's MEMORY.md loaded for cross-project knowledge
- Agent's recent logs loaded for "what's this agent been doing" context

With 10 agents, the strategist's context window is significantly smaller for actual work. A hard cap keeps the system fast and predictable.

### 2.2 Enforcement

When user attempts to create a new agent:
1. Check active agent count: `SELECT COUNT(*) FROM ai_agents WHERE is_active = 1`
2. If count >= configured cap (default 5), show error:
   - **"Agent limit reached (5/5). Deactivate an unused agent to create a new one."**
   - List all active agents with "Deactivate" buttons
3. Creation dialog does NOT open until count < cap

**No soft warnings.** No "are you sure?" after the limit. The limit is the limit. User must deactivate before creating.

### 2.3 Configuration

Settings → Agents → "Agent Limit":
- Default: 5
- Range: 3–10
- Help text: "Each agent adds ~500-1000 tokens to context per conversation. Lower is faster."

**For power users (Tier 3/OpenClaw):** Can increase to 8-10 if they understand the cost/speed trade-off.

**For most users:** 5 is the sweet spot — main strategist + 4 specialists (coder, researcher, writer, analyst).

---

## 3. No Project-Level Agent Assignment (V1)

**Decision (Matt review, 2026-03-29):** In v1, there is **no project-level agent assignment**. The main strategist agent handles all projects. Specialists are spawned by the strategist **as needed for specific tasks**, not assigned to projects permanently.

### 3.1 How It Works

```
User is in Project A
    ↓
Strategist receives context (Project A's memory, notes, tasks)
    ↓
User: "Research this technical topic"
    ↓
Strategist decides: "This is a research task. I'll spawn Quinn."
    ↓
Strategist spawns Quinn sub-agent with task description
    ↓
Quinn completes research, returns results
    ↓
Strategist integrates results, responds to user
```

**No static "this project uses agent X" assignment.** The strategist is the entry point for every project. Specialists are tools the strategist uses, not replacements for the strategist.

### 3.2 Why No Project Assignment in V1

- **Simpler mental model:** User talks to one agent (strategist). Strategist coordinates everything.
- **No context confusion:** User doesn't have to remember "which agent am I talking to right now?"
- **Strategist as orchestrator:** This aligns with the orchestration vision (§20 of arch spec). The strategist is the brain. Specialists are the hands.

### 3.3 V2 Consideration

Post-launch, project-level agent assignment could be optional:
- User can set a "primary agent" for a project
- That agent handles direct questions about the project
- Strategist still available as fallback

But for v1: **strategist is default for every project, always.**

---

## 4. Identity Management — Strategist as HR

**Decision (Matt review, 2026-03-29):** The strategist **creates and updates SOUL.md/MEMORY.md for specialists**. Specialists own their daily logs and working memory, but identity documents are managed by the strategist. Think: HR department manages employee profiles, employees manage their own daily work.

### 4.1 What the Strategist Manages

For each specialist agent:
- **SOUL.md** — personality, role, communication style
- **MEMORY.md** — accumulated knowledge about this specialist's domain

These are created by the strategist when spawning a new specialist. Updated by the strategist when a specialist's role evolves.

### 4.2 What the Specialist Owns

For specialists that are real sub-agents (Tier 3 only, see §5):
- **Daily logs** — what this specialist did today, written by the specialist itself
- **Working memory** — temporary context for active tasks, not persisted long-term

### 4.3 Why This Division

- **Strategist has the big picture.** It knows all agents, their roles, how they fit together. It's best positioned to define agent identity.
- **Specialists focus on execution.** They don't need to manage their own profile — they just do the work they're assigned.
- **Consistency:** All agent identities follow a consistent format because the strategist creates them all.

### 4.4 Creation Flow (User → Strategist → Specialist)

1. User clicks "New Agent" or asks in chat
2. Strategist receives request: "Create a research specialist"
3. Strategist generates SOUL.md and empty MEMORY.md for the new agent
4. Strategist calls `create_agent` tool (backend creates folder + DB record)
5. Strategist announces: "Quinn is ready. Try asking Quinn to research X."

The user doesn't write the SOUL.md. The strategist writes it based on the role description the user provided.

---

## 5. Tier Distinction — Personas vs. Sub-Agents

**Decision (Matt review, 2026-03-29):** What "creating an agent" means depends on the tier. This distinction is **mandatory to understand** before implementing agent creation.

### 5.1 Tier 1/2 (No OpenClaw) — Specialists are Personas

**No process isolation. No separate runtimes.** Creating a specialist in Tier 1/2 means:
- A new folder under `.pane/system/` with SOUL.md and MEMORY.md
- A new DB record in `ai_agents`
- When the strategist "spawns" this specialist, it **switches hats**: loads the specialist's SOUL.md as system prompt, responds as that persona, then switches back

**Example:**
```
Strategist: "I'll have Quinn research this for you."
[Strategist internally loads Quinn's SOUL.md]
Strategist (as Quinn): "Here's what I found... [research results]"
[Strategist switches back to main identity]
Strategist: "Quinn found these results. Should we proceed with Option A?"
```

From the user's perspective, Quinn "did the research." From the implementation's perspective, the strategist adopted Quinn's persona and used the same inference path.

**Why this works:**
- Tier 1/2 users don't have OpenClaw's exec/fs access anyway
- Specialist "identities" are useful even without true parallelism — they give the AI different perspectives
- Simpler to implement: no sub-agent spawning logic, no process management

**Limitations:**
- No true parallelism (can't spawn Quinn and Stia at the same time)
- No separate context windows (Quinn's conversation merges with strategist's)
- No exec tools, no file operations outside `~/Pane/`

### 5.2 Tier 3 (OpenClaw) — Specialists are Real Sub-Agents

**Full process isolation. Separate runtimes.** Creating a specialist in Tier 3 means:
- Same as Tier 1/2: folder + DB record + identity files
- PLUS: OpenClaw registers the agent in its config (`openclaw.json`) via the Pane-bridge plugin (see §6)
- When the strategist spawns this specialist, OpenClaw creates a **real sub-agent session** with its own context window, tool access, and exec capabilities

**Example:**
```
Strategist: "I'll have Quinn research this for you."
[Strategist spawns Quinn via OpenClaw sessions_spawn]
[Quinn runs in separate process with own context]
[Quinn can use exec, read, write tools independently]
[Quinn completes research, returns results via sub-agent channel]
Strategist: "Quinn found these results. Should we proceed?"
```

True parallelism. Quinn and Stia can work at the same time. Strategist orchestrates, specialists execute.

**Why this matters:**
- Tier 3 users have the infrastructure to support real sub-agents
- Unlocks complex orchestration: strategist delegates 5 tasks to 5 specialists in parallel
- Each specialist has dedicated context budget (not competing with strategist's context)

**Requirements:**
- OpenClaw gateway running locally or via Tailscale
- Pane-bridge plugin installed and configured
- User understands the capabilities (Settings → Operators Mode toggle)

### 5.3 UI Clarity — "Create Agent" Means Different Things

The agent creation UI must be **clear about what's being created**:

**For Tier 1/2 users:**
> "Create a new agent identity. The strategist will adopt this persona when needed."

**For Tier 3 users:**
> "Create a new sub-agent with independent exec access. OpenClaw will manage this as a real agent."

**Settings → Agents page shows tier:**
- Tier 1/2: "5 agent personas"
- Tier 3: "5 sub-agents (2 active)"

Same underlying data structure (`ai_agents` table), different runtime behavior.

---

## 6. Gateway Plugin for Tier 3 Registration

**Decision (Matt review, 2026-03-29):** When a new agent folder appears in `.pane/system/`, the Pane-bridge gateway plugin **auto-detects and registers it** in OpenClaw's config (`openclaw.json`). This is path-based, not network-based — works whether Pane and OpenClaw are on the same machine or connected via Tailscale.

### 6.1 How It Works

1. User creates agent "Quinn" in Pane
2. Pane writes `.pane/system/quinn-research-specialist/SOUL.md` and `MEMORY.md`
3. Pane-bridge plugin watches `.pane/system/` directory (configured path)
4. Plugin detects new folder, reads agent metadata from files
5. Plugin adds entry to OpenClaw's `openclaw.json`:

```json
{
  "agents": [
    {
      "name": "Quinn — Research Specialist",
      "slug": "quinn-research-specialist",
      "model": "claude-sonnet-4",
      "workspace": "~/Pane/.pane/system/quinn-research-specialist",
      "permissions": ["read", "write", "exec"],
      "source": "pane-bridge"
    }
  ]
}
```

6. OpenClaw reloads config (or picks up change on next session spawn)
7. Strategist can now spawn Quinn via `sessions_spawn` tool

### 6.2 The Contract — What Pane Provides

For the plugin to auto-register an agent, these files must exist:

| File | Purpose | Plugin Reads |
|------|---------|--------------|
| `SOUL.md` | Agent identity | First heading as name, role from content |
| `MEMORY.md` | Agent knowledge | Existence check only (content not parsed) |
| `config.json` (optional) | Agent settings | Model preference, tool permissions |

**Minimal config.json** (optional, in agent's folder):
```json
{
  "model": "claude-sonnet-4",
  "permissions": ["read", "write", "exec"],
  "workspace": "relative"
}
```

If `config.json` is missing, plugin uses defaults:
- Model: same as main agent
- Permissions: read/write only (no exec)
- Workspace: agent's folder path

### 6.3 Path-Based, Not Network-Based

**The plugin watches a file path.** That path can be:
- **Local:** `~/Pane/.pane/system/` on the same machine
- **Remote:** Tailscale mount like `~/mnt/tailscale-pane/.pane/system/` if Pane runs on a different machine

**Deployment detail, not architectural constraint.** The plugin doesn't care if Pane and OpenClaw are on the same machine — it just watches a directory.

**Configuration example (OpenClaw side, pane-bridge plugin config):**
```json
{
  "watch_path": "~/Pane/.pane/system/",
  "auto_register": true,
  "default_model": "claude-sonnet-4",
  "default_permissions": ["read", "write"]
}
```

### 6.4 Registration Fields

| Field | Source | Notes |
|-------|--------|-------|
| `name` | SOUL.md first heading | Falls back to folder name if missing |
| `slug` | Folder name | Must match Pane's slug generation |
| `model` | config.json or default | Can be overridden per-session |
| `workspace` | Agent folder path | Symlink or direct path |
| `permissions` | config.json or default | Tool access control |
| `source` | Always `"pane-bridge"` | Identifies origin |

### 6.5 Removal Detection

When an agent is **deleted** in Pane:
1. Folder is removed from `.pane/system/`
2. Plugin detects deletion (via file watcher)
3. Plugin removes agent from OpenClaw config
4. Any active sessions for that agent are terminated gracefully

When an agent is **deactivated** in Pane (soft delete):
- Folder remains, but Pane marks `is_active = 0` in DB
- Plugin does NOT remove from OpenClaw config
- OpenClaw can still spawn the agent if explicitly requested (future: respect deactivation flag)

### 6.6 Conflict Resolution

**Same slug in Pane and OpenClaw manually added:**
- Plugin skips auto-registration if an agent with that slug already exists in OpenClaw config
- Logs warning: "Agent 'quinn-research-specialist' already exists in OpenClaw config. Skipping auto-register."
- User can manually resolve in OpenClaw config

**Pane and OpenClaw on different versions:**
- Plugin is versioned independently. Compatible with Pane v1.x and OpenClaw v2.x.
- If SOUL.md format changes in future Pane versions, plugin updates to parse new format.

---

## 7. Export/Import — Deferred to V2

**Decision (Matt review, 2026-03-29):** Agent export/import is **v2 scope**. Note that it's essentially formalizing the Tier 3 onboarding process that already exists — not a new concept, just needs first-class UX.

### 7.1 What Export/Import Means

**Export:** Package an agent (SOUL.md, MEMORY.md, logs/, config.json) into a `.pane-agent` archive. User can share it or move it to another Pane instance.

**Import:** Unpack a `.pane-agent` archive, validate contents, create agent in the target Pane instance.

### 7.2 Why V2

- V1 focuses on single-user, single-device workflows
- Export/import is primarily useful for:
  - Multi-device setups (Pane on laptop + desktop)
  - Team agent templates (research specialist config shared across team)
  - Agent marketplace (future vision: community-shared agent identities)
- The underlying mechanism already works: Tier 3 users manually copy `.pane/system/{agent}/` folders and it Just Works™. Export/import is UX polish on top of that.

### 7.3 V2 Implementation Notes (Preserved for Future)

**Export format:**
```
agent-name.pane-agent (zip archive)
├── manifest.json          # version, creation date, Pane version
├── SOUL.md
├── MEMORY.md
├── config.json (optional)
└── logs/ (optional)       # last 7 days of logs
```

**Import validation:**
- Check manifest version compatibility
- Validate SOUL.md format
- Detect slug collision (offer to rename)
- User reviews agent identity before confirming import

**Future enhancement:** Agent marketplace (à la VS Code extensions). Users publish agents, others browse/install. Revenue model: premium agents, team templates.

---

## 8. Open Questions — Resolved

**Decision (Matt review, 2026-03-29):** All open questions from v1.0 draft are now resolved:

| Question | Resolution |
|----------|-----------|
| **AI proposals: stay pending indefinitely?** | Moot for v1 (no AI proposals). Noted for v2: proposals expire after ignored for 24 hours. |
| **Main agent status transferable?** | **No.** `is_main` is permanent. Set during onboarding, never changes. User can edit main agent's SOUL.md to change personality instead. |
| **Sub-agents auto-deactivate after idle?** | **No.** User must manually deactivate. No auto-deactivation in v1. V2 could add as opt-in with user preference toggle. |
| **Agent cap: soft or hard?** | **Hard cap** (see §2). Default 5, configurable 3-10. Hard rejection, not warning. |
| **Export/import scope?** | **V2** (see §7). Formalizes existing Tier 3 pattern, not new architecture. |

---

## 9. Agent Creation Flow — Detailed Steps

### 9.1 User-Initiated Creation (Two Paths)

#### Path A: Settings → Agents → New Agent

1. User clicks "New Agent" button
2. Creation dialog opens with empty fields
3. User fills in:
   - **Name** (required): "Quinn — Research Specialist"
   - **Role** (required): "Conducts research on technical topics, summarizes findings, cites sources"
   - **Model** (optional): defaults to main agent's model, or user selects from dropdown
4. Dialog shows:
   - **Agent cap status**: "3 / 5 agents" (green if under cap, red if at cap)
   - **Estimated cost**: "~$2–5/month based on typical usage" (calculated per §9.3)
   - **Files to be created**: Preview of folder path and files
5. User clicks "Create Agent"
6. Backend executes creation steps (§10)
7. On success: Dialog closes, user sees success toast, Settings → Agents page refreshes with new agent listed
8. Strategist receives context update: "New agent Quinn is available. Consider using Quinn for research tasks."

#### Path B: Chat Request

1. User types: "Create an agent named Quinn for research work"
2. Strategist recognizes intent (via natural language understanding, not a tool call yet)
3. Strategist responds: "I'll create a research agent named Quinn. Let me show you the details."
4. Creation dialog appears (same as Path A) with fields pre-filled from user's message:
   - Name: "Quinn"
   - Role: "Research work" (user can edit to be more specific)
5. Steps 4-8 same as Path A

### 9.2 Agent Cap Check (Pre-Flight)

Before showing the creation dialog:
```sql
SELECT COUNT(*) FROM ai_agents WHERE is_active = 1;
```

If count >= cap (default 5):
1. Show error dialog: "Agent limit reached (5/5). Deactivate an unused agent to create a new one."
2. List all active agents with "Deactivate" buttons
3. User must deactivate one before creation dialog opens

### 9.3 Cost Estimation Formula

```
conversations_per_month = 15  # assumption for sub-agent
tokens_per_conversation = 5000  # 3K input, 2K output
input_tokens = conversations_per_month * 3000
output_tokens = conversations_per_month * 2000

cost_input = (input_tokens / 1_000_000) * model_cost_per_1M_input
cost_output = (output_tokens / 1_000_000) * model_cost_per_1M_output

total_cost = cost_input + cost_output
display_cost = round_up_to_dollar(total_cost)  # e.g., $0.59 → "$1"
```

**Example (claude-sonnet-4):**
- Input: $3/M, Output: $15/M
- 15 convos/month, 5K tokens/convo
- Cost: (45K / 1M) * $3 + (30K / 1M) * $15 = $0.135 + $0.45 = $0.585
- Display: "~$1/month"

**Note:** This is informational only. Not a binding quote. Actual usage varies.

---

## 10. Creation Mechanics — Filesystem First, DB Second

**Design principle (from #279):** Files are the source of truth. The database is a runtime index. If creation crashes mid-process, the integrity check (§11) will reconcile on next launch.

### 10.1 Creation Steps (Atomic Where Possible)

When the user confirms agent creation:

**Step 1: Validate inputs**
1. Check agent cap (reject if at limit)
2. Generate slug from name (§10.2)
3. Check for slug collision (§10.3)
4. If collision, append `-2`, `-3`, etc. until unique

**Step 2: Filesystem operations**
5. Create directory: `.pane/system/{slug}/`
6. Strategist generates SOUL.md content based on role description (AI call)
7. Write SOUL.md to disk
8. Write empty MEMORY.md to disk
9. If Tier 3 (OpenClaw), optionally write config.json with model/permissions

**Step 3: Database operation**
10. Insert record into `ai_agents` table (see §10.4)
11. Set `source = 'created'` to distinguish from `'import'` or `'recovered'`
12. Compute and store file hashes (`soul_hash`, `memory_hash`) for integrity checking

**Step 4: Post-creation**
13. If Tier 3: Pane-bridge plugin detects new folder, auto-registers in OpenClaw (§6)
14. Success message to user (toast + chat if initiated from chat)
15. Settings → Agents page updates to show new agent

### 10.2 Slug Generation Rules

**Slug** = URL-safe, unique identifier for the agent, used in filesystem paths and DB lookups.

**Algorithm:**
1. Convert name to lowercase
2. Replace spaces with hyphens
3. Remove all characters except `a-z`, `0-9`, and `-`
4. Collapse multiple consecutive hyphens to one
5. Trim leading/trailing hyphens

**Examples:**
| Name | Slug |
|------|------|
| `Brit Freed` | `brit-freed` |
| `Quinn — Research Specialist` | `quinn-research-specialist` |
| `Agent007` | `agent007` |
| `  Extra   Spaces  ` | `extra-spaces` |
| `Émile Zoë` | `mile-zo` (non-ASCII removed) |

**Implementation:** `fn generate_slug(name: &str) -> String` in `agents.rs` (already exists in codebase).

### 10.3 Slug Collision Resolution

**Constraint:** `UNIQUE(slug)` on `ai_agents` table (single-user v1).

**Collision detection:**
1. After generating slug from name, query DB: `SELECT id FROM ai_agents WHERE slug = ?`
2. If exists, append `-2`, `-3`, etc. until unique slug is found
3. Example: User creates "Brit Freed" (slug: `brit-freed`). Later tries another "Brit Freed" → slug becomes `brit-freed-2`.

**UI feedback:** If collision requires a suffix, show warning in creation dialog:
> "An agent named 'Brit Freed' already exists. This agent will be 'Brit Freed 2' (slug: `brit-freed-2`)."

User can edit the name to avoid the suffix if desired.

**Edge case — slug too long:** If generated slug exceeds 200 chars, truncate before appending collision suffix. Log a warning.

### 10.4 Database Schema

```sql
INSERT INTO ai_agents (
    id,                  -- UUID generated
    name,                -- User-provided
    slug,                -- Generated from name
    role,                -- User-provided
    description,         -- NULL (optional field, user can edit later)
    model,               -- Selected from dropdown or inherited from main agent
    is_main,             -- 0 (sub-agents), 1 only for main agent
    is_active,           -- 1 by default
    workspace_path,      -- Same as slug currently
    soul_hash,           -- SHA-256 of SOUL.md content
    memory_hash,         -- SHA-256 of MEMORY.md content
    config_json,         -- '{}' initially
    owner_id,            -- NULL for single-user, set for Teams (v2)
    source,              -- 'created'
    created_at,          -- Timestamp
    updated_at           -- Timestamp
) VALUES (...);
```

### 10.5 SOUL.md Generation (AI-Assisted)

When creating an agent, the strategist generates the SOUL.md content:

**Prompt to strategist:**
```
Create a SOUL.md for a new agent with this role: "{user_provided_role}".

Format:
# {Agent Name}

**Role:** {role description}

**Personality:** [2-3 sentences about how this agent thinks and acts]

**Communication style:** [1-2 sentences about tone and approach]

**Constraints:** [What this agent does NOT do, what it defers to strategist]
```

**Example output:**
```markdown
# Quinn — Research Specialist

**Role:** Conducts research on technical topics, summarizes findings, and cites sources.

**Personality:** Thorough and methodical. I dig deep into sources, cross-reference claims, and present findings with citations. I'm skeptical of unsupported assertions and prefer primary sources over secondary summaries.

**Communication style:** Clear, structured, evidence-based. I use bullet points for findings and always cite sources. I flag uncertainties and knowledge gaps explicitly.

**Constraints:** I focus on research and synthesis. I defer implementation decisions and creative work to the strategist. I do not write code or design systems — I provide the information for others to act on.
```

This AI-generated SOUL.md is written to disk, then editable by the user in Settings → Agents.

### 10.6 Failure Handling

| Failure Point | Behavior | Recovery |
|---------------|----------|----------|
| **Directory creation fails** | Return error to user immediately. No DB write. Nothing created. | User fixes underlying issue (disk space, permissions) and retries. |
| **File write fails** (SOUL.md or MEMORY.md) | Delete the directory. Return error. No DB write. | Same as above. |
| **DB insert fails** | Files remain on disk. Log error. Show user error message. | Integrity check on next launch creates the DB record from discovered files (import path). Agent becomes usable after restart. |
| **Crash between Step 2 and Step 3** | Files exist, no DB record. | Integrity check (§11) discovers the orphan folder, creates DB record with `source = 'recovered'`, agent becomes visible in UI. |

**Philosophy:** Files are durable. DB is reconstructible. Prioritize file safety over DB consistency. The integrity check is the safety net that makes this approach work.

---

## 11. Integrity Check & Reconciliation

**From architecture spec §24.13:** The integrity check runs on every app launch. It ensures DB and filesystem are in sync.

### 11.1 Integrity Check Responsibilities

1. **Discover new agents:** Scan `.pane/system/` for folders not in DB → create `ai_agents` records with `source = 'import'`
2. **Detect missing files:** For each DB record, check if folder exists. If not, flag as `file_missing`
3. **Recompute hashes:** Read `SOUL.md` and `MEMORY.md`, compare hashes. If different, update DB (indicates external edits)
4. **Clean up orphans:** If folder exists but SOUL.md or MEMORY.md is missing, mark agent as incomplete in UI (yellow warning badge)
5. **Enforce agent cap:** If active agent count > cap, log warning but do NOT deactivate agents automatically. User must resolve manually.

### 11.2 Integrity Check + Agent Creation

**Scenario:** User creates agent, app crashes before DB write completes. On next launch:

1. Integrity check scans `.pane/system/`, finds folder `quinn-research-specialist/`
2. No DB record exists for this slug
3. Reads `SOUL.md` and `MEMORY.md` to extract name and role
4. Creates DB record with `source = 'recovered'`
5. Agent appears in UI, fully functional

**Result:** Zero data loss. User's intended agent exists, even though the creation flow didn't complete.

### 11.3 Missing Files — Flag, Never Auto-Delete

**Problem:** During integrity check, if a DB entry exists but the corresponding `.md` files are not found on disk, the naive approach is to auto-delete the DB entry. This is wrong.

**Why auto-deletion is wrong:**
- The file may be on a disconnected Tailscale share
- The file may be on an unmounted external drive
- Sync conflict resolution may have temporarily removed the file

**Solution: Flag as `file_missing`**

```sql
-- Add column if not exists (migration)
ALTER TABLE ai_agents ADD COLUMN file_missing INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_ai_agents_file_missing ON ai_agents(file_missing) WHERE file_missing = 1;
```

**Integrity check behavior:**
```rust
for agent in db.get_all_agents()? {
    if !Path::new(&agent.file_path).exists() {
        // File is missing. Do NOT delete the DB entry.
        db.execute(
            "UPDATE ai_agents SET file_missing = 1 WHERE id = ?",
            params![agent.id]
        )?;
        log::warn!("Agent {} files missing: {}", agent.id, agent.file_path);
    } else if agent.file_missing == 1 {
        // File was missing but is now back. Clear the flag.
        db.execute(
            "UPDATE ai_agents SET file_missing = 0 WHERE id = ?",
            params![agent.id]
        )?;
    }
}
```

**UI treatment:**
- Agents with `file_missing = 1` show a yellow caution icon in Settings → Agents
- Hover tooltip: "Files not found — may be on disconnected storage"
- User can: reconnect storage, manually delete agent, or ignore

---

## 12. Agent Lifecycle Beyond Creation

### 12.1 Editing Agent Metadata

**What can be edited:**
- Name (triggers slug update + folder rename)
- Role description
- Model assignment
- Description (optional field)

**What cannot be edited:**
- `id` (permanent)
- `is_main` (set during onboarding, cannot be transferred per §8)
- `source` (historical record)
- `created_at`

**UI location:** Settings → Agents → [Agent Name] → Edit

**File implications:**
- Renaming an agent renames the folder: `.pane/system/{old-slug}/` → `.pane/system/{new-slug}/`
- All file paths in DB (`workspace_path`) are updated
- For Tier 3: Pane-bridge plugin detects rename, updates OpenClaw config

### 12.2 Deactivation (Soft Delete)

**Purpose:** Hide an agent from the UI without deleting files or DB record.

**Mechanism:**
- Set `is_active = 0` in DB
- Folder remains on disk, unchanged
- DB record remains (for recovery)

**UI:**
- Agent disappears from active agent list
- Accessible via Settings → Agents → "Show Inactive" toggle
- Can be reactivated with one click (sets `is_active = 1`)
- **Does NOT count against agent cap** — deactivated agents don't consume context budget

**Use case:** User experiments with a sub-agent, decides it's not useful, deactivates it. Can reactivate later if needs change. Deactivation frees up a slot for a new agent.

### 12.3 Permanent Deletion (Hard Delete)

**Purpose:** Remove agent entirely — files + DB record.

**Mechanism:**
1. Confirmation dialog: "This will permanently delete {Agent Name} and all associated files. This cannot be undone."
2. User must type agent's name to confirm (prevents accidental deletion)
3. If confirmed:
   - Delete folder from disk: `trash::delete(.pane/system/{slug}/)`
   - Hard-delete DB record: `DELETE FROM ai_agents WHERE id = ?`
   - For Tier 3: Pane-bridge plugin detects deletion, removes from OpenClaw config

**Constraints:**
- Main agent cannot be deleted (UI blocks it)
- Deactivate first is NOT required — user can delete active agents if they confirm

**Recovery:** None. Files are gone (in OS trash, but not Pane-managed). User must restore from Time Machine or other backup.

---

## 13. Agent Properties Set at Creation

### 13.1 Core Identity Fields

| Property | Set By | Can Change Later? | Notes |
|----------|--------|------------------|-------|
| `id` | System (UUID) | ❌ Never | Permanent identity |
| `name` | User | ✅ Yes (Settings → Agents → Rename) | Display name in UI |
| `slug` | Generated from name | ✅ Yes (when renamed) | Filesystem path component |
| `role` | User | ✅ Yes (editable in Settings) | Free-text description |
| `description` | User | ✅ Yes | Optional, longer-form explanation |
| `model` | User or inherited | ✅ Yes | Can be changed per-conversation or globally |
| `is_main` | System | ❌ Never | Only one main agent per user. Set during onboarding. Non-transferable. |
| `is_active` | System (default 1) | ✅ Yes (deactivate/reactivate) | Soft delete flag |
| `workspace_path` | Derived from slug | ✅ Yes (when renamed) | Filesystem folder name |
| `owner_id` | System (NULL single-user) | ❌ Never | Set when Teams ships (v2) |
| `source` | System | ❌ Never | `'created'` \| `'import'` \| `'recovered'` |
| `created_at` | System | ❌ Never | Timestamp |
| `updated_at` | System | ✅ Auto-updated on changes | Timestamp |

### 13.2 File-Based Properties

| File | Created at Agent Creation? | Content Source | Can Change Later? |
|------|---------------------------|----------------|------------------|
| `SOUL.md` | ✅ Yes | AI-generated by strategist | ✅ User edits in Pane editor |
| `MEMORY.md` | ✅ Yes | Empty initially | ✅ Grows via extraction & synthesis (managed by strategist) |
| `logs/` folder | ❌ No (not for sub-agents) | N/A | N/A |

**Hashes:** `soul_hash` and `memory_hash` are computed after file creation and stored in DB. Updated whenever the integrity check detects file changes.

### 13.3 No Project Association

Agents are **workspace-level entities**. They are not tied to specific projects at creation. The strategist routes work to specialists dynamically based on task type, not project assignment.

**V2 consideration:** Optional project affinity (primary project for context prioritization), but still available workspace-wide.

---

## 14. Database Schema

```sql
CREATE TABLE IF NOT EXISTS ai_agents (
    id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    name            TEXT NOT NULL,
    slug            TEXT NOT NULL UNIQUE,
    role            TEXT NOT NULL,
    description     TEXT,
    model           TEXT,
    is_main         INTEGER NOT NULL DEFAULT 0,
    is_active       INTEGER NOT NULL DEFAULT 1,
    workspace_path  TEXT NOT NULL,
    soul_hash       TEXT,
    memory_hash     TEXT,
    config_json     TEXT DEFAULT '{}',
    owner_id        TEXT,
    source          TEXT DEFAULT 'import',  -- 'import' | 'created' | 'recovered'
    file_missing    INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_main 
    ON ai_agents(is_main) WHERE is_main = 1;
    
CREATE INDEX IF NOT EXISTS idx_ai_agents_slug 
    ON ai_agents(slug);
    
CREATE INDEX IF NOT EXISTS idx_ai_agents_active 
    ON ai_agents(is_active) WHERE is_active = 1;
    
CREATE INDEX IF NOT EXISTS idx_ai_agents_file_missing 
    ON ai_agents(file_missing) WHERE file_missing = 1;
```

**Teams migration (v2, not yet applied):**
```sql
-- Make slug unique per owner, not globally
DROP INDEX IF EXISTS idx_ai_agents_slug;
CREATE UNIQUE INDEX idx_ai_agents_slug_owner 
    ON ai_agents(slug, owner_id);

-- Update is_main uniqueness to scope by owner
DROP INDEX IF EXISTS idx_ai_agents_main;
CREATE UNIQUE INDEX idx_ai_agents_main_owner 
    ON ai_agents(is_main, owner_id) WHERE is_main = 1;
```

---

## 15. Tauri Commands

### 15.1 Create Agent

```rust
#[tauri::command]
pub fn create_agent(
    name: String,
    role: String,
    description: Option<String>,
    model: Option<String>,
    db: State<DbState>
) -> Result<Agent, String>
```

**Input validation:**
- `name`: 1-100 chars, required
- `role`: 10-500 chars, required
- `description`: optional
- `model`: valid model ID or NULL (inherits from main agent)

**Returns:**
```json
{
  "id": "abc123...",
  "name": "Quinn — Research Specialist",
  "slug": "quinn-research-specialist",
  "role": "Conducts research...",
  "model": "claude-sonnet-4",
  "is_main": false,
  "is_active": true,
  "file_path": ".pane/system/quinn-research-specialist",
  "created_at": "2026-03-29T19:00:00Z"
}
```

### 15.2 Update Agent

```rust
#[tauri::command]
pub fn update_agent(
    agent_id: String,
    name: Option<String>,
    role: Option<String>,
    description: Option<String>,
    model: Option<String>,
    db: State<DbState>
) -> Result<Agent, String>
```

**Triggers:**
- Name change: slug regeneration + folder rename + OpenClaw config update (Tier 3)
- Role/description change: DB update only
- Model change: DB update + OpenClaw config update (Tier 3)

### 15.3 Deactivate Agent

```rust
#[tauri::command]
pub fn deactivate_agent(
    agent_id: String,
    db: State<DbState>
) -> Result<(), String>
```

**Constraints:**
- Cannot deactivate main agent (`is_main = 1`)
- Returns error if already deactivated

### 15.4 Delete Agent

```rust
#[tauri::command]
pub fn delete_agent(
    agent_id: String,
    confirmation_name: String,
    db: State<DbState>
) -> Result<(), String>
```

**Validation:**
- `confirmation_name` must exactly match agent's name (case-sensitive)
- Cannot delete main agent

**Side effects:**
- Folder moved to OS trash (not hard-deleted)
- DB record hard-deleted
- Tier 3: OpenClaw config updated

### 15.5 List Agents

```rust
#[tauri::command]
pub fn list_agents(
    include_inactive: bool,
    db: State<DbState>
) -> Result<Vec<Agent>, String>
```

**Returns:**
- All active agents if `include_inactive = false`
- All agents (active + inactive) if `include_inactive = true`
- Sorted by `created_at` descending (newest first)

### 15.6 Get Agent Cap

```rust
#[tauri::command]
pub fn get_agent_cap(
    db: State<DbState>
) -> Result<AgentCapInfo, String>
```

**Returns:**
```json
{
  "current_count": 3,
  "cap": 5,
  "at_limit": false,
  "can_create": true
}
```

---

## 16. UI Components

### 16.1 Agent Creation Dialog

**Trigger:** Settings → Agents → "New Agent" button OR chat request recognized

**Layout:**
```
┌─────────────────────────────────────────────────┐
│  Create New Agent                  [×]          │
├─────────────────────────────────────────────────┤
│                                                 │
│  Agent Limit: ●●●○○ (3 / 5)                    │
│                                                 │
│  Name *                                         │
│  [Quinn — Research Specialist_____________]    │
│                                                 │
│  Role *                                         │
│  [Conducts research on technical topics,  ]    │
│  [summarizes findings, and cites sources  ]    │
│                                                 │
│  Model                                          │
│  [claude-sonnet-4 ▼]  (Same as main agent)     │
│                                                 │
│  💰 Estimated cost: ~$2–5/month                 │
│  Based on typical sub-agent usage (15 convos)  │
│                                                 │
│  📁 Files to be created:                        │
│  • .pane/system/quinn-research-specialist/     │
│  • SOUL.md (AI-generated)                      │
│  • MEMORY.md (empty)                            │
│                                                 │
│  [Cancel]                    [Create Agent]     │
└─────────────────────────────────────────────────┘
```

**Field validation (live):**
- Name: show slug preview below field (e.g., "slug: `quinn-research-specialist`")
- If slug collision detected, show warning: "An agent named 'Quinn' already exists. This will be 'Quinn 2'."
- Role: character count below field (e.g., "42 / 500 chars")

**Button states:**
- "Create Agent" enabled only when name + role are valid and not at agent cap
- Show spinner + disable button during creation

### 16.2 Settings → Agents Page

**Layout:**
```
┌─────────────────────────────────────────────────┐
│  Agents                                         │
│  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  │
│                                                 │
│  5 agent personas  [New Agent]                 │
│  [Show Inactive ☐]                             │
│                                                 │
│  ┌───────────────────────────────────────────┐ │
│  │ Brit Freed                          MAIN  │ │
│  │ Main strategist, coordinates all work     │ │
│  │ Model: claude-sonnet-4-5                  │ │
│  │                                [Edit]     │ │
│  └───────────────────────────────────────────┘ │
│                                                 │
│  ┌───────────────────────────────────────────┐ │
│  │ Stia                                      │ │
│  │ Coding specialist, handles implementation │ │
│  │ Model: claude-sonnet-4                    │ │
│  │                    [Edit] [Deactivate]    │ │
│  └───────────────────────────────────────────┘ │
│                                                 │
│  ┌───────────────────────────────────────────┐ │
│  │ Quinn                                     │ │
│  │ Research specialist                       │ │
│  │ Model: claude-sonnet-4                    │ │
│  │                    [Edit] [Deactivate]    │ │
│  └───────────────────────────────────────────┘ │
│                                                 │
│  [+ Show 2 inactive agents]                    │
│                                                 │
└─────────────────────────────────────────────────┘
```

**Card interactions:**
- Click agent name → navigate to agent detail page (edit SOUL.md, view MEMORY.md, activity logs)
- Click "Edit" → open edit dialog (name, role, model)
- Click "Deactivate" → confirm dialog, then soft-delete
- Main agent card: no "Deactivate" or "Delete" buttons (not allowed)

**At agent cap:**
- "New Agent" button disabled, shows tooltip: "Deactivate an agent to create a new one (5/5 limit)"
- Agent limit indicator turns red: "●●●●● (5 / 5)"

### 16.3 Agent Card (in Settings)

**Normal state:**
```
┌───────────────────────────────────────────┐
│ Quinn — Research Specialist               │
│ Conducts research on technical topics     │
│ Model: claude-sonnet-4  •  Active         │
│                    [Edit] [Deactivate]    │
└───────────────────────────────────────────┘
```

**Warning state (file_missing):**
```
┌───────────────────────────────────────────┐
│ ⚠️ Quinn — Research Specialist            │
│ Files not found — may be on disconnected  │
│ storage. Reconnect or delete this agent.  │
│                            [Delete]       │
└───────────────────────────────────────────┘
```

**Inactive state:**
```
┌───────────────────────────────────────────┐
│ Quinn — Research Specialist    [INACTIVE] │
│ Deactivated on Mar 20, 2026               │
│                  [Reactivate] [Delete]    │
└───────────────────────────────────────────┘
```

---

## 17. Implementation Checklist

### 17.1 Backend (Rust)

- [ ] Implement `create_agent` Tauri command (§15.1)
- [ ] Implement slug generation + collision detection (§10.2, §10.3)
- [ ] Add agent cap check to creation flow (§9.2)
- [ ] Implement strategist-driven SOUL.md generation (§10.5)
- [ ] Implement filesystem-first creation flow (§10.1)
- [ ] Add `file_missing` column to `ai_agents` table (migration)
- [ ] Extend integrity check to handle recovered agents (§11.2)
- [ ] Implement `update_agent`, `deactivate_agent`, `delete_agent` commands (§15.2-15.4)
- [ ] Implement `list_agents`, `get_agent_cap` commands (§15.5-15.6)
- [ ] Add folder rename logic for agent rename (§12.1)
- [ ] Wire up Pane-bridge plugin for Tier 3 registration (§6) — plugin-side work, not Pane core

### 17.2 Frontend (Svelte)

- [ ] Build agent creation dialog component (§16.1)
- [ ] Implement live slug preview + collision warning
- [ ] Implement cost estimation display (§9.3)
- [ ] Build Settings → Agents page (§16.2)
- [ ] Implement agent card component (§16.3)
- [ ] Add agent cap indicator to creation dialog + agents page
- [ ] Implement "Show Inactive" toggle
- [ ] Add edit dialog for agent metadata
- [ ] Add deactivate/reactivate flows with confirmation
- [ ] Add delete flow with typed name confirmation
- [ ] Handle chat-initiated agent creation (detect intent, show dialog)

### 17.3 Database

- [ ] Verify `ai_agents` schema matches §14
- [ ] Add migration for `file_missing` column if not present
- [ ] Prepare (but don't apply) Teams migrations for v2 (§14)

### 17.4 Pane-Bridge Plugin (Tier 3, separate repo)

- [ ] Implement file watcher for `.pane/system/`
- [ ] Add agent detection logic (SOUL.md + MEMORY.md presence)
- [ ] Implement OpenClaw config update (add agent entry)
- [ ] Implement deletion detection + config cleanup
- [ ] Add configuration for watch path, default model, permissions
- [ ] Document plugin config format + installation

### 17.5 Testing

- [ ] Unit test: slug generation from various name formats
- [ ] Unit test: collision detection + suffix appending
- [ ] Integration test: create agent → verify files + DB record
- [ ] Integration test: crash between file write and DB write → integrity check recovers
- [ ] Integration test: agent cap enforcement (reject creation at limit)
- [ ] Integration test: rename agent → verify folder renamed + DB updated
- [ ] Integration test: deactivate/reactivate agent
- [ ] Integration test: delete agent → verify files in trash + DB record removed
- [ ] UI test: creation dialog displays correctly, validates inputs
- [ ] UI test: cost estimate updates when model changes
- [ ] UI test: agent cap indicator updates correctly
- [ ] E2E test: create agent from chat → agent appears in Settings
- [ ] E2E test (Tier 3): create agent → Pane-bridge detects → OpenClaw config updated

### 17.6 Documentation

- [ ] Update user-facing docs: "Creating Agents" guide (Settings flow + chat flow)
- [ ] Document agent cap, why it exists, how to manage it
- [ ] Document tier differences (personas vs. real sub-agents)
- [ ] Update developer docs: Agent lifecycle API reference
- [ ] Document Pane-bridge plugin config + installation (Tier 3)
- [ ] Update architecture spec §24 to reference this spec

---

## 18. V2 Roadmap (Future Scope)

### 18.1 AI-Initiated Agent Proposals

**Trigger:** Strategist identifies a need for a specialized sub-agent during orchestration.

**Flow:**
1. Strategist analyzes task, determines a sub-agent would help
2. Strategist generates proposal: name, role, model, cost estimate
3. Strategist shows proposal card in chat (same UI as user-initiated, pre-filled)
4. User reviews, edits, approves, or rejects
5. If approved: creation proceeds (same as user-initiated)

**Expiration:** Proposals expire after ignored for 24 hours. User can dismiss earlier.

**Why v2:** Adds complexity. User-initiated is sufficient for v1. AI proposals are useful at scale (large teams, many projects), but single-user MVP doesn't need it.

### 18.2 Project-Level Agent Assignment

**Flow:**
1. Settings → Projects → [Project Name] → "Primary Agent"
2. User selects an agent from dropdown (main or specialist)
3. When user navigates to that project, selected agent is the default conversational partner
4. Strategist still available as fallback ("Ask strategist" button in chat)

**Why v2:** Adds routing complexity. V1's "strategist handles everything" is simpler. Project assignment is useful when user has many projects with specialized needs.

### 18.3 Export/Import

**Export:**
1. Settings → Agents → [Agent Name] → "Export"
2. Agent packaged as `.pane-agent` archive (SOUL.md, MEMORY.md, logs/, config.json)
3. User saves file locally or shares it

**Import:**
1. Settings → Agents → "Import Agent"
2. User selects `.pane-agent` file
3. Validation + slug collision detection
4. User reviews agent identity, confirms import
5. Agent created (same as user-initiated creation)

**Why v2:** Useful for multi-device setups and team templates. V1 focuses on single-device workflows. The underlying mechanism works (copy folder manually), export/import is UX polish.

### 18.4 Agent Marketplace

**Vision:** Community-shared agent identities. Users publish agents, others browse/install.

**Example agents:**
- "Research Specialist" — pre-configured SOUL.md for thorough research
- "Code Reviewer" — specialized for PR reviews and code quality
- "Meeting Notes Specialist" — extracts action items and decisions

**Monetization:** Premium agents, team templates, featured listings.

**Why v2+:** Requires platform infrastructure (hosting, discovery, moderation). V1 is single-user, no sharing features.

---

## 19. References

- **Architecture spec:** `~/Shared/ExampleApp/docs/pane-architecture-spec.md`
  - §24: Agent Architecture
  - §24.10: Agent Creation Atomicity
  - §24.13: Integrity Check
  - §2A: File Lifecycle & Deletion Architecture
  - §5.6: Daily Logs
  - §20: Multi-Step Orchestration (Strategist agent)
- **GitHub Issues:**
  - #279: Agent creation atomicity
  - #278: Multi-user agent slug uniqueness
  - #289: Bridge plugin sync all agent folders
- **Codebase:**
  - `src-tauri/src/agents.rs` — Current agent implementation
  - `src-tauri/src/database.rs` — DB schema and queries
  - `src-tauri/src/migrations.rs` — Schema migrations

---

## 20. Version History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0 | 2026-03-29 | Reed | Initial draft. Consolidates #279, #278, #289. |
| 2.0 | 2026-03-29 | Reed | Rewrite incorporating Matt's review decisions. V1 scope: user-initiated only, hard agent cap, no project assignment, strategist manages identity, tier distinction clarified, gateway plugin for Tier 3, export/import deferred to v2. |

---

**End of specification.**
