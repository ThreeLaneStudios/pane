# Spooner Task Prompt Template (Grok-Optimized)

## Task: {brief_task_description}

**Issue:** #{issue_number}

### Think Step-by-Step
Before writing any code, write down your step-by-step plan in a scratchpad to ensure you understand the request fully.

### Full Context
Below is the full context of our project's architecture and onboarding process. Read it carefully before proceeding.

**Architecture Cheat Sheet:**
```
{architecture_cheat_sheet_content}
```

**Agent Onboarding Guide:**
```
{pane_onboarding_for_agents_content}
```

### Your Specific Task
{detailed_task_instructions}

### Process
*   Follow your SOUL.md build standards precisely.
*   `cargo check` and `cargo test` must pass.
*   Commit with a clear message referencing the issue number.
*   Do NOT build the DMG unless explicitly told to do so.

### Completion
*   Append a summary of your work to `~/.openclaw/workspace/memory/2026-04-15.md`.