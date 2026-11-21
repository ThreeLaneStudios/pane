# Amendment 2 — §31.2 Due Today: row order + metadata line (design-phase)

**Type:** Follow-up spec PR to §31.2 (supplement style, same as PR #2094 — add subsections, supersede specific lines, preserve merged history).
**Date:** 2026-07-18
**Source:** §31 design phase. Two behavior decisions with spec impact; everything else from design is visual-only.
**Status of §31.2:** TARGET — NOT BUILT.
**ADR:** ADR-0057 stands. No data-model change, no §19 change.

Adds §31.2.6 (row order) and §31.2.7 (metadata line). §31.2.7 supersedes the "parent note's title" wording in merged §31.2.1 and in Amendment 1 (#2094).

---

## §31.2.6 (new) — Row order (deterministic)

Due Today renders rows in a **fixed, deterministic order**. Four groups, always in this sequence:

1. **Open tasks** — A–Z
2. **Open subtasks** — A–Z
3. **Done tasks** — A–Z
4. **Done subtasks** — A–Z

Where:

- **Group key** is `(done?, kind)` in the order above: open before done, and within each done-state, tasks before subtasks.
- **Within a group**, sort **A–Z, case-insensitive, by the row's display text** — task rows by note title, subtask rows by subtask text.
- **Tiebreak** (identical display text): by row `id` ascending, so order is fully deterministic — never render-order- or query-plan-dependent.

This refines the union query in §31.2.1: the merged task+subtask result is presented in this order. Implementation (SQL `ORDER BY` vs client-side sort of the merged list) is the code PR's call; the observable order is the contract.

> **📌 Why This Exists — deterministic, not "most useful" (2026-07-18, design phase):** A fixed order is a testable invariant. Two devices, two renders, or two query plans must produce byte-identical row order for the same data — so the order is CI-assertable and never drifts. Open-before-done keeps the actionable items at the top; task-before-subtask and A–Z are arbitrary-but-fixed, chosen so the rule has no ambiguity to re-litigate. Do not replace this with a "smart"/relevance sort without superseding this callout.

---

## §31.2.7 (new) — Row metadata line

Each row shows a metadata line under its primary text. Exact format:

- **Task row:** `in {Project}` — plain text, **no icons**.
- **Subtask row:** `in {Parent note} · {Project}` — the **note icon appears on the parent-note reference only**; `{Project}` has no icon. Separator is a middot (`·`).

Rules:

- **Projects have no icon in Pane** — never render an icon before a project name, on either row type.
- **The note icon attaches to the parent-note reference only** (subtask rows), nowhere else.
- **Task-vs-subtask differentiation still rides the subtask tag** (the existing "Subtask" label/badge, unchanged). The metadata line is context, not the differentiator; the tag remains the primary signal, now reinforced by the presence of a parent-note reference on subtask rows.

### Supersession

Merged §31.2.1, original:

> a subtask row is labeled as a subtask and shows its parent note's title for context

> **Amended 2026-07-18 (design phase):** "parent note's title for context" is superseded by the exact metadata-line format in §31.2.7: subtask rows show `in {Parent note} · {Project}` with the note icon on the parent-note reference only; task rows show `in {Project}`, no icons. The subtask tag remains the task-vs-subtask differentiator.

---

## No change required / flag

- **§19.5.1d notification drawer label** is brought into line with §31.2.7 in a **companion file** (`notifications-19.5.1d-drawer-metadata-amendment.md`) that lands in **this same PR / one Reed audit**. The drawer's subtask row adopts the `in {Parent note} · {Project}` format (note icon on parent-note ref only, no project icon), referencing §31.2.7 as the single source of truth. Task-row drawer formatting is unchanged.
- core-data-model §1.9, ADR-0057, the §19.5.1b watcher, and all of Amendment 1's other behavior (check-off, done-visible, date-anchor/picker, self-identifying date) are unchanged.

---

## Amendment summary (for the PR description)

| Change | Lands in | Supersedes | Watch-out |
| ------------------------------------------------------------ | -------------------------------------- | -------------------------------------------- | ------------------------------------------------------------ |
| Deterministic row order (open tasks → open subtasks → done tasks → done subtasks, A–Z, id tiebreak) | new §31.2.6 | — (refines §31.2.1 union) | order is the contract; CI-assert it |
| Metadata line: task `in {Project}`; subtask `in {Parent note} · {Project}`, note icon on parent-note ref only, no project icon | new §31.2.7 | §31.2.1 "parent note's title for context" | projects never get an icon; subtask tag still differentiates |
| Drawer subtask row adopts §31.2.7 format | companion §19.5.1d amendment (same PR) | §19.5.1d "parent note's title" wording | format defined once in §31.2.7; drawer references it |
