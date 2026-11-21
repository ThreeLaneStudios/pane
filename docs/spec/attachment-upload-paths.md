# Design — Attachment Upload Paths (#1799)

**Date:** 2026-06-30
**Status:** Proposed (spec session)
**Supersedes the dead approach in #1805** (the `(file as any).path` branch).
**`spec-covered-by:` this session. Implements: #1799.**
**Home:** standalone design doc under `docs/spec/`. If you prefer, the size-limit contract (below) can be mirrored into `file-system.md` §2 attachments; flagged, not assumed (see "Spec-contract note").

---

## Problem (verified)

All three upload entry points (drag, picker, paste) deliver standard WKWebView `File` objects with **no `.path`**. The #1805 native-copy branch keys on `(file as any).path`, which is permanently `undefined`, so it never runs; every upload falls to FileReader → base64 → IPC, which OOMs in JavaScriptCore at ≥~100 MB. `dragDropEnabled: false` additionally disables Tauri's native drop interception. (Reed investigation; Tauri APIs verified — see session notes table.)

## Model: route by source, acquire real paths natively

| Entry | Path acquisition | Backend call | Memory path | Cap |
| --------------- | ---------------------------------------------- | --------------------------- | ---------------------- | ------------------------- |
| **Drag-drop** | `onDragDropEvent` → `payload.paths` (absolute) | `copy_attachment_from_path` | none (disk `fs::copy`) | 1 GB (Rust) |
| **File picker** | `plugin-dialog` `open()` → absolute path(s) | `copy_attachment_from_path` | none (disk `fs::copy`) | 1 GB (Rust) |
| **Paste** | clipboard `File` (no path exists) | `save_attachment` (base64) | JS heap | 50 MB (`PASTE_MAX_BYTES`) |

No size-based branching *within* a path. The old `LARGE_FILE_THRESHOLD_BYTES` router is removed.

---

## Path 1 — Drag-drop

**Acquisition.** Set `dragDropEnabled: true`. Register `getCurrentWebview().onDragDropEvent(cb)` (from `@tauri-apps/api/webview`) once, on editor mount; store the returned unlisten fn and call it on unmount (the API requires this). On `event.payload.type === 'drop'`, iterate `event.payload.paths` (each an absolute FS path); for each, call `copy_attachment_from_path(basename(path), path, noteMeta?.project_id)`, then insert the attachment node via the existing insert flow (same as the resolved-`{id, filename}` shape returned today).

**Remove** the DOM `drop` handler in `Attachment.ts` — under native interception it never fires (verified), so it would be dead code. Keep the `paste` handler.

**Wiring details to specify for the build:**

- **Drop position.** `onDragDropEvent` gives a physical-pixel `position`, not a ProseMirror position. Map it to an insert position (`view.posAtCoords` after converting physical→client coords, accounting for `devicePixelRatio`); fall back to the current selection if mapping fails. (The old DOM handler used `view.posAtCoords({left, top})`; the native event needs the conversion.)
- **No File metadata.** The event carries paths only — no MIME, no size. `copy_attachment_from_path` derives those from the file on disk (stat for size; extension for MIME). Confirm it does (GATE-2).
- **Multiple files.** `paths` is an array — handle N drops.
- **Listener scope.** One listener per webview; unlisten on unmount to avoid duplicate inserts across editor remounts.

