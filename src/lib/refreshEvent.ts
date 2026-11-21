// src/lib/refreshEvent.ts — the single data-change refresh event (#2972,
// tracked on #2874). `docs/spec/data-refresh.md` §34 is the authority.
//
// Mirrors `src-tauri/src/refresh_event.rs` exactly — same event name, same
// `entity_type`/`reason` string literals — so neither side (nor any new
// listener this file's constants get used by) retypes them by hand. This
// module is the frontend's single source of truth for those literals;
// every per-screen listener added in Stage 2 (§34.11 point 2) imports from
// here rather than hardcoding `'pane-data-changed'`/`'note'`/`'edited'`/etc.
//
// Stage 3 (#2972, this file's current scope) adds the two pieces Stage 2
// deliberately left out: the app-side SENDER (`dispatchDataChanged`, every
// `src/lib/api.ts`/component mutation below it) and origin-tag echo
// suppression (§34.4, `makeOriginTag`/`registerOwnOriginTag`/
// `isOwnOriginTag`) — plus `subscribeDataChanged`, the one shared helper
// (item 4) every listener (Stage 2's seven `listen()`-only sites, plus any
// new Stage 3 listener) now uses to hear BOTH channels instead of a
// hand-paired `addEventListener` beside a `listen()` call.

import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/** Event name — reused `pane-data-changed` (§34.1). Delivered two ways:
 *  - Backend (Tauri `emit()`) → received via `listen()` from
 *    `@tauri-apps/api/event` (Tauri IPC is invisible to
 *    `window.addEventListener`, #2215).
 *  - App (future Stage 3 sender) → received via `window.addEventListener`
 *    as a DOM `CustomEvent` with this same name. */
export const EVENT_DATA_CHANGED = 'pane-data-changed' as const;

/** `entity_type` values — exactly these five (§34.1's schema block; #3018
 *  adds `category` as the fifth). Mirrors `src-tauri/src/refresh_event.rs`
 *  exactly — a drift between the two means events silently never match.
 *  `entity_id` for `category` is the category name itself. The same four
 *  `reason` values apply unchanged: add = `created`, rename/recolor =
 *  `edited`, delete = `deleted`. No new `reason` value. */
export const ENTITY_NOTE = 'note' as const;
export const ENTITY_SHEET = 'sheet' as const;
export const ENTITY_FOLDER = 'folder' as const;
export const ENTITY_PROJECT = 'project' as const;
export const ENTITY_CATEGORY = 'category' as const;

export type EntityType =
  | typeof ENTITY_NOTE
  | typeof ENTITY_SHEET
  | typeof ENTITY_FOLDER
  | typeof ENTITY_PROJECT
  | typeof ENTITY_CATEGORY;

/** `reason` values — exactly these four, no fifth (§34.1). */
export const REASON_CREATED = 'created' as const;
export const REASON_EDITED = 'edited' as const;
export const REASON_MOVED = 'moved' as const;
export const REASON_DELETED = 'deleted' as const;

export type Reason =
  | typeof REASON_CREATED
  | typeof REASON_EDITED
  | typeof REASON_MOVED
  | typeof REASON_DELETED;

/** The one data-change event's payload shape (§34.1). `origin_tag` is
 *  required on every real emit, but Stage 2 listeners never compare
 *  against it (echo suppression is Stage 3, §34.4) — typed as a plain
 *  `string` here, not optional, to match the wire shape exactly. */
export interface DataChangedPayload {
  entity_id: string;
  entity_type: EntityType;
  reason: Reason;
  origin_tag: string;
}

// ── Origin tags & echo suppression (§34.4) ──────────────────────────────

/** Mints a fresh origin tag, unique to one save CALL — not to the note/
 *  entity being saved (§34.4: "The origin tag must be scoped to the save
 *  call, not the entity — two different saves to the same note, even from
 *  the same editor instance moments apart, are two different origin
 *  tags"). Every app sender in §34.1.2/§34.1.3 that doesn't receive an
 *  explicit tag from its caller mints its own via this function, so the
 *  wire payload's `origin_tag` is never empty. */
