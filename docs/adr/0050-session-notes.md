# #1799 — Gate Closure + Final Edits

**Date:** 2026-06-30 (supersedes the open GATE-1/GATE-2 items in `0049-session-notes.md`)
Source-read against the three pasted files: `Cargo.toml`, `capabilities/default.json`, `attachments.rs` (post-#1775/#1799).

---

## GATE-1 — Dialog plugin: CLOSED, present. No prerequisite PR.

- `Cargo.toml`: `tauri-plugin-dialog = "=2.7.1"` (pinned). `tauri = "=2.11.2"` confirmed; `protocol-asset` feature is enabled (matters for P6).
- `capabilities/default.json`: `"dialog:default"` is in the permissions list (`dialog:default` bundles `open`).

So the file-picker path needs **no dependency/capability PR** — just swap the `<input type="file">` for `open()`. Quick confirm at build (not blockers, almost certainly present given the pinned crate + capability): `.plugin(tauri_plugin_dialog::init())` in `lib.rs`, and `@tauri-apps/plugin-dialog` in `package.json`.

## GATE-2 — Copy command: signature confirmed, guard confirmed, cap MISSING (must add).

**Signature (real):**

```rust
copy_attachment_from_path(db, project_id: String, source_path: String, filename: String, mime_type: String)
```

Four caller args → JS invoke must pass `{ projectId, sourcePath, filename, mimeType }`. **Reed's earlier 3-arg shape was wrong** — `mime_type` is required. Drag/picker have no `File.type`, so derive MIME from the extension (`api.ts` already has `guessMimeType` per the #401 inventory) and pass it.

**Guard (`validate_copy_source_path`) — requirement (c) CLOSED, compatible:** rejects literal `..`, `canonicalize()`s (resolves symlinks + verifies existence), requires absolute + no post-canonical `..` + `is_file()`. Drag (`onDragDropEvent.paths`) and picker (`open()`) both return absolute, existing, regular-file paths → **accepted.** (Documented TOCTOU window between validate and `fs::copy` is acknowledged in-source and acceptable for the desktop threat model.)

- Wiring note: a dragged **folder** yields a directory path → guard returns "not a regular file." Surface a friendly "folders can't be attached" message rather than the raw error.

**1 GB cap — NOT enforced on the copy path (this was the open question; answer: must add).** `copy_attachment_from_path` validates the path then `fs::copy`s with **no size check**. `MAX_ATTACHMENT_SIZE_BYTES` (1 GB) is enforced only in `save_attachment`. Per the locked "1 GB cap stays" + the Decision #4 coherence model, add a stat-based check before copy.

*(The BEFORE/AFTER code edit for `attachments.rs` belongs to the #1799 build PR, not this docs PR.)*

This closes the coherence model: paste → 50 MB (`PASTE_MAX_BYTES`, JS); drag/picker → 1 GB (now enforced in the copy path). No path is unguarded.

---

## P1 — `dragDropEnabled: false` origin: CLOSED.

`git blame` shows `dragDropEnabled: false` was set in commit `f89b9453` ("Trove 2.0.0 - Notes + Tasks app", Stia, Feb 12, 2026) — the initial Trove 2.0 scaffolding commit. No PR, no linked issue, no commit message reasoning. `true` is the Tauri default, so `false` was a deliberate non-default choice, but there is no recorded rationale for it.

The flip-to-`true` re-test items carry forward as #1799 build requirements:
- TipTap in-editor node drag (text blocks, attachments)
- Kanban card reordering (if applicable)
- File drop → `onDragDropEvent` path delivery verification

*(The BEFORE/AFTER `tauri.conf.json` edit belongs to the #1799 build PR, not this docs PR.)*

---

## P6 — NEW push-back (the upload-only framing hides this): large files upload but won't *render*.

The native-copy path makes a large file land on disk without OOM. But display still runs through `resolveAttachmentUrls` → `get_attachment_data` (base64 the whole file into the WebView). A 375 MB file therefore uploads successfully and then **OOMs at render** — the user still sees a failure. **#1799's upload fix is not end-to-end without a non-base64 display path.**

The plumbing already exists: `attachments.rs` now has `resolve_attachment_path` (returns the absolute path for `convertFileSrc`), explicitly labelled "#1777 PR2b — not yet wired into the render path," and `protocol-asset` is enabled in `Cargo.toml`. So the fix is to switch the resolver to `resolve_attachment_path` + `convertFileSrc` (the other session's Decision #1).

**Recommendation:** make #1777's resolver switch a **gating dependency for closing #1799** — do not mark #1799 done until a large file both **uploads and renders**. Otherwise we ship the same "Upload failed"-class outcome at a different stage. (At minimum, #1799 must route large-file *display* off base64, even if the full #1777 refactor lands later.)

---

## Cross-session notes (for the attachments storage/rendering track, not #1799 work)

- **Prior-session P3 is partially resolved in code already.** `attachment_dir_for_project_id` (#1782) routes: real project UUID → `Projects/{name}/Assets/`; `__global__`/empty/unknown (thread IDs) → legacy `.pane/attachments/{project_id}/`. So home-chat/thread attachments do have a defined home. Still open: that legacy dir is **not** under the asset/fs scope (below) and #1775's backup `_assets.tar.gz` targets `Projects/*/Assets` only — global attachments wouldn't be backed up or, post-#1777, loadable via `convertFileSrc`.
- **fs capability scope** (`capabilities/default.json`) allows reads under `$HOME/Pane/Projects/*/Assets/**` only — **not** `.pane/attachments/**`. Rust `std::fs::copy` in the copy command is unaffected (capability scopes don't bind Rust std), but `convertFileSrc`/asset-protocol display of legacy/global attachments will need scope coverage. Confirm the `assetProtocol` scope in `tauri.conf.json` (not in the files I have) covers `Projects/*/Assets` for #1777's render switch.

---

## Status

- GATE-1: **closed** (plugin present).
- GATE-2: **closed** (signature + guard confirmed; cap edit specified in design doc).
- P1: **closed** (initial scaffolding, no recorded reason; re-test items carry forward as build requirements).

Design doc + these notes are final and ready for the #1799 build to cite `spec-covered-by:`.
