# Scoping Audit Template

Checklist for Reed scoping audits on UI-facing defects.

## Required Sections

1. **Consequence per site** — trace to actual DB row and UI visibility
2. **Available context per site** — enumerate what each call site knows at creation time
3. **Root cause** — never wired, or deliberately deferred
4. **Reuse ENUMERATED, file:line** — existing patterns and whether each site can adopt one
5. **Spec ruling** — does existing spec cover, or is a new clause needed
6. **Blast radius** — other paths not in the audit, downstream consumers

## Process Rules

- **Reuse references must be enumerated:** Any "reuse/match X" claim must list exactly what of X is adopted, with file:line refs and walkthrough. Bare name references = FAIL.
- **UI reachability gate (added 2026-08-04, ref: #2413 TASKS-panel miss):** For UI-triggered defects, the scope must confirm the trigger surface is reachable — rendered, and its gate conditions satisfiable by a user — before assigning severity. A stubbed data path or a catch-22 gate (toggle button inside its own conditional block) means the surface is dead UI, not a live defect. Severity is "dead code cleanup," not the consequence the code *would* produce if reachable.