export function makeOriginTag(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  // crypto.randomUUID() is unavailable in some non-browser/non-HTTPS test
  // contexts; this fallback is only ever reached there, never in the real
  // Tauri webview (always secure-context) — still unique enough that two
  // calls in the same tick can't collide, which is all suppression needs.
  return `tag-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

// A save call that wants to skip its OWN resulting echo (§34.4's first
// rule — "a listener that is the thing that just saved... skips") mints a
// tag via makeOriginTag(), registers it here BEFORE issuing the save
// (synchronously — see registerOwnOriginTag's own comment for why), then
// checks isOwnOriginTag() against every incoming event for the entity it
// just saved. Non-destructive and TTL-expired (not consumed on first
// read) deliberately: more than one local listener may need to inspect
// the SAME save's echo (e.g. Editor.svelte's own metadata patch AND
// +page.svelte's reload-editor bridge both react to one Editor save), and
// a "first reader wins, deletes it" design would make the second reader
// see a false negative — they'd wrongly treat their own save as foreign.
const ownOriginTags = new Set<string>();
const OWN_ORIGIN_TAG_TTL_MS = 15000;

/** Registers `tag` as one of "my own" saves, due to expire automatically
 *  in `OWN_ORIGIN_TAG_TTL_MS` (so a short-lived registry never grows
 *  without bound). MUST be called synchronously, before the `await` on
 *  the save call that will eventually dispatch an event carrying this
 *  tag — the dispatch happens inside that save's async body (after its
 *  own internal `invoke()` resolves), so as long as the registration
 *  happens before the save is even started, it is unconditionally in
 *  place by the time the dispatch's listeners run, regardless of how the
 *  save's own await chain interleaves. */
export function registerOwnOriginTag(tag: string): void {
  ownOriginTags.add(tag);
  setTimeout(() => ownOriginTags.delete(tag), OWN_ORIGIN_TAG_TTL_MS);
}

/** Non-destructive membership check — §34.4's suppression test. Does NOT
 *  remove `tag` (see the registry's own comment on why: multiple local
 *  listeners may each need to check the same save's echo). */
export function isOwnOriginTag(tag: string): boolean {
  return ownOriginTags.has(tag);
}

/** Test-only escape hatch — clears every registered tag so one test's
 *  registrations can never leak into another's assertions. Not imported
 *  by any production (non-`*.test.ts`) file. */
export function _clearOwnOriginTagsForTest(): void {
  ownOriginTags.clear();
}

// ── The one shared app-sender helper (§34.1.2/§34.1.3, item 1/4) ───────

/** Dispatches the data-change event on the APP channel (DOM
 *  `CustomEvent`, §34.1's sender #2) — the one place every app-initiated
 *  mutation in `src/lib/api.ts` and every component sender
 *  (`Editor.svelte`, `NotesBrowser.svelte`, `+page.svelte`) builds the
 *  payload from, so no call site hand-rolls its own
 *  `window.dispatchEvent(new CustomEvent(...))` with a shape that can
 *  drift from the backend's `emit_data_changed` (`refresh_event.rs`). */
export function dispatchDataChanged(
  entityId: string,
  entityType: EntityType,
  reason: Reason,
  originTag: string = makeOriginTag(),
): void {
  if (typeof window === 'undefined') return;
  const detail: DataChangedPayload = {
    entity_id: entityId,
    entity_type: entityType,
    reason,
    origin_tag: originTag,
  };
  window.dispatchEvent(new CustomEvent<DataChangedPayload>(EVENT_DATA_CHANGED, { detail }));
}

// ── The one shared subscribe helper (§34.1's two-channel design, item 4) ─

/** Subscribes ONE handler to BOTH data-changed channels — the backend
 *  Tauri `emit()` (invisible to `window.addEventListener`, #2215 — must
 *  go through `listen()`) and the app DOM `CustomEvent` this file's
 *  `dispatchDataChanged` sends. Every per-screen listener (the seven
 *  Stage 2 `listen(EVENT_DATA_CHANGED, ...)`-only sites, plus any new
 *  Stage 3 listener) calls this instead of pairing its own `listen()`
 *  with a second, hand-written `window.addEventListener` — exactly the
 *  pattern #2972 stage 3 item 4 forbids.
 *
 *  Returns a single `Promise<UnlistenFn>` covering both registrations,
 *  so every call site keeps the identical
 *  `unlistenDataChanged?.then(fn => fn())` onDestroy shape Stage 2
 *  already established — callers don't need to know this now covers two
 *  registrations instead of one. */
export function subscribeDataChanged(
  handler: (payload: DataChangedPayload) => void,
): Promise<UnlistenFn> {
  const domHandler = (e: Event) => {
    const detail = (e as CustomEvent<DataChangedPayload>).detail;
    if (detail) handler(detail);
  };
  if (typeof window !== 'undefined') {
    window.addEventListener(EVENT_DATA_CHANGED, domHandler);
  }
  return listen<DataChangedPayload>(EVENT_DATA_CHANGED, (event) => {
    handler(event.payload);
  }).then((unlistenTauri): UnlistenFn => () => {
    unlistenTauri();
    if (typeof window !== 'undefined') {
      window.removeEventListener(EVENT_DATA_CHANGED, domHandler);
    }
  });
}
