<script lang="ts">
  import { onMount } from 'svelte';
  import { chatSidebarOpen, activeChatThreadId, activeSessionOrigin, activeProject, activeNote, activeNotebook, projects, jobSessionBackTarget } from '$lib/stores/app';
  import { goToJobSessionBackTarget, shouldShowJobSessionBackButton } from '$lib/services/jobSessionBack';
  import { get } from 'svelte/store';
  import { canvasState } from '$lib/stores/canvas';
  // probe: instrument all $activeNote write sites — investigating #2214 delete-close latch
  const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';
  function logAnSet(caller: string, newVal: unknown): void {
    if (!DEBUG) return;
    const oldId = get(activeNote)?.meta?.id ?? 'null';
    const newId = (newVal as any)?.meta?.id ?? 'null';
    console.log(`[PANE-DEBUG] $activeNote SET in ${caller} | old="${oldId}" new="${newId}"`);
    console.log('[PANE-DEBUG] stack:', new Error().stack?.split('\n').slice(1, 5).join('\n'));
  }
  import type { ChatMessage, ChatThread, Task, TaskStatus, TaskPriority, Session } from '$lib/types';
  import type { CanvasViewType } from '$lib/stores/canvas';
  import * as api from '$lib/api';
  import { shouldCompact, runCompaction, getMemoryContext } from '$lib/chat-memory';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { streamManager } from '$lib/services/streamManager';
  import { navigateTo } from '$lib/services/navigation';
  import { buildStreamErrorContent, hasPartialText } from '$lib/utils/streamErrorContent';
  import { decideNavigationAction } from '$lib/utils/navigationDispatch';
  import ChatPanel from './ChatPanel.svelte';
  import { sessionStore } from '$lib/stores/sessionStore';
  import { checkSessionReadOnly, resolveBoundAgent } from '$lib/services/sessionReadOnlyCheck';
  import { jobAwareEmptyStateHint } from '$lib/scheduledPanel';
  import type { Agent } from '$lib/types';
  import { isVisibleLoadedMessage } from '$lib/services/messageVisibility';

  // Props for Home view customization + layout mode
  interface Props {
    layoutMode?: 'centered' | 'sidebar';
    alwaysOpen?: boolean;
    showClose?: boolean;
    showHeader?: boolean;
    homePlaceholder?: string;
  }

  let {
    layoutMode = 'sidebar',
    alwaysOpen = false,
    showClose = true,
    showHeader = true,
    homePlaceholder = ''
  }: Props = $props();

  // All messages across all threads (not filtered)
  let allMessages: ChatMessage[] = $state([]);
  let isLoading = $state(false);
  let isCompacting = $state(false);
  let memoryIndicator = $state('');
  let connectionError = $state('');
  // #3036 PR2 item 4 — visible error banner for a navigate stream payload
  // that `decideNavigationAction` couldn't resolve (unrecognized view, no
  // target at all, or a project/note id that doesn't resolve). Never
  // silent — see navigationDispatch.ts's doc comment for the bug this
  // replaces.
  let navError: string | null = $state(null);

  // §5.4B: Memory extraction toast state (#641)
  let memoryToast = $state('');
  let memoryToastId = $state('');
  let memoryToastTimer: ReturnType<typeof setTimeout> | null = null;

  let lastCreatedNote: { title: string; slug: string; project: string } | null = $state(null);
  let lastCreatedTask: { title: string; id: string } | null = $state(null);
  let lastUserMessageContent = $state('');

  // Streaming state
  let streamingMessageId: string | null = $state(null);
  let isStreaming = $state(false);
  let streamError: string | null = $state(null);
  let streamRetryCount = $state(0); // FIX #404: Track retry attempts
  const MAX_STREAM_RETRIES = 1;

  // §22C.12.4 — Reconnect state machine (#945)
  // States: 'idle' | 'attempting' | 'reconnecting' | 'failed'
  // - idle:         No active chat request (or last completed successfully).
  // - attempting:   First attempt in flight.
  // - reconnecting: First attempt failed, backoff timer running (Send disabled).
  // - failed:       Budget exhausted or hard-fail; §22C.12.3 error copy surfaces.
  type ReconnectState = 'idle' | 'attempting' | 'reconnecting' | 'failed';
  let reconnectState = $state<ReconnectState>('idle');
  let reconnectAttempt = $state(0); // 1-indexed retry count; 0 = original attempt
  // §22C.12.3 error code emitted by backend for #946 frontend copy rendering
  let gatewayErrorCode = $state<string | undefined>(undefined);

  // Pagination state
  let hasMoreMessages = $state(true);
  let loadingOlder = $state(false);
  let messageOffset = $state(0);

  // Track previous thread to only reload on actual navigation
  let previousThreadId = $state('');

  // Derived: filter messages by current thread
  // This prevents cross-thread message bleed during navigation
  let messages = $derived(
    allMessages.filter(m => m.thread_id === $activeChatThreadId)
  );

  // True when there's an active session with no messages yet — shows fresh-session hint in ChatPanel.
  let isFreshSession = $derived(
    messages.length === 0 && $sessionStore.activeSession !== null && !$sessionStore.loading
  );

  // C19 (#3120 PR 3): job-aware copy for a job session's own empty state,
  // replacing the generic "Fresh session…" hint when the zero-message
  // session on screen IS a job's own session. `null` = not a job session
  // (the common case, every interactive session) or not yet resolved —
  // ChatPanel falls back to its existing generic hint either way, so a
  // slow/failed resolution here degrades to today's behavior, never a
  // broken empty state.
  let jobAwareHint: string | null = $state(null);

  async function resolveJobAwareHint(sessionId: string | undefined): Promise<void> {
    if (!sessionId) {
      jobAwareHint = null;
      return;
    }
    try {
      const jobs = await api.listScheduledJobs();
      const job = jobs.find((j) => j.session_id === sessionId);
      jobAwareHint = job ? jobAwareEmptyStateHint(job) : null;
    } catch (e) {
      console.error('[C19] resolveJobAwareHint failed:', e);
      jobAwareHint = null;
    }
  }

  $effect(() => {
    if (isFreshSession) {
      void resolveJobAwareHint($sessionStore.activeSession?.id);
    } else {
      jobAwareHint = null;
    }
  });
  
  // Canvas navigation listener — registered once, cleaned up on unmount
  let cleanupFunctions: Array<() => void> = [];
  let streamManagerReady = false;

  function stripToolBlocks(text: string): string {
    if (!text) return text;
    let result = text.replace(/```json\s*\{[^`]*?"tool"\s*:[^`]*?\}\s*```/gs, "");
    result = result.replace(/^\s*\{"tool"\s*:.*?\}\s*$/gm, "");
    result = result.replace(/\n{3,}/g, "\n\n").trim();
    return result;
  }

  /**
   * Resolves the display name to use for canned greeting templates (#2604 family, RC-B).
   * Fallback chain: session's BOUND agent (activeSession.agent_id) → default/main agent
   * (get_main_agent) → 'Pane'. Greetings must never speak in the default agent's voice
   * when the session is bound to a different agent — that false identity persists in
   * conversation context until the user challenges it.
   */
  async function resolveGreetingAgentName(): Promise<string> {
    const boundAgentId = $sessionStore.activeSession?.agent_id ?? null;
    if (boundAgentId) {
      try {
        const agents = await api.getAgents();
        const bound = agents.find(a => a.id === boundAgentId);
        if (bound && bound.name && bound.name.trim()) {
          return bound.name.trim();
        }
      } catch (e) {
        console.error('[GREETING] Failed to resolve bound agent, falling back to default:', e);
      }
    }

    const agentResult = await invoke<{ name: string } | null>('get_main_agent').catch(() => null);
    if (agentResult && agentResult.name && agentResult.name.trim()) {
      return agentResult.name.trim();
    }
    // #2794: diagnostic log at the final 'Pane' fallback — names which lookup
    // path failed so a future "greeting says Pane" report can be triaged from
    // the console instead of re-deriving the fallback chain from scratch.
    console.warn('[GREETING] Falling back to "Pane" — bound agent lookup: %s, get_main_agent: %s',
      boundAgentId ? 'miss (agent_id bound but not found or empty name)' : 'skip (no agent_id on session)',
      agentResult ? 'miss (returned but name empty/whitespace)' : 'miss (returned null or threw)'
    );
    return 'Pane';
  }

  // Reactive current project ID
  let currentProjectId = $derived($activeProject?.id);
  let currentProjectName = $derived($activeProject?.name || 'General');
  let isNavigating = $state(false);

  // D22 (#2649 family): Home never mounts SessionDropdownHeader (showHeader
  // is gated on $activeProject !== null, which is always false on Home), so
  // the lock banner + composer-disable state that SessionDropdownHeader
  // normally computes for project-scoped sessions never fires there. Matt's
  // ruling (Option B, surgical): compute the SAME read-only state
  // independently on Home via the shared sessionReadOnlyCheck helper, and
  // pass it into ChatPanel directly — no header/dropdown UI added, the clean
  // Home look stays. See docs/audits/2026-08-22-d22-lock-banner-scope.md.
  let homeReadOnlyState = $state<{ reason: 'agent' | 'provider'; agent: Agent } | null>(null);
  // #2643: Home also never mounts SessionDropdownHeader, which is the ONLY
  // thing that normally populates ChatPanel's bound-agent-name state (via
  // its onBoundAgentChange callback) — so the speaker label fell through to
  // the hardcoded 'Pane' literal on Home until a message's DB-sourced
  // agent_name landed (i.e. until onDone). Same root-cause shape as D22
  // above, same fix pattern: resolve the bound agent's name independently on
  // Home and pass it into ChatPanel as a prop. Unlike homeReadOnlyState, this
  // must resolve UNCONDITIONALLY — checkSessionReadOnly() only returns a
  // value when the session IS locked, so the healthy/common case (which is
  // what's happening during ordinary streaming) would otherwise discard the
  // resolved name. resolveBoundAgent() is a pure, synchronous helper already
  // exported from sessionReadOnlyCheck.ts — call it directly with a fresh
  // getAgents() fetch instead of piggybacking on checkSessionReadOnly's
  // result. See docs/audits/2026-08-22-2641-2643-card-colors-home-header-scope.md §2.4.
  let homeBoundAgentName = $state<string | null>(null);
  // #2604: mirrors SessionDropdownHeader's `agentsLoaded` — true once
  // getAgents() has resolved (success or failure) at least once for the
  // CURRENT thread-open on Home. `homeBoundAgentName === null` is ambiguous
  // between "genuinely unbound" and "haven't checked yet"; this flag
  // disambiguates for ChatPanel's speaker-label fallback (agentsLoadedProp).
  // Reset to false alongside homeBoundAgentName on every thread change below
  // so a session switch re-enters the loading state instead of carrying the
  // previous session's resolved-ness forward.
  let homeAgentsLoaded = $state(false);
  let isHomeSurface = $derived(layoutMode === 'centered' && $activeProject === null);

  // D22/#2643 root-cause fix: on real Home, $sessionStore.activeSession is
  // undefined when this effect first fires — nothing populates the session
  // store on Home (SessionsDrawer.svelte manages its own local session list
  // and never touches sessionStore; only SessionDropdownHeader, a
  // project-surface-only component, calls sessionStore.loadSessions). The
  // thread instead resolves via the NAV event (pane-navigation-complete,
  // navigation.ts) AFTER mount, which sets $activeChatThreadId — but the old
  // effect keyed off $sessionStore.activeSession, so it never re-ran once NAV
  // resolved. Keying off $activeChatThreadId instead means this effect reacts
  // correctly when NAV sets it. The session itself is resolved via
  // sessionStore.getActiveSession('workspace') — the same store action
  // SessionDropdownHeader implicitly relies on via loadSessions, and the same
  // 'workspace' surface constant loadOrCreateThread/navigateTo already use for
  // Home. This also populates $sessionStore.activeSession as a side effect,
  // which fixes isFreshSession and resolveGreetingAgentName's bound-agent
  // lookup above — both read $sessionStore.activeSession and shared the exact
  // same undefined-on-Home root cause.
  //
  // #2794: extracted from the inline $effect body into a standalone,
  // directly-callable function. Root cause of #2794 (onboarding sender label
  // stuck on "Pane"): this resolution previously only ran inside the $effect
  // below, which is keyed on $activeChatThreadId/isHomeSurface. On a fresh
  // install, navigation.ts sets $activeChatThreadId at app boot — BEFORE
  // onboarding has created the first ai_agents row — so this resolves once
  // to homeBoundAgentName=null/homeAgentsLoaded=true and never re-runs,
  // because $activeChatThreadId never changes again across the onboarding
  // flow (see docs/audits/2026-08-31-2794-2783-formal-diagnosis.md). Having
  // this as a callable function lets a 'pane-onboarding-complete' listener
  // (below) re-trigger the exact same resolution once the agent actually
  // exists, without duplicating the logic or touching the effect's own
  // thread-change trigger.
  async function resolveHomeBoundAgent(): Promise<void> {
    const threadId = $activeChatThreadId;
    const onHome = isHomeSurface;
    if (!onHome || !threadId) {
      homeReadOnlyState = null;
      homeBoundAgentName = null;
      // #2604: not on Home / no thread yet — there's nothing to load, so this
      // isn't a "still loading" state. Leaving it true would let a stale
      // resolved name/false leak across an unrelated surface; leaving it
      // false would (correctly) keep ChatPanel in the neutral loading
      // treatment, which is also fine since ChatPanel only consults
      // agentsLoadedProp when showHeader is false AND there's an actual
      // thread to render messages for.
      homeAgentsLoaded = false;
      return;
    }
    // #2604: a new thread just became active on Home — reset to the loading
    // state so ChatPanel doesn't carry the PREVIOUS session's resolved
    // bound-agent name/loaded-flag forward while this one is still in
    // flight (same false-null-vs-not-loaded ambiguity this fix targets,
    // just on session switch instead of session open).
    homeAgentsLoaded = false;
    try {
      const session = await sessionStore.getActiveSession('workspace');
      // Staleness guard: bail if the thread changed again while this
      // resolution was in flight (rapid nav), or if the surface's active
      // session doesn't match the thread id we keyed off (race between NAV
      // setting $activeChatThreadId and the session promote landing).
      if ($activeChatThreadId !== threadId || session.id !== threadId) return;
      checkSessionReadOnly(session).then(state => {
        homeReadOnlyState = state;
      });
      try {
        const allAgents = await api.getAgents();
        const boundAgent = resolveBoundAgent(session, allAgents);
        homeBoundAgentName = boundAgent?.name ?? null;
        homeAgentsLoaded = true;
        // #2604 probe: mirrors SessionDropdownHeader's probe for the Home
        // surface — confirms whether Home hits the same load-order window.
        // #2682: gated behind the existing `DEBUG` (pane-debug localStorage)
        // flag — was previously unconditional, polluting production console
        // output on every Home session's agent-roster load.
        if (DEBUG) {
          console.log('[2604-PROBE] ChatSidebar (Home) agentsLoaded=true (getAgents resolved)', {
            agentCount: allAgents.length,
            threadId,
            boundAgentName: homeBoundAgentName,
            ts: performance.now(),
          });
        }
      } catch (e) {
        console.error('[HOME-AGENT-NAME] Failed to resolve bound agent name:', e);
        homeBoundAgentName = null;
        homeAgentsLoaded = true;
      }
    } catch (e) {
      console.error('[HOME-SESSION] Failed to resolve active session for Home thread:', threadId, e);
      homeReadOnlyState = null;
      homeBoundAgentName = null;
      homeAgentsLoaded = true;
    }
  }

  $effect(() => {
    // Reads $activeChatThreadId/isHomeSurface synchronously so Svelte tracks
    // them as this effect's reactive dependencies — resolveHomeBoundAgent()
    // re-reads the same values itself, so this re-fires the effect on every
    // thread/surface change exactly as the original inline body did.
    void $activeChatThreadId;
    void isHomeSurface;
    resolveHomeBoundAgent();
  });

  // #2794: onboarding creates the first agent (Onboarding.svelte
  // ensureMainAgent()) entirely outside the $activeChatThreadId lifecycle —
  // the thread id is already set at app boot, before onboarding runs, and
  // never changes again across onboarding completion, so the effect above
  // never re-fires on its own. Re-invoking resolveHomeBoundAgent() here, in
  // response to the 'pane-onboarding-complete' event already dispatched at
  // Onboarding.svelte, closes that gap: it re-resolves the now-existing
  // default agent using the exact same logic, so homeBoundAgentName stops
  // being permanently null and resolveSenderName() (ChatPanel.svelte) stops
  // falling through to the 'Pane' literal for the rest of that session-open.
  $effect(() => {
    const handlePaneOnboardingComplete = () => {
      resolveHomeBoundAgent();
    };
    window.addEventListener('pane-onboarding-complete', handlePaneOnboardingComplete);
    return () => {
      window.removeEventListener('pane-onboarding-complete', handlePaneOnboardingComplete);
    };
  });

  // ───────────────────────────────────────────────────────────────────────
  // §30.11.4's three-gate proactive-message poll (open + focused + OpenClaw
  // runtime) is REMOVED (§36.6, #3050 PR 7) — superseded by a per-OC-agent
  // background pull on the scheduler tick, which runs for every OC-bound
  // session (interactive and job sessions alike, open or not, focused or
  // not; Pane being open is the only requirement). `pull_oc_transcript_
  // for_session_cmd`/`session_uses_openclaw_runtime_cmd` stay registered
  // (still real, reusable primitives — the new backend path calls the
  // underlying Rust functions directly, not through this Tauri-command
  // layer) but nothing in this component calls them anymore. The open
  // session's own live-transcript reload is now driven by the
  // `pane-oc-message-ingested` event (this component's own listener, below
  // — pinned by `ChatSidebar.ocMessageIngested.test.ts`), not a
  // poll-and-reload here. See `ai::scheduler::run_oc_background_pickup`.
  // ───────────────────────────────────────────────────────────────────────

  // One-time setup on mount
  onMount(() => {
    // Register navigation-complete listener
    const handleNavigationComplete = (event: Event) => {
      const customEvent = event as CustomEvent;
      const { messages, shouldClearMessages, shouldLoadMessages, threadId } = customEvent.detail;
      
      console.log('[CHAT] Navigation complete, threadId:', threadId, 'previousThreadId:', previousThreadId, 'shouldLoadMessages:', shouldLoadMessages, 'messageCount:', messages.length);
      
      // Patch 1 (#2051): Gate clear on session change — skip the wipe entirely when the
      // thread hasn't changed (e.g. Project → Folder within the same project resolves the
      // same session/threadId, so no wipe is needed and the streaming placeholder survives).
      const isSameThread = previousThreadId !== '' && threadId === previousThreadId;
      
      if (shouldClearMessages && !isSameThread) {
        // Patch 2 (#2051): Protect the streaming placeholder — even on a genuine session
        // switch, never remove the live in-flight message from allMessages. onDone will
        // finalize and re-map it once the stream completes.
        allMessages = allMessages.filter((m: ChatMessage) =>
          m.thread_id !== threadId ||
          (isStreaming && streamingMessageId !== null && m.id === streamingMessageId)
        );
      }
      
      if (shouldLoadMessages) {
        // Add new messages (filter out duplicates)
        const existingIds = new Set(allMessages.map((m: ChatMessage) => m.id));
        const newMessages = messages.filter((m: ChatMessage) => !existingIds.has(m.id));
        allMessages = [...allMessages, ...newMessages];
        previousThreadId = threadId;
      }

      // Patch 3 (#2051): Wire consumePendingResult into the nav-complete handler.
      // loadMessages() is NOT called on the navigateTo path, so the existing
      // consumePendingResult call inside loadMessages() never fires here. If onDone
      // fired between nav-start and this handler (the race window), the response is
      // already saved to DB but the UI snapshot from STEP 4 missed it. Consuming here
      // and reloading closes that race.
      if (streamManager.consumePendingResult(threadId)) {
        console.log('[CHAT] Pending result found on nav-complete, reloading from DB for thread:', threadId);
        (async () => {
          try {
            const freshMsgs = await api.getChatMessages(threadId, 50);
            freshMsgs.reverse();
            // #3166 C55: shared predicate (`isVisibleLoadedMessage`) —
            // this used to be an independent copy of the same filter
            // `loadMessages` below also hand-rolled, and the two
            // disagreed about `synthetic` rows. See messageVisibility.ts.
            const freshFiltered = freshMsgs.filter(isVisibleLoadedMessage);
            allMessages = [
              ...allMessages.filter((m: ChatMessage) => m.thread_id !== threadId),
              ...freshFiltered
            ];
            console.log('[CHAT] Reloaded', freshFiltered.length, 'messages after pending result on nav-complete');
          } catch (e) {
            console.error('[CHAT] Failed to reload after pending result on nav-complete:', e);
          }
        })();
      }

      // Fix #1541: Seed project greeting on the navigateTo() path.
      // loadMessages() is NOT called here, so seedProjectGreetingIfNeeded() was never
      // reached for projects opened via navigation.ts. $activeProject is already set by
      // STEP 2 before this event fires, so currentProjectName is correct (not stale).
      // allMessages is populated above, so the empty-thread guard is accurate.
      // Double-seed safe: the length > 0 guard no-ops on subsequent opens.
      seedProjectGreetingIfNeeded(threadId).catch(e =>
        console.error('[CHAT] seedProjectGreetingIfNeeded on nav-complete failed:', e)
      );
    };
    
    window.addEventListener('pane-navigation-complete', handleNavigationComplete);
    cleanupFunctions.push(() => window.removeEventListener('pane-navigation-complete', handleNavigationComplete));
    
    // Initialize stream manager and listeners async
    (async () => {
      await streamManager.init();
      streamManagerReady = true;
      
      // Register pane-navigate listener for AI tool calls — now calls navigateTo()
      const { listen } = await import('@tauri-apps/api/event');
      // §5.4B: Memory extraction toast listener (#641)
      const unlistenMemory = await listen('memory-extracted', (event: any) => {
        // Respect the Settings toggle (default: on)
        if (localStorage.getItem('pane_memory_toasts') === 'false') return;
        const { facts } = event.payload;
        if (!facts || facts.length === 0) return;
        // Show the last surviving fact from the batch (single-fact display per §5.4B)
        const toastFact = facts[facts.length - 1];
        const truncated = toastFact.fact.length > 60
          ? toastFact.fact.substring(0, 60) + '…'
          : toastFact.fact;
        memoryToast = `💾 Remembered: ${truncated}`;
        memoryToastId = toastFact.id;
        // 5-second auto-fade
        if (memoryToastTimer) clearTimeout(memoryToastTimer);
        memoryToastTimer = setTimeout(() => {
          memoryToast = '';
          memoryToastId = '';
        }, 5000);
      });
      cleanupFunctions.push(unlistenMemory);

      const unlisten = await listen('pane-navigate', async (event: any) => {
        const { view } = event.payload;
        console.log('[CHAT] pane-navigate received, view:', view);
        
        // Call navigateTo instead of manual state updates
        await navigateTo({ view: view as CanvasViewType });
      });
      cleanupFunctions.push(unlisten);

      // §36.6 (#3050 PR 7): replaces the deleted §30.11.4 poll-and-reload.
      // The backend scheduler tick now drives OC transcript pickup for
      // every OC-bound session, including the currently open one — this
      // event is the ONLY thing telling the open chat a new row landed
      // (ai::scheduler::run_oc_background_pickup emits it once per session
      // per tick when that session's own pull inserted >= 1 row AND the
      // session is the one currently open; the inverse case, closed
      // session, fires `agent_message` instead — never both for the same
      // ingest). Scoped to the CURRENT thread at event-arrival time, not
      // the thread captured at listener-registration time (the user may
      // have switched sessions since).
      const unlistenOcMessageIngested = await listen<{ session_id: string }>('pane-oc-message-ingested', async (event) => {
        const { session_id } = event.payload;
        if (session_id === $activeChatThreadId) {
          console.log('[OC-PICKUP] pane-oc-message-ingested for open session — reloading:', session_id);
          await loadMessages(session_id);
        }
      });
      cleanupFunctions.push(unlistenOcMessageIngested);
      
      // Register onboarding-complete listener for home greeting (fixed template, #1541)
      const handleOnboardingComplete = async () => {
        let threadId: string = 'workspace';
        try {
          const greetingSent = await invoke('get_ai_setting', { key: 'greeting_sent' });
          if (greetingSent && greetingSent === 'true') {
            console.log('[CHAT] Greeting already sent, skipping');
            return;
          }

          console.log('[CHAT] Onboarding complete, seeding fixed home greeting...');

          // Wait for active thread to be ready
          if (!$activeChatThreadId) {
            await loadOrCreateThread('workspace');
          }

          threadId = $activeChatThreadId || 'workspace';

          // Check if AI provider is configured.
          // #2564 B1 fix (Path B), item #15: was a direct `invoke('get_ai_setting', { key:
          // 'provider' })` bypassing aiLoadConfig() — the retired `ai_settings.provider`
          // key is never written again post-fix, so this raw read would always resolve to
          // null and permanently show the "no provider" welcome message even for fully
          // configured installs. Re-pointed to aiLoadConfig(), whose backend `provider`
          // field is now derived from ai_agents.runtime_type (see ai_load_config in
          // notes.rs / derive_active_provider_label in runtime_resolution.rs).
          const config = await api.aiLoadConfig();
          const provider = config?.provider;
          if (!provider || provider === '' || provider === 'none') {
            // No provider — show static welcome instead of blank screen (preserved verbatim, spec-covered-by: #2082)
            const welcomeId = `greeting-${Date.now()}`;
            await invoke('save_chat_message', {
              id: welcomeId,
              threadId,
              projectId: 'workspace',
              role: 'assistant',
              content: "Welcome to Pane! To start chatting, set up an AI provider in **Settings → AI**. You can use OpenRouter, Anthropic, or a local Ollama server."
            });
            allMessages = [...allMessages, {
              id: welcomeId,
              thread_id: threadId,
              role: 'assistant',
              content: "Welcome to Pane! To start chatting, set up an AI provider in **Settings → AI**. You can use OpenRouter, Anthropic, or a local Ollama server.",
              created_at: new Date().toISOString()
            }];
            await invoke('set_ai_setting', { key: 'greeting_sent', value: 'true' });
            return;
          }

          // Fixed-template home greeting (#1541, spec-covered-by: #2082)
          // #2604 RC-B: resolve session's BOUND agent, not the default agent (see resolveGreetingAgentName)
          const agentName = await resolveGreetingAgentName();
          const greetingText = `Hey there, I'm ${agentName}. Very nice to meet you. What would you like to do first?`;

          const greetingId = `greeting-${Date.now()}`;
          await invoke('save_chat_message', {
            id: greetingId,
            threadId,
            projectId: 'workspace',
            role: 'assistant',
            content: greetingText
          });
          allMessages = [...allMessages, {
            id: greetingId,
            thread_id: threadId,
            role: 'assistant',
            content: greetingText,
            created_at: new Date().toISOString()
          }];
          await invoke('set_ai_setting', { key: 'greeting_sent', value: 'true' });
        } catch (e) {
          console.error('[GREETING]', e);
          await invoke('set_ai_setting', { key: 'greeting_sent', value: 'true' }).catch(() => {});
        }
      };
      
      window.addEventListener('pane-onboarding-complete', handleOnboardingComplete);
      cleanupFunctions.push(() => window.removeEventListener('pane-onboarding-complete', handleOnboardingComplete));
    })();
    
    return () => {
      cleanupFunctions.forEach(fn => fn());
      cleanupFunctions = [];
    };
  });

  let navigationContext: string = $state('');

  async function loadOrCreateThread(projectId?: string) {
    const startTime = Date.now();
    console.log('[NAV-DIAG] loadOrCreateThread START: projectId=' + (projectId || 'undefined') + ' at ' + startTime);
    
    try {
      isLoading = true;
      isNavigating = true;
      console.log('[CHAT-THREAD] loadOrCreateThread called with projectId:', projectId);
      const session = await api.getActiveSession(projectId || 'workspace');
      const threadId = session.id;
      console.log('[CHAT-THREAD] getActiveSession returned threadId:', threadId, 'for projectId:', projectId);
      $activeChatThreadId = threadId;
      // #3155 C49b: mirror the resolved session's own origin — see
      // `activeSessionOrigin`'s own doc comment (`$lib/stores/app.ts`).
      $activeSessionOrigin = session.origin ?? null;
      
      const endTime = Date.now();
      console.log('[NAV-DIAG] loadOrCreateThread END: projectId=' + (projectId || 'undefined') + ' → threadId=' + threadId + ' took ' + (endTime - startTime) + 'ms');
    } catch (error) {
      console.error('Failed to load/create chat thread:', error);
      const endTime = Date.now();
      console.log('[NAV-DIAG] loadOrCreateThread END (ERROR): projectId=' + (projectId || 'undefined') + ' took ' + (endTime - startTime) + 'ms');
    } finally {
      isLoading = false;
      isNavigating = false;
    }
  }




  // Phase 2: Synthesis review state
  let pendingSynthesis = $state<{ noteId: string; oldContent: string; newContent: string; oldLength: number; newLength: number } | null>(null);
  let showSynthesisReview = $state(false);

  // Phase 3: Protected edit review state
  let pendingProtectedEdit = $state<{ noteId: string; noteType: string; proposedContent: string } | null>(null);
  let showProtectedEditReview = $state(false);

  // Phase 4: Delete confirmation state
  let pendingDelete = $state<{ noteId: string; noteTitle: string } | null>(null);
  let showDeleteConfirm = $state(false);

  async function extractAndSaveMemory(userMessage: string, assistantResponse: string, assistantMessageId?: string, userMessageId?: string) {
    try {
      const status: any = await invoke('ai_status');
      if (!status.configured) return;

      // Queue extraction job for background worker (§27.7 — async extraction pipeline)
      const threadId = $activeChatThreadId || 'workspace';
      await invoke('ai_enqueue_extraction', {
        threadId,
        messageId: assistantMessageId || '',
        userMessageId: userMessageId || null,
      });

      // No toast here — user doesn't wait for extraction to complete
    } catch (e) {
      console.error('[MEMORY] Failed to enqueue extraction (non-blocking):', e);
    }
  }

  onMount(() => {
    // Memory settings initialization (if needed)
  });



  /**
   * Called by SessionDropdownHeader (project dropdown) and SessionsDrawer (home drawer)
   * when the user switches to a different session.
   * Sets the active thread id and reloads the message list for the new session.
   * (#395 PR7, #395 fix)
   *
   * Exported so +page.svelte can pass it to SessionsDrawer as onSessionSwitch.
   */
  export async function handleSessionSwitch(session: Session) {
    // #3155 C49b: this is the "manual navigation" choke point Reed's
    // diagnosis named — the ONE session-switch path that bypasses
    // `navigateTo` entirely (see this function's own doc comment below), so
    // it is also the one path that previously never cleared a pending job-
    // session back target. Before this fix, entering a job session via the
    // Scheduled-panel card or the notification bell, then manually picking
    // an unrelated session from the sidebar/drawer, left `jobSessionBackTarget`
    // non-null — the floating back button then rendered on whatever session
    // the user landed on (Matt's repro: it followed him onto Aurum). Cleared
    // FIRST, unconditionally, before anything else in this function runs.
    jobSessionBackTarget.set(null);
    $activeChatThreadId = session.id;
    // #3155 C49b: mirror the switched-to session's own origin — see
    // `activeSessionOrigin`'s own doc comment (`$lib/stores/app.ts`).
    $activeSessionOrigin = session.origin ?? null;
    // §36.7 (#3050 PR 6): this is the ONE session-switch path that bypasses
    // `navigateTo` entirely (SessionsDrawer/SessionDropdownHeader call this
    // directly, not navigateTo — see this function's own doc comment above)
    // — so `navigateTo`'s own `setViewingSession` call never fires for it.
    // Non-fatal on failure, same as navigateTo's own call: a stale gate
    // means a notification fires it shouldn't, not a crash.
    try {
      await api.setViewingSession(session.id);
    } catch (e) {
      console.warn('[CHAT] setViewingSession failed (non-fatal):', e);
    }
    await loadMessages(session.id);
  }

  // #3155 C49b: the actual predicate is `shouldShowJobSessionBackButton`
  // (`$lib/services/jobSessionBack.ts`) — extracted as a pure function so
  // it has a real Vitest surface with no component mount required; see
  // its own doc comment for the full "why two conditions" reasoning.
  let isActiveSessionAJobSession = $derived(
    shouldShowJobSessionBackButton($jobSessionBackTarget, $activeSessionOrigin)
  );

  async function loadMessages(threadId: string) {
    const startTime = Date.now();
    console.log('[NAV-DIAG] loadMessages START: threadId=' + threadId + ' at ' + startTime);
    console.log('[CHAT-LOAD] loadMessages threadId=' + threadId, 'isStreaming=' + isStreaming, 'streamingMessageId=' + streamingMessageId);
    
    try {
      isLoading = true;
      // Reset pagination state when switching threads
      messageOffset = 0;
      hasMoreMessages = true;
      const loadedMessages = await api.getChatMessages(threadId, 50);
      loadedMessages.reverse(); // DB returns DESC, display needs ASC
      console.log('[CHAT-PERSIST] Loaded', loadedMessages.length, 'messages for thread:', threadId);
      // #3166 C55: shared predicate (`isVisibleLoadedMessage`,
      // messageVisibility.ts) — this used to be `loadMessages`'s own
      // independent copy, with a blanket `!m.synthetic` meant for §36.2.1's
      // "Continue." row that ALSO hid job/wake dispatch briefs
      // (`synthetic=1`, `job_id` set, rendered specially by ChatPanel).
      // `navigateTo`'s own filter (navigation.ts) had NO synthetic
      // exclusion at all — the two loaders' disagreement is what let a
      // brief, once stripped from `allMessages` here, get re-added later
      // as raw markdown on a same-thread sidebar re-click. One shared
      // predicate now; see messageVisibility.ts's own doc comment.
      const filteredMessages = loadedMessages.filter(isVisibleLoadedMessage);

      // Remove old messages for this thread, then add the loaded ones
      allMessages = [
        ...allMessages.filter(m => m.thread_id !== threadId),
        ...filteredMessages
      ];

      // Preserve streaming message if active — guard at the source
      if (isStreaming && streamingMessageId) {
        const hasStreamMsg = allMessages.find(m => m.id === streamingMessageId);
        if (!hasStreamMsg) {
          const currentBuffer = streamManager.getBuffer(threadId);
          console.log('[CHAT-PERSIST] Preserving streaming message after reload, buffer length:', currentBuffer.length);
          allMessages = [...allMessages, {
            id: streamingMessageId,
            thread_id: threadId,
            role: 'assistant' as const,
            content: currentBuffer || '',
            created_at: new Date().toISOString(),
          }];
        }
      }

      // If StreamManager has a pending result for this thread, the response was
      // saved while the component was destroyed. Reload to pick it up.
      if (streamManager.consumePendingResult(threadId)) {
        console.log('[CHAT-PERSIST] Found pending result, reloading from DB');
        const freshMessages = await api.getChatMessages(threadId, 50);
        freshMessages.reverse(); // DB returns DESC, display needs ASC
        // #3166 C55: shared predicate — see messageVisibility.ts.
        const freshFiltered = freshMessages.filter(isVisibleLoadedMessage);
        allMessages = [
          ...allMessages.filter(m => m.thread_id !== threadId),
          ...freshFiltered
        ];
        // ISSUE 3 DIAGNOSTIC: Log message count and last message preview
        console.log('[CHAT-UI-DIAG] After freshFiltered assignment: count=' + freshFiltered.length + ', lastMsg=' + (freshFiltered.length > 0 ? freshFiltered[freshFiltered.length - 1].content.substring(0, 50) : 'none'));
      }

      // If StreamManager has an active stream for this thread but we have no
      // streaming state (component was recreated), re-adopt the stream
      if (streamManager.isStreamingThread(threadId) && !isStreaming) {
        console.log('[CHAT-PERSIST] Re-adopting active stream for thread:', threadId);
        const currentBuffer = streamManager.getBuffer(threadId);
        const placeholderId = `adopted-${Date.now()}`;
        streamingMessageId = placeholderId;
        isStreaming = true;

        // Add placeholder with current buffer content
        allMessages = [...allMessages, {
          id: placeholderId,
          thread_id: threadId,
          role: 'assistant' as const,
          content: currentBuffer || '',
          created_at: new Date().toISOString(),
        }];

        // Re-register callbacks so stream updates go to THIS component
        const newCallbacks = createStreamCallbacks(threadId, placeholderId);
        streamManager.reattachCallbacks(threadId, newCallbacks);
      }
      
      // Seed project greeting for empty threads (#1541)
      await seedProjectGreetingIfNeeded(threadId);

      const endTime = Date.now();
      const messageCount = allMessages.filter(m => m.thread_id === threadId).length;
      console.log('[NAV-DIAG] loadMessages END: threadId=' + threadId + ' returned ' + messageCount + ' messages, took ' + (endTime - startTime) + 'ms');
    } catch (error) {
      console.error('Failed to load messages:', error);
      const endTime = Date.now();
      console.log('[NAV-DIAG] loadMessages END (ERROR): threadId=' + threadId + ' took ' + (endTime - startTime) + 'ms');
    } finally {
      isLoading = false;
    }
  }

  async function loadOlderMessages(): Promise<boolean> {
    loadingOlder = true;
    const threadId = $activeChatThreadId;
    if (!threadId) {
      loadingOlder = false;
      return false;
    }
    
    messageOffset += 50;
    const olderMessages = await api.getChatMessages(threadId, 50, messageOffset);
    olderMessages.reverse(); // DB returns DESC, display needs ASC
    
    if (olderMessages.length === 0) {
      hasMoreMessages = false;
      loadingOlder = false;
      return false;
    }
    
    // #3166 C55: shared predicate — see messageVisibility.ts.
    const filtered = olderMessages.filter(isVisibleLoadedMessage);
    
    // Prepend older messages (they're older, go before existing)
    // But getChatMessages returns DESC order, so reverse them
    const existing = allMessages.filter(m => m.thread_id === threadId);
    const existingIds = new Set(existing.map(m => m.id));
    const newOlder = filtered.filter(m => !existingIds.has(m.id));
    
    allMessages = [
      ...allMessages.filter(m => m.thread_id !== threadId),
      ...newOlder,
      ...existing
    ];
    
    loadingOlder = false;
    return true;
  }

  /**
   * Creates stream callbacks for sendMessage.
   * Captures threadId and messageId at call time for nav-away safety.
   */
  function createStreamCallbacks(threadIdForStream: string, msgId: string, projectIdForStream: string | null = null) {
    return {
      onChunk: (text: string) => {
        // ISSUE 3 DIAGNOSTIC: Log chunk arrival
        console.log('[CHAT-UI-DIAG] onChunk fired, textLength=' + text.length + ', preview=' + text.substring(0, 30));
        // Update allMessages regardless of current thread - the derived filter will handle visibility
        const exists = allMessages.find(m => m.id === msgId);
        if (exists) {
          allMessages = allMessages.map(m =>
            m.id === msgId ? { ...m, content: text } : m
          );
          console.log('[CHAT-UI-DIAG] onChunk updated existing message, allMessages.length=' + allMessages.length);
        } else {
          // Placeholder was lost (effect reloaded messages), re-add with current text
          allMessages = [...allMessages, {
            id: msgId,
            thread_id: threadIdForStream,
            role: 'assistant' as const,
            content: text,
            created_at: new Date().toISOString(),
          }];
          console.log('[CHAT-UI-DIAG] onChunk re-added placeholder, allMessages.length=' + allMessages.length);
        }
      },
      onDone: async (data: { fullText: string; model?: string; agentId?: string; agentName?: string; toolResults: string[]; navigate: any; toolCalls: any[]; image_attachment_ids?: string[] }) => {
        const { fullText, model, agentId, agentName, toolResults, navigate, toolCalls, image_attachment_ids } = data;
        // ISSUE 3 DIAGNOSTIC: Log onDone firing
        console.log('[CHAT-UI-DIAG] onDone fired, fullTextLength=' + fullText.length + ', toolResults=' + (toolResults?.length || 0) + ', navigate=' + (navigate ? 'yes' : 'no'));

        // 1. Compute final display text
        let displayText = stripToolBlocks(fullText);

        // 2. If display text is empty, try tool results
        if (!displayText.trim()) {
          const mutationPrefixes = ['CONFIRM_DELETE', 'PROTECTED_EDIT', 'REFUSED', 'Note updated',
            'Created ', 'Deleted ', 'Status set', 'Priority set', 'Starred', 'Unstarred',
            '# Memory', 'Learned from', 'No memory stored', 'No memory found', '[INTERNAL]'];
          const toolText = toolResults
            .filter((r: string) => r.startsWith('\u2705'))
            .map((r: string) => r.replace(/^\u2705\s*/, ''))
            .filter((r: string) => !mutationPrefixes.some(p => r.startsWith(p)))
            .join('\n');
          if (toolText) displayText = toolText;
        }

        // 3. If STILL empty, try fullText tool results with broader match
        if (!displayText.trim() && toolResults?.length) {
          const displayResults = toolResults
            .map((r: string) => r.replace(/^[\u2705\u274C]\s*/, ''))
            .filter((r: string) => !r.includes('CONFIRM_DELETE') && !r.includes('PROTECTED_EDIT'))
            .join('. ');
          if (displayResults) displayText = displayResults;
        }

        // 4. If STILL empty and no navigate, retry or show fallback (FIX #404)
        if (!displayText.trim() && !navigate) {
          if (streamError) {
            displayText = "Something went wrong — please try sending your message again.";
            console.error('[CHAT-ERROR] Stream error, showing error message:', streamError);
            streamRetryCount = 0; // Reset retry counter on error
          } else if (streamRetryCount < MAX_STREAM_RETRIES) {
            // First empty response — retry once
            console.log('[CHAT] Empty response received, retrying (attempt ' + (streamRetryCount + 1) + ')...');
            streamRetryCount++;
            
            // Remove the empty assistant message before retrying
            allMessages = allMessages.filter(m => m.id !== msgId);
            streamingMessageId = null;
            isStreaming = false;
            
            // Re-send the same message
            const lastUserMsg = allMessages[allMessages.length - 1];
            if (lastUserMsg && lastUserMsg.role === 'user') {
              console.log('[CHAT] Retrying with last user message:', lastUserMsg.content.substring(0, 50));
              await handleSendMessage(lastUserMsg.content, lastUserMsg.attachment_ids, true);
            }
            return; // Don't continue with the rest of onDone
          } else {
            // Second empty response — show improved fallback
            displayText = "I wasn't able to generate a response after two attempts. Please try rephrasing your message.";
            console.log('[CHAT] Empty response after retry, showing fallback');
            streamRetryCount = 0; // Reset for next message
          }
        } else {
          // Successful response — reset retry counter
          streamRetryCount = 0;
        }

        // 5. ALWAYS persist to DB using captured context (§4A.4)
        const ctx = streamManager.getCapturedContext(threadIdForStream);
        const persistThreadId = ctx?.threadId || threadIdForStream;
        console.log('[CHAT-PERSIST] Using context - captured:', ctx?.threadId, 'fallback:', threadIdForStream);
        
        if (displayText.trim()) {
          const toolCallsJson = toolCalls?.length ? JSON.stringify(toolCalls) : null;
          const toolResultsJson = toolResults?.length ? JSON.stringify(toolResults) : null;
          try {
            await api.saveAssistantMessage(persistThreadId, projectIdForStream || 'workspace', displayText, msgId, toolCallsJson, toolResultsJson, model || null, image_attachment_ids?.join(',') || null);
            console.log('[CHAT-PERSIST] Assistant message saved to thread:', persistThreadId);
            streamManager.setPendingResult(persistThreadId);
          } catch (e) {
            console.error('[CHAT-PERSIST] Save failed:', e);
          }
        }

        // 6. Update allMessages (UI visibility handled by derived filter)
        console.log('[CHAT-UI-DIAG] onDone updating allMessages, threadIdForStream=' + threadIdForStream + ', displayText.length=' + displayText.length);
        const exists = allMessages.find(m => m.id === msgId);
        if (exists) {
          allMessages = allMessages.map(m =>
            m.id === msgId ? {
              ...m,
              content: displayText,
              tool_calls: toolCalls?.length ? JSON.stringify(toolCalls) : undefined,
              tool_results: toolResults?.length ? JSON.stringify(toolResults) : undefined,
              // Working as intended: per-message model badge reflects the model that
              // actually ran for THIS message, not current provider state. Required
              // for smart-routing history (see #1448). Do not "fix" by refreshing on
              // provider switch — that destroys the audit trail.
              model: model || undefined,  // FIX #384: Set model immediately for label display
              // #2604 RC-B: set agent_id/agent_name on the live message object,
              // mirroring `model` above — this is the SAME dispatching agent
              // `save_chat_message` already stamped onto the DB row, so the live
              // rendered label matches what a reload will show instead of falling
              // back to the app-wide default agent (ChatPanel.svelte speaker label).
              agent_id: agentId || m.agent_id,
              agent_name: agentName || m.agent_name,
              // #3161 PR1 round 2 (Reed audit finding 2): set attachment_ids on the LIVE
              // message object immediately, mirroring model/agent_id/agent_name above —
              // without this, ChatPanel.svelte's renderer (which keys only on
              // `message.attachment_ids`, a comma-joined string) has nothing to draw
              // until the thread is reloaded/revisited, even though the DB persist two
              // lines above already has the right value. §39.7 step 2's live walk
              // ("thumbnail in chat") exercises exactly this path.
              attachment_ids: image_attachment_ids?.length ? image_attachment_ids.join(',') : m.attachment_ids
            } : m
          );
          console.log('[CHAT-UI-DIAG] onDone updated existing message, allMessages.length=' + allMessages.length);
        } else {
          // Placeholder was lost, add the completed message
          allMessages = [...allMessages, {
            id: msgId,
            thread_id: threadIdForStream,
            role: 'assistant' as const,
            content: displayText,
            created_at: new Date().toISOString(),
            tool_calls: toolCalls?.length ? JSON.stringify(toolCalls) : undefined,
            tool_results: toolResults?.length ? JSON.stringify(toolResults) : undefined,
            // Working as intended: per-message model badge reflects the model that
            // actually ran for THIS message, not current provider state. Required
            // for smart-routing history (see #1448). Do not "fix" by refreshing on
            // provider switch — that destroys the audit trail.
            model: model || undefined,  // FIX #384: Set model immediately for label display
            // #2604 RC-B: same as above — set agent attribution immediately.
            agent_id: agentId,
            agent_name: agentName,
            // #3161 PR1 round 2 (Reed audit finding 2): see matching comment above.
            attachment_ids: image_attachment_ids?.length ? image_attachment_ids.join(',') : undefined
          }];
          console.log('[CHAT-UI-DIAG] onDone added completed message, allMessages.length=' + allMessages.length);
        }

        // Handle navigation and UI updates (§4A.4 navigate tool exception)
        const navCtx = streamManager.getCapturedContext(threadIdForStream);
        const userStillInContext = navCtx && $activeChatThreadId === navCtx.threadId;
        
        if (userStillInContext) {
          // Handle navigation only if user hasn't navigated away
          if (navigate) {
            console.log('[CHAT-NAV] Executing navigate - user still in context');
            handleNavigateFromStream(navigate);
          }
        } else {
          console.log('[CHAT-NAV] Ignoring navigate - user navigated away (current:', $activeChatThreadId, 'captured:', navCtx?.threadId, ')');
        }
        
        // Memory extraction and other UI updates only if still on same thread
        if ($activeChatThreadId === threadIdForStream) {

          // Handle memory extraction
          const memEnabled = localStorage.getItem('pane_memory_enabled') !== 'false';
          if (memEnabled) {
            const compactionMarkers = [
              '## Conversation Summary', 'Conversation Summary', '### Key Decisions',
              '### Current Status', '### Content Added', '**Folders created:**',
              '**Notes created:**', '### Action Items', 'Previous conversation summary:',
              '[COMPACTION]', '## Session Summary', '**Summary of Conversation**'
            ];
            const countMarkers = (text: string) => compactionMarkers.filter(m => text.includes(m)).length;
            const isCompactionSummary = countMarkers(lastUserMessageContent) >= 2 || countMarkers(displayText) >= 2;
            const wordCount = lastUserMessageContent.split(/\s+/).filter(Boolean).length;

            if (isCompactionSummary) {
              console.log('[MEMORY] Skipped extraction: compaction summary message detected');
            } else {
              const structuralTools = ['create_note', 'create_folder', 'create_project', 'edit_note', 'set_status', 'move_note', 'delete_note', 'delete_folder'];
              const aiUsedStructuralTool = toolCalls.some((tc: any) => structuralTools.includes(tc.name));
              const userMessageLongEnough = lastUserMessageContent && wordCount >= 5;
              const shouldExtract = aiUsedStructuralTool || userMessageLongEnough;

              if (!shouldExtract) {
                console.log(`[MEMORY] Skipped extraction: no structural tools and message too short (${wordCount} words)`);
              } else {
                if (aiUsedStructuralTool) {
                  console.log(`[MEMORY] Triggering extraction: AI used structural tool(s): ${toolCalls.filter((tc: any) => structuralTools.includes(tc.name)).map((tc: any) => tc.name).join(', ')}`);
                } else {
                  console.log(`[MEMORY] Triggering extraction for: "${lastUserMessageContent.slice(0, 50)}" word count: ${wordCount}`);
                }
                try {
                  const lastUserMsg = allMessages[allMessages.length - 2];
                  const lastUserMessageId = lastUserMsg?.id || undefined;
                  await extractAndSaveMemory(lastUserMessageContent, displayText, msgId, lastUserMessageId);
                } catch (e) {
                  console.error('[MEMORY] Extraction failed (non-blocking):', e);
                }
              }
            }
          }

          // Handle star/unstar and protected edit / delete confirm from tool results
          if (toolResults?.length) {
            for (const r of toolResults) {
              // Phase 3: Handle protected edit requests
              if (typeof r === 'string' && r.includes('PROTECTED_EDIT:')) {
                const peIdx = r.indexOf('PROTECTED_EDIT:');
                const parts = r.substring(peIdx).split(':');
                const peNoteId = parts[1];
                const peNoteType = parts[2];
                try {
                  const versions: any[] = await invoke('get_note_versions', { noteId: peNoteId });
                  const pending = versions.find((v: any) => v.source === 'pending_ai_edit');
                  if (pending) {
                    const fullVersion: any = await invoke('get_note_version', { versionId: pending.id });
                    pendingProtectedEdit = { noteId: peNoteId, noteType: peNoteType, proposedContent: fullVersion.content };
                    showProtectedEditReview = true;
                  }
                } catch (e) { console.error('[PROTECTED] Failed to load pending edit:', e); }
              }
              // Phase 4: Handle delete confirmation requests
              if (typeof r === 'string' && r.includes('CONFIRM_DELETE:')) {
                const cdIdx = r.indexOf('CONFIRM_DELETE:');
                const parts = r.substring(cdIdx).split(':');
                const delNoteId = parts[1];
                const delTitle = parts[2];
                pendingDelete = { noteId: delNoteId, noteTitle: delTitle };
                showDeleteConfirm = true;
              }
              if (typeof r === 'string' && (r.includes('Starred') || r.includes('Unstarred'))) {
                // is_favorite is now DB-backed; execute_star_project updated the DB.
                // Refresh the $projects store so sidebar re-derives pinnedProjects.
                try {
                  $projects = await api.listProjects();
                } catch {}
              }
            }
          }

          // checkAndRunCompaction(); // REMOVED v0.15.0: compaction now handled by backend (§16.4)
        }

        // 7. Clean up streaming state
        streamingMessageId = null;
        isStreaming = false;
        connectionError = '';
        // §22C.12.4 — success: return to Idle
        reconnectState = 'idle';
        reconnectAttempt = 0;
        gatewayErrorCode = undefined;
      },
      onReconnecting: (retryAttempt: number) => {
        // §22C.12.4 — Attempting → Reconnecting: backend is retrying, disable Send
        console.log('[CHAT-RECONNECT] Reconnecting, retry_attempt:', retryAttempt);
        reconnectState = 'reconnecting';
        reconnectAttempt = retryAttempt;
        // Keep isStreaming true so the assistant message placeholder persists
        // Clear any partial text in the placeholder (spec: discard partial on retry)
        allMessages = allMessages.map(m =>
          m.id === msgId ? { ...m, content: '' } : m
        );
      },
      onError: async (errorMessage: string, partial: { fullText: string; toolCalls: any[]; toolResults: string[]; image_attachment_ids?: string[] }, errorCode?: string) => {
        // §22C.12.4 — budget exhausted or hard-fail: transition to Failed
        console.log('[CHAT-RECONNECT] onError, errorCode:', errorCode, 'reconnectState:', reconnectState);
        reconnectState = 'failed';
        reconnectAttempt = 0;
        gatewayErrorCode = errorCode;

        // Fix #1358: if the stream produced text before the error, preserve it.
        // Display format and persistence logic delegated to shared pure helpers
        // in streamErrorContent.ts so the test suite exercises the same code.
        const hasPartial = hasPartialText(partial);
        const displayContent = buildStreamErrorContent(errorMessage, partial);

        // Update allMessages regardless of current thread (same as before)
        // #3161 PR1 round 2 (Reed audit finding 2): mirror onDone — set
        // attachment_ids on the live message object here too, so an image
        // generated just before a stream error still shows live, not just
        // after persisting + a reload.
        allMessages = allMessages.map(m =>
          m.id === msgId ? {
            ...m,
            content: displayContent,
            attachment_ids: partial.image_attachment_ids?.length ? partial.image_attachment_ids.join(',') : m.attachment_ids
          } : m
        );
        connectionError = errorMessage;
        streamError = 'connection';  // Track error for fallback handling
        streamingMessageId = null;
        isStreaming = false;

        // Fix #1358: persist to DB so partial text survives page reload.
        // Mirrors the onDone persist path (ChatSidebar.svelte ~line 571).
        if (hasPartial) {
          const ctx = streamManager.getCapturedContext(threadIdForStream);
          const persistThreadId = ctx?.threadId || threadIdForStream;
          const toolCallsJson = partial.toolCalls?.length ? JSON.stringify(partial.toolCalls) : null;
          const toolResultsJson = partial.toolResults?.length ? JSON.stringify(partial.toolResults) : null;
          try {
            await api.saveAssistantMessage(
              persistThreadId,
              projectIdForStream || 'workspace',
              displayContent,
              msgId,
              toolCallsJson,
              toolResultsJson,
              null, // model not available on error path
              partial.image_attachment_ids?.join(',') || null,
            );
            console.log('[CHAT-PERSIST] Partial stream saved on error for thread:', persistThreadId);
          } catch (e) {
            console.error('[CHAT-PERSIST] Failed to save partial stream on error:', e);
          }
        }
      }
    };
  }

  /**
   * #3151 PR1 Commit D: the job-output link's navigation hand-off
   * (`ChatPanel`'s new `onNoteLinkClick` callback — see that prop's own
   * doc comment). A `pane://note/{id}` click only carries a note id, not
   * the `project_id`/`folder_id` a `navigate` stream payload already has
   * — this resolves it via `get_note` (the same Tauri command
   * `handleNavigateFromStream`'s own `open_note` case already calls,
   * further down, for its 500ms-delayed `activeNote.set` step) and then
   * hands off to `handleNavigateFromStream` itself with a synthesized
   * payload shaped exactly like a real `navigate` stream event. **No
   * third parser/navigation path** — `decideNavigationAction`'s
   * `open_note` branch is the one place "open a note in its project"
   * logic lives; this reuses it verbatim rather than re-deriving
   * `$activeNotebook`/`navigateTo`/`activeNote.set` a second time.
   */
  export async function navigateToNoteById(noteId: string) {
    try {
      const noteData: any = await invoke('get_note', { noteId });
      if (!noteData?.meta?.project_id) {
        navError = `Could not open note: note "${noteId}" was not found.`;
        console.error('[NAV] ' + navError);
        return;
      }
      await handleNavigateFromStream({
        note_id: noteId,
        project_id: noteData.meta.project_id,
        folder_id: noteData.meta.folder_id ?? undefined,
      });
    } catch (e) {
      navError = `Could not open note: ${e}`;
      console.error('[NAV] Failed to resolve note for pane://note/ link:', e);
    }
  }

  async function handleNavigateFromStream(nav: any) {
    isNavigating = true;
    try {
      // #3036 PR2 — pure decision first (never silently does nothing; see
      // navigationDispatch.ts for the bug this replaces), THEN perform the
      // async side effects the decision names.
      const decision = decideNavigationAction(nav);

      switch (decision.kind) {
        case 'error': {
          navError = decision.message;
          console.error('[NAV] ' + decision.message);
          return;
        }

        case 'toggle_kanban': {
          window.dispatchEvent(new CustomEvent('pane-toggle-kanban'));
          return;
        }

        case 'set_filter': {
          window.dispatchEvent(new CustomEvent('pane-set-filter', { detail: { tab: decision.tab } }));
          return;
        }

        case 'open_view': {
          logAnSet(`ChatSidebar.nav[${decision.view}]`, null);
          activeNote.set(null);
          await navigateTo({ view: decision.view });
          return;
        }

        case 'open_project': {
          // #3036 PR2 — the real fix: a bare `navigate({project_id})`
          // (or a project-scoped show_view screen) now reaches here
          // instead of being swallowed by the old view-only switch above.
          const allProjects = await api.listProjects();
          const project = allProjects.find((p: any) => p.id === decision.projectId);
          if (!project) {
            navError = `Could not navigate: project "${decision.projectId}" was not found.`;
            console.error('[NAV] ' + navError);
            return;
          }
          $activeNotebook = nav.project_name || project.name;
          logAnSet('ChatSidebar.nav[project_only]', null);
          activeNote.set(null);
          await navigateTo({
            view: decision.view,
            projectId: decision.projectId
          });
          return;
        }

        case 'open_note': {
          const allProjects = await api.listProjects();
          const project = allProjects.find((p: any) => p.id === decision.projectId);
          if (!project) {
            navError = `Could not open note: project "${decision.projectId}" was not found.`;
            console.error('[NAV] ' + navError);
            return;
          }
          const notebook = nav.folder_name ? `${nav.project_name || project.name}/${nav.folder_name}` : (nav.project_name || project.name);
          $activeNotebook = notebook;

          await navigateTo({
            view: 'notes_browser',
            projectId: decision.projectId,
            noteId: decision.noteId,
            folderId: decision.folderId || undefined
          });

          setTimeout(async () => {
            try {
              const noteData: any = await invoke('get_note', { noteId: decision.noteId });
              if (noteData) { const _nd = { meta: { ...noteData.meta, slug: noteData.meta.id, notebook }, content: noteData.content }; logAnSet('ChatSidebar.nav[note+project]', _nd); activeNote.set(_nd); }
              window.dispatchEvent(new CustomEvent('pane-select-note', { detail: { noteId: decision.noteId } }));
            } catch (e) { console.error('[NAV] Failed:', e); }
          }, 500);
          return;
        }
      }
    } finally {
      isNavigating = false;
    }
  }

  /**
   * Seeds a fixed project greeting for empty threads (#1541, spec-covered-by: #2082).
   * Template: "Hey there, I'm [agent]. I'm here to help with [project-name]. Where would you like to start?"
   * Idempotency: the empty-thread check IS the guard — seeding makes the thread non-empty.
   */
  async function seedProjectGreetingIfNeeded(threadId: string) {
    const projName = currentProjectName;
    // Only seed for real project threads, not home/workspace
    if (!projName || projName === 'General' || projName === 'Pane System') {
      return;
    }

    // Check if the thread has zero messages
    const threadMessages = allMessages.filter(m => m.thread_id === threadId);
    if (threadMessages.length > 0) {
      return;
    }

    console.log('[CHAT] Empty project thread detected, seeding project greeting for:', projName);

    try {
      // #2604 RC-B: resolve session's BOUND agent, not the default agent (see resolveGreetingAgentName)
      const agentName = await resolveGreetingAgentName();
      const greetingText = `Hey there, I'm ${agentName}. I'm here to help with ${projName}. Where would you like to start?`;
      const greetingId = `project-greeting-${Date.now()}`;

      await invoke('save_chat_message', {
        id: greetingId,
        threadId,
        projectId: currentProjectId || 'workspace',
        role: 'assistant',
        content: greetingText
      });
      allMessages = [...allMessages, {
        id: greetingId,
        thread_id: threadId,
        role: 'assistant',
        content: greetingText,
        created_at: new Date().toISOString()
      }];
    } catch (e) {
      console.error('[PROJECT-GREETING] Failed to seed project greeting:', e);
    }
  }

  // REMOVED v0.15.0: Compaction now handled by backend (§16.4)
  // async function checkAndRunCompaction() {
  //   try {
  //     if (!$activeChatThreadId) return;
  //     const needsCompaction = await shouldCompact($activeChatThreadId);
  //     if (!needsCompaction) return;

  //     isCompacting = true;
  //     memoryIndicator = 'Saving memory...';

  //     const cleanMessages = messages
  //       .filter(m => (m.role === 'user' || m.role === 'assistant') && m.content.trim() && m.content !== '...')
  //       .map(m => ({ role: m.role, content: m.content }));

  //     const summary = await runCompaction($activeChatThreadId, currentProjectId, cleanMessages);

  //     if (summary) {
  //       console.log('[CHAT-LOAD] Source: compaction');
  //       await loadMessages($activeChatThreadId);
  //       memoryIndicator = '\u{1F4BE} Memory saved';
  //       setTimeout(() => { memoryIndicator = ''; isCompacting = false; }, 3000);
  //     } else {
  //       isCompacting = false;
  //       memoryIndicator = '';
  //     }
  //   } catch (compactionError) {
  //     console.error('Compaction failed:', compactionError);
  //     isCompacting = false;
  //     memoryIndicator = '';
  //   }
  // }

  // #3044 item 4: the free-text navigation shortcuts that used to live here
  // (fuzzyMatch + navMap lookup, the "go to [project]"/"open [note]" regexes,
  // and the fake "Done — navigated" assistant replies) are deleted outright,
  // not gated. Matt's ruling: every chat message goes to the agent — chat is
  // not a second, parallel command parser. Deleting this also retires
  // userReachableScan.ts's own exclusion carved out specifically for this
  // block (#3043 item 2's "handleSendMessage's if (navView) {...} block"
  // exclusion) — see that file's header comment, updated in the same commit.
  // §36.2.3 (#3050 PR 3): the Stop button's frontend half. Sets the
  // per-session cancel flag backend-side (`ai::chat::stop_turn`) — keyed on
  // the same session id (`$activeChatThreadId`) every other dispatch in this
  // file already uses (`handleSendMessage`'s own `threadId` local, the
  // `streamManager` calls, etc.). Fire-and-forget from the UI's perspective:
  // the stream's own `done:true` (with the "Stopped by you after N tool
  // calls." text, §36.2.1) is what actually ends the visible stream — this
  // call only flips the flag the running loop polls.
  async function handleStopTurn() {
    if (!$activeChatThreadId) return;
    try {
      await invoke('stop_turn', { sessionId: $activeChatThreadId });
    } catch (e) {
      console.error('[STOP-TURN] Failed to stop turn:', e);
    }
  }

  async function handleSendMessage(content: string, attachmentIds?: string, isRetry: boolean = false) {
    if (!$activeChatThreadId) return;
    if (!content.trim() && !attachmentIds) return;

    try {
      lastUserMessageContent = content;
      streamError = null;  // Reset error flag for new message
      if (!isRetry) streamRetryCount = 0;  // FIX #404: Reset only on genuine new user message
      // §22C.12.4 — new message resets reconnect budget (fresh cycle)
      reconnectState = 'attempting';
      reconnectAttempt = 0;
      gatewayErrorCode = undefined;

      isStreaming = true;  // Set BEFORE any await to guard effects — must be outside !isRetry gate

      if (!isRetry) {
        const userMessage: ChatMessage = {
          id: `temp-${Date.now()}`, thread_id: $activeChatThreadId || 'temp',
          role: 'user', content, created_at: new Date().toISOString(),
          attachment_ids: attachmentIds,
        };
        allMessages = [...allMessages, userMessage];

        console.log('[CHAT-PERSIST] Saving user message, threadId:', $activeChatThreadId, 'content:', content.substring(0, 50));
        try {
          const savedUserMsgId = await api.sendChatMessage($activeChatThreadId, currentProjectId || 'workspace', content, undefined, attachmentIds);
          // Update local message with actual DB ID (fixes #618 — extraction needs real ID)
          allMessages = allMessages.map(m => 
            m.id === userMessage.id ? { ...m, id: savedUserMsgId } : m
          );
          console.log('[CHAT-PERSIST] User message saved successfully, ID:', savedUserMsgId);
        } catch (e) {
          console.error('[CHAT-PERSIST] User save FAILED:', e);
        }
      } // end !isRetry: skip user message creation and DB persistence on retry

      const assistantMessage: ChatMessage = {
        id: `assistant-${Date.now()}`, thread_id: $activeChatThreadId || 'temp',
        role: 'assistant', content: '', created_at: new Date().toISOString(),
      };
      allMessages = [...allMessages, assistantMessage];
      streamingMessageId = assistantMessage.id;
      // isStreaming already true from above

      const canvasProject = $canvasState.context.projectName;
      const canvasProjectId = $canvasState.context.projectId;
      let folderNames: string[] = [];
      const ctxProjectName = canvasProject || currentProjectName;
      if (ctxProjectName) {
        try {
          const allProjects = await api.listProjects();
          const parent = allProjects.find((p: any) => p.name === ctxProjectName && !p.parent_id);
          if (parent) folderNames = allProjects.filter((p: any) => p.parent_id === parent.id).map((p: any) => p.name);
        } catch {}
      }
      let noteContent = '';
      const ctxNoteId = $canvasState.context.noteId || $activeNote?.meta?.id;
      const ctxNoteTitle = $canvasState.context.noteTitle || $activeNote?.meta?.title;
      if (ctxNoteId) {
        try {
          const noteData: any = await invoke('get_note', { noteId: ctxNoteId });
          noteContent = noteData?.content || noteData?.meta?.content_preview || '';
        } catch {}
      }

      const context = {
        currentView: $canvasState.view, projectId: canvasProjectId || currentProjectId,
        projectName: ctxProjectName, navigationContext: navigationContext || undefined,
        noteId: ctxNoteId, noteTitle: ctxNoteTitle,
        noteContent: noteContent ? noteContent.substring(0, 500) : undefined,
        taskId: $canvasState.context.taskId, searchQuery: $canvasState.context.searchQuery,
        folderName: $canvasState.context.folderName, folders: folderNames.length > 0 ? folderNames : undefined,
      };

      const cleanHistory = messages
        .filter(m => {
          if (m.role !== 'user' && m.role !== 'assistant') return false;
          const c = m.content.trim();
          if ((!c || c === '...') && !m.attachment_ids) return false;
          if (c.startsWith('\u2705') || c.startsWith('\u274C') || c.startsWith('[SYSTEM') || c.includes('[TROVE APP CONTEXT]')) return false;
          if (c.startsWith('## Session Summary') || c.includes('summarize the key decisions')) return false;
          return true;
        })
        .slice(0, -1)
        .map(m => ({ role: m.role, content: m.content, ...(m.id ? { id: m.id } : {}), ...(m.attachment_ids ? { attachment_ids: m.attachment_ids } : {}) }));
      cleanHistory.push({ role: 'user', content, ...(attachmentIds ? { attachment_ids: attachmentIds } : {}) });

      const memorySummary = await getMemoryContext(currentProjectId);
      if (memorySummary) {
        cleanHistory.unshift({ role: 'system', content: `Previous conversation summary:\n${memorySummary}` });
      }

      const threadIdForStream = $activeChatThreadId;
      const projectIdForStream = currentProjectId || null;
      // ADR-0062 D4/D7: session_id lets the backend resolve message attribution
      // (and the daily-log creation site) from the session's immutable agent
      // binding instead of ambient is_default. 'workspace'/'temp' aren't real
      // session rows, so only forward a real UUID-shaped thread id.
      //
      // Bug 2 (#3050 fix PR, live-test #1) investigation note — Reed's scope
      // audit asked whether this filter itself is the stale half of the bug
      // (now that Home has a real chat_sessions row) or whether 'workspace'
      // is still a genuine non-row sentinel. Evidence, not a guess: grep
      // confirms the NORMAL path (`navigation.ts::navigateTo`) never sets
      // `$activeChatThreadId` to anything but a real `session.id` from
      // `getActiveSession` (get-or-create, always a real row). The literal
      // string 'workspace' is still written here today by exactly one live
      // path: `navigateToSafe`'s own catch-block recovery fallback
      // (`activeChatThreadId.set('workspace')` on navigation failure) — a
      // genuine, intentional "I don't have a real session, don't pretend I
      // do" sentinel, not stale. 'temp' is dead-but-harmless defensive code
      // (never actually assigned to the store anywhere; only used as a
      // local `|| 'temp'` display fallback for optimistic messages). This
      // filter is therefore CORRECT as written — the actual bug was
      // entirely on the backend side of an `undefined` sessionId:
      // `ai_chat_stream`'s own `None` fallback (`chat.rs`) used to resolve
      // to the literal, non-existent string `"workspace"` instead of
      // get-or-creating a real session; fixed there
      // (`get_or_create_active_session_id`), not here.
      const sessionIdForStream = threadIdForStream && threadIdForStream !== 'workspace' && threadIdForStream !== 'temp' ? threadIdForStream : undefined;
      const streamId = await invoke('ai_chat_stream', { messages: cleanHistory, context, sessionId: sessionIdForStream }) as string;
      console.log('[CHAT] Started stream:', streamId);

      streamManager.startStream(streamId, threadIdForStream, projectIdForStream, createStreamCallbacks(threadIdForStream, assistantMessage.id, projectIdForStream));
    } catch (error) {
      console.error('Failed to send message:', error);
      const failedStreamId = streamingMessageId;
      streamingMessageId = null;
      isStreaming = false;
      streamError = 'send_failed';  // Track send failure
      // §22C.12.4 — command-level failure (before stream started): transition to Failed
      reconnectState = 'failed';
      const errStr = String(error);
      // D22 (#2649 family): the backend's deactivated-bound-agent guard
      // (agents::check_bound_agent_active, agents.rs:366-403) refuses dispatch
      // with a message naming the agent and explaining why — e.g. "Session is
      // bound to agent 'X' which is currently inactive. Start a new session or
      // reconnect the agent's provider." Previously this fell through to the
      // generic branch and rendered as a raw "\u26A0 Something went wrong:
      // <error>" bubble, which is technically correct but reads as a crash
      // rather than an explainable lock state. Checked BEFORE isGateway:
      // the guard's own copy says "reconnect the agent's provider", which
      // would otherwise false-match the isGateway regex's "connect" term.
      const isDeactivated = /currently inactive|has been deactivated|bound to .* which/i.test(errStr);
      const isGateway = !isDeactivated && /gateway|connect|ECONNREFUSED|network|timeout|fetch/i.test(errStr);
      const displayError = isDeactivated
        ? `\u26A0 ${errStr}`
        : isGateway
        ? '\u26A0 Couldn\'t reach the AI. Check your connection and try again.'
        : `\u26A0 Something went wrong: ${errStr}`;
      if (failedStreamId) {
        allMessages = allMessages.map(m =>
          m.id === failedStreamId ? { ...m, content: displayError } : m
        );
      } else {
        const errorMessage: ChatMessage = {
          id: `error-${Date.now()}`, thread_id: $activeChatThreadId || 'temp',
          role: 'assistant', content: displayError, created_at: new Date().toISOString(),
        };
        allMessages = [...allMessages, errorMessage];
      }
    }
  }

  function closeSidebar() {
    $chatSidebarOpen = false;
  }

  function parseNaturalDate(dateText: string): string | undefined {
    const today = new Date();
    const tomorrow = new Date(today);
    tomorrow.setDate(today.getDate() + 1);
    const text = dateText.toLowerCase().trim();
    if (text.includes('today')) return today.toISOString().split('T')[0];
    if (text.includes('tomorrow')) return tomorrow.toISOString().split('T')[0];
    if (text.includes('next week')) {
      const nextMonday = new Date(today);
      nextMonday.setDate(today.getDate() + (7 - today.getDay() + 1) % 7 || 7);
      return nextMonday.toISOString().split('T')[0];
    }
    const dayNames = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'];
    for (let i = 0; i < dayNames.length; i++) {
      if (text.includes(dayNames[i])) {
        const targetDay = new Date(today);
        const daysAhead = (i - today.getDay() + 7) % 7 || 7;
        targetDay.setDate(today.getDate() + daysAhead);
        return targetDay.toISOString().split('T')[0];
      }
    }
    const inDaysMatch = text.match(/in (\d+) days?/);
    if (inDaysMatch) {
      const days = parseInt(inDaysMatch[1]);
      const targetDate = new Date(today);
      targetDate.setDate(today.getDate() + days);
      return targetDate.toISOString().split('T')[0];
    }
    const isoMatch = text.match(/(\d{4}-\d{2}-\d{2})/);
    if (isoMatch) return isoMatch[1];
    return undefined;
  }

  function refreshCanvas() {
    $canvasState = { ...$canvasState };
  }

  async function handleCanvasCommand(canvasCommand: any) {
    try {
      console.log('Handling canvas command:', canvasCommand);
      const view = canvasCommand.view;
      const params = canvasCommand.params || {};
      
      await navigateTo({
        view: view as CanvasViewType,
        projectId: params.project_id || currentProjectId,
        folderId: params.folder_id,
        noteId: params.note_id,
        taskId: params.task_id,
        searchQuery: params.query || params.search
      });
    } catch (error) { console.error('Failed to handle canvas command:', error); }
  }

