# PR #424 Review: Kanban Infinite Loop Fix

**Commit:** `ebfd5bd` — "fix: remove global event listener from kanban, use scoped refreshes (#424)"  
**File:** `src/lib/components/KanbanBoard.svelte`  
**Reviewer:** Reed  
**Date:** 2026-04-17

---

## 1. Data Path Trace

### Problem (Before)
- **Trigger:** Editor save → fires `pane-data-changed` global event
- **Listener:** KanbanBoard line 42 → `handleDataChanged()` (line 38-42)
- **Action:** 300ms debounced call to `loadTasks()` (line 40)
- **Effect:** Re-render → Editor destroy/mount → save → fires event → **LOOP**

### Solution (After)
- **Removed:** `pane-data-changed` listener entirely (was line 42, 48)
- **Removed:** `handleDataChanged()` function (was line 38-42)
- **Removed:** `reloadTimeout` debounce timer (was line 26, 46)
- **Removed:** `isLoadingTasks` re-entry guard (was line 25, 82, 99)

### New Refresh Paths
1. **onMount** (line 40) → `loadTasks()` — initial load ✅
2. **handleDrop** (line 143) → optimistic local update to `tasks`, only calls `loadTasks()` on error ✅
3. **handleTaskCreated** (line 169) → `tasks = [...tasks, newTask]` — local array update ✅
4. **handleTaskDeleted** (line 181) → `tasks = tasks.filter(...)` — local array update ✅

All refresh paths verified: no global event listener, only scoped local updates.

---

## 2. Database/State Check

**No database changes.** This is a UI event-handling fix.

