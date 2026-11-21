/**
 * refreshEvent.categorySpecialChars.test.ts — #2972 Stage 4, Reed's test
 * gap: "Category names with quotes, emoji, slashes and long strings —
 * these are user-controlled text used as `entity_id`; prove they survive
 * the wire and match on the listener side."
 *
 * `entity_id` for `category` is the category name itself (§34.1: "there
 * is no other stable identifier — `category_colors` has no surrogate id
 * column"), and a category name is free-form user text — Settings.svelte
 * imposes no character restriction on `newCategoryName` beyond a
 * non-empty trim. Every existing category test (`api.categoryRefresh.
 * test.ts`, `Sidebar.categoryRefresh.test.ts`, `ProjectsView.
 * categoryRefresh.test.ts`, `Settings.categoryRefresh.test.ts`) uses a
 * plain-ASCII name like `'Office'` or `'Work'` — none of them prove a
 * name containing a double quote (breaks naive string-concatenation
 * payloads), an emoji (multi-byte UTF-16 surrogate pair — a naive
 * length/truncation bug would slice through it), a forward slash (looks
 * like a path separator to some codepaths), or a long string (buffer/
 * truncation limits) survives the actual wire: `dispatchDataChanged()`'s
 * `CustomEvent` serialization, AND the Tauri backend-channel JSON
 * round-trip `simulateTauriEvent` mimics (`JSON.stringify`/`JSON.parse`,
 * the same encoding `emit_data_changed`'s `serde_json::json!` macro and
 * the real `app.emit()` perform).
 */

import { describe, it, expect, afterEach, vi } from 'vitest';
import { simulateTauriEvent } from '../../vitest-setup';
import {
  EVENT_DATA_CHANGED,
  ENTITY_CATEGORY,
  REASON_EDITED,
  dispatchDataChanged,
  subscribeDataChanged,
  type DataChangedPayload,
} from './refreshEvent';

const win = globalThis.window as any;
if (typeof win.addEventListener !== 'function') {
  win.addEventListener = globalThis.addEventListener?.bind(globalThis) ?? (() => {});
}
if (typeof win.removeEventListener !== 'function') {
  win.removeEventListener = globalThis.removeEventListener?.bind(globalThis) ?? (() => {});
}
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

const SPECIAL_NAMES = [
  { label: 'double quotes', value: 'Say "Hello" Category' },
  { label: 'single quote / apostrophe', value: "Matt's Category" },
  { label: 'emoji (multi-byte UTF-16 surrogate pair)', value: '🎉 Launch 🚀 Category 🔥' },
  { label: 'forward slashes (path-like)', value: 'Work/Personal/Side-Project' },
  { label: 'backslash + mixed quotes', value: 'C:\\Projects\\"Pane"\'s Category' },
  { label: 'long string (500 chars)', value: 'X'.repeat(500) },
  { label: 'newline + tab embedded', value: 'Line One\nLine\tTwo' },
  { label: 'empty-ish but non-empty (whitespace-padded)', value: '  Padded Category  ' },
];

let unlisten: (() => void) | null = null;

afterEach(async () => {
  if (unlisten) {
    unlisten();
    unlisten = null;
  }
  vi.restoreAllMocks();
});

describe('category entity_id with special characters — app CustomEvent channel (#2972 stage 4, Reed\'s gap)', () => {
  for (const { label, value } of SPECIAL_NAMES) {
    it(`survives dispatchDataChanged -> subscribeDataChanged unmangled: ${label}`, async () => {
      const received: DataChangedPayload[] = [];
      unlisten = null;
      const unlistenPromise = subscribeDataChanged((payload) => {
        received.push(payload);
      });
      unlisten = await unlistenPromise.then((fn) => fn);

      dispatchDataChanged(value, ENTITY_CATEGORY, REASON_EDITED, 'special-char-origin-tag');

      expect(received.length).toBe(1);
      expect(received[0].entity_id).toBe(value);
      expect(received[0].entity_id.length).toBe(value.length);
      expect(received[0].entity_type).toBe(ENTITY_CATEGORY);
    });
  }
});

describe('category entity_id with special characters — backend Tauri channel JSON round-trip (#2972 stage 4, Reed\'s gap)', () => {
  for (const { label, value } of SPECIAL_NAMES) {
    it(`survives a JSON.stringify/parse round-trip (mirrors serde_json + app.emit()) unmangled: ${label}`, async () => {
      const received: DataChangedPayload[] = [];
      const unlistenPromise = subscribeDataChanged((payload) => {
        received.push(payload);
      });
      unlisten = await unlistenPromise.then((fn) => fn);

      // Mimics exactly what crosses the real Tauri IPC boundary: the
      // backend's serde_json::json!({...}) macro serializes entity_id to a
      // JSON string (escaping quotes/backslashes/control chars per the
      // JSON spec, UTF-8-encoding the emoji's codepoints), and the
      // frontend's listen() callback receives the ALREADY-DESERIALIZED
      // JS value — so round-tripping the raw value through
      // JSON.stringify/JSON.parse here before handing it to
      // simulateTauriEvent reproduces the same encode/decode the real
      // wire performs, not just a same-process object reference.
      const wireRoundTripped = JSON.parse(JSON.stringify({ entity_id: value })).entity_id;

      simulateTauriEvent(EVENT_DATA_CHANGED, {
        entity_id: wireRoundTripped,
        entity_type: ENTITY_CATEGORY,
        reason: REASON_EDITED,
        origin_tag: 'special-char-backend-origin-tag',
      });

      expect(received.length).toBe(1);
      expect(received[0].entity_id).toBe(value);
      expect(received[0].entity_id.length).toBe(value.length);
    });
  }

  it('a category event with an emoji name matches on the listener side by exact string equality, not just truthiness', async () => {
    const CATEGORY_NAME = '🎨 Design Review 🖌️';
    let matchedExactly = false;
    const unlistenPromise = subscribeDataChanged((payload) => {
      if (payload.entity_type === ENTITY_CATEGORY && payload.entity_id === CATEGORY_NAME) {
        matchedExactly = true;
      }
    });
    unlisten = await unlistenPromise.then((fn) => fn);

    const wireRoundTripped = JSON.parse(JSON.stringify({ entity_id: CATEGORY_NAME })).entity_id;
    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: wireRoundTripped,
      entity_type: ENTITY_CATEGORY,
      reason: REASON_EDITED,
      origin_tag: 'emoji-match-origin-tag',
    });

    expect(matchedExactly).toBe(true);
  });
});
