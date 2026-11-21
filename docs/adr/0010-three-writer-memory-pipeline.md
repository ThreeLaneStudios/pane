# ADR-0010: Three-Writer Memory Pipeline with ai_memory as Source of Truth

**Status:** Accepted
**Date:** 2026-04-23
**Author:** Matt Carano, Claude (session)

## Context

Memory synthesis (§5.5) today fuses two operations into one step: routing new facts to destination files AND rewriting those files into clean prose. Every routing event pays the rewrite cost. This is the root of #646 — a 90-second timeout when MEMORY.md grows to ~1400 words and every synthesis pass has to regenerate the whole file.

Memory files also grow without a mechanical ceiling. Any `fact` or `decision` without a project context routes to agent MEMORY.md, so agent memory accumulates a landfill of general knowledge alongside the identity/preference/style content that actually belongs there. This matters concretely: OpenClaw caps bootstrap files at 12K chars, and Matt recently trimmed his Brit agent's MEMORY.md from 20K+ back under the cap.

Separately, user edit/delete semantics are dishonest. Memory Settings lets a user edit or delete a fact, but that same fact also lives as a bullet in a memory file (post-router) and eventually as a sentence in a prose paragraph (post-consolidation). Deleting only the DB row leaves the fact in the files. The "forget this" button is a lie.

## Decision

Split memory writes into three distinct writers, each owning a specific target with a specific trigger. Make `ai_memory` the source of truth for memory content; memory files become rendered views.

1. **Router** — no LLM. Appends fact bullets to memory files (and removes bullets on delete). Deterministic routing by category and channel. Triggered when N unrouted facts accumulate. Mutex-protected.
2. **End-of-day agent** — LLM. Deduplicates yesterday's daily log and prepends a 3–6 sentence narrative header. Triggered by the first message of a new day. Daily log only; never touches memory files.
3. **Consolidator** — LLM, rare. Reads the existing memory file for prose style and the active `ai_memory` fact list for content, then rewrites the file. Triggered by word-count threshold plus a staleness backstop. Memory files only; never touches daily logs.

Consolidator reads from `ai_memory` filtered by `is_active = 1`. This means deletes propagate cleanly at the next consolidation, edits propagate at the next consolidation, and `ai_memory` becomes authoritative.

Only identity, preference, style, and goal route to agent MEMORY.md. Everything else either routes to project Memory (fact/decision/goal in a project channel) or to the daily log only (deadline, reflection, general fact/decision from the Home channel). Agent MEMORY.md is now bounded by category, not just by consolidation.

## Consequences

### Positive

- **Resolves #646.** Routing is a mutex-protected append, measured in milliseconds. Only the rare consolidator pays the rewrite cost, and it runs on word-count threshold plus 14-day staleness instead of every 5 facts.
- **Agent MEMORY.md is bounded by category.** Identity/preference/style/goal only. Stays well under OpenClaw's 12K cap by construction, not by manual trimming.
- **Delete semantics become honest.** `is_active = 0` removes the fact from consolidation input; router removes the bullet from the memory file immediately. "Forget this" means forgotten now.
- **One writer per file invariant preserved.** Each writer owns a specific target. No overlaps.
- **Daily log becomes the audit trail.** Memory edits and deletes append marker entries (`[memory edited]`, `[memory deleted]`). AI sees corrections via daily log context even before consolidation catches up.

### Negative

- **More moving parts.** Three pipelines to reason about where there was one. Mitigated by each pipeline having a clearly bounded job and a deterministic trigger.
- **Manual edits to memory files get overwritten.** "Open in Editor" on MEMORY.md is technically possible but anything the user types that isn't backed by an `ai_memory` row disappears at the next consolidation. Memory Settings is the supported edit surface; the editor opens these files read-only.
- **Edit drift window.** Between routing and consolidation (up to ~14 days), a memory file bullet can show stale text while `ai_memory` holds the correction. The AI sees both and the daily log audit entry reconciles them, but the file is briefly out of sync.
- **Consolidator prompt shifts.** From "clean up this file" to "render these active facts in the style of this prose." Behavioral change requires careful prompt engineering and prose-style preservation testing.

### Risks

- **Mutex contention.** Router appends and consolidator rewrites share a per-file lock. Mitigated by router critical sections being short (~ms) and consolidator runs being rare (every few weeks typical).
- **Consolidator misreads prose style.** If the prompt doesn't preserve voice well, memory files feel inconsistent over time. Detectable via Matt's real-workflow verification; fixable via prompt iteration.
- **ai_memory corruption has bigger blast radius.** When it was one of two representations, DB loss was recoverable from files. Now files are rendered views, so a corrupted `ai_memory` renders corrupted files on next consolidation. Mitigated by existing automated DB backups (§2B) and the pre-synthesis backup path inside consolidator.

## What if we didn't decide

- **#646 stays open.** Synthesis keeps timing out on large memory files. Users either lose content when the LLM call fails or accept visible lag on every synthesis pass.
- **Memory files keep bloating.** The "general fact" landfill continues dumping into agent MEMORY.md. OpenClaw users keep hitting the 12K limit and manually trimming.
- **Delete/edit stays dishonest.** Users lose trust in the memory surface when deleted facts keep appearing in AI responses. "Forget this" becomes a known-broken button.
- **Every downstream feature carries the architectural debt.** Memory health dashboard (#164), search scoring improvements (§5.15), sync, and any future memory work has to accommodate the fused-responsibility model. The cost of this decision compounds the longer it's deferred.

## Alternatives considered

**Keep single synthesis, reduce trigger frequency.** Simpler change. Rejected: doesn't solve the timeout, just hits it less often. Large memory files still take 90s+ to rewrite whenever they do run. Also doesn't address bloat.

**Append-only memory files, no consolidation step.** Simplest change. Rejected: files grow as bullet lists forever. Prose readability degrades and duplicates accumulate. Matt's empirical experience with bloated agent memory shows this path doesn't work long-term.

**Memory files as source of truth, ai_memory as an index.** Preserves today's model. Rejected: deletes can't propagate cleanly. The AI would have to rewrite the file on every delete, reintroducing the timeout on every user "forget this" click. Worse than today.

**Cron-based consolidator.** Considered during the session. Rejected: Pane has no cron infrastructure. Desktop app + cron = fragile (app closed means cron doesn't run). Event-driven triggers (threshold + startup staleness check) cover the same ground without new infrastructure.

## References

- Issue #646 — personal synthesis timeout
- Issue #641 — extraction toast missing
- Issue #647 — dual-write to daily log
- Spec sections to update: §5.4, §5.4A, §5.5, §5.6, §5.7, §5.12, §23.3
- INVARIANTS.md §5 additions (companion file)
- Session artifact: `memory-architecture-decisions-2026-04-23.md`
- Related: ADR-0005 (DB primary, flat files backup) — this ADR extends that principle into memory specifically
