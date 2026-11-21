/**
 * ADR-0063 Stage 3+4 (#2522) — Dual-provider concurrent stream harness.
 *
 * New test infrastructure. Prior to this stage the suite had no test that
 * exercised two concurrent `startStream()` calls on different threads —
 * "concurrency work the suite has never had" per the Stage 3+4 scope audit
 * (2026-08-17-adr0063-phase2-scope.md).
 *
 * History (Task 0 → Task 4 of the build): this file was first written
 * against the PRE-demolition `StreamManager` (singular `streamId`/
 * `callbacks`/etc. fields) specifically to prove the harness catches the
 * bug — starting thread-B's stream tore down thread-A's in-flight stream
 * via the old unconditional `this.cancel()` at the top of `startStream()`.
 * That run showed 3 of 4 cases red, confirming the harness was sound.
 *
 * Post-demolition (current state of this file): `streamManager.ts` is now
 * `Map<threadId, StreamState>`-backed, so every assertion below expects the
 * FIXED behavior — thread-A survives thread-B starting, both threads
 * receive their own events independently, and a per-thread
 * `cancel(threadId)` only affects the targeted thread.
 *
 * The backend counterpart to this harness is
 * `src-tauri/src/ai/runtime_resolution.rs::tests::test_two_concurrent_runtimes_resolve_independently_no_shared_state`.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

// Capture the handler registered for 'ai-stream-chunk' so tests can
// synthesize backend events without a real Tauri runtime / webview.
let capturedHandler: ((event: { payload: any }) => void) | null = null;

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((eventName: string, handler: (event: { payload: any }) => void) => {
    if (eventName === 'ai-stream-chunk') {
      capturedHandler = handler;
    }
    // Real `listen()` resolves to an unlisten function.
    return Promise.resolve(() => {});
  }),
}));

import { streamManager } from './streamManager';

function emitChunk(payload: Record<string, any>) {
  if (!capturedHandler) {
    throw new Error('streamManager.init() was not called before emitChunk() — no handler captured');
  }
  capturedHandler({ payload });
}

describe('Dual-provider concurrent streams (#2522 Stage 3+4 harness)', () => {
  beforeEach(async () => {
    await streamManager.init();
    // Clean slate between tests. Pre-demolition `cancel()` is global (no
    // threadId param exists yet); Task 4 updates this call site once the
    // per-thread `cancel(threadId)` signature lands, and adds an explicit
    // cancel-all variant if one is still needed for teardown.
    streamManager.cancel();
  });

  it('thread-A survives thread-B starting a new stream, and both receive their own events independently', () => {
    const threadAChunks: string[] = [];
    const threadBChunks: string[] = [];

    streamManager.startStream('stream-a', 'thread-a', null, {
      onChunk: (text) => threadAChunks.push(text),
      onDone: () => {},
      onError: () => {},
    });

    // thread-A receives a chunk before thread-B starts.
    emitChunk({ id: 'stream-a', text: 'hello from A' });
    expect(threadAChunks).toEqual(['hello from A']);

    // Starting thread-B's stream must NOT tear down thread-A's — this is
    // the exact bug D7 items 2+3 fix. Pre-demolition, StreamManager's
    // unconditional `this.cancel()` at the top of `startStream()` clobbers
    // thread-A here; post-demolition, thread-A's entry in the
    // `Map<threadId, StreamState>` is untouched by a different thread's
    // `startStream()` call.
    streamManager.startStream('stream-b', 'thread-b', null, {
      onChunk: (text) => threadBChunks.push(text),
      onDone: () => {},
      onError: () => {},
    });

    // Thread-A must still be tracked as actively streaming.
    expect(streamManager.isStreamingThread('thread-a')).toBe(true);
    expect(streamManager.isStreamingThread('thread-b')).toBe(true);

    // A further chunk tagged with stream-a's id must still reach thread-A's
    // callbacks — event dispatch matches per-stream id against the map, not
    // a single stored `this.streamId`.
    emitChunk({ id: 'stream-a', text: ' — still alive' });
    expect(threadAChunks).toEqual(['hello from A', 'hello from A — still alive']);

    // Thread-B receives its own chunk independently, with no cross-talk.
    emitChunk({ id: 'stream-b', text: 'hello from B' });
    expect(threadBChunks).toEqual(['hello from B']);
    expect(threadAChunks).toEqual(['hello from A', 'hello from A — still alive']);
  });

  it('cancel(threadId) only cancels the targeted thread, not others', () => {
    const threadAChunks: string[] = [];
    const threadBChunks: string[] = [];

    streamManager.startStream('stream-a', 'thread-a', null, {
      onChunk: (text) => threadAChunks.push(text),
      onDone: () => {},
      onError: () => {},
    });
    streamManager.startStream('stream-b', 'thread-b', null, {
      onChunk: (text) => threadBChunks.push(text),
      onDone: () => {},
      onError: () => {},
    });

    expect(streamManager.isStreamingThread('thread-a')).toBe(true);
    expect(streamManager.isStreamingThread('thread-b')).toBe(true);

    streamManager.cancel('thread-a');

    expect(streamManager.isStreamingThread('thread-a')).toBe(false);
    expect(streamManager.isStreamingThread('thread-b')).toBe(true);

    // Cancelled thread's events are dropped; the surviving thread's are not.
    emitChunk({ id: 'stream-a', text: 'should be dropped' });
    expect(threadAChunks).toEqual([]);

    emitChunk({ id: 'stream-b', text: 'still receiving' });
    expect(threadBChunks).toEqual(['still receiving']);
  });

  it('starting a second stream for the SAME threadId cancels only that thread\'s prior stream', () => {
    const firstChunks: string[] = [];
    const secondChunks: string[] = [];

    streamManager.startStream('stream-1', 'thread-a', null, {
      onChunk: (text) => firstChunks.push(text),
      onDone: () => {},
      onError: () => {},
    });
    streamManager.startStream('stream-2', 'thread-a', null, {
      onChunk: (text) => secondChunks.push(text),
      onDone: () => {},
      onError: () => {},
    });

    // The old stream-1 for thread-a must no longer deliver events…
    emitChunk({ id: 'stream-1', text: 'stale' });
    expect(firstChunks).toEqual([]);

    // …but the new stream-2 for the same thread does.
    emitChunk({ id: 'stream-2', text: 'fresh' });
    expect(secondChunks).toEqual(['fresh']);
  });

  it('reattachCallbacks(threadId, callbacks) updates only that thread\'s callbacks', () => {
    const threadAChunksOld: string[] = [];
    const threadAChunksNew: string[] = [];
    const threadBChunks: string[] = [];

    streamManager.startStream('stream-a', 'thread-a', null, {
      onChunk: (text) => threadAChunksOld.push(text),
      onDone: () => {},
      onError: () => {},
    });
    streamManager.startStream('stream-b', 'thread-b', null, {
      onChunk: (text) => threadBChunks.push(text),
      onDone: () => {},
      onError: () => {},
    });

    streamManager.reattachCallbacks('thread-a', {
      onChunk: (text) => threadAChunksNew.push(text),
      onDone: () => {},
      onError: () => {},
    });

    emitChunk({ id: 'stream-a', text: 'routed to new callbacks' });
    emitChunk({ id: 'stream-b', text: 'thread-b untouched' });

    expect(threadAChunksOld).toEqual([]);
    expect(threadAChunksNew).toEqual(['routed to new callbacks']);
    expect(threadBChunks).toEqual(['thread-b untouched']);
  });
});
