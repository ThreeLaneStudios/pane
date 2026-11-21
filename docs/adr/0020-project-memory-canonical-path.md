# ADR-0020: Project Memory Lives at Projects/{name}/Memory.md

*Line citations predate the #3097 module split.*

**Status:** Proposed
**Date:** 2026-04-30
**Author:** Matt Carano

## Context

The §11 memory pipeline audit (2026-04-30) found three different paths for project memory in three places:

- `docs/spec/memory-pipeline.md` §5.2 layer diagram: `.pane/system/{agent}/memory/projects/{project-id}.md`
- `docs/spec/memory-pipeline.md` §5.2 prose: `Projects/{name}/AI/Memory.md`
- `src-tauri/src/ai/memory_router.rs:101` and `ai/commands.rs:6354`: `Projects/{name}/Memory.md`

These are not three competing options. They are three eras of the same path, layered on top of each other in the same spec section without the older forms being removed:

- **Pre-Mar 16:** `.pane/system/{agent}/memory/projects/{project-id}.md` (agent-keyed, project_id, hidden)
- **Mar 16 – Apr 16:** `Projects/{name}/AI/Memory.md` (project-keyed, AI subfolder, user-visible). CHANGELOG entry: "Per-project AI folder: auto-created with project, Memory note auto-created. Replaces memory/projects/ under agent folder."
- **Apr 16 onward:** `Projects/{name}/Memory.md` (project-keyed, project root, user-visible). Migration #470 (v0.37.0) moved files out of the AI subfolder; the AI folder concept was retired.

Existing user data — Tea, Gum, Beach — lives at the Apr 16 path. The router code targets the Apr 16 path. Audit #476 (file `Projects/{project_name}/Memory.md`, project root) confirms it. The two older spec forms are unmerged historical layers, not deliberate alternatives.

The audit also found the router's project-memory write path has been broken since first commit (separate finding #1, mechanical fix). When that fix lands, it lands at whatever path this ADR names.

A spec edit alone could resolve the contradiction by deleting the two older forms. An ADR is filed instead because two architectural facts underneath the path question have never been recorded:

- **Visibility posture asymmetry.** Agent memory is hidden in `.pane/system/`. Project memory is visible at `Projects/{name}/`. The asymmetry is intentional but only inferable from §5.2's "Project members can see" line.
- **Project rename behavior.** Name-based paths mean project rename moves the Memory.md file with the directory. This is the existing pattern for any project file, but is undocumented for system notes.

ADR-0010 (Three-Writer Memory Pipeline) and ADR-0011 (System Notes Read-Only) interact with this path but do not constrain it. ADR-0011's read-only guard is keyed on `note_type = 'memory'`, not on path.

## Decision

Project Memory notes live at `Projects/{name}/Memory.md` — at the project's root directory in the user-visible Projects/ tree.

- **Path is name-based.** When a project is renamed, the project directory rename moves the Memory.md file with it; the file watcher updates the DB row's `file_path` via the existing rename-detection path used for any project file.
- **Visibility is intentional.** Project Memory is visible to the user under their project, consistent with §5.2's framing that project memory is project-scoped and (post-launch) team-visible. This is asymmetric with agent MEMORY.md, which is hidden in `.pane/system/{agent}/MEMORY.md`. The asymmetry is correct: agent memory is always personal; project memory is always project-scoped.
- **`note_type = 'memory'` is the authoritative identifier**, not the path or title. ADR-0011's read-only guard, the router's destination resolution, and Memory Settings inline edit/delete all key on `note_type`. Path is for user visibility and filesystem location only.
- **Lazy creation continues.** Memory.md is created on first synthesis touching the project (existing behavior). Issue #800 will move creation to project-creation-time at this same path; the path decision does not change.

## Consequences

### Positive

- **Code, files, and spec agree.** The two stale spec forms get removed; the active form matches what's already on disk and in the codebase.
- **No file migration.** Existing user files (Tea, Gum, Beach) are already at the canonical path. Zero data movement, zero risk of file-move bugs.
- **Project memory is discoverable.** Users see Memory.md alongside their other project files. This matches the "shared workspace" framing — Pane is not a notes app with AI bolted on, the AI's project memory is a first-class project artifact.
- **#796 router fix has a clear destination.** The mechanical fix lands at this path with no design ambiguity.
- **#800 auto-creation has a clear destination.** Same path. No additional design needed.

### Negative

