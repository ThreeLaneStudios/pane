# Amendment (companion) — §19.5.1d drawer subtask-row metadata

**Type:** §19 amendment, **companion to the §31.2 design amendment** (`note-components-31.2-design-amendment.md`). Both files land in **one spec PR / one Reed audit**.
**Date:** 2026-07-18
**Source:** §31 design phase — the notification drawer's subtask row mirrors the Due Today metadata format for consistency across the two surfaces.
**Status:** TARGET — NOT BUILT.
**ADR:** ADR-0057 stands. No data-model change; no notification-architecture change (ADR-0021/0022 untouched).

Supplement style: supersede the one line in §19.5.1d, preserve merged history.

---

## §19.5.1d supersession — drawer subtask-row metadata

Merged §19.5.1d, drawer-label bullet, original:

> **Drawer label:** the panel renderer distinguishes a `target_kind='subtask'` row from a task row — it is marked as a subtask and shows the parent note's title for context. This is the drawer-side of the same differentiation Due Today applies (§31.2.1). Label selection keys off `target_kind`, not off the message text.

> **Amended 2026-07-18 (design phase):** a `target_kind='subtask'` drawer row shows the **§31.2.7 metadata-line format**. This replaces "shows the parent note's title for context." The subtask row is still marked with the subtask tag, and label/format selection still keys off `target_kind`, not the message text.

**Single source of truth:** the metadata format lives in §31.2.7. §19.5.1d **references** it — it does not restate the format — so the Due Today list and the notification drawer cannot drift. If §31.2.7 changes, the drawer follows automatically.

**Out of scope:** task-row (`target_kind='note'`) drawer formatting is unchanged by this amendment. Only the subtask row is brought into line with §31.2.7.

---

## Summary (for the PR description — this file)

| Change | Lands in | Supersedes | Watch-out |
| ------------------------------------------------------------ | -------- | ------------------------------------------ | ------------------------------------------------------------ |
| Drawer subtask row adopts §31.2.7 metadata-line format (by reference) | §19.5.1d | §19.5.1d "parent note's title for context" | format is defined once in §31.2.7; §19.5.1d references it — keep it a reference, don't inline the format |

**PR note:** this file + `note-components-31.2-design-amendment.md` are one PR. §31.2.7 is the canonical metadata definition; this amendment points the drawer at it.
