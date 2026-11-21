# AI Team System — §25

> **Status:** NOT BUILT (spec'd 2026-03-09 by Matt/Brit decision). Originally outlined under §19; section number reassigned to §25 on 2026-05-02 after §19 was claimed by the notification spec it had always been the canonical home for per §0 Philosophy. Body content unchanged from original Mar 9 spec.

> **Section history:** This content was authored as the §19 spec on 2026-03-09. Between 2026-03-17 and 2026-05-02, README.md's TOC and the §0 Philosophy referenced §19 as "Notification & Reminder System" while this content sat in a section also numbered §19, producing the collision documented in ADR-0021 / ADR-0022. The collision was resolved by acknowledging that §19 had been notifications since the §0 Philosophy was written, and re-homing this content at §25.

## §25.1 Vision

> **Status:** NOT BUILT (spec'd Mar 9)

Users can create, manage, and assign AI teammates through conversation. Teams are visible on the Home screen — not hidden config, but named agents with identity, personality, and presence. The AI helps users BUILD their team through natural conversation, not configuration files.

## §25.2 Team Member Structure

> **Status:** NOT BUILT (spec'd Mar 9)

Each teammate is a named AI agent with:

| Component | Storage | Purpose |
|-----------|---------|---------|
| Name | `ai_agents` table | Display name (e.g., "Scout", "Stia") |
| Role | `ai_agents` table | One-line description (e.g., "Research specialist") |
| Avatar | `ai_agents` table | Generated or user-selected |
| Soul note | Agent's named folder in Pane System project (e.g., "Brit Freed/", "Stia/") | Personality, approach, communication style |
| Memory note | Agent's named folder in Pane System project | Domain knowledge, accumulated context |
| Model | `ai_agents` table | Which LLM powers this agent (can differ per agent) |
| Status | `ai_agents` table | active, idle, working |

```sql
CREATE TABLE ai_agents (
    id          TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    name        TEXT NOT NULL,
    role        TEXT,
    avatar_path TEXT,
    model       TEXT,           -- e.g., 'anthropic/claude-sonnet-4' or 'openclaw:coder'
    soul_note_id    TEXT,       -- FK to notes table
    memory_note_id  TEXT,       -- FK to notes table
    status      TEXT DEFAULT 'idle',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE project_agents (
    project_id  TEXT NOT NULL,
    agent_id    TEXT NOT NULL,
    added_at    TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (project_id, agent_id)
);
```

> **Schema cross-reference:** ADR-0021 adds an `is_coordinator INTEGER NOT NULL DEFAULT 0` column to this table. The coordinator-only check on the `emit_notification` AI tool resolves against that flag. See `docs/spec/notifications.md` §19.5.3 and §19.9.

## §25.3 Team Creation (Conversational)

> **Status:** NOT BUILT (spec'd Mar 9)

Users create teammates through chat on the Home screen:

**User:** "I need someone to help with research"
**Pane:** "What kind of research? Technical, market, competitive, academic?"
**User:** "Competitive analysis for SaaS products"
**Pane:** "How about 'Scout' — a competitive analysis specialist who's thorough, data-driven, and presents findings with clear comparisons? I can set them up with that personality. Want to adjust anything?"
**User:** "Perfect"
**Pane:** *Creates Scout: Soul note with personality, empty Memory note, assigns default model, shows card on Home*

The AI suggests: name, role description, personality traits, and model recommendation. User confirms or adjusts.

## §25.4 Home Screen — Team Section

> **Status:** NOT BUILT (spec'd Mar 9)

- Team cards displayed on Home dashboard (horizontal scrollable row or grid)
- Each card shows: avatar, name, role, status indicator (green=active, gray=idle, teal=working)
- Click a card → opens that agent's Soul note for editing
- "Add Teammate" button → starts conversational wizard
- "Manage Team" → full list with edit/delete/model assignment

## §25.5 Project Assignment

> **Status:** NOT BUILT (spec'd Mar 9)

- In a project's settings or via chat: "Add Scout to this project"
- Assigned agents appear in the project's sidebar or header
- When an assigned agent responds in a project chat, messages show their name (not generic "Pane")
- Multiple agents can be assigned to one project
- The user or primary AI can delegate to a teammate: "Scout, research the top 5 competitors in this space"

## §25.6 Context Loading Per Agent

> **Status:** NOT BUILT (spec'd Mar 9)

When an agent responds, the system prompt includes:
1. That agent's Soul note (personality)
2. That agent's Memory note (domain knowledge)
3. Project-level Memory + Soul + Rules (if in a project)
4. Workspace-level Memory (always)

Injection order: Agent Soul → Project Soul → Workspace Soul → Rules → Agent Memory → Project Memory → Workspace Memory → Context

## §25.7 OpenClaw Integration

> **Status:** NOT BUILT (spec'd Mar 9)

For OpenClaw users, teammates can map to OpenClaw agents:
- `model: "openclaw:coder"` routes to the Stia agent
- `model: "openclaw:researcher"` routes to Reed
- Full agent capabilities (exec, browser, tools) come through the OpenClaw pipeline
- Non-OpenClaw users: agents are powered by direct API calls with different Soul/Memory/model per agent

## §25.8 Pricing Tier

> **Status:** NOT BUILT (spec'd Mar 9)

- **Free/Pro:** 1 AI agent (the default "Pane" assistant)
- **AI tier ($25):** Up to 3 custom agents
- **Team tier ($12/seat):** Unlimited agents + shared team agents across seats

## §25.9 Future Extensions

> **Status:** NOT BUILT (spec'd Mar 9)

- Agent-to-agent communication (one agent delegates to another)
- Agent activity feed (what each agent has been doing)
- Agent performance metrics (tasks completed, quality)
- Marketplace for pre-built agent templates ("Install the Marketing Team: Copywriter + Designer + Analyst")
- Agent training: user feedback improves agent behavior over time
