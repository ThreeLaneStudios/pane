# Project Brief / Description Persistence

**Status:** BUILT — wired in fix/project-brief-persistence (relates to #2140)

---

## Overview

The **Project Brief** is a user-editable text field in the Project Settings panel
(`src/lib/components/ProjectSettings.svelte`, bound to the `projectBrief` variable).
It is stored in the `projects.description` column in the SQLite database.

---

## Storage

| Layer | Location |
|-------|----------|
| DB column | `projects.description TEXT` (nullable) |
| Read command | `get_project_settings` (`src-tauri/src/ai/connections.rs`) |
| Write command | `update_project_settings` (`src-tauri/src/ai/connections.rs`) |
| Frontend | `ProjectSettings.svelte` — `projectBrief` variable |

The `description` column has existed in the `projects` table since schema v1.
No migration is needed — the column was always there, just never wired through
the settings commands.

---

## Read Path

`get_project_settings` SELECTs `description` along with all other project fields
and returns it in the `ProjectSettings` struct as `description: Option<String>`.

The frontend reads `settings.description ?? ''` and assigns it to `projectBrief`.

---

## Write Path

There are **two commands** that can write `projects.description`. They are designed
to coexist safely:

### 1. `update_project_settings` (settings save path)
Called on every Project Settings save. Accepts `description: Option<String>`:

- `None` (JS param absent or `undefined`) → **don't touch** the column
- `Some("")` → clear the description (stores empty string in SQLite)
- `Some(value)` → set description to value

The frontend passes `projectBrief` directly (no `|| null` coercion), so an empty
brief field sends `""` (empty string). SQLite stores it as an empty string, not NULL.
The read side uses `settings.description ?? ''`, making empty-string and NULL
indistinguishable in the UI — but the write path is explicit about intent.

### 2. `update_project` in `projects.rs` (name-change path)
Called only when the project name changes. It accepts an optional `description`
parameter but the settings form never passes a `description` to this path —
it only passes `name`. So this path never clobbers the brief during a rename.

**There is no clobber risk:** the two paths are called in separate code paths,
`update_project` never receives a `description` from the settings UI, and
`update_project_settings` uses `None = don't touch` semantics for the description
field when it is not explicitly provided.

---

## Tests

`src-tauri/src/ai/connections.rs` — module `tests_2140_project_brief_persistence`:

| Test | What it verifies |
|------|-----------------|
| `test_get_reads_description` | `description` is returned when set in DB |
| `test_get_reads_null_description` | `description` is `None` when DB column is NULL |
| `test_update_sets_description` | `Some(value)` sets description |
| `test_update_none_does_not_clobber_description` | `None` leaves existing description intact |
| `test_update_empty_string_clears_description` | `Some("")` sets description to empty string |
| `test_description_survives_unrelated_update` | A name-only UPDATE does not touch description |