**Risks (push-backs P1, P2):** confirm *why* `dragDropEnabled:false` was set (in-app HTML5 DnD may regress when flipped) and runtime-verify that `payload.paths` arrives intact on 2.11.2/macOS (Tauri #13698 garbled paths on an older version). Both must be checked at build time, not assumed.

> **P1 resolved (2026-06-30):** `git blame` shows `dragDropEnabled: false` was set in commit `f89b9453` ("Trove 2.0.0 - Notes + Tasks app", Feb 12, 2026) — the initial Trove 2.0 scaffolding. No PR, no linked issue, no recorded reasoning. This was part of the initial app setup, not a deliberate targeted choice to prevent a known regression. The flip-to-`true` re-test items (TipTap node drag, kanban reordering, file-drop→path delivery) carry forward as #1799 build requirements.

---

## Path 2 — File picker

**Acquisition.** Replace the toolbar `<input type="file">` (Editor.svelte ~1635) with `@tauri-apps/plugin-dialog`:

```
import { open } from '@tauri-apps/plugin-dialog';
const sel = await open({ multiple: true /*, filters: [...] */ });
if (sel) for (const path of (Array.isArray(sel) ? sel : [sel])) {
  const a = await api.copyAttachmentFromPath(basename(path), path, noteMeta?.project_id);
  // insert node {id: a.id, filename: a.filename}
}
```

`open()` returns absolute path(s) or `null` on cancel (verified). Route to `copy_attachment_from_path` — identical to drag.

**Prerequisite (GATE-1 / P3):** requires `tauri-plugin-dialog` (Rust crate) + `@tauri-apps/plugin-dialog` (JS) + `tauri_plugin_dialog::init()` in the builder + capability `dialog:default` (or `dialog:allow-open`) in `capabilities/*.json`. If not already present, this is a small prerequisite PR before the picker change.

---

## Path 3 — Paste

Clipboard files expose no filesystem path (no native API provides one; streaming deferred per scope), so paste keeps the base64 path through `handleFileUpload` → `save_attachment`. Add a frontend guard:

```
if (file.size > PASTE_MAX_BYTES) {
  // show: "This file is too large to paste (NN MB). Drag it in or use the file picker instead."
  return; // do NOT call FileReader
}
```

`PASTE_MAX_BYTES = 50 * 1024 * 1024`. Rationale: Reed observed OOM onset ~100 MB; base64 (×1.33) + JSC UTF-16 (×2) + a transient duplicate copy ≈ ×2.7–3.5 peak heap. 50 MB → ~270 MB peak, safely under failure. The message names the working alternatives (drag / picker), which now handle large files.

---

## Dead-code removal (Decision #5)

Remove the C3 branch and its comment in `Editor.svelte` (the `(file as any).path` block from #1805). `handleFileUpload` becomes paste-only; replace the block with the `PASTE_MAX_BYTES` guard above. Also remove the now-unused `LARGE_FILE_THRESHOLD_BYTES` constant (search for other references first).

---

## Threshold / cap coherence (Decision #4)

- Paste: `PASTE_MAX_BYTES` 50 MB — the JS-heap OOM guard. Binding limit for paste.
- Drag + picker: 1 GB Rust cap (`MAX_ATTACHMENT_SIZE_BYTES`), enforced in `copy_attachment_from_path` by stat-ing the source before copy. **Confirm/-add** — the cap currently lives only in `save_attachment` (GATE-2). Without it, a >1 GB drag would copy unbounded.
- No overlap, no gap: every byte size on every path hits exactly one clear limit and one clear message. The #1805 silent gap (50 MB–1 GB drags → base64 → OOM) is closed because drag never uses base64.

---

## Spec-contract note

This design governs upload *acquisition* mechanics (code), not a storage/ref contract — so no `file-system.md`/`core-data-model.md` edit is strictly required. The one specced-contract candidate is the **size-limit table** (paste 50 MB / native 1 GB). `file-system.md` §2 does **not** currently document an attachment size limit — no mirror edit needed. If a size-limit section is added to `file-system.md` in a future spec session, the table above should be mirrored there.

## Build gates (must close before/within the build)

- **GATE-1 / P3:** dialog plugin present? (`Cargo.toml` + `capabilities/*.json`)
- **GATE-2 / P4:** `validate_copy_source_path` accepts absolute drag/picker paths; 1 GB cap enforced in the copy path (`attachments.rs` post-#1805)
- **P1:** ~~reason `dragDropEnabled:false` was set~~ → **CLOSED** (initial scaffolding, no recorded reason; re-test items carry forward)
- **P2:** runtime-assert `onDragDropEvent` path integrity on 2.11.2/macOS
- **P4:** integration-test the copy path end-to-end (it has never executed)

---

## Image resize — drag anchor clamp (PR #1843 round 5)

Image resize (§full-inline mode): the drag anchor (`dragStartW`/`dragStartH`) is clamped to the live column width at mousedown. Natural dimensions remain in node attrs; only the drag interaction is bounded. See `AttachmentImageInline.svelte:startDrag`.

Invariant: `dragStartW ≤ columnMaxPx` at all times so `applyDrag`’s column clamp does not override every mousemove result from the first pixel of travel. Option B was chosen over Option A to preserve natural dims in node attrs — the column cap is a render-time and drag-time bound, not a data mutation. This matters if column width changes or a full-width mode is added.

---

## data-size attribute (PR #1867)

The serialized `<div data-attachment-id="...">` wrapper now includes an optional `data-size="N"` attribute (bytes as integer string) when file size is known at insert time. The attribute is additive and backward-compatible: old markdown without `data-size` parses to `size: null`, and `formatBytes(null)` returns `''` (no empty segment rendered). No schema or migration change needed — `size_bytes` was already in the DB; this PR wires it through to the node attrs and card meta line.

`spec-skip: attachment size + type prefix is a UI wiring pass — DB row already has size_bytes populated; adding data-size attr is additive and backward-compatible; no behavioral or schema change`