Current state after fix:
- `tasks` array is updated locally via state mutations
- No external data dependencies changed
- Editor saves still work (just don't trigger kanban reload)

---

## 3. Full Code Path Test

### Before (Broken)
```
User edits task in Editor
→ Editor save
→ fires pane-data-changed (global)
→ KanbanBoard handleDataChanged (debounced 300ms)
→ loadTasks()
→ tasks array changes
→ Svelte re-renders columns
→ Editor destroy/mount (because it's in reactive context)
→ Editor save on mount
→ fires pane-data-changed
→ INFINITE LOOP
```

### After (Fixed)
```
User edits task in Editor
→ Editor save
→ fires pane-data-changed (ignored by KanbanBoard)
→ No reload
→ No re-render
→ No loop
```

**Why this works:**
- Editor is outside reactive column rendering (line 424, inside `kanban-split-view` but after `kanban-left-panel`)
- Task mutations happen locally (optimistic updates)
- Only onMount, drag-drop, create, delete trigger refreshes
- Editor save does NOT trigger kanban reload

---

## 4. Caller Analysis

### Removed Functions
- `handleDataChanged` had **1 caller**: `pane-data-changed` event listener (removed)
- No other code references these removed functions

### Remaining Functions
**loadTasks()** is called from:
1. Line 40: `onMount()` — initial load
2. Line 157: `handleDrop()` error handler — only on API failure (revert)

**tasks array** is mutated locally from:
1. Line 143: `handleDrop()` — optimistic update
2. Line 169: `handleTaskCreated()` — append new task
3. Line 181: `handleTaskDeleted()` — filter out deleted task
4. Line 176: `handleTaskUpdated()` — map replace

All callers verified. No external dependencies broken.

---

## 5. Edge Cases

### Case: User drags task, API fails
- Line 143: Optimistic update applied immediately
- Line 153: API call fails
- Line 157: Reverts via `tasks = await api.listTasks()`
- **Result:** Correct behavior, no loop

### Case: User creates task while Editor is open
- Line 169: `tasks = [...tasks, newTask]`
- Local update, no `loadTasks()` call
- Editor remains stable, no destroy/mount
- **Result:** Correct

### Case: User deletes task while Editor is open
- Line 181: `tasks = tasks.filter(...)`
- Local update, no global event
- **Result:** Correct

### Case: External data change (e.g., background sync)
- **Before:** Would trigger `pane-data-changed` → reload
- **After:** Kanban does NOT reload automatically
- **Impact:** If an external process updates tasks, kanban won't auto-refresh until user manually navigates away and back
- **Assessment:** Acceptable tradeoff — prevents infinite loop, user can manually refresh if needed

---

## 6. Silent Failures

**Error handling verified:**
- Line 154-157: `handleDrop` error logs and reverts via full `loadTasks()` ✅
- Line 88-95: `loadTasks` error logs and sets `error` state message ✅
- Line 92: Provides empty array fallback ✅

No silent failures detected.

---

## 7. Backward Compatibility

**No breaking changes:**
- `pane-new-task` event listener still present (line 41) ✅
- Task creation, deletion, status updates still work ✅
- Editor integration unchanged (still renders at line 424) ✅
- Only change: removed reactive reload on `pane-data-changed`

**Impact:** Users who relied on automatic kanban refresh from external data changes will need to manually refresh. This is acceptable to fix the infinite loop.

---

## Verification Checklist

- [x] `pane-data-changed` listener completely removed (grep confirms no references)
- [x] `handleDataChanged` deleted (no references remain)
- [x] `reloadTimeout` deleted (no references remain)
- [x] `isLoadingTasks` deleted (no references remain)
- [x] Remaining refresh paths (`onMount`, `handleDrop`, `handleTaskCreated`, `handleTaskDeleted`) verified
- [x] No NEW global event listeners added (only `pane-new-task` remains, which is correct)
- [x] `pane-new-task` listener still present (line 41)
- [x] Editor placement outside reactive column rendering (line 424)
- [x] Error handling preserved
- [x] No database/migration changes required

---

## Code Evidence

### Lines Removed (git diff):
```diff
-  let isLoadingTasks = false; // line 25
-  let reloadTimeout: ReturnType<typeof setTimeout> | null = null; // line 26

-  function handleDataChanged() { // lines 38-42
-    if (reloadTimeout) clearTimeout(reloadTimeout);
-    reloadTimeout = setTimeout(() => {
-      loadTasks();
-    }, 300);
-  }

-    window.addEventListener('pane-data-changed', handleDataChanged); // line 42
-    if (reloadTimeout) clearTimeout(reloadTimeout); // line 46
-    window.removeEventListener('pane-data-changed', handleDataChanged); // line 48

-    if (isLoadingTasks) return; // line 82
-    isLoadingTasks = true; // line 83
-      isLoadingTasks = false; // line 99
```

### Key Remaining Lines:
- Line 40: `loadTasks()` in `onMount`
- Line 41: `window.addEventListener('pane-new-task', handleNewTaskEvent)`
- Line 143: `tasks = tasks.map(...)` in `handleDrop`
- Line 157: `tasks = await api.listTasks()` in error handler
- Line 169: `tasks = [...tasks, newTask]` in `handleTaskCreated`
- Line 181: `tasks = tasks.filter(...)` in `handleTaskDeleted`
- Line 424: `{#if $activeNote}` — Editor outside column loop

---

## Verdict

**✅ SHIP**

**Reasoning:**
1. Root cause correctly identified and eliminated (global `pane-data-changed` listener)
2. All old loop-inducing code removed (debounce, re-entry guard, event handler)
3. Scoped refresh paths verified and correct (local mutations, no global listeners)
4. Editor placement confirmed outside reactive rendering
5. Error handling preserved
6. No breaking changes to existing features
7. Acceptable tradeoff: loses auto-refresh on external changes, gains stable editor

**Fix Quality:** Surgical. Removes exactly the problematic code without introducing new complexity.

**Risk:** Low. No new global state, no new listeners, no database changes.

**Testing Recommendation:** 
- Manual: Open kanban, edit task in Editor, verify no infinite loop
- Manual: Drag-drop task, verify optimistic update + API success
- Manual: Create/delete task while Editor open, verify no crashes

---

**Reviewed by:** Reed  
**Date:** 2026-04-17 08:06 EDT