</script>

{#if alwaysOpen || $chatSidebarOpen}
  <div class="chat-sidebar layout-{layoutMode}">
    <ChatPanel
      {messages}
      {isStreaming}
      {streamingMessageId}
      onSendMessage={handleSendMessage}
      onStop={handleStopTurn}
      placeholder={homePlaceholder || 'Message your agents…'}
      projectName={currentProjectName}
        projectId={currentProjectId || "__global__"}
      showHeader={showHeader && $activeProject !== null && $activeProject?.name !== 'Pane System'}
      headerTitle={currentProjectName}
      readOnlyState={homeReadOnlyState}
      boundAgentNameProp={homeBoundAgentName}
      agentsLoadedProp={homeAgentsLoaded}

      onClose={showClose ? closeSidebar : undefined}
      {memoryIndicator}
      {memoryToast}
      {memoryToastId}
      onDismissMemoryToast={async () => {
        if (!memoryToastId) return;
        try {
          await invoke('ai_dismiss_memory', { memoryId: memoryToastId });
        } catch (e) {
          console.error('[MEMORY] Failed to dismiss memory:', e);
        }
        memoryToast = '';
        memoryToastId = '';
        if (memoryToastTimer) clearTimeout(memoryToastTimer);
      }}
      {isCompacting}

      {connectionError}
      {gatewayErrorCode}
      onClearConnectionError={() => connectionError = ''}
      {navError}
      onClearNavError={() => navError = null}
      onOpenSettings={() => { navigateTo({ view: 'settings' }); }}
      onSessionSwitch={handleSessionSwitch}
      activeThreadId={$activeChatThreadId}
      onChipInserted={(jobId, sessionId) => { void loadMessages(sessionId); }}
      onNoteLinkClick={navigateToNoteById}
      showBackButton={isActiveSessionAJobSession}
      onBackClick={() => { void goToJobSessionBackTarget(); }}
      onLoadOlder={loadOlderMessages}
      {hasMoreMessages}
      {loadingOlder}
      sendDisabled={reconnectState === 'reconnecting'}
      reconnectIndicator={reconnectState === 'reconnecting'
        ? `Reconnecting… (attempt ${reconnectAttempt} of 5)`
        : ''}
      {isFreshSession}
      {jobAwareHint}
    />
  </div>
{/if}

<!-- Phase 2: Synthesis Review Dialog -->
{#if showSynthesisReview && pendingSynthesis}
<div class="modal-overlay" role="dialog" aria-modal="true">
  <div class="modal-panel">
    <h3 class="modal-title">Memory Synthesis Review</h3>
    <p class="modal-desc">Synthesis removed {Math.round((1 - pendingSynthesis.newLength / pendingSynthesis.oldLength) * 100)}% of content ({pendingSynthesis.oldLength} -> {pendingSynthesis.newLength} chars). Review the changes before applying.</p>
    <div class="modal-diff">
      <div class="diff-col">
        <span class="diff-label">Current</span>
        <pre class="diff-content synthesis-content">{pendingSynthesis.oldContent}</pre>
      </div>
      <div class="diff-col">
        <span class="diff-label">Proposed</span>
        <pre class="diff-content synthesis-content">{pendingSynthesis.newContent}</pre>
      </div>
    </div>
    <div class="modal-actions">
      <button class="modal-btn modal-btn-secondary" onclick={async () => { showSynthesisReview = false; pendingSynthesis = null; }}>Reject</button>
      <button class="modal-btn modal-btn-primary" onclick={async () => { if (pendingSynthesis) { await invoke('update_note', { request: { note_id: pendingSynthesis.noteId, content: pendingSynthesis.newContent } }); window.dispatchEvent(new CustomEvent('pane-reload-editor', { detail: { noteId: pendingSynthesis.noteId, oldText: '' } })); } showSynthesisReview = false; pendingSynthesis = null; }}>Accept</button>
    </div>
  </div>
</div>
{/if}

<!-- Phase 3: Protected Edit Review Dialog -->
{#if showProtectedEditReview && pendingProtectedEdit}
<div class="modal-overlay" role="dialog" aria-modal="true">
  <div class="modal-panel">
    <h3 class="modal-title">Protected Note Edit</h3>
    <p class="modal-desc">The AI wants to edit your {pendingProtectedEdit.noteType} note. Review the proposed changes.</p>
    <div class="modal-diff">
      <div class="diff-col">
        <span class="diff-label">Proposed Content</span>
        <pre class="diff-content">{pendingProtectedEdit.proposedContent.slice(0, 800)}{pendingProtectedEdit.proposedContent.length > 800 ? '...' : ''}</pre>
      </div>
    </div>
    <div class="modal-actions">
      <button class="modal-btn modal-btn-danger" onclick={() => { showProtectedEditReview = false; pendingProtectedEdit = null; }}>Reject</button>
      <button class="modal-btn modal-btn-primary" onclick={async () => {
        if (pendingProtectedEdit) {
          await invoke('update_note', { request: { note_id: pendingProtectedEdit.noteId, content: pendingProtectedEdit.proposedContent } });
        }
        showProtectedEditReview = false;
        pendingProtectedEdit = null;
      }}>Accept Edit</button>
    </div>
  </div>
</div>
{/if}

<!-- Phase 4: Delete Confirmation Dialog -->
{#if showDeleteConfirm && pendingDelete}
<div class="modal-overlay" role="dialog" aria-modal="true">
  <div class="modal-panel">
    <h3 class="modal-title">Confirm Delete</h3>
    <p class="modal-desc">Are you sure you want to delete "{pendingDelete!.noteTitle}"? This action will move the note to trash.</p>
    <div class="modal-actions">
      <button class="modal-btn modal-btn-secondary" onclick={() => { showDeleteConfirm = false; pendingDelete = null; allMessages = [...allMessages, { id: `msg-cancel-${Date.now()}`, thread_id: $activeChatThreadId || 'temp', role: 'assistant', content: 'Delete cancelled.', created_at: new Date().toISOString() }]; }}>Cancel</button>
      <button class="modal-btn modal-btn-danger" onclick={async () => {
        if (pendingDelete) {
          const noteTitle = pendingDelete.noteTitle;
          await invoke('force_delete_note', { noteId: pendingDelete.noteId });
          allMessages = [...allMessages, { id: `msg-deleted-${Date.now()}`, thread_id: $activeChatThreadId || 'temp', role: 'assistant', content: `Deleted "${noteTitle}".`, created_at: new Date().toISOString() }];
        }
        showDeleteConfirm = false;
        pendingDelete = null;
      }}>Delete</button>
    </div>
  </div>
</div>
{/if}

<style>
  .chat-sidebar {
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
    overflow: hidden;
    position: relative;
    height: 100%;
  }
  
  /* Sidebar layout (default) — for project views */
  .chat-sidebar.layout-sidebar {
    width: 100%;
    min-width: 0;
  }
  
  /* Centered layout — for home view */
  .chat-sidebar.layout-centered {
    flex: 1;
    min-width: 0;
    flex-shrink: 1; /* overrides base .chat-sidebar flex-shrink:0 so the drawer can push us */
    max-width: 800px;
    margin: 0 auto;
    padding: 0 24px;
    overflow: hidden;
  }

  /* Modal styles for protection dialogs */
  .modal-overlay {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
    background: rgba(0, 0, 0, 0.7);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 9999;
  }
  .modal-panel {
    background: #0a0a0f;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 4px;
    padding: 24px;
    max-width: 700px;
    width: 90%;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .modal-title {
    font-size: 16px;
    font-weight: 600;
    color: #e0e0e0;
    margin: 0 0 12px;
  }
  .modal-desc {
    font-size: 13px;
    color: #999;
    margin: 0 0 16px;
    line-height: 1.5;
  }
  .modal-diff {
    display: flex;
    gap: 12px;
    margin-bottom: 20px;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  .diff-col {
    flex: 1;
    min-width: 0;
    max-height: 50vh;
    overflow-y: auto;
  }
  .diff-label {
    display: block;
    font-size: 11px;
    font-weight: 600;
    color: #666;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 6px;
  }
  .diff-content {
    background: rgba(255, 255, 255, 0.05);
    border-radius: 4px;
    padding: 12px;
    font-size: 12px;
    color: #ccc;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 300px;
    overflow-y: auto;
    margin: 0;
    font-family: system-ui, -apple-system, sans-serif;
  }
  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .modal-btn {
    padding: 8px 16px;
    border-radius: 4px;
    font-size: 13px;
    font-weight: 600;
    border: none;
    cursor: pointer;
    transition: opacity 0.15s;
  }
  .modal-btn:hover { opacity: 0.85; }
  .modal-btn-primary {
    background: rgba(0, 228, 228, 0.2);
    color: #00E4E4;
  }
  .modal-btn-secondary {
    background: rgba(255, 255, 255, 0.1);
    color: #888;
  }
  .modal-btn-danger {
    background: rgba(255, 68, 68, 0.2);
    color: #FF4444;
  }
  .synthesis-content {
    max-height: 400px;
    overflow-y: auto;
    white-space: pre-wrap;
    font-family: monospace;
    font-size: 12px;
  }
</style>
