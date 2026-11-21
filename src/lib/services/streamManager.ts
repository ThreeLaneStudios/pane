import { listen } from '@tauri-apps/api/event';

/**
 * §22C.12.3 — Locked copy for structured gateway error categories.
 * If `errorCode` is set (OC topology), returns category-specific message.
 * Falls back to regex heuristic on `errText` for non-OC providers.
 */
export function getGatewayErrorMessage(errorCode: string | undefined, errText: string): string {
  switch (errorCode) {
    case 'gateway_unreachable':
      return "Pane can't reach your gateway. Make sure the gateway is running on your other machine, then try again.";
    case 'gateway_auth_failed':
      return 'Your gateway login is no longer valid. Click Re-pair to reconnect.';
    case 'gateway_cert_mismatch':
      return "Your gateway's fingerprint has changed. This is normal if you reinstalled or moved it \u2014 but if you didn't, stop and check before reconnecting.";
    case 'gateway_connection_failed':
      return "Pane couldn't connect to your gateway. Check that both machines are online, then try again.";
    case 'gateway_provider_error':
      // ADR-0067 D4 / spec §4A.2.5: a gateway 4xx is OC reporting a real
      // provider-side problem (e.g. "Multiple agents are configured...") —
      // OC's own message text is the correct, actionable copy here, not a
      // locked generic string. Never retried on the backend, so this is
      // always the terminal message for the attempt.
      return errText;
    default: {
      // Fallback for non-OC providers or legacy errors without a structured code
      const isGateway = /gateway|connect|ECONNREFUSED|network|timeout|fetch|Request failed/i.test(errText);
      return isGateway
        ? "Couldn't reach the AI. Check your connection and try again."
        : `Something went wrong: ${errText}`;
    }
  }
}

interface StreamCallbacks {
  onChunk: (text: string) => void;
  onDone: (data: { fullText: string; model?: string; agentId?: string; agentName?: string; toolResults: string[]; navigate: any; toolCalls: any[]; image_attachment_ids?: string[] }) => void;
  /** Called when the stream terminates with an error.
   *
   * `message`   — display-friendly error string.
   * `fullText`  — text accumulated before the error (may be empty).
   * `toolCalls` — tool calls accumulated before the error (may be empty).
   * `toolResults` — tool results accumulated before the error (may be empty).
   * `errorCode` — §22C.12.3 structured error code (e.g. `gateway_unreachable`); may be undefined for non-gateway errors.
   *
   * Fix #1358: callers MUST persist `fullText` to the DB when non-empty.
   */
  onError: (message: string, partial: { fullText: string; toolCalls: any[]; toolResults: string[]; image_attachment_ids?: string[] }, errorCode?: string) => void;
  /**
   * Called when a retry is about to begin after a failed attempt (§22C.12.4).
   * Frontend should transition to `Reconnecting` state and disable Send.
   *
   * `retryAttempt` — 1-indexed attempt number (1 = first retry after initial failure).
   */
  onReconnecting?: (retryAttempt: number) => void;
}

interface PendingResult {
  threadId: string;
  savedToDb: boolean;
}

interface CapturedContext {
  threadId: string;
  projectId: string | null;
  streamId: string;
}

/**
 * ADR-0063 Stage 3+4 (#2522) — per-thread stream state.
 *
 * Replaces the old singular `streamId`/`callbacks`/`buffer`/`streamThreadId`/
 * `pendingResult`/`capturedContext` fields with one entry per thread, keyed
 * by threadId in `StreamManager.streams`. This is what makes two concurrent
 * streams (e.g. two agents on two different runtimes) survive each other —
 * starting thread-B's stream no longer tears down thread-A's, because each
 * thread owns its own entry instead of every thread sharing one set of
 * fields.
 *
 * `streamId` is nullable and doubles as the per-thread "is this stream
 * currently live" flag — mirrors the old code's use of the singular
 * `streamId: string | null` for the same purpose (`isStreamingThread` used
 * to check `this.streamId !== null`; now it checks the per-entry
 * `streamId !== null`). It is nulled on completion (done/error) or explicit
 * cancel, but `capturedContext` and `pendingResult` deliberately are NOT
 * cleared on natural completion — this matches the old singleton's exact
 * behavior (its done/error branches never touched `capturedContext` or
 * `pendingResult`; only `cancel()` cleared `capturedContext`, and nothing
 * but `startStream()`/`consumePendingResult()` ever cleared `pendingResult`).
 * ChatSidebar relies on this: it reads `getCapturedContext()` and calls
 * `setPendingResult()` from INSIDE the onDone callback, after the stream has
 * already completed — those calls would silently no-op if this entry were
 * deleted or wiped at done-time.
 */
