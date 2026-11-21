/**
 * refreshEvent.test.ts — #2972 stage 3 (`docs/spec/data-refresh.md` §34),
 * specifically §34.10's Stage 3 acceptance criteria exercised at the
 * shared-helper level (every per-screen component test exercises these
 * same helpers again through a real component; this file is the one place
 * the helpers themselves are tested directly, in isolation).
 *
 * Covers:
 *  - §34.4 echo suppression, BOTH halves in one test per §34.10's explicit
 *    requirement ("a test proving only 'doesn't refresh on its own save'
 *    without also proving 'still refreshes on an independent save to the
 *    same note' would pass a broken implementation that simply stopped
 *    listening").
 *  - §34.1's "identical shape, both senders": a mutation fired through
 *    BOTH channels (the app DOM CustomEvent via dispatchDataChanged, and a
 *    simulated backend Tauri emit via simulateTauriEvent) must produce
 *    structurally identical payloads — same keys, same types — to the one
 *    shared `subscribeDataChanged` handler.
 *  - §34.1's "both channels, one shared helper" (item 4): subscribeDataChanged
 *    fires its handler for an event on EITHER channel.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { simulateTauriEvent } from '../../vitest-setup';
import {
  EVENT_DATA_CHANGED,
  ENTITY_NOTE,
  ENTITY_CATEGORY,
  REASON_EDITED,
  REASON_CREATED,
  subscribeDataChanged,
  dispatchDataChanged,
  makeOriginTag,
  registerOwnOriginTag,
  isOwnOriginTag,
  _clearOwnOriginTagsForTest,
  type DataChangedPayload,
} from './refreshEvent';

const win = globalThis.window as any;
win.addEventListener = globalThis.addEventListener.bind(globalThis);
win.removeEventListener = globalThis.removeEventListener.bind(globalThis);
win.dispatchEvent = globalThis.dispatchEvent.bind(globalThis);

beforeEach(() => {
  _clearOwnOriginTagsForTest();
});

describe('makeOriginTag/registerOwnOriginTag/isOwnOriginTag (§34.4)', () => {
  it('two calls to makeOriginTag() never collide', () => {
    const a = makeOriginTag();
    const b = makeOriginTag();
    expect(a).not.toEqual(b);
  });

  it('BOTH halves of echo suppression in one test: a registered tag reads as own, an independent tag for the same note does not (§34.10)', () => {
    const ownTag = makeOriginTag();
    registerOwnOriginTag(ownTag);

    // Half 1: the save call's own echo, carrying the exact tag it minted
    // and registered, is recognized as its own.
    expect(isOwnOriginTag(ownTag)).toBe(true);

    // Half 2: a SECOND, independent event for the very same entity — e.g.
    // an agent/background edit arriving moments later — carries a
    // DIFFERENT tag (§34.4: "scoped to the save call, not the entity") and
    // must NOT be mistaken for the first save's own echo. A suppression
    // bug broad enough to key off "this note" rather than "this save
    // call" would make this assertion fail (it would also report true).
    const independentTag = makeOriginTag();
    expect(isOwnOriginTag(independentTag)).toBe(false);
  });

  it('an unregistered tag is never mistaken for an own tag', () => {
    expect(isOwnOriginTag(makeOriginTag())).toBe(false);
  });
});

describe('subscribeDataChanged — both channels, one shared helper (item 4)', () => {
  it('fires the handler for an event on the APP (DOM CustomEvent) channel', async () => {
    const handler = vi.fn();
    const unlisten = await subscribeDataChanged(handler);
    try {
      dispatchDataChanged('note-app-channel', ENTITY_NOTE, REASON_EDITED, 'tag-app');
      expect(handler).toHaveBeenCalledTimes(1);
      expect(handler).toHaveBeenCalledWith({
        entity_id: 'note-app-channel',
        entity_type: ENTITY_NOTE,
        reason: REASON_EDITED,
        origin_tag: 'tag-app',
      });
    } finally {
      unlisten();
    }
  });

  it('fires the handler for an event on the BACKEND (Tauri emit) channel', async () => {
    const handler = vi.fn();
    const unlisten = await subscribeDataChanged(handler);
    try {
      simulateTauriEvent(EVENT_DATA_CHANGED, {
        entity_id: 'note-backend-channel',
        entity_type: ENTITY_NOTE,
        reason: REASON_CREATED,
        origin_tag: 'tag-backend',
      });
      expect(handler).toHaveBeenCalledTimes(1);
      expect(handler).toHaveBeenCalledWith({
        entity_id: 'note-backend-channel',
        entity_type: ENTITY_NOTE,
        reason: REASON_CREATED,
        origin_tag: 'tag-backend',
      });
    } finally {
      unlisten();
    }
  });

  it('a single subscription hears BOTH channels without a second, hand-written registration', async () => {
    const seen: DataChangedPayload[] = [];
    const unlisten = await subscribeDataChanged((payload) => seen.push(payload));
    try {
      dispatchDataChanged('note-x', ENTITY_NOTE, REASON_EDITED, 'tag-x');
      simulateTauriEvent(EVENT_DATA_CHANGED, {
        entity_id: 'note-y',
        entity_type: ENTITY_NOTE,
        reason: REASON_CREATED,
        origin_tag: 'tag-y',
      });
      expect(seen).toHaveLength(2);
      expect(seen.map((p) => p.entity_id)).toEqual(['note-x', 'note-y']);
    } finally {
      unlisten();
    }
  });

  it('unlisten() stops both registrations', async () => {
    const handler = vi.fn();
    const unlisten = await subscribeDataChanged(handler);
    unlisten();
    dispatchDataChanged('note-after-unlisten', ENTITY_NOTE, REASON_EDITED);
    simulateTauriEvent(EVENT_DATA_CHANGED, {
      entity_id: 'note-after-unlisten-2',
      entity_type: ENTITY_NOTE,
      reason: REASON_EDITED,
      origin_tag: 'whatever',
    });
    expect(handler).not.toHaveBeenCalled();
  });
});

describe('identical shape, both senders (§34.1)', () => {
  it('a mutation fired through the app channel and one fired through the backend channel produce structurally identical payloads — same keys, same types', async () => {
    const captured: DataChangedPayload[] = [];
    const unlisten = await subscribeDataChanged((payload) => captured.push(payload));
    try {
      // App sender (the frontend's own shared helper).
      dispatchDataChanged('note-shape-check', ENTITY_NOTE, REASON_EDITED, makeOriginTag());
      // Backend sender — this is exactly the shape `emit_data_changed`
      // (src-tauri/src/refresh_event.rs) builds and `app.emit()`s; backend
      // mints its own fresh UUID per call (see that module's own test,
      // `test_emit_data_changed_fires_exactly_once_with_correct_payload`),
      // mirrored here via simulateTauriEvent since this is a frontend-only
      // test file.
      simulateTauriEvent(EVENT_DATA_CHANGED, {
        entity_id: 'note-shape-check',
        entity_type: ENTITY_NOTE,
        reason: REASON_EDITED,
        origin_tag: crypto.randomUUID(),
      });

      expect(captured).toHaveLength(2);
      const [appPayload, backendPayload] = captured;

      // Same keys.
      expect(Object.keys(appPayload).sort()).toEqual(Object.keys(backendPayload).sort());
      expect(Object.keys(appPayload).sort()).toEqual(
        ['entity_id', 'entity_type', 'origin_tag', 'reason'].sort(),
      );

      // Same types, field by field.
      for (const key of Object.keys(appPayload) as (keyof DataChangedPayload)[]) {
        expect(typeof appPayload[key]).toBe(typeof backendPayload[key]);
        expect(typeof appPayload[key]).toBe('string');
      }
    } finally {
      unlisten();
    }
  });

  // #3018: category is a fifth entity_type — this identical-shape proof
  // must hold for it too, not just note/sheet/folder/project. entity_id
  // for a category is its name (a string), same wire type as every other
  // entity_type, so the shape assertion below is unchanged in substance;
  // this test exists so a future entity_type addition is forced to extend
  // THIS test too, not just add a new one that happens to pass.
  it('the same identical-shape guarantee holds for entity_type: "category" (#3018)', async () => {
    const captured: DataChangedPayload[] = [];
    const unlisten = await subscribeDataChanged((payload) => captured.push(payload));
    try {
      dispatchDataChanged('Work', ENTITY_CATEGORY, REASON_EDITED, makeOriginTag());
      simulateTauriEvent(EVENT_DATA_CHANGED, {
        entity_id: 'Work',
        entity_type: ENTITY_CATEGORY,
        reason: REASON_EDITED,
        origin_tag: crypto.randomUUID(),
      });

      expect(captured).toHaveLength(2);
      const [appPayload, backendPayload] = captured;

      expect(Object.keys(appPayload).sort()).toEqual(Object.keys(backendPayload).sort());
      expect(Object.keys(appPayload).sort()).toEqual(
        ['entity_id', 'entity_type', 'origin_tag', 'reason'].sort(),
      );
      expect(appPayload.entity_type).toBe('category');
      expect(backendPayload.entity_type).toBe('category');

      for (const key of Object.keys(appPayload) as (keyof DataChangedPayload)[]) {
        expect(typeof appPayload[key]).toBe(typeof backendPayload[key]);
        expect(typeof appPayload[key]).toBe('string');
      }
    } finally {
      unlisten();
    }
  });
});
