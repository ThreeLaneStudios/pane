/**
 * `SYSTEM_NOTE_TYPES` — the canonical frontend list of `note_type` values
 * that are system-managed: their flat file lives at a fixed, role-derived
 * path (`SOUL.md`, `MEMORY.md`, a given day's `logs/{date}.md`, etc.),
 * never a slug computed from an editable title.
 *
 * #3029 (`~/Shared/ExampleApp/docs/audits/2026-10-12-scope-3029-system-
 * duplicates.md`): before this module existed, this exact 7-value array
 * was hand-typed THREE times on the frontend alone — `Editor.svelte`
 * (the real consumer), `Editor.test.ts`, and referenced again in
 * `Editor.mount-no-dirty.test.ts` — with zero Rust-side equivalent and
 * nothing tying any of the copies together. This module is now the ONE
 * hand-typed TypeScript copy; `Editor.svelte` and its tests import this
 * array instead of re-declaring it.
 *
 * The Rust-side canonical source is `SYSTEM_NOTE_TYPES` in
 * `src-tauri/src/notes.rs` — `update_note_internal` rejects a title
 * change on any note whose `note_type` is in that list. A true single
 * source across the Rust/TypeScript process boundary isn't achievable
 * here without either a runtime IPC round-trip (rejected for the same
 * reason #3026/item-64 rejected it for `VALID_COLUMN_TYPES`: this data
 * is `const` and never changes at runtime, so IPC buys no freshness and
 * forces a loading state or a synchronous fallback array that defeats
 * the single-source goal) or a build-time codegen step (no such tooling
 * exists in this repo). So this module and `notes.rs`'s constant are two
 * mirrors, kept honest by `systemNoteTypes.parity.test.ts`, which reads
 * BOTH files' literal source text and fails if they ever drift — the
 * exact mechanism #3026/item-64 established for `VALID_COLUMN_TYPES` /
 * `ShHeaderMenu.svelte`'s `TYPES`.
 */
export const SYSTEM_NOTE_TYPES = [
  'soul',
  'memory',
  'rules',
  'identity',
  'user',
  'tools',
  'daily',
] as const;