interface StreamState {
  streamId: string | null;
  callbacks: StreamCallbacks | null;
  buffer: string;
  pendingResult: PendingResult | null;
  capturedContext: CapturedContext | null;
  /**
   * Echo suppression (#2410, §30.11.4 PR 4) — wall-clock `Date.now()` timestamp of
   * this thread's most recent `done:true` event (set for BOTH the success and
   * done+error branches below — once a dispatch reaches `done`, no more chunks are
   * expected for it, so either outcome starts the same post-dispatch grace window).
   * `null` until this thread's first stream ever completes. Per-thread (not a
   * single component-level field) so switching away from an actively-streaming
   * thread to a different, idle thread never leaks that other thread's grace
   * window onto the one the user is now viewing — the exact stale-state hazard a
   * single top-level `isStreaming`/`lastDispatchDoneAt` boolean would have had.
   */
  lastDoneAt: number | null;
}

function emptyStreamState(): StreamState {
  return { streamId: null, callbacks: null, buffer: '', pendingResult: null, capturedContext: null, lastDoneAt: null };
}

class StreamManager {
  /** ADR-0063 Stage 3+4 (#2522): Map<threadId, StreamState> — replaces every
   * former singular field. Each thread's stream state lives entirely in its
   * own map entry, so concurrent streams on different threads never observe
   * or clobber each other. */
  private streams: Map<string, StreamState> = new Map();
  private initialized = false;
  private deadStreams = new Set<string>(); // FIX #261: Track completed streams

  /** Find the [threadId, StreamState] entry whose LIVE streamId matches an
   * incoming event's `data.id`. Replaces the old single-field comparison
   * (`data.id !== this.streamId`) — every event is now matched against the
   * map instead of one stored id, since multiple streams can be live at
   * once. Entries whose stream already completed (`streamId === null`)
   * never match, same as the old code ignoring events once `this.streamId`
   * had been nulled. */
  private findEntryByStreamId(streamId: string): [string, StreamState] | undefined {
    for (const entry of this.streams.entries()) {
      if (entry[1].streamId === streamId) return entry;
    }
    return undefined;
  }

  private getOrCreate(threadId: string): StreamState {
    let state = this.streams.get(threadId);
    if (!state) {
      state = emptyStreamState();
      this.streams.set(threadId, state);
    }
    return state;
  }

  private markDead(streamId: string) {
    this.deadStreams.add(streamId);
    // Keep last 100 dead stream IDs to prevent unbounded growth
    if (this.deadStreams.size > 100) {
      const first = this.deadStreams.values().next().value;
      if (first) this.deadStreams.delete(first);
    }
  }