- **Asymmetry to explain.** Agent memory hidden, project memory visible. New contributors will ask why; the answer must be in the spec and in the ADR. Pre-empted by this ADR's "visibility posture asymmetry" framing.
- **Name-based path is fragile to rename bugs.** If the watcher's project-rename handling has a bug, the DB's `file_path` can drift from disk. Existing pattern for any project file, not unique to Memory.md, but the consequence for Memory.md is "router can't find the file" — which manifests as the same #719-class symptom users have already seen. Mitigated by rename watcher verification (see Risks).
- **`Projects/{name}/Memory.md` collides with a user-named note titled "Memory."** Today, system note identification uses `note_type='memory'` not title, so the DB-level guard holds — but two files with the same name in the same directory is confusing. Mitigated by `is_system` / `note_type` enforcement in the create-note path, which already blocks user creation of system-typed notes.

### Risks

- **Project rename → file watcher → DB update chain has not been re-verified for system notes specifically.** The pattern works for user notes; system notes share the same code path but their `note_type` flag could trigger guards that block the rename. Verification task spawned; results will be filed before #796 router fix ships.
- **Future "share project with team" feature lands on a project tree that already contains AI-managed files.** Project Memory becoming visible to other team members is the intended behavior per §5.2, but the access-control story for Memory.md within a shared project is post-launch work and not addressed here.

## Alternatives Considered

**Hidden under the agent.** Move project memory to `.pane/system/{agent}/memory/projects/{project-id}.md` to match the agent MEMORY.md pattern. Rejected: contradicts §5.2's stated visibility intent for work memory, hides project artifacts from users in their own project tree, and would require migrating Matt's existing files (Tea, Gum, Beach) out of his Projects/ tree without a corresponding product gain. The agent pattern is correct for *agent* memory because agent memory is personal; project memory is project-scoped and the visibility model is different on purpose.

**Restore the AI subfolder.** Use `Projects/{name}/AI/Memory.md` to group AI artifacts inside each project. Rejected: this was the path between Mar 16 and Apr 16, retired by #470 with its own audit, migration, and changelog entry. Reverting that decision now needs its own architectural justification, which doesn't exist — the AI folder was retired because it was opinionated and added a hierarchy users didn't ask for.

**Use project_id in the filename.** `Projects/{name}/Memory-{project-id}.md` to be rename-stable without going hidden. Rejected: solves a problem (rename robustness) that the existing watcher pattern already solves for every other file in a project. Adding a UUID suffix to one specific file in the project tree creates a worse user experience for a marginal robustness gain.

**Skip the ADR, just edit the spec.** Resolve the contradiction by deleting the two older spec forms and stopping there. Rejected: leaves the visibility-posture asymmetry undocumented, leaves project rename behavior undocumented, and gives the next person who finds the spec/code mismatch no decision trail to read.

## What if we didn't decide

- **The router fix for finding #1 ships against ambiguous spec.** Stia picks one of three paths to make the test pass, the spec stays out of date, the next audit finds the same contradiction.
- **#800 auto-creation lands at whatever path Stia chooses for #796 router fix.** Two PRs, two implementation choices, no spec edit, no ADR — exactly the drift pattern the dev-hygiene playbook is supposed to prevent.
- **The visibility-posture asymmetry stays implicit.** The next person to look at the diagram and ask "why is agent memory hidden but project memory visible?" gets no answer beyond "that's how the code works."

## References

- Issue #796 — router project-memory write path broken since first commit (audit finding #1, blocked on this decision)
- Issue #800 — project Memory not auto-created at project creation (blocked on this decision)
- Issue #719 — sync failed for Memory.md and logs (related symptom; may resolve as side effect of router fix)
- Audit `docs/audits/2026-04-30-memory-pipeline-audit.md` — finding #5 (path mismatch), finding #1 (router broken)
- Audit `docs/audits/470-post-build-review.md` — confirms `Projects/{name}/Memory.md` as the post-#470 canonical path
- Audit `docs/audits/476-memory-in-settings-audit.md` — Memory note has no visible UI home post-#470
- ADR-0010 — Three-writer memory pipeline (defines writers; this ADR sits one level below)
- ADR-0011 — System notes read-only (path-independent; protection via `note_type='memory'`)
- ADR-0019 — Pre-launch schema management clarification (precedent for clarification-type ADRs)
- `docs/spec/memory-pipeline.md` §5.2, §5.3A, §5.5 — sections this ADR clarifies
- `src-tauri/src/ai/memory_router.rs:101` — code path
