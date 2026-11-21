# Spec: Default Project ("My Notes") — One-Shot Creation

**Status:** BUILT  
**Issue:** #2142  
**Branch:** fix/2142-my-notes-reappears

---

## Summary

> **Superseded 2026-07-28 (#2030):** No default project is created on any onboarding path. Fresh install creates zero user projects (ratifies Matt 2026-07-24). `resolveWorkshopProjectId()` no longer creates a project; the `initial_project_created` guard and retroactive-flag path are moot. Workshop docs attach to a sentinel scope, not a project (agents.md §24.10.1e). Preserved for the two contracts that still hold: deletion-permanence and zero-project viability (#2142).

"My Notes" is **no longer created** during onboarding.

---

## Behaviour

### First Run

1. `resolveWorkshopProjectId()` is called during onboarding.
2. If `initial_project_created` is **not** set in `ai_settings`, the function creates a new project named "My Notes" and sets the flag.
3. The flag is written as: `INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('initial_project_created', 'true')`.

### Subsequent Runs

1. `resolveWorkshopProjectId()` checks `initial_project_created` before creating anything.
2. If the flag is `'true'`, no new project is created — even if zero user projects exist.
3. The user's deletion intent is respected permanently.

### Upgrade Path (Existing Users)

Existing users who already have projects when this code first ships are retroactively given the flag so "My Notes" does not appear for them either.

---

## Contract

| Condition | Behaviour |
|---|---|
| Fresh install | Create nothing; zero user projects |
| Flag set, projects exist | Do nothing |
| Flag set, projects empty | Do nothing — user deleted them |
| Flag missing (DB reset or upgrade) | Flag is set retroactively on next app open |

**Retained live contracts:** "the app functions normally with zero user projects" and "deleting a project is permanent."

⚠ The `initial_project_created` `ai_settings` key becomes inert. Leave it in place — removing it is a schema change for no gain; an unread key is harmless.

---

## Implementation

- **Guard:** `resolveWorkshopProjectId()` in `src/lib/components/Onboarding.svelte` reads the flag from `ai_settings` before calling `create_project`.
- **Flag key:** `initial_project_created`
- **Flag value:** `'true'`
- **Trash crate:** Project/folder directory deletion uses the [`trash`](https://crates.io/crates/trash) crate (cross-platform) instead of macOS `osascript`. A trash failure stops the delete — no `remove_dir_all()` fallback exists (#3028). Aligns with §2A.2/§2A.6a spec.

---

## Tests

- `test_delete_project_removes_from_db` — verifies hard DELETE from DB (`src-tauri/src/projects.rs`)
- `test_initial_project_created_flag_persists` — verifies the flag guard logic (`src-tauri/src/projects.rs`)