  async init() {
    if (this.initialized) return;
    this.initialized = true;
    console.log('[STREAM-MGR] Initialized');

    await listen('ai-stream-chunk', (event: any) => {
      const data = event.payload;

      // FIX #261: Silently drop events from dead streams (no logging)
      if (data.id && this.deadStreams.has(data.id)) {
        return;
      }

      const entry = data.id ? this.findEntryByStreamId(data.id) : undefined;
      if (!entry) {
        console.debug('[STREAM-MGR] Ignoring event - no active thread matches stream id:', data.id);
        return;
      }
      const [threadId, state] = entry;

      console.log('[STREAM-MGR] Event received for thread:', threadId, 'done:', !!data.done, 'text:', !!data.text, 'error:', !!data.error, 'reconnecting:', !!data.reconnecting);

      // §22C.12.4 — Reconnecting event: backend is retrying, frontend transitions to Reconnecting state
      if (data.reconnecting) {
        const retryAttempt = typeof data.retry_attempt === 'number' ? data.retry_attempt : 1;
        console.log('[STREAM-MGR] Reconnecting event, retry_attempt:', retryAttempt, 'thread:', threadId);
        if (state.callbacks?.onReconnecting) {
          state.callbacks.onReconnecting(retryAttempt);
        }
        return;
      }

      if (data.done) {
        const cb = state.callbacks;
        const completedStreamId = data.id; // FIX #261: Capture stream ID before clearing
        // Mark this thread's stream as no-longer-live. Deliberately do NOT
        // touch capturedContext/pendingResult here — see StreamState doc.
        state.streamId = null;
        state.callbacks = null;
        state.buffer = '';
        // Echo suppression (#2410, §30.11.4 PR 4): stamp the post-dispatch grace
        // window start for THIS thread, on both the success and done+error
        // branches below — either way, no more transcript segments for this
        // dispatch are expected once `done:true` fires, so both outcomes are the
        // right moment to start the poll's suppression clock.
        state.lastDoneAt = Date.now();

        // FIX #261: Mark stream as dead to ignore future events
        if (completedStreamId) {
          this.markDead(completedStreamId);
        }

        // Check for error FIRST — Rust sends {done: true, error: "..."} on failures
        if (data.error) {
          const errText = typeof data.error === 'string' ? data.error : 'Unknown error';
          console.log('[STREAM-MGR] Stream error (done+error) for thread:', threadId, errText, 'error_code:', data.error_code);
          // §22C.12.3 — structured error_code drives locked copy; regex fallback for non-OC providers
          const errorCode: string | undefined = typeof data.error_code === 'string' ? data.error_code : undefined;
          const displayError = getGatewayErrorMessage(errorCode, errText);
          // Fix #1358: pass accumulated partial text / tool state through onError so
          // ChatSidebar can persist whatever was streamed before the failure.
          const partial = {
            fullText: typeof data.fullText === 'string' ? data.fullText : '',
            toolCalls: Array.isArray(data.toolCalls) ? data.toolCalls : [],
            toolResults: Array.isArray(data.tool_results) ? data.tool_results : [],
            // #3161 PR 1 (§39.5): the done+error emit site carries this field
            // too (chat.rs's provider-error "done": true block) — a turn
            // that generated an image before failing still gets the
            // attachment persisted on whatever partial text survives.
            image_attachment_ids: Array.isArray(data.image_attachment_ids) ? data.image_attachment_ids : [],
          };
          if (cb?.onError) cb.onError(displayError, partial, errorCode);
        } else {
          const fullText = data.fullText || '';
          console.log('[STREAM-MGR] Stream done for thread:', threadId, 'calling onDone');
          if (cb?.onDone) {
            // #2604 RC-B: thread the backend-resolved dispatching agent through so
            // the live message can be labeled correctly without waiting on a DB
            // reload (see ChatSidebar.svelte's onDone — sets agent_id/agent_name
            // on the live message object, mirroring how `model` is already set).
            cb.onDone({
              fullText,
              model: data.model || undefined,
              agentId: typeof data.agent_id === 'string' ? data.agent_id : undefined,
              agentName: typeof data.agent_name === 'string' ? data.agent_name : undefined,
              toolResults: data.tool_results || [],
              navigate: data.navigate || null,
              toolCalls: data.toolCalls || [],
              // #3161 PR 1 (§39.5): the one accumulator this turn's
              // generate_image calls wrote into, carried by chat.rs's
              // own "done": true payload — forwarded verbatim.
              image_attachment_ids: Array.isArray(data.image_attachment_ids) ? data.image_attachment_ids : [],
            });
          }
        }
      } else if (data.text) {
        state.buffer += data.text;

        const trimmed = state.buffer.trim();
        const looksLikeToolCall = trimmed.includes('```json') ||
          (trimmed.startsWith('{') && trimmed.includes('"tool"')) ||
          /"tool"\s*:/.test(trimmed);

        if (!looksLikeToolCall && state.callbacks?.onChunk) {
          state.callbacks.onChunk(state.buffer);
        }
      } else if (data.error) {
        const cb = state.callbacks;
        const errorStreamId = data.id; // FIX #261: Capture stream ID before clearing
        state.streamId = null;
        state.callbacks = null;
        state.buffer = '';

        // FIX #261: Mark stream as dead to ignore future events
        if (errorStreamId) {
          this.markDead(errorStreamId);
        }

        const errText = typeof data.error === 'string' ? data.error : 'Unknown error';
        console.log('[STREAM-MGR] Stream error for thread:', threadId, errText, 'error_code:', data.error_code);
        // §22C.12.3 — structured error_code drives locked copy; regex fallback for non-OC providers
        const errorCode: string | undefined = typeof data.error_code === 'string' ? data.error_code : undefined;
        const displayError = getGatewayErrorMessage(errorCode, errText);
        // Fix #1358: pass partial data (fullText may be non-empty if a mid-stream chunk
        // error arrives on the {done:false, error:"..."} path).
        const partial = {
          fullText: typeof data.fullText === 'string' ? data.fullText : '',
          toolCalls: Array.isArray(data.toolCalls) ? data.toolCalls : [],
          toolResults: Array.isArray(data.tool_results) ? data.tool_results : [],
          // #3161 PR 1: this is the {done:false, error:...} mid-stream path —
          // the backend never attaches image_attachment_ids here (that field
          // only exists on a "done": true payload), so this is always empty.
          // Present for type-shape parity with the done+error branch above.
          image_attachment_ids: Array.isArray(data.image_attachment_ids) ? data.image_attachment_ids : [],
        };
        if (cb?.onError) cb.onError(displayError, partial, errorCode);
      }
    });
  }

