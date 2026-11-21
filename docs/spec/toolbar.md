# Toolbar Spec — Responsive Overflow + ⋯ Menu

**Status:** BUILT — v0.68.40  
**Issues:** #2123 (responsive overflow), #2130 (⌘⇧T regression fix)  
**PRs:** #2133 (overflow + ⋯ menu + ⌘⇧T), #2135 (measurement-based overflow trigger)

---

## 1. Toolbar Layout

The editor toolbar lives in `Editor.svelte` (`.editor-toolbar`). It always shows as a horizontal
flex container with the ⋯ button pinned to the right (via `.toolbar-autosave { margin-left: auto }`
on the autosave indicator that precedes it).

### 1.1 Always-Visible Items (all widths)

| # | Label | Handler |
|---|-------|---------|
| 1 | **B** Bold | TipTap `toggleBold()` |
| 2 | **I** Italic | TipTap `toggleItalic()` |
| 3 | **S** Strikethrough | TipTap `toggleStrike()` |
| — | sep | — |
| 4 | **T** Normal Text | TipTap `setParagraph()` |
| 5 | **H1** Heading 1 | TipTap `toggleHeading({ level: 1 })` |
| 6 | **H2** Heading 2 | TipTap `toggleHeading({ level: 2 })` |
| 7 | **H3** Heading 3 | TipTap `toggleHeading({ level: 3 })` |
| — | sep | — |
| 8 | **•** Bullet List | TipTap `toggleBulletList()` |
| 9 | **1.** Numbered List | TipTap `toggleOrderedList()` |
| — | autosave indicator | `isSaving` / `lastSaved` |
| — | **⋯** More | `moreMenuOpen` toggle |

### 1.2 Overflow Items (`.toolbar-btn--overflow`)

These items are **hidden** at narrow width and **appear in the ⋯ Formatting group** instead.

| # | Label | Handler function |
|---|-------|-----------------|
| 10 | Link (inline) | `handleToolbarLink()` |
| 11 | Code Block | `handleToolbarCodeBlock()` |
| 12 | Undo | `handleToolbarUndo()` |
| 13 | Redo | `handleToolbarRedo()` |
| 14 | Clock / Version History | `openVersionHistory()` |
| 15 | Pin / Unpin | `handlePinToggle()` |
| 16 | Insert Link (attachment) | `handleToolbarInsertLink()` |
| 17 | Checklist | `handleToolbarChecklist()` |
| 18 | Due Today | `handleToolbarDueToday()` |
| 19 | Attach File | `handleToolbarAttachFile()` |

---

## 2. Responsive Overflow (measurement-based)

A **ResizeObserver** watches `.editor-toolbar` and determines `isNarrow` by measuring actual
toolbar overflow — **not** a fixed pixel breakpoint. On each resize callback, a two-pass approach
runs synchronously before paint:

1. **Pass 1:** Temporarily remove `toolbar--narrow` class (direct DOM mutation) to reveal all
   overflow items. Force a synchronous layout read: compare `scrollWidth` vs `clientWidth`.
2. **Pass 2:** If `scrollWidth > clientWidth` (content would overflow), immediately re-add
   `toolbar--narrow` before the browser paints — no visible flash.
3. Sync the `isNarrow` Svelte state to match.

This replaces the original fixed 330px threshold (#2133) with content-aware collapse (#2135),
eliminating the clipping band where toolbar icons could be pushed off-screen before the
breakpoint triggered.

```css
.editor-toolbar.toolbar--narrow .toolbar-btn--overflow,
.editor-toolbar.toolbar--narrow .toolbar-sep--overflow {
  display: none;
}
```

The overflow items move to the **Formatting group** at the **top** of the ⋯ menu (only rendered
when `isNarrow`). At wide width (no overflow), the Formatting group is not shown in the ⋯ menu.

The ⋯ button is **always visible at every width**: it is in `.tpl-more-wrap` (last item,
`flex-shrink: 0`) and the `.toolbar-autosave { margin-left: auto }` pushes everything after the
always-visible group to the far right. Because the trigger is measurement-based, ⋯ can never
be clipped or pushed off-screen — overflow items collapse before that happens.

**Edge cases:**
- **Zero-width container** (hidden panel / unmounting): early-return guard when `clientWidth === 0`
- **Rapid resize / panel drag**: handled natively by ResizeObserver batching
- **No infinite loop**: ResizeObserver tracks container `clientWidth` (unchanged by show/hide of
  overflow items — only `scrollWidth` changes), so no re-observation cycle is triggered
- **Cleanup**: `$effect` return disconnects the observer on unmount

---

## 3. ⋯ Menu Contents

### At all widths:

```
[Formatting group — only at narrow width]
  ├ Link
  ├ Insert link
  ├ Code block
  ├ Undo
  ├ Redo
  ├ Version history
  ├ Pin / Unpin
  ├ Checklist
  ├ Due today
  └ Attach file
─────────────────  (separator: only when Formatting group visible)
Templates
  ├ Apply template…  ⌘⇧T
  └ Save as template…
─────────────────
Pin note  (disabled when !noteId || loadingPin; reflects isPinned state)
Move to trash  ← danger/red styling; calls api.moveToTrash(noteId) then activeNote.set(null)
```

### Copy link: DEFERRED

Copy link is **not implemented** in this PR. A `pane://note/<id>` or share URL scheme design is
required before it can be wired. See issue labeled `post-launch`: "Copy link in ⋯ menu
(URL-scheme design needed)".

---

## 4. Pin Note — Two Render Paths

Pin note appears in **two places**:
1. **Toolbar button** (`toolbar-btn--overflow`) — visible only at wide width
2. **⋯ menu item** — always present, regardless of width

Both paths share `handlePinToggle()` and respect:
- `disabled={!noteId || loadingPin}` — button disabled when no note or pin in-flight
- `isPinned` — reflects the current pin state (fetched in `loadPinState()` on mount)

---

## 5. Move to Trash

`handleMoveToTrash()` in `Editor.svelte`:
1. Guards on `!noteId || moveToTrashLoading`
2. Calls `api.moveToTrash(noteId)`
3. Sets `activeNote.set(null)` — closes the editor (follows NotesBrowser pattern)
4. Dispatches `pane-data-changed` — refreshes note lists

Danger styling: `.tpl-mi--danger` (red text + red icon + red hover background).

---

## 6. ⌘⇧T Fix (#2130)

**Diagnosis (2026-07-21):** Two existing paths for ⌘⇧T were wired correctly per static analysis
(TipTap `handleKeyDown` → custom event → `templateHandler`; and `svelte:window onkeydown`
direct assignment). Runtime probes were applied to identify the failure path, but interactive
testing was not possible in the subagent environment.

**Fix (defense-in-depth):** A capture-phase `window.addEventListener('keydown', ...)` is
registered in `onMount` in `+page.svelte`. Capture phase fires before ProseMirror's bubble-phase
handler, ensuring `showTemplates = true` is set regardless of TipTap/editor focus state or
extension event handling.

The existing paths (TipTap → custom event and `svelte:window`) are retained unchanged.
