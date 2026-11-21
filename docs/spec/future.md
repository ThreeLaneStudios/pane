# Future Considerations

**Last Updated:** 2026-04-19  
**Source:** Extracted from pane-architecture-spec.md  
**Sections:** §23

---

## 23. Future Considerations
> **Status:** N/A (roadmap)

### 23.1 Custom Panes

> **Status:** N/A (roadmap)

The north star: AI decides what UI to show based on intent. User says "plan my week" and the AI renders a week-planning interface — not just chat text. Requires:
- Component registry (Svelte components the AI can invoke)
- Structured output from LLM → UI component mapping
- Sandboxed rendering area in the chat panel

### 23.2 Voice Input/Output

> **Status:** N/A (roadmap)

- Speech-to-text for note dictation and AI chat
- Text-to-speech for AI responses (accessibility + hands-free)
- Tauri native audio APIs + Whisper (local) or cloud STT

### 23.3 Multi-Model Support

> **Status:** N/A (roadmap)

- Different models for different tasks: fast model for memory extraction, capable model for generation
- Per-project model configuration
- Cost tracking per model per project
- `ModelProvider` trait in Rust for abstraction

#

### 23.4 Provider-Native Memory
> **Status:** N/A (roadmap)

LLM providers (Anthropic, OpenAI) are adding built-in memory features. When these mature:

- **Option A: Ignore provider memory.** Pane's memory is more transparent, portable, and user-controlled. Provider memory is opaque.
- **Option B: Sync provider memory.** Two-way sync between Pane memory and provider memory. Complex, potential conflicts.
- **Option C: Let user choose.** Settings toggle: "Use Pane memory" vs "Use provider memory" vs "Both."

**Recommendation:** Option A for MVP. Pane memory is a feature, not a workaround. Provider memory is a convenience for users who don't have Pane.

### 23.5 Context Window Scaling
> **Status:** N/A (roadmap)

As context windows grow (1M+ tokens already available on some models):

- **Memory extraction becomes optional.** With infinite context, you could just load everything.
- **But memory is still valuable for:**
  - Prioritization (what's important vs what's just there)
  - Cross-session continuity (context windows reset per conversation)
  - Team sharing (memory can be shared, raw context can't)
  - Cost efficiency (smaller context = cheaper tokens)

**Architecture is ready:** Context budgets in §3 are configurable. As windows grow, we raise limits. Memory extraction continues for the values above.

### 23.6 Agent Economy Integration
> **Status:** N/A (roadmap)

When crypto-based agent economies mature (per Brian Roemmele's vision):

- Persistent sub-agents (§16.4) have reputation based on work quality
- Agents can be "hired" across users based on track record
- Pane becomes a platform where users can rent specialized agents
- Payment via JouleWork or similar agent-native currency

This requires: persistent agent identity, verifiable work history, reputation scoring, economic layer. Foundation is in §16.4.

## 23.4 Extension Marketplace

> **Status:** N/A (roadmap)

- In-app discovery of community extensions
- Security review process
- Auto-updates
- Revenue sharing for extension developers

### 23.7 Mobile Adaptation

> **Status:** N/A (roadmap)

- Tauri 2.0 Mobile (iOS first, then Android)
- Quick capture → Today view → Browse → Search
- Shared Rust backend, adapted Svelte UI
- Mobile-specific: share sheet integration, widgets, push notifications

### 23.8 AI Agents (Autonomous)

> **Status:** N/A (roadmap)

- Long-running AI tasks: "Organize this project overnight"
- Agent runs asynchronously, logs actions to audit log
- User reviews results in the morning
- Requires strong permission controls and undo capability

---


# future.md addition — Repo Workspace (parked, post-§36)

> **Status:** PARKED — outline only. Decided in principle 2026-10-25 (Matt). Needs its own spec and ADR after the §36 runtime sprint ships. Depends on §36 spawn, wake-on-complete, job sessions and the tool-policy struct.

**Goal.** Let Pane agents do code work the way Matt's OC agents do today, without giving them a shell: a connected repo, an isolated place to work, and human consent at the points that matter.

**Shape (eight decisions to confirm at spec time):**

1. **Explicit connection, per project.** User connects one repo (local path or GitHub URL) to a Pane project; Pane clones to `.pane/workspaces/{project}/`. Nothing connected by default; the agent never sees `~`.
2. **Worktree per job.** Branch `pane/{job-id}`; parallel jobs can't collide; cleanup is `git worktree remove`.
3. **Isolated runner, one interface, one backend per platform.** "Give me an isolated process with this worktree mounted, these commands allowed, this network allowlist." macOS: Apple `container` (macOS 26) or `sandbox-exec`; Linux: `bwrap`; Windows: Docker Desktop/WSL2 required. No backend available → feature off with a clear message; **never** a fallback to the raw shell.
4. **Network allowlist.** Package registries + github.com; anything else is a consent prompt.
5. **Commands by allowlist.** `git` (read, commit on its branch), the project's declared test/build/lint commands from a `pane.toml`, language runtimes. No `curl`, no `sudo`.
6. **Two hard consent gates** (withholding, as §36.5.1): **push**, and **anything leaving the sandbox** (open PR, post comment). Commit is free; a human clicks push.
7. **GitHub via API** with a fine-grained token scoped to the connected repo, keychain-stored like the Venice key. Reads free; writes through gate 6.
8. **Command output is external content** — `ExternalSource::CommandOutput`, delimited and tainted like web and attachments.

**Sequencing.** Two sprints: (A) connection + worktree + GitHub API tools + push gate, commands limited to `git`; (B) the isolated runner for tests and builds, per platform.

**What it never gives:** arbitrary shell, access outside the workspace, secrets from keychain or env.
