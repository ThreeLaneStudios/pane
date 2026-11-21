# Matt's Checklist — Directing Traffic

*Short, actionable reference for directing Pane development. Updated 2026-04-22.*

---

## When you have a new idea or request

1. **Tell Brit.** Describe what you want — Brit creates the GitHub issue, links to spec, routes to the right agent.
2. **If it's urgent**, say so. Brit will reprioritize.
3. **If it changes architecture**, Brit will flag it and ask you to confirm before anyone writes code.

---

## When you want something built

1. **Say "go ahead."** That's the gate. Nothing builds without it.
2. **Specify priority** if it's not obvious — "do this next" or "after the current build."
3. **Brit handles routing**: Stia (code), Reed (research/review), Lane (design), Quinn (QA).
4. You don't need to know which agent — just say what you want.

---

## When a DMG arrives

1. **Read the announcement** — version number, what changed, which issues it touches.
2. **Install and test the specific changes** — on a safe database or with a backup if schema changed. Does the thing it claims to fix actually work?
3. **Spot-check your workflow** — open Pane, click around, make sure nothing else broke.
4. **Report back**:
   - ✅ "Works" → Brit closes the issue.
   - ❌ "Broken — [what's wrong]" → Brit files a bug, routes the fix.
5. **This is the final gate.** Nothing closes without your confirmation.

---

## When reviewing a PR

1. **CI must be green** — you can't merge if checks fail (enforced).
2. **Read the PR description** — does the change make sense? Does it match what you asked for?
3. **Check for Reed's PR comments** — Reed posts diff review findings directly on the PR. Read them before applying labels. If Reed hasn't commented yet, don't apply `reed-reviewed`. Pre-audits of issues don't count — only review of the actual diff.
4. **Apply `reed-reviewed` label yourself** — only you can do this (enforced by CI, #627). Brit cannot apply it. This is how you sign off on the architecture review.
5. **Approve** based on Reed's review + CI green + the change matches what you asked for. Ask Brit to explain if unsure.

  > **Why This Exists (2026-04-22):** Brit applied `reed-reviewed` to PRs #625 and #626 without Reed reviewing the actual diffs — three times in one day. The label was meaningless when anyone could apply it. Now it's matt-only: you read Reed's comments, you decide if the review is sufficient, you apply the label.

---

## Weekly check-in (Sundays)

1. **Review what shipped** — Brit posts a summary of the week.
2. **Spec audit** — Reed runs, reports drift.
3. **ADR sweep** — anything from the week worth writing up?
4. **Playbook review** — does the dev hygiene playbook still match reality? Any rules being ignored because they don't work?
5. **Reprioritize** — anything change? New ideas? Shift direction?
6. **Flag blockers** — anything you're waiting on that isn't moving?

## Quarterly security audit

1. **Token audit** — Review all GitHub PATs, API keys, and service credentials. Revoke anything unused. Confirm each token is scoped to the correct account and minimum permissions.
2. **Identity audit** — Confirm Brit, Stia, Reed each authenticate only as their own GitHub accounts. No cross-identity access.
3. **Gate audit** — Verify matt-only gates still work: try applying `reed-reviewed` as a non-Matt account, confirm it's rejected.

  > **Why This Exists (2026-04-22):** Identity collapse went undetected from initial setup (Feb 6) until Apr 22 — 2.5 months. Quarterly checks catch this class of slow-moving structural risk.

---

## When Brit asks you something

1. **If Brit flags an architectural conflict or WARN from Reed** — read the explanation, ask clarifying questions if needed, approve or redirect. Don't defer.
2. **If Brit asks between two equally valid approaches** — pick one and say why, or say "your call" explicitly. Don't leave it ambiguous.
3. **If an architectural decision is being made, ask: "should this be an ADR?"** — if yes, Brit writes it.
4. **If you don't know**, say "I don't know — what do you recommend?" Brit will make a call or escalate to Reed.

---

## When you think Brit is wrong

1. **Push back.** Ask her to verify against source or logs.
2. **Don't accept "trust me" on technical claims** — she's gotten things wrong before.
3. **If she can't verify, that itself is useful information** — flag that we're in guessing territory.
4. **Catch your own overconfidence too.** Brit has pushed back on your wrong calls too.

---

## When something breaks

Severity levels — tell Brit which one:

1. **Cosmetic** — UI looks wrong but works. Normal bug report, next build.
2. **Functional** — a feature doesn't work. Normal bug report, next build. Don't ship new work before it's fixed.
3. **Blocking** — can't use the app at all. Stop other work, fix immediately.
4. **Data** — my real data is at risk or corrupted. ROLLBACK NOW. Brit executes rollback procedure, we investigate after.

---

## What you never need to do

- Create GitHub issues (Brit does it)
- Route work to agents (Brit does it)
- Write or review code (agents + CI do it)
- Manage the build pipeline (Brit + Quinn do it)
- Remember process rules (they're mechanized in CI)

## What only you can do

- Say "go ahead" (build approval)
- Confirm a fix works (issue close gate)
- Approve PRs (branch protection)
- Set priorities and direction
- Make architectural decisions when Brit flags them