  /**
   * Re-attach callbacks for a specific thread's in-flight stream (e.g. after
   * a component was recreated on navigation). ADR-0063 Stage 3+4: takes
   * `threadId` explicitly rather than implicitly operating on "the" stream —
   * there can be more than one live stream now. Only succeeds if that
   * thread's stream is still live (mirrors the old `if (!this.streamId)
   * return false`).
   */
  reattachCallbacks(threadId: string, callbacks: StreamCallbacks) {
    const state = this.streams.get(threadId);
    if (!state || state.streamId === null) {
      console.log('[STREAM-MGR] Cannot reattach - no active stream for thread:', threadId);
      return false;
    }
    console.log('[STREAM-MGR] Reattaching callbacks for stream:', state.streamId, 'thread:', threadId);
    state.callbacks = callbacks;
    return true;
  }

  /**
   * Start tracking a new stream for `threadId`. ADR-0063 Stage 3+4: only
   * cancels a PRIOR stream for the SAME threadId (superseding it), not every
   * other thread's stream — this is the fix for the bug the dual-provider
   * harness (`streamManagerConcurrency.test.ts`) catches: starting thread-B's
   * stream must not tear down thread-A's.
   */
  startStream(streamId: string, threadId: string, projectId: string | null, callbacks: StreamCallbacks) {
    console.log('[STREAM-MGR] Starting stream:', streamId, 'thread:', threadId, 'projectId:', projectId);
    // Cancel only this thread's own prior stream, if one exists — leaves
    // every other thread's entry in the map untouched. Mirrors the old
    // `startStream()` calling the (then-global) `this.cancel()` first.
    this.cancel(threadId);

    const state = this.getOrCreate(threadId);
    state.streamId = streamId;
    state.buffer = '';
    state.callbacks = callbacks;
    state.pendingResult = null;
    state.capturedContext = { threadId, projectId, streamId };
    console.log('[STREAM-MGR] Captured context:', state.capturedContext);
  }

  /**
   * Cancel a specific thread's stream (clears `streamId`, `callbacks`,
   * `buffer`, `capturedContext` — mirrors the old global `cancel()`, which
   * cleared the same four fields but deliberately left `pendingResult`
   * untouched). With no `threadId`, cancels ALL active streams — the
   * closest equivalent to the old global `cancel()` semantics for callers
   * that genuinely want a full teardown (e.g. app-level reset), but this is
   * no longer what `startStream()` calls implicitly for every new stream.
   */
  cancel(threadId?: string) {
    if (threadId === undefined) {
      console.log('[STREAM-MGR] Cancelling ALL active streams:', Array.from(this.streams.keys()));
      for (const state of this.streams.values()) {
        state.streamId = null;
        state.callbacks = null;
        state.buffer = '';
        state.capturedContext = null;
      }
      return;
    }
    const state = this.streams.get(threadId);
    if (!state) return;
    console.log('[STREAM-MGR] Cancelled stream:', state.streamId, 'thread:', threadId);
    state.streamId = null;
    state.callbacks = null;
    state.buffer = '';
    state.capturedContext = null;
  }

  /** True if ANY thread currently has a live stream. */
  isActive(): boolean {
    for (const state of this.streams.values()) {
      if (state.streamId !== null) return true;
    }
    return false;
  }

  /** True if ANY thread currently has a live stream. Alias of `isActive()`
   * kept for existing call sites. */
  isStreaming(): boolean {
    return this.isActive();
  }

  getCapturedContext(threadId: string): CapturedContext | null {
    return this.streams.get(threadId)?.capturedContext ?? null;
  }

  getBuffer(threadId: string): string {
    return this.streams.get(threadId)?.buffer ?? '';
  }

  setPendingResult(threadId: string) {
    const state = this.getOrCreate(threadId);
    state.pendingResult = { threadId, savedToDb: true };
    console.log('[STREAM-MGR] Pending result stored for thread:', threadId);
  }

  consumePendingResult(threadId: string): boolean {
    const state = this.streams.get(threadId);
    if (state?.pendingResult && state.pendingResult.threadId === threadId && state.pendingResult.savedToDb) {
      console.log('[STREAM-MGR] Consuming pending result for thread:', threadId);
      state.pendingResult = null;
      return true;
    }
    return false;
  }

  /** True if `threadId` currently has a LIVE stream (mirrors the old
   * `this.streamThreadId === threadId && this.streamId !== null`, now
   * scoped per-entry instead of against a single stored thread id). */
  isStreamingThread(threadId: string): boolean {
    const state = this.streams.get(threadId);
    return state !== undefined && state.streamId !== null;
  }

  /**
   * Echo suppression (#2410, §30.11.4 PR 4): `Date.now()` timestamp of this
   * thread's most recent `done:true` event, or `null` if this thread has never
   * had a stream complete. Per-thread — mirrors `isStreamingThread`'s own
   * per-entry scoping, so the poll gate's grace-window check is correctly
   * scoped to whichever thread it's evaluating, not leaked from a different
   * thread's most recent dispatch.
   */
  getLastDoneAt(threadId: string): number | null {
    return this.streams.get(threadId)?.lastDoneAt ?? null;
  }
}

export const streamManager = new StreamManager();
