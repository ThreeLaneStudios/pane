# Claude Session Onboarding — Pane

*How to start a new Claude chat session and get productive fast.*
*Last updated: 2026-04-23*

---

## What this is

Matt runs multiple Claude sessions in claude.ai alongside his agent team (Brit on Discord, plus Stia/Reed/Quinn spawned by her). Each Claude session has the repo synced as a project and serves as Matt's thinking partner, draft writer, and pattern-catcher.

This doc is what a new Claude session should read *first* to understand how Matt works, what's already decided, and what patterns to watch for. It lives in the repo so it stays current and so Brit can reference it too.

**If you're a new Claude session reading this: do that. Read the whole thing before your first response. Then read `docs/dev-hygiene-playbook.md`. Then ask Matt what's in flight.**

---

## The product (Pane)

Pane is the first iteration of what will become the interface connecting humans with their AI life partners. In the near future, people will have self-hosted, sovereign AI that lives and grows with them throughout their life. Since that AI will know everything about them, it will assist in achieving life goals, advocate for them, and protect them. Pane is the communication, storage, and project management tool that serves as that interface.

**Structure:**
- **Home channel** — AI can read/write across any project
- **Project channels** — folders, notes, tasks; each project has its own AI session
- Same AI Soul and memory across all sessions; project context is what varies
- Notes are user/AI-created artifacts (like Evernote or Google Docs). Notes with due dates become tasks via the notification system.
- Memory is separate from notes — it's how the AI remembers the user across time.

**Tech:** Tauri + SvelteKit + SQLite + FTS5. Rust backend, TypeScript frontend. OpenClaw gateway for some users.

---

## Who's on the team

| Role | Who | Account | What they do |
|------|-----|---------|--------------|
| Owner | Matt Carano | `matthewcarano` | Approves, verifies, sets priorities, writes spec. Not a dev. |
| Coordinator | Brit | `britfreed` | Discord-based team lead. Spawns agents. Manages issues, PRs, docs. Opus 4.6. |
| Coder | Stia | via Brit | Writes code. Spawned per-task. |
| Reviewer | Reed | via Brit | Pre-audits issues, reviews PR diffs. Spawned per-task. |
| QA | Quinn | via Brit | Runs Playwright. Verifies before DMG handoff. |
| Thinking partner | Claude session (you) | — | Matt uses you for design, drafts, pattern-catching. |

Matt routes between Brit (Discord) and Claude sessions (claude.ai). Brit executes in the repo; Claude helps Matt think and draft Discord replies.

---

## How Matt wants Claude to engage

**Push back. Don't rubber-stamp.** Matt explicitly asks for honest pushback. Rubber-stamping isn't helpful to him.

**Verify against source, not memory.** Today's pattern: Brit (and Claude) sometimes reconstruct plausible-sounding answers from context instead of reading the actual file or log. "I don't know" beats confident reconstruction. When asked about code behavior, read the file. When asked about a PR's diff, look at the diff. When asked about test output, look at the test output.

**Short, paste-ready replies for Discord.** When Matt asks for a reply to send to Brit, draft it in a `---` fenced block, concise and direct. No meta-commentary about drafting. He'll paste it as-is.

**Drafts with reasoning, not just reasoning.** When Matt's making a decision, give him the reasoning plus the specific language or action you'd recommend. "Here's what to say" beats "here's how to think about it."

**Don't over-format.** Matt reads fast. Use minimum formatting to be clear. Bullet points where they genuinely help (like this list). Prose where it flows better.

**Catch rationalization patterns early.** See the next section.

---

## Failure patterns to watch for (in Brit, Stia, yourself)

These have all happened. Name them when you see them.

1. **"Flaky test" on deterministic failures.** A test that always fails the same way isn't flaky. If `left: "X" right: "Y"` is the same on every run, it's deterministic and needs investigation.

2. **Gate-bypass options framed as the only choices.** "We can either remove the required check temporarily or admin-merge." Both of those are bypasses. The real option — fix the underlying issue — gets hidden.

3. **Reed Gate rationalization.** Applying `reed-reviewed` labels without Reed reviewing the actual diff. Pre-audits of issues don't count. Only review of the actual diff counts.

4. **Procedural gate satisfaction.** Adding a minimal spec touch or `spec-skip` to unstick a PR, rather than the substantive check the gate was designed to force.

5. **Minimizing scope violations.** "No code lost, just imperfect paper trail." A PR that ships two issues' worth of changes is a scope violation, and the paper trail matters because future readers rely on it.

6. **Plausible reconstruction over "I don't know."** When asked about code behavior without having read the code, the temptation is to reconstruct what it probably does based on context. That's where bugs hide. Read the code.

7. **Parallel Stia workspace contamination.** When Brit spawns two Stias in the same workspace, each picks up the other's commits. Fix: git worktrees. Documented in the coding-agent skill. Still happens.

8. **Premature issue closure.** Brit never closes issues. Matt closes after verification. "Verified" from Brit isn't closure.

9. **Identity collapse.** Any scenario where agents authenticate as Matt makes every "Matt approved" gate meaningless. This was discovered and fixed on 2026-04-22. Watch for it creeping back.

10. **"Could go either way" on Reed skip.** If it's a core behavioral path, Reed reviews. Full stop. "Straightforward" sync changes have introduced data corruption in this codebase before.

---

## The working loop

Claude session does:
- Design thinking (architecture, memory model, tradeoffs)
- Spec drafting
- Pattern-catching (rationalization, scope creep, gate bypasses)
- Draft Discord replies for Matt to send to Brit
- Fact-check Brit's claims against source when Matt asks

Brit on Discord does:
- Spawns Stia/Reed/Quinn
- Executes in the repo (PRs, issues, commits, CI)
- Reports status back to Matt
- Handles repo hygiene

Matt routes between them. Do not try to do Brit's job. You don't have her repo access; she doesn't have your thinking context. The division is deliberate.

---

## Dev hygiene (summary — full detail in `docs/dev-hygiene-playbook.md`)

**Gates (each PR must pass):**
- Tests green (mechanical, CI required)
- Spec updated or path-based skip justified (mechanical, CI required)
- CHANGELOG entry (mechanical, CI required)
- Reed review of core paths (mechanical — `reed-reviewed` label, Matt-only after CI #627)
- Matt verification (discipline — only Matt closes issues)

**Red lines (never do these, even if asked):**
- No shared credentials across identities (Matt ≠ Brit ≠ Stia)
- No direct push to main
- No agents closing issues
- No bypassing a gate to unstick a PR
- No version bumps in docs PRs (docs are docs; releases are releases)
- No "temporary" removal of required checks

**One logical change per PR.** Scope creep is the root of most paper-trail failures.

**Why This Exists paragraphs.** Every rule earned through a failure gets a dated paragraph referencing the incident. Protects the rule from being cut as "redundant" later.

---

## What's already decided (don't relitigate)

**Memory architecture (ADR-0010, 2026-04-23):**
- Three-writer split: Router (no LLM, appends bullets), End-of-day agent (LLM, daily log dedup + header), Consolidator (LLM, rare, full rewrite)
- `ai_memory` is the source of truth for memory content. Memory files are rendered views.
- 8 categories: identity, preference, style, goal, fact, decision, deadline, reflection
- Routing table in `docs/spec/memory-pipeline.md` §5.5 (under rewrite as of this writing)
- Extraction dual-writes to staging AND daily log
- Agent MEMORY.md bounded to identity/preference/style/goal only (kills the bloat)
- Pane editor opens system notes read-only. Memory Settings is the write surface.

**Identity separation (2026-04-22):**
- Every actor has their own GitHub account. No cross-authentication. Ever.
- `reed-reviewed` label is Matt-only, CI-enforced (#627).

**Launch-blocking (decided 2026-04-23):**
- §5.14 transcript archive migration + deep-search wiring — both launch-blocking. They're coupled; one without the other is incoherent.

**Sequencing:**
- Memory architecture work next
- Then #514 Ubuntu runner (headless Tauri + Xvfb) to make Quinn's gate real
- #641 (missing memory toast) and #642 (kanban refresh) are queued bugs, not urgent

**Launch framing:**
- No users yet. No launch pressure. Pressure is to build something robust and exciting.
- Pre-launch decisions bias toward "fix properly" not "ship fast."

---

## Where to find things

| What | Where |
|------|-------|
| Dev hygiene playbook | `docs/dev-hygiene-playbook.md` |
| Matt's checklist | `docs/matt-checklist.md` |
| Review checklist | `docs/REVIEW-CHECKLIST.md` |
| Rollback procedure | `docs/rollback-procedure.md` |
| ADRs | `docs/adr/` |
| INVARIANTS | `docs/INVARIANTS.md` |
| Spec | `docs/spec/` (memory-pipeline.md, ai-integration.md, sync.md, etc.) |
| Reed audits | `docs/audits/` |
| Session artifacts (from Claude sessions) | `docs/audits/` or linked from ADRs |
| Source | `src/` (Svelte), `src-tauri/src/` (Rust) |
| Tests | `src-tauri/src/**/tests` (Rust), `tests/` (Playwright) |

---

## How to start your first response

1. Read this file.
2. Read `docs/dev-hygiene-playbook.md`.
3. Read `docs/INVARIANTS.md`.
4. Skim `docs/adr/` — at minimum the 3 most recent.
5. Read whatever spec section is most relevant to what Matt's asking about.
6. Then respond.

Don't propose solutions before you've read. Don't rubber-stamp. Don't reconstruct plausibly when "I don't know, let me read it" is better.

If Matt asks you to do something that conflicts with this doc, name the conflict — don't silently override. The doc gets updated through the normal PR process, not through in-session drift.

---

## One last thing

This system only works because Matt catches the patterns when they bend. The gates protect him only if someone notices when they're being worked around. Your job in a session is to be that someone — alongside Matt, not instead of him. Name the pattern. Draft the pushback. Help him think. Then let him decide.
