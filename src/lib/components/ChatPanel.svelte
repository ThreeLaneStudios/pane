<script lang="ts">
  import { marked } from 'marked';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { open } from '@tauri-apps/plugin-shell';
  import * as api from '$lib/api';
  import type { ChatMessage, Session } from '$lib/types';
  import SessionDropdownHeader from '$lib/components/SessionDropdownHeader.svelte';
  import ReadOnlySessionNotice from '$lib/components/ReadOnlySessionNotice.svelte';
  import { formatModelName } from '$lib/utils/modelNames';
  import { FRESH_SESSION_HINT, FRESH_SESSION_HINT_HOME, sessionStore } from '$lib/stores/sessionStore';
  import { isSystemNoiseMessage } from '$lib/services/messageVisibility';
  import { renderMarkdown } from '$lib/utils/markdown';
  import { getAgents } from '$lib/api';
  import { agentRuntimeType } from '$lib/utils/agentDisplay';
  import type { Agent } from '$lib/types';
  import { chipDisplayText, isTerminalJobStatus, type JobStatusSnapshot } from '$lib/jobStatusChip';

  // #2682: gates the [2604-PROBE] console diagnostics below. Off by default —
  // set localStorage.pane-debug=1 to enable (same runtime flag already used
  // by ChatSidebar.svelte, Editor.svelte, KanbanBoard.svelte, etc. for
  // verbose diagnostic logging). Was previously unconditional, polluting
  // production console output on every session's first stream token.
  const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';

  // Custom marked extension for status indicators: ::status[overdue], ::status[due-today], ::status[on-track]
  const statusExtension = {
    name: 'status',
    level: 'inline' as const,
    start(src: string) { return src.indexOf('::status['); },
    tokenizer(src: string) {
      const match = src.match(/^::status\[([\w-]+)\]/);
      if (match) {
        return {
          type: 'status',
          raw: match[0],
          value: match[1]
        };
      }
    },
    renderer(token: any) {
      const statuses = new Set(['overdue', 'due-today', 'on-track', 'done', 'blocked']);
      const status = statuses.has(token.value) ? token.value : 'unknown';
      const label = token.value.replace(/-/g, ' ');
      return `<span class="status-indicator status--${status}"><span class="status-dot"></span>${label}</span>`;
    }
  };

  // Custom marked extension for priority pills: ::priority[high], ::priority[medium], ::priority[low]
  const priorityExtension = {
    name: 'priority',
    level: 'inline' as const,
    start(src: string) { return src.indexOf('::priority['); },
    tokenizer(src: string) {
      const match = src.match(/^::priority\[([\w-]+)\]/);
      if (match) {
        return {
          type: 'priority',
          raw: match[0],
          value: match[1]
        };
      }
    },
    renderer(token: any) {
      const priorities = new Set(['high', 'critical', 'medium', 'low']);
      const priority = priorities.has(token.value) ? token.value : 'low';
      return `<span class="priority-pill priority--${priority}">${token.value}</span>`;
    }
  };

  marked.use({ extensions: [statusExtension, priorityExtension] });

  interface Props {
    messages: ChatMessage[];
    isStreaming?: boolean;
    streamingContent?: string;
    streamingMessageId?: string | null;
    onSendMessage: (content: string, attachmentIds?: string) => void;
    /**
     * §36.2.3 (#3050 PR 3): the Stop button's click handler. Shown while
     * `isStreaming` is true (new UI — no Stop affordance existed in the
     * frontend before this PR, confirmed by Reed's scope audit §3.4).
     * Undefined = the button isn't rendered (callers that don't support
     * stopping a turn, if any, simply omit the prop).
     */
    onStop?: () => void;
    placeholder?: string;
    projectName?: string;
    projectId?: string;
    disabled?: boolean;
    showHeader?: boolean;
    headerTitle?: string;
    headerSubtitle?: string;
    onClose?: () => void;
    memoryIndicator?: string;
    memoryToast?: string;
    memoryToastId?: string;
    onDismissMemoryToast?: () => void;
    isCompacting?: boolean;
    connectionError?: string;
    /** §22C.12.3 — structured error code from backend; drives Re-pair button for gateway_auth_failed (#946). */
    gatewayErrorCode?: string;
    onClearConnectionError?: () => void;
    /**
     * #3036 PR2 item 4 — visible banner for a `navigate` stream payload
     * `navigationDispatch.ts`'s `decideNavigationAction` couldn't resolve
     * (unrecognized view, missing target, or an id that doesn't resolve).
     * Null/empty = no banner.
     */
    navError?: string | null;
    onClearNavError?: () => void;
    onOpenSettings?: () => void;
    /** Called after a session switch; ChatSidebar loads messages for the new session. */
    onSessionSwitch?: (session: Session) => void;
    onLoadOlder?: () => Promise<boolean>;
    hasMoreMessages?: boolean;
    loadingOlder?: boolean;
    /** §22C.12.4 — Send-only disabled; input field remains editable (#945).
     * Set to true while in Reconnecting state: user can type but not send. */
    sendDisabled?: boolean;
    /** §22C.12.4 — Passive reconnect status text shown near the input area (#945).
     * Undefined/empty = no indicator. Non-empty = show indicator. */
    reconnectIndicator?: string;
    /**
     * True when the active session was just created or has no messages yet.
     * When true and messages.length === 0, show the fresh-session hint instead
     * of the generic placeholder.
     */
    isFreshSession?: boolean;
    /**
     * C19 (#3120 PR 3): job-aware copy for a job session's own empty
     * state ("Morning brief · Brit · next run tomorrow 07:30. Reports
     * land here."), computed by the caller (`ChatSidebar.svelte`, via
     * `jobAwareEmptyStateHint`) when `isFreshSession` resolves true for a
     * session that IS a job's own session. `null`/undefined = not a job
     * session (or not yet resolved) — falls back to the existing generic
     * `FRESH_SESSION_HINT`/`FRESH_SESSION_HINT_HOME` copy, unchanged.
     * Composer stays enabled either way (Matt's ruling, C19's other
     * half) — `isFreshSession` has never gated the composer, only this
     * hint text, so no change was needed there.
     */
    jobAwareHint?: string | null;
    /**
     * D22 (#2649 family): externally-computed read-only/lock state for
     * surfaces where SessionDropdownHeader never mounts (showHeader=false),
     * i.e. Home. ChatSidebar computes this via the shared
     * `sessionReadOnlyCheck.checkSessionReadOnly()` helper (the same logic
     * SessionDropdownHeader uses internally) and passes it straight through.
     * Ignored when showHeader is true — SessionDropdownHeader's own
     * onReadOnlyChange callback is the source of truth in that case.
     */
    readOnlyState?: { reason: 'agent' | 'provider'; agent: Agent } | null;
    /**
     * #2643: externally-computed bound-agent name for surfaces where
     * SessionDropdownHeader never mounts (showHeader=false), i.e. Home.
     * ChatSidebar resolves this via `resolveBoundAgent()` (the shared,
     * pure helper exported from sessionReadOnlyCheck.ts) and passes it
     * straight through. Ignored when showHeader is true —
     * SessionDropdownHeader's own onBoundAgentChange callback is the
     * source of truth in that case.
     */
    boundAgentNameProp?: string | null;
    /**
     * #2604: externally-computed "has the agent roster loaded at least once"
     * flag for surfaces where SessionDropdownHeader never mounts (Home).
     * ChatSidebar resolves this alongside `boundAgentNameProp` via its own
     * `getAgents()` call and passes it straight through. Ignored when
     * showHeader is true — SessionDropdownHeader's own onAgentsLoadedChange
     * callback is the source of truth in that case. See
     * `internalAgentsLoaded`/`sessionAgentsLoaded` below.
     */
    agentsLoadedProp?: boolean;
    /**
     * §36.7 (#3050 PR 6): the session currently on screen, for filtering
     * `pane-chip-changed` events — ChatPanel only owns `messages` (already
     * filtered to one thread by the caller), never a thread id of its own,
     * so without this an insert into a brand-new (zero-message) session
     * couldn't be told apart from an event belonging to a DIFFERENT open
     * session. `null`/undefined = never matches (defensive: no spurious
     * re-query before the caller has resolved a thread).
     */
    activeThreadId?: string | null;
    /**
     * §36.7/§2 (#3050 PR 6): called when a `pane-chip-changed` event with
     * `status: "inserted"` arrives for `activeThreadId` — a brand-new chip
     * message that ChatPanel's own `messages` prop doesn't have yet. Only
     * the caller (ChatSidebar) owns `allMessages`/`loadMessages`, so
     * ChatPanel can't re-query itself; this is the one hand-off. A status
     * FLIP (`sent`/`done`/`failed`/`running`) on an EXISTING chip never
     * reaches this callback — that's handled entirely inside ChatPanel via
     * `jobStatuses`, see the chip-changed listener below.
     */
    onChipInserted?: (jobId: string, sessionId: string) => void;
    /**
     * #3151 PR1 Commit D: called when the user clicks a job-output link —
     * a `pane://note/{id}` anchor rendered from a `[title](pane://note/
     * {id})` token (`output_link_token`, `ai/tools.rs`) a job turn's
     * `create_note`/`create_sheet` call appended to its own-session reply
     * or the origin receipt. `ChatPanel` doesn't own cross-project
     * navigation (confirmed — `ChatSidebar.svelte`'s own
     * `handleNavigateFromStream`/`decideNavigationAction` is the one real
     * mechanism, scoped to live-stream `navigate` payloads today); this
     * callback is the hand-off, the same shape every other cross-
     * component action in this component already uses (`onSessionSwitch`,
     * `onChipInserted`, …). `undefined` = the caller doesn't support it
     * (e.g. `WorkshopReview.svelte`'s onboarding chat, which never renders
     * job output) — `handleLinkClick` below just does nothing for a
     * `pane://note/` href in that case, same as it already does for any
     * other non-`http(s)` href.
     */
    onNoteLinkClick?: (noteId: string) => void;
    /**
     * #3151 PR1 Commit F: true when there's a one-deep back target to
     * return to (`$jobSessionBackTarget !== null`, `$lib/stores/app.ts`)
     * — set by the caller (`ChatSidebar.svelte`), which owns that store
     * subscription; `ChatPanel` itself stays store-free, matching every
     * other externally-computed prop on this component
     * (`readOnlyState`/`boundAgentNameProp`/etc.). `false`/undefined =
     * no button rendered at all (not disabled-but-visible) — this IS the
     * "second press is a no-op" mechanism, by construction.
     */
    showBackButton?: boolean;
    /** Calls `goToJobSessionBackTarget` (`$lib/services/jobSessionBack.ts`)
     * — the one shared "go back" action the `Cmd+[` shortcut
     * (`+page.svelte`) also calls, so there is exactly one implementation
     * of what going back does. */
    onBackClick?: () => void;
  }

  let {
    messages,
    isStreaming = false,
    streamingContent = '',
    streamingMessageId = null,
    onSendMessage,
    onStop,
    // C20 (#3120 PR 3): one placeholder string app-wide, replacing the
    // old 'Ask anything...' default (this component's own fallback, used
    // whenever a caller — Onboarding.svelte's workshop chat — doesn't
    // override it) and ChatSidebar.svelte's own 'Ask anything about your
    // notes…'/'…about this project…' template (fixed at that call site).
    placeholder = 'Message your agents…',
    projectName,
    projectId = '__global__',
    disabled = false,
    showHeader = false,
    headerTitle = 'AI CHAT',
    headerSubtitle = '',
    onClose,
    memoryIndicator = '',
    memoryToast = '',
    memoryToastId = '',
    onDismissMemoryToast,
    isCompacting = false,
    connectionError = '',
    gatewayErrorCode = '',
    onClearConnectionError,
    navError = null,
    onClearNavError,
    onOpenSettings,
    onSessionSwitch,
    onLoadOlder,
    hasMoreMessages = true,
    loadingOlder = false,
    sendDisabled = false,
    reconnectIndicator = '',
    isFreshSession = false,
    jobAwareHint = null,
    readOnlyState: readOnlyStateProp = null,
    boundAgentNameProp = null,
    agentsLoadedProp = false,
    activeThreadId = null,
    onChipInserted,
    onNoteLinkClick,
    showBackButton = false,
    onBackClick,
  }: Props = $props();

  let messageInput = $state('');
  let messagesContainer: HTMLElement;
  let textareaEl: HTMLTextAreaElement;
  let isDragOver = $state(false);
  let pendingAttachments: Array<{id: string; filename: string; mime_type: string; size: number; path: string; previewUrl?: string}> = $state([]);
  let mainAgentName = $state('Pane'); // Default fallback
  // #2604 RC-B: the ACTIVE SESSION's own bound agent name (via
  // SessionDropdownHeader's onBoundAgentChange — same `boundAgent` derivation
  // used for the binding line: `activeSession.agent_id` → `allAgents.find`).
  // This is what the speaker-label fallback must use, NEVER `mainAgentName`
  // (the app-wide default) — see docs/audits/2026-08-20-c8-session-binding
  // -investigation.md Root Cause B / fix spec #2604.
  let internalBoundAgentName = $state<string | null>(null);
  // #2643: on Home, SessionDropdownHeader never mounts, so
  // internalBoundAgentName never updates via onBoundAgentChange. Fall back
  // to the externally-computed `boundAgentNameProp` ChatSidebar passes in
  // (resolved via the shared `resolveBoundAgent()` helper) in that case —
  // same mirrored pattern as `readOnlyState` below.
  let sessionBoundAgentName = $derived(
    showHeader ? internalBoundAgentName : (boundAgentNameProp ?? null)
  );
  // #2604: mirrors SessionDropdownHeader's `agentsLoaded` (project surfaces)
  // or ChatSidebar's equivalent Home-surface flag (agentsLoadedProp) —
  // true once the agent roster has resolved at least once for the CURRENT
  // session-open. While false, `sessionBoundAgentName` being null does NOT
  // mean "no bound agent"; it means "haven't checked yet", so the render
  // below must not fall through to the 'Pane' literal in that window.
  let internalAgentsLoaded = $state(false);
  let sessionAgentsLoaded = $derived(
    showHeader ? internalAgentsLoaded : agentsLoadedProp
  );

  // ADR-0063 D6: derived, never-stored read-only state — the active session's
  // bound agent is inactive, or its own runtime is genuinely disconnected. Set
  // by SessionDropdownHeader's onReadOnlyChange (it already loads the agent +
  // runtime-connection facts for the binding line, so this reuses that fetch
  // instead of duplicating it here).
  let internalReadOnlyState = $state<{ reason: 'agent' | 'provider'; agent: Agent } | null>(null);
  // D22 (#2649 family): when showHeader is false (Home), SessionDropdownHeader
  // never mounts, so internalReadOnlyState never updates via onReadOnlyChange.
  // Fall back to the externally-computed `readOnlyStateProp` ChatSidebar passes
  // in from the shared sessionReadOnlyCheck helper in that case.
  let readOnlyState = $derived(showHeader ? internalReadOnlyState : (readOnlyStateProp ?? null));
  // #2563 B2 fix: previously read `ai_settings.provider` (the retired global
  // scalar) via `aiLoadConfig()`, which could disagree with the bound agent's
  // own identity. The banner's label is now derived directly from the bound
  // agent's own runtime — same rule the header uses — so header and banner
  // always agree.
  let readOnlyProvider = $derived(readOnlyState ? agentRuntimeType(readOnlyState.agent) : 'none');

  async function handleStartNewSessionFromReadOnly() {
    try {
      const agents = await getAgents();
      const defaultAgent = agents.find(a => a.is_default) ?? agents[0];
      if (!defaultAgent) return;
      await sessionStore.createSession(projectId === '__global__' ? 'workspace' : projectId, defaultAgent.id);
    } catch (e) {
      console.error('[ChatPanel] failed to start new session from read-only notice:', e);
    }
  }

  function handleOpenReadOnlyCause() {
    if (readOnlyState?.reason === 'provider') {
      onOpenSettings?.();
    } else {
      onOpenSettings?.();
    }
  }

  // Error toast state (#1822)
  let errorToastMsg = $state<string | null>(null);
  let errorToastTimer: ReturnType<typeof setTimeout> | null = null;

  function showErrorToast(msg: string) {
    errorToastMsg = msg;
    if (errorToastTimer) clearTimeout(errorToastTimer);
    errorToastTimer = setTimeout(() => { errorToastMsg = null; }, 5000);
  }

  // FIX #421: Reset input when switching projects
  let previousProjectId = $state(projectId);
  $effect(() => {
    if (projectId !== previousProjectId && previousProjectId !== undefined) {
      messageInput = '';
      pendingAttachments = [];
    }
    previousProjectId = projectId;
  });

  // Load main agent name on mount and when renamed.
  // #2604 RC-B: no longer listens for `pane-agent-switched` (the app-wide
  // default-agent-change event). A bound session's speaker labels must not
  // relabel when the user changes the *default* agent elsewhere in the app —
  // `mainAgentName` still exists (used for other legacy/non-attributed UI
  // copy) but is deliberately NOT reactive to that event anymore, and is
  // NEVER used as the speaker-label fallback (see chat-sender render below).
  $effect(() => {
    loadMainAgentName();

    // Listen for agent rename events and onboarding completion only.
    const handleAgentRename = () => loadMainAgentName();
    const handleOnboardingComplete = () => loadMainAgentName();
    window.addEventListener('pane-agent-renamed', handleAgentRename);
    window.addEventListener('pane-onboarding-complete', handleOnboardingComplete);

    return () => {
      window.removeEventListener('pane-agent-renamed', handleAgentRename);
      window.removeEventListener('pane-onboarding-complete', handleOnboardingComplete);
    };
  });

  async function loadMainAgentName() {
    try {
      const agent = await invoke<{ name: string } | null>('get_main_agent');
      if (agent && agent.name) {
        mainAgentName = agent.name;
        return;
      }
    } catch (e) {
      console.log('No main agent found, using default name:', e);
    }
  }

  // Save to Memory state (#69)
  let saveMemoryMessage = $state<ChatMessage | null>(null);
  let saveMemoryFact = $state('');
  let saveMemoryCategory = $state('fact');
  let saveMemoryToastText = $state('');
  const memoryCategories = ['identity', 'preference', 'style', 'goal', 'fact', 'decision', 'deadline', 'reflection'];

  function openSaveMemory(message: ChatMessage) {
    saveMemoryMessage = message;
    // Pre-fill with a condensed version - strip markdown and truncate
    const plain = message.content.replace(/[#*_~`>\[\]()!]/g, '').trim();
    saveMemoryFact = plain;
    saveMemoryCategory = 'fact';
  }

  function closeSaveMemory() {
    saveMemoryMessage = null;
    saveMemoryFact = '';
    saveMemoryCategory = 'fact';
  }

  async function confirmSaveMemory() {
    if (!saveMemoryFact.trim() || !saveMemoryMessage) return;
    try {
      await invoke('save_to_memory', {
        fact: saveMemoryFact.trim(),
        category: saveMemoryCategory,
        sourceMessageId: saveMemoryMessage.id || null,
        projectId: projectId === '__global__' ? null : projectId,
      });
      closeSaveMemory();
      window.dispatchEvent(new CustomEvent('pane-memory-changed'));
      saveMemoryToastText = 'Saved to memory';
      setTimeout(() => { saveMemoryToastText = ''; }, 2500);
    } catch (e) {
      console.error('Failed to save to memory:', e);
    }
  }

  function autoResize() {
    if (!textareaEl) return;
    textareaEl.style.height = 'auto';
    textareaEl.style.height = Math.min(textareaEl.scrollHeight, 300) + 'px';
  }

  let expandedToolMsgs: Set<string> = $state(new Set());

  // #1706: long user-message collapse
  let tallUserMsgs: Set<string> = $state(new Set());
  let expandedUserMsgs: Set<string> = $state(new Set());

  function measureUserMsg(el: HTMLElement, params: { id: string; role: string }) {
    if (params.role !== 'user') return { destroy() {} };
    requestAnimationFrame(() => {
      if (el.scrollHeight > 168) {
        tallUserMsgs = new Set([...tallUserMsgs, params.id]);
      }
    });
    return { destroy() {} };
  }

  function stripToolBlocks(text: string): string {
    if (!text) return text;
    let result = text.replace(/\`\`\`json\s*\{[^\`]*?"tool"\s*:[^\`]*?\}\s*\`\`\`/gs, '');
    result = result.replace(/^\s*\{"tool"\s*:.*?\}\s*$/gm, '');
    // Strip embedded JSON error objects from AI reply text (#137)
    result = result.replace(/\{"error"\s*:\s*"[^"]*"\}/g, (match) => {
      console.warn('[TOOL-ERROR-HIDDEN-TEXT]', match);
      return '';
    });
    result = result.replace(/\n{3,}/g, '\n\n').trim();
    return result;
  }

  // #3151 PR1 Commit D: the job-output link's own scheme, checked ahead of
  // the http(s)-only branch below. Matches `output_link_token`'s
  // (`ai/tools.rs`) literal `pane://note/{id}` shape — `id` is everything
  // after the one `/`, no further parsing (a note/sheet id is an opaque
  // UUID string, never itself containing a `/`).
  const PANE_NOTE_LINK_PREFIX = 'pane://note/';

  // FIX #242: Open links in external browser instead of inside WebView
  function handleLinkClick(event: MouseEvent) {
    const target = event.target as HTMLElement;
    const anchor = target.closest('a');
    if (anchor && anchor.href) {
      event.preventDefault();
      event.stopPropagation();
      const href = anchor.getAttribute('href') || '';
      // #3151 PR1 Commit D: a job-output link — never opened externally
      // (it isn't a real URL any shell/browser could resolve), handed to
      // the caller instead. Checked BEFORE the http(s) branch so it can
      // never fall through to "open externally" by accident.
      if (href.startsWith(PANE_NOTE_LINK_PREFIX)) {
        const noteId = href.slice(PANE_NOTE_LINK_PREFIX.length);
        if (noteId && onNoteLinkClick) {
          onNoteLinkClick(noteId);
        }
        return;
      }
      // Only open external URLs, not internal anchors
      if (href.startsWith('http://') || href.startsWith('https://')) {
        open(href);
      }
    }
  }

  function scrollToMessage(messageId: string) {
    const el = messagesContainer?.querySelector(`[data-message-id="${messageId}"]`);
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' });
      el.classList.add('highlight-flash');
      setTimeout(() => el.classList.remove('highlight-flash'), 2000);
    }
  }

  async function handleScroll() {
    if (!messagesContainer || !onLoadOlder || !hasMoreMessages || loadingOlder) return;

    if (messagesContainer.scrollTop < 100) {
      const prevHeight = messagesContainer.scrollHeight;
      const prevScroll = messagesContainer.scrollTop;

      const success = await onLoadOlder();

      if (success) {
        // Wait for DOM update
        setTimeout(() => {
          const newHeight = messagesContainer.scrollHeight;
          messagesContainer.scrollTop = prevScroll + (newHeight - prevHeight);
        }, 0);
      }
    }
  }

  let attachmentCache: Map<string, any> = new Map();

  function getAttachmentRenderer(mimeType: string): 'image' | 'audio' | 'video' | 'pdf' | 'text' | 'file' {
    if (mimeType.startsWith('image/')) return 'image';
    if (mimeType.startsWith('audio/')) return 'audio';
    if (mimeType.startsWith('video/')) return 'video';
    if (mimeType === 'application/pdf') return 'pdf';
    if (mimeType.startsWith('text/')) return 'text';
    return 'file';
  }

  function formatFileSize(bytes: number): string {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
    return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
  }

  async function handleFilePaste(e: ClipboardEvent) {
    const files = e.clipboardData?.files;
    if (!files || files.length === 0) return;
    e.preventDefault();
    for (const file of Array.from(files)) {
      await addFileAttachment(file);
    }
  }

  async function addFileAttachment(file: File) {
    if (file.size > 50 * 1024 * 1024) {
      showErrorToast('Files over 50 MB aren\'t supported in chat.');
      return;
    }
    const reader = new FileReader();
    reader.onload = async () => {
      const base64Full = reader.result as string;
      const base64Data = base64Full.split(',')[1];
      try {
        const result = await api.uploadAttachment(file.name, base64Data, projectId);
        const previewUrl = file.type.startsWith('image/') ? base64Full : undefined;
        pendingAttachments = [...pendingAttachments, {
          id: result.id,
          filename: result.filename || file.name,
          mime_type: result.mime_type || file.type,
          size: result.size || file.size,
          path: result.path,
          previewUrl,
        }];
      } catch (err) {
        console.error('Failed to upload attachment:', err);
      }
    };
    reader.readAsDataURL(file);
  }

  function removeAttachment(id: string) {
    pendingAttachments = pendingAttachments.filter(a => a.id !== id);
    api.deleteAttachment(id).catch(() => {});
  }

  /**
   * #3050 PR 9, docs/spec/runtime.md §36.9.2 — the picker upload path: the
   * one real gap Reed's scope audit found between notes' three upload
   * paths (drag-drop, picker, paste) and chat's prior two (drag-drop,
   * paste). Mirrors `Editor.svelte::handleToolbarAttachFile` almost
   * verbatim — same two calls (`plugin-dialog`'s `open()`, then
   * `api.copyAttachmentFromPath`), same native `fs::copy` path (1 GB cap,
   * no 50 MB client-side base64 cap since it bypasses the FileReader
   * pipeline entirely) — just pushing onto `pendingAttachments` instead of
   * inserting a Tiptap node. `open()` returning `null` (user cancelled) is
   * a deliberate no-op, not an error path.
   */
  async function handleAttachFilePicker() {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const sel = await open({ multiple: true });
    if (!sel) return;
    const paths = Array.isArray(sel) ? sel : [sel];
    for (const filePath of paths) {
      const filename = filePath.split('/').pop() || filePath;
      try {
        const result = await api.copyAttachmentFromPath(filename, filePath, projectId);
        pendingAttachments = [...pendingAttachments, {
          id: result.id,
          filename: result.filename || filename,
          mime_type: result.mime_type,
          size: result.size,
          path: result.path,
        }];
      } catch (err) {
        console.error('Failed to attach file via picker:', err);
        showErrorToast(err instanceof Error ? err.message : 'Failed to attach file.');
      }
    }
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    isDragOver = true;
  }

  function handleDragLeave(e: DragEvent) {
    e.preventDefault();
    isDragOver = false;
  }

  async function handleDrop(e: DragEvent) {
    e.preventDefault();
    isDragOver = false;
    const files = e.dataTransfer?.files;
    if (!files) return;
    for (const file of Array.from(files)) {
      await addFileAttachment(file);
    }
  }

  async function loadAttachmentMeta(attachmentId: string): Promise<any> {
    if (attachmentCache.has(attachmentId)) return attachmentCache.get(attachmentId);
    try {
      const meta = await api.getAttachment(attachmentId);
      // For images/audio/video, load base64 data for rendering
      if (meta.mime_type && (meta.mime_type.startsWith('image/') || meta.mime_type.startsWith('audio/') || meta.mime_type.startsWith('video/'))) {
        try {
          const dataResult = await api.getAttachmentData(attachmentId);
          meta.dataUrl = `data:${meta.mime_type};base64,${dataResult.data}`;
        } catch (e) { console.error('[ATTACHMENT] Failed to load data:', e); }
      }
      attachmentCache.set(attachmentId, meta);
      return meta;
    } catch { return null; }
  }

  export function scrollToBottom() {
    setTimeout(() => {
      if (messagesContainer) {
        messagesContainer.scrollTop = messagesContainer.scrollHeight;
      }
    }, 50);
  }

  $effect(() => {
    messages;
    streamingContent;
    scrollToBottom();
  });

  function sendMessage() {
    // §22C.12.4: sendDisabled blocks Send while in Reconnecting state but does NOT
    // block the input field — user can type but not submit during backoff.
    if ((!messageInput.trim() && pendingAttachments.length === 0) || disabled || sendDisabled) return;
    const content = messageInput.trim();
    const attachIds = pendingAttachments.map(a => a.id).join(',');
    messageInput = '';
    // Reset textarea height
    if (textareaEl) {
      textareaEl.style.height = 'auto';
    }
    pendingAttachments = [];
    onSendMessage(content, attachIds || undefined);
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  }

  function formatTimestamp(timestamp: string): string {
    const date = new Date(timestamp.endsWith('Z') ? timestamp : timestamp + 'Z');
    if (isNaN(date.getTime())) return '';
    const now = new Date();
    const isToday = date.toDateString() === now.toDateString();
    const hours = date.getHours();
    const minutes = date.getMinutes().toString().padStart(2, '0');
    const ampm = hours >= 12 ? 'PM' : 'AM';
    const displayHours = hours % 12 || 12;
    const timeStr = `${displayHours}:${minutes} ${ampm}`;
    if (isToday) return timeStr;
    return `${date.getMonth() + 1}/${date.getDate()} ${timeStr}`;
  }

  // #2604: resolves the speaker-label state for an assistant message using
  // the SAME fallback chain as the render below, plus a one-time-per-message
  // console probe so a live repro's timing (agent-loaded state at first-token
  // render) can be inspected without instrumenting the template itself.
  // Reserves the 'Pane' literal for the genuine ADR-0059 D5 case — no bound
  // agent AND the roster has already loaded — never for the "still loading"
  // window that used to leak through as a false null (#2604). While loading,
  // returns `null` (name unresolved) rather than the literal 'Pane' string;
  // the template renders the neutral thinking-pulse treatment (reusing the
  // existing empty-content visual language, ChatPanel.svelte ~line 643) for
  // that case instead of text.
  const probedFirstTokenMessageIds = new Set<string>();
  function resolveSenderName(message: ChatMessage): string | null {
    const attributed = message.agent_name ?? null;
    const isFirstToken = streamingMessageId === message.id && !probedFirstTokenMessageIds.has(message.id);
    if (isFirstToken) {
      probedFirstTokenMessageIds.add(message.id);
      if (DEBUG) {
        console.log('[2604-PROBE] first-token render for message', message.id, {
          attributedAgentName: attributed,
          sessionBoundAgentName,
          sessionAgentsLoaded,
          showHeader,
          ts: performance.now(),
        });
      }
    }
    if (attributed) return attributed;
    if (sessionBoundAgentName) return sessionBoundAgentName;
    if (!sessionAgentsLoaded) return null;
    // #2794 backend-echo diagnostic: the 'Pane' literal fallback below is the
    // exact symptom of #2794 (onboarding sender label stuck on "Pane"). The
    // [2604-PROBE] console log above only fires when isFirstToken AND DEBUG
    // are both true — invisible in a production log capture. This echoes the
    // fallback hit through a lightweight backend command (eprintln!, no DEBUG
    // gate) every time it's reached, so it's visible in process logs during
    // live tests without requiring localStorage['pane-debug']. Fire-and-forget:
    // never blocks or throws into the render path (see #2794 diagnosis doc).
    invoke('log_sender_name_fallback_cmd', {
      threadId: message.thread_id,
      sessionAgentsLoaded,
    }).catch(() => {});
    return 'Pane';
  }

  // #3166 C55: the first 6 checks are shared with `navigation.ts`'s
  // `navigateTo` and `ChatSidebar.svelte`'s `loadMessages` via
  // `isSystemNoiseMessage` (messageVisibility.ts) — this component's
  // own render-time filter deliberately does NOT also exclude synthetic
  // rows (unlike those two loaders): a job/wake dispatch brief IS
  // synthetic and must flow through to the job-brief branch below.
  let visibleMessages = $derived(
    messages.filter(m =>
      !isSystemNoiseMessage(m) &&
      !m.content.includes('Session nearing compaction') &&
      !m.content.startsWith('Previous conversation summary:') &&
      !m.content.startsWith('📝 Previous conversation summary:') &&
      !m.content.startsWith('You just completed setup. Please introduce yourself')
    )
  );

  // §36.4/§36.4.5 (#3050 PR 5) — Matt's "chips show live status" addition.
  // `jobStatuses` is a plain Record (not a Map — Svelte 5's `$state` proxy
  // tracks plain-object key writes directly), keyed by `job_id`, holding
  // the last-known snapshot for every spawn/schedule chip currently
  // visible. The chip template (`jobStatuses[message.job_id]`) reads this
  // reactively — no manual DOM update needed, same as every other `$state`
  // read in this component.
  let jobStatuses: Record<string, JobStatusSnapshot> = $state({});
  let jobStatusPollHandle: ReturnType<typeof setInterval> | null = null;

  // Every chip's own job_id currently in view, whose last-known status
  // (or lack of one yet) is NOT terminal — these are the only ids worth
  // polling. Re-derives automatically as `visibleMessages`/`jobStatuses`
  // change (a newly-terminal job drops out next tick, same loop never
  // polls it again).
  let pollableJobIds = $derived(
    Array.from(
      new Set(
        visibleMessages
          .filter(m => m.job_id && !m.synthetic && !m.model)
          .map(m => m.job_id as string)
      )
    ).filter(id => !isTerminalJobStatus(jobStatuses[id]?.status))
  );

  /**
   * Shared merge primitive (One Function Per Operation) — both the 5s
   * poll loop below AND the `pane-chip-changed` listener's single-id
   * refresh write through this one spot, so "does this status actually
   * differ from what we have" and "how jobStatuses gets updated" can
   * never drift between the two call sites. Returns whether it wrote.
   */
  function applyJobStatus(id: string, status: JobStatusSnapshot | null): boolean {
    if (!status) return false;
    if (JSON.stringify(jobStatuses[id]) === JSON.stringify(status)) return false;
    jobStatuses = { ...jobStatuses, [id]: status };
    return true;
  }

  async function pollJobStatuses() {
    const ids = pollableJobIds;
    if (ids.length === 0) return;
    const results = await Promise.all(
      ids.map(async id => {
        try {
          return [id, await api.getJobStatus(id)] as const;
        } catch {
          // Defensive — a transient read failure must not crash the poll
          // loop or the component; just skip this id this tick.
          return [id, null] as const;
        }
      })
    );
    for (const [id, status] of results) {
      applyJobStatus(id, status);
    }
  }

  /**
   * §2 (#3050 PR 6): the `pane-chip-changed` listener's status-FLIP arm
   * (`sent`/`done`/`failed`/`running`/… — anything but `"inserted"`) — a
   * single targeted re-read of the one job that flipped, through the SAME
   * `get_job_status` command and the SAME `applyJobStatus` merge the 5s
   * poll uses, so the chip's "done in Ns" (which needs `started_at`/
   * `finished_at`, not just the event's own `status` string) renders
   * correctly without waiting up to 5s for the next poll tick to catch up.
   */
  async function handleChipStatusFlip(jobId: string) {
    try {
      applyJobStatus(jobId, await api.getJobStatus(jobId));
    } catch {
      // Defensive — same as pollJobStatuses: a transient read failure must
      // not crash the listener or the component.
    }
  }

  /**
   * §2 (#3050 PR 6): live chip refresh — Reed's audit's single most
   * consequential finding alongside `set_viewing_session`'s own frontend
   * gap (zero listeners anywhere in `src/` for this event, confirmed by
   * grep before this fix). Filters on `event.payload.session_id ===
   * activeThreadId`: the backend's own `chip_target_session`/
   * `chip_target_api` (see `scheduler.rs`'s `emit_chip_changed` call
   * sites) is always "the chat this job's chip/confirmation message
   * actually lives in" — the SAME filter the old 5s poll gets for free by
   * only ever looking at `visibleMessages`' own job ids. A job running in
   * a DIFFERENT session a user isn't looking at must not trigger any
   * re-query here — that's still the 5s poll's job for ITS OWN open chat
   * once it scrolls into view.
   *
   * Two sub-cases, matching the backend's own `status` values:
   *   - `"inserted"`: a brand-new chip message ChatPanel's `messages` prop
   *     doesn't have at all yet — hands off to `onChipInserted` so the
   *     caller (which owns `allMessages`/`loadMessages`) can re-query.
   *   - anything else (`sent`/`done`/`failed`/`running`/…): an EXISTING
   *     chip's status flipped — handled entirely in-component via
   *     `handleChipStatusFlip`/`jobStatuses`, no message reload needed.
   */
  $effect(() => {
    let cancelled = false;
    let unlistenFn: (() => void) | null = null;
    listen<{ job_id: string; session_id: string; status: string }>('pane-chip-changed', (event) => {
      const { job_id, session_id, status } = event.payload;
      if (!activeThreadId || session_id !== activeThreadId) return;
      if (status === 'inserted') {
        onChipInserted?.(job_id, session_id);
        return;
      }
      void handleChipStatusFlip(job_id);
    }).then(fn => {
      if (cancelled) {
        fn();
      } else {
        unlistenFn = fn;
      }
    });
    return () => {
      cancelled = true;
      unlistenFn?.();
    };
  });

  $effect(() => {
    // Poll on a fixed interval while there's at least one non-terminal
    // chip in view; tear down entirely once none remain (and on unmount,
    // via the returned cleanup) rather than polling forever in the
    // background for a session nobody's looking at.
    if (pollableJobIds.length === 0) {
      if (jobStatusPollHandle !== null) {
        clearInterval(jobStatusPollHandle);
        jobStatusPollHandle = null;
      }
      return;
    }
    if (jobStatusPollHandle === null) {
      jobStatusPollHandle = setInterval(pollJobStatuses, 5000);
      // Fire once immediately rather than waiting a full interval for the
      // first read — a chip that's already done by the time it scrolls
      // into view shouldn't sit at "running" for 5s for no reason.
      void pollJobStatuses();
    }
    return () => {
      if (jobStatusPollHandle !== null) {
        clearInterval(jobStatusPollHandle);
        jobStatusPollHandle = null;
      }
    };
  });
</script>

<div class="chat-panel" ondragover={handleDragOver} ondragleave={handleDragLeave} ondrop={handleDrop} class:drag-over={isDragOver}>
  {#if showHeader}
    <!-- §395 PR7: SessionDropdownHeader replaces the old .chat-header div.
         onClose is threaded through so the collapse button still works. -->
    <SessionDropdownHeader
      projectId={projectId || '__global__'}
      projectName={headerTitle}
      {onClose}
      {onSessionSwitch}
      onReadOnlyChange={(state) => { internalReadOnlyState = state; }}
      onBoundAgentChange={(name) => { internalBoundAgentName = name; }}
      onAgentsLoadedChange={(loaded) => { internalAgentsLoaded = loaded; }}
    />
  {/if}

  {#if reconnectIndicator}
    <!-- §22C.12.4: Passive reconnect indicator — does NOT block input (#945) -->
    <div class="reconnect-indicator" role="status" aria-live="polite">
      <span class="reconnect-icon">↻</span>
      <span>{reconnectIndicator}</span>
    </div>
  {/if}

  {#if connectionError}
    <div class="connection-error">
      <span>{connectionError}</span>
      <div class="connection-error-actions">
        {#if gatewayErrorCode === 'gateway_auth_failed'}
          <!-- §22C.12.3 — Re-pair navigates to Settings where the re-pair flow lives (#946) -->
          <button onclick={() => onOpenSettings?.()}>Re-pair</button>
        {:else}
          <button onclick={() => onOpenSettings?.()}>Settings</button>
        {/if}
        <button onclick={() => onClearConnectionError?.()}>✕</button>
      </div>
    </div>
  {/if}

  {#if navError}
    <!-- #3036 PR2 item 4 — visible, dismissible nav-failure banner. Reuses
         .connection-error's styling (same "something went wrong" banner
         shape) but with no Re-pair/Settings action — a navigation failure
         isn't a connection problem. -->
    <div class="connection-error" role="alert">
      <span>{navError}</span>
      <div class="connection-error-actions">
        <button onclick={() => onClearNavError?.()}>✕</button>
      </div>
    </div>
  {/if}

  <div class="messages-area" bind:this={messagesContainer} onscroll={handleScroll}>
    {#if messages.length === 0}
      <div class="empty-state">
        {#if isFreshSession}
          <span>{jobAwareHint ?? (projectId === '__global__' ? FRESH_SESSION_HINT_HOME : FRESH_SESSION_HINT)}</span>
        {:else}
          <span>{placeholder}</span>
        {/if}
      </div>
    {:else}
      {#each visibleMessages as message (message.id)}
        {#if message.continued}
          <!-- §36.2.1/§36.2.3 (#3050 PR 3): the progress divider. The row's
               own `content` IS the exact copy ("Continuing — N tool calls so
               far.", written by `insert_continuation_messages`) — no new UI
               state needed (§36.2.3's own framing), just a different
               rendering for this one flag instead of a normal chat bubble. -->
          <div class="continuation-divider" data-message-id={message.id}>
            <span class="continuation-divider-line"></span>
            <span class="continuation-divider-text">{message.content}</span>
            <span class="continuation-divider-line"></span>
          </div>
        {:else if message.job_id && !message.synthetic && !message.model}
          <!-- §36.3.6/§36.3.7 (#3050 PR 4) + §7.2/§36.4 (#3050 PR 5): the
               Pane-written schedule/spawn confirmation chip ("Scheduled:
               Monday reminder · next Mon 9:00 AM" / "Spawned agent on
               '...'"), written by `insert_schedule_confirmation_message` on
               a successful `schedule_job`/`spawn_job` call — never by the
               model. Same non-chat-bubble treatment as the continuation
               divider above (no sender name, no avatar, no save-to-memory
               button): this is Pane reporting a fact about what it did, not
               the agent speaking.

               `&& !message.model` (#3050 PR 5, Reed's scope audit §7.2) is
               the discriminator that keeps the REPORT-BACK message (also
               `job_id` set, also `synthetic=0` — both written after PR 5)
               OUT of this branch: `insert_schedule_confirmation_message`
               never sets `model` (NULL for every Pane-templated chip),
               while report-back's `ExternalMessage.model` carries the job's
               own model. A report-back message therefore falls through to
               the `{:else}` normal-bubble branch below, picking up its
               sender name/avatar/save-to-memory button automatically — it
               IS the agent speaking, with full attribution, per §36.8.1.

               4d-skip: visual rendering needs Matt's live pass (no
               connected node this session to click through — both the
               chip's own rendering AND the report-back-renders-as-a-normal-
               bubble behavior); the DATA reaching here is tested end to end
               via real round trips (`ai::tools::schedule_family_
               tests_3050_pr4::test_schedule_job_from_project_chat_row_in_
               home_query_and_chip_with_job_id`; `ai::scheduler::
               scheduler_v2_tests::test_spawn_job_report_back_lands_in_
               origin_session_attributed_and_tagged` for report-back's own
               `model`/`job_id` values). -->
          <div class="schedule-chip" data-message-id={message.id} data-job-id={message.job_id} data-job-status={jobStatuses[message.job_id as string]?.status ?? 'pending'}>
            <span class="schedule-chip-icon">⏱</span>
            <span class="schedule-chip-text">{chipDisplayText(message.content, jobStatuses[message.job_id as string])}</span>
          </div>
        {:else if message.job_id && message.synthetic && message.role === 'user'}
          <!-- C32 (#3128): the job/wake dispatch BRIEF — written by
               `insert_job_brief_message` with role='user', synthetic=1,
               job_id set — fell through to the normal chat-bubble branch
               below, which only reads `message.role` (hardcoded 'user' by
               that insert). Result: a Pane-authored dispatch brief, no
               human involved, rendered pixel-identical to something the
               user typed. This is the missing FOURTH branch: distinct
               from the chip branch above (that one requires
               `!synthetic`) and from the normal bubble below, same
               "this is bookkeeping, not a person" visual language as the
               continuation divider. -->
          <div class="job-brief" data-message-id={message.id} data-job-id={message.job_id}>
            <span class="job-brief-label">
              {#if showBackButton}
                <!-- #3155 C49b: the one-deep back button moved HERE, out
                     of its old floating position, attached to the left
                     edge of the JOB BRIEF strip — rendered only while
                     `showBackButton` is true, which (per
                     `ChatSidebar.svelte`'s own derivation) requires BOTH a
                     pending back target AND the active session's own
                     `origin` being a job session's (`'spawn'`/`'schedule'`).
                     This `.job-brief` block ALSO renders for a wake job's
                     brief sitting in its ORIGIN chat (that chat's own
                     `origin` is never 'spawn'/'schedule') — the two halves
                     compose for free here: the glyph is absent there
                     because `showBackButton` is false there, never because
                     this markup special-cases job KIND. One-deep, same as
                     before: the `{#if}` guard IS the "second press is a
                     no-op" mechanism — once `onBackClick` clears the store
                     (`goToJobSessionBackTarget`), `showBackButton` goes
                     false and the glyph is gone from the DOM, not merely
                     disabled. `Cmd+[` (`+page.svelte`) calls the same
                     shared action. -->
                <button
                  type="button"
                  class="job-brief-back-button"
                  onclick={() => onBackClick?.()}
                  title="Back (⌘[)"
                  aria-label="Back to previous session"
                >←</button>
              {/if}
              Job brief
            </span>
            <span class="job-brief-text">{message.content}</span>
          </div>
        {:else}
        <div class="message" data-message-id={message.id} class:user={message.role === 'user'} class:assistant={message.role === 'assistant'} class:system={message.role === 'system'}>
          {#if message.role === 'assistant'}
            {@const senderName = resolveSenderName(message)}
            <!-- spec-covered-by: #2030 (§24.8); #2604 RC-B — attributed agent name is
                 authoritative. Fallback chain: 1) the SESSION's own bound agent
                 (sessionBoundAgentName, never the app-wide default) for messages
                 that haven't round-tripped the DB's agent_name LEFT JOIN yet
                 (e.g. a live-streamed message before onDone's agent_name lands
                 or before the next loadMessages() reload); 2) genuine legacy-NULL
                 copy (ADR-0059 D5: written before attribution shipped, or no
                 session/bound agent found at all) — NEVER mainAgentName.
                 #2643: sessionBoundAgentName is now a $derived value sourced
                 from either SessionDropdownHeader's onBoundAgentChange callback
                 (project surfaces, showHeader=true) or the externally-resolved
                 boundAgentNameProp ChatSidebar passes in for Home
                 (showHeader=false), mirroring the readOnlyState prop pattern.
                 #2604: `senderName === null` means the agent roster hasn't
                 finished its first load for this session-open yet — render the
                 neutral thinking-pulse treatment instead of the 'Pane' literal,
                 which is reserved for the genuine ADR-0059 D5 nothing-to
                 -attribute case (roster loaded, no bound agent found). -->
            {#if senderName === null}
              <div class="chat-sender chat-sender--loading">...</div>
            {:else}
              <div class="chat-sender">{senderName}</div>
            {/if}
          {/if}
          <div class="message-content" onclick={handleLinkClick} use:measureUserMsg={{ id: message.id, role: message.role }} class:collapsed={message.role === 'user' && tallUserMsgs.has(message.id) && !expandedUserMsgs.has(message.id)} class:thinking={message.role === 'assistant' && (message.content === '...' || (streamingMessageId === message.id && !message.content.trim()))} class:streaming={streamingMessageId === message.id}>
            {#if message.role === 'assistant' && message.content !== '...'}
              {#if (message as any)._hideContent && streamingMessageId === message.id}
                <span class="tool-working">Working...</span><span class="cursor">&#9610;</span>
              {:else if streamingMessageId === message.id && !message.content.trim()}
                <span class="thinking-indicator"><span class="cursor">&#9610;</span></span>
              {:else}
                {@html renderMarkdown(stripToolBlocks(message.content) || '')}{#if streamingMessageId === message.id}<span class="cursor">&#9610;</span>{/if}
              {/if}
            {:else}
              {message.content}{#if streamingMessageId === message.id}<span class="cursor">&#9610;</span>{/if}
            {/if}
          </div>
          {#if message.role === 'user' && tallUserMsgs.has(message.id)}
            <button class="show-more-btn" onclick={() => { const s = new Set(expandedUserMsgs); if (s.has(message.id)) { s.delete(message.id); } else { s.add(message.id); } expandedUserMsgs = s; }}>
              {expandedUserMsgs.has(message.id) ? 'Show less ↑' : 'Show more ↓'}
            </button>
          {/if}
          {#if message.role === 'assistant' && message.tool_calls}
            {@const toolCallsParsed = (() => { try { const parsed = JSON.parse(message.tool_calls); return Array.isArray(parsed) && parsed.length > 0 ? parsed : null; } catch { return null; } })()}
            {#if toolCallsParsed}
              {@const toolResultsParsed = (() => { try { const raw = message.tool_results ? JSON.parse(message.tool_results) : []; return raw.map((r: any) => { const s = String(r); if (s.includes('"error":') || s.includes('"error" :') || s.startsWith('{"error"') || (typeof r === 'object' && r !== null && 'error' in r)) { console.warn('[TOOL-ERROR-HIDDEN]', s); return null; } return r; }); } catch { return []; } })()}
              <div class="tool-calls-section" class:expanded={expandedToolMsgs.has(message.id)}>
                <button class="tool-calls-toggle" onclick={() => { const s = new Set(expandedToolMsgs); if (s.has(message.id)) { s.delete(message.id); } else { s.add(message.id); } expandedToolMsgs = s; }}>
                  Tools used ({toolCallsParsed.length})
                </button>
                <div class="tool-calls-list">
                  {#each toolCallsParsed as tool, i}
                    <div class="tool-call-item">
                      <span class="tool-call-name">{tool.name || 'unknown'}({Object.keys(tool.arguments || tool.params || {}).slice(0, 3).join(', ')})</span>
                      {#if toolResultsParsed[i] != null}
                        <span class="tool-call-arrow"> -- </span>
                        <span class="tool-call-result">{String(toolResultsParsed[i]).substring(0, 120)}</span>
                      {/if}
                    </div>
                  {/each}
                </div>
              </div>
            {/if}
          {/if}
          {#if message.attachment_ids}
            <div class="message-attachments">
              {#each message.attachment_ids.split(',').filter(Boolean) as attId (attId)}
                {#await loadAttachmentMeta(attId) then meta}
                  {#if meta}
                    {#if getAttachmentRenderer(meta.mime_type) === 'image'}
                      <img src={meta.dataUrl} alt={meta.original_filename} class="attachment-image" />
                    {:else if getAttachmentRenderer(meta.mime_type) === 'audio'}
                      <audio controls src={meta.dataUrl} class="attachment-audio"></audio>
                    {:else if getAttachmentRenderer(meta.mime_type) === 'video'}
                      <video controls src={meta.dataUrl} class="attachment-video"></video>
                    {:else}
                      <div class="attachment-file">
                        <!-- C30 (#3128, icon-parity only — click-to-open is explicitly OUT of
                             scope, no lightbox mechanism exists anywhere in this file to reuse):
                             same uppercased-extension convention `pendingAttachments`' own
                             `.preview-file-icon` already uses below, closing the "bare chip"
                             complaint without building a new open-attachment feature. -->
                        <span class="attachment-file-icon">{meta.original_filename.split('.').pop()?.toUpperCase() || 'FILE'}</span>
                        <span class="attachment-file-name">{meta.original_filename}</span>
                        <span class="attachment-file-size">{formatFileSize(meta.size_bytes)}</span>
                      </div>
                    {/if}
                  {/if}
                {/await}
              {/each}
            </div>
          {/if}
          {#if message.role !== 'system'}
            <div class="message-meta">
              <div class="message-timestamp">
                {formatTimestamp(message.created_at)}
              </div>
              {#if message.role === 'assistant' && (message as any).model}
                <!-- Working as intended: per-message model badge reflects the model that
                     actually ran for THIS message, not current provider state. Required
                     for smart-routing history (see #1448). Do not "fix" by refreshing on
                     provider switch — that destroys the audit trail. -->
                <span class="model-label">{formatModelName((message as any).model)}</span>
              {/if}
              <button
                class="save-memory-btn"
                onclick={() => openSaveMemory(message)}
                title="Save to Memory"
              >
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
                  <path d="M2 2h9l3 3v9H2z"/>
                  <path d="M5 2v4h5V2"/>
                  <path d="M5 10h6v4H5z"/>
                </svg>
              </button>
            </div>
          {/if}
        </div>
        {/if}
      {/each}
    {/if}
  </div>

  {#if memoryToast}
    <!-- §5.4B memory extraction toast: auto-fades 5s, dismiss button soft-deletes (#641) -->
    <div style="margin: 0 12px 8px 12px; padding: 8px 12px; background: rgba(0,228,228,0.05); border: 1px solid rgba(0,228,228,0.2); border-radius: 4px; display: flex; align-items: center; gap: 8px; animation: toastFadeIn 0.3s ease;">
      <span style="color: #e0e0e0; font-size: 12px; font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1;">{memoryToast}</span>
      {#if onDismissMemoryToast}
        <button
          onclick={onDismissMemoryToast}
          style="background: none; border: none; color: rgba(0,228,228,0.6); font-size: 11px; font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif; cursor: pointer; padding: 0 2px; flex-shrink: 0; opacity: 0.7;"
          title="Dismiss and remove this memory"
        >✕ wrong</button>
      {/if}
    </div>
  {/if}

  {#if saveMemoryToastText}
    <div style="margin: 0 12px 8px 12px; padding: 8px 12px; background: rgba(0,228,228,0.05); border: 1px solid rgba(0,228,228,0.2); border-radius: 4px; display: flex; align-items: center; gap: 8px; animation: toastFadeIn 0.3s ease;">
      <span style="color: #00E4E4; font-size: 12px; flex-shrink: 0;">&#10003;</span>
      <span style="color: #e0e0e0; font-size: 12px; font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;">{saveMemoryToastText}</span>
    </div>
  {/if}

  {#if memoryIndicator}
    <div class="memory-indicator" class:compacting={isCompacting}>
      {memoryIndicator}
    </div>
  {/if}

  {#if pendingAttachments.length > 0}
    <div class="attachment-strip">
      {#each pendingAttachments as att (att.id)}
        <div class="attachment-preview" class:is-file={!att.previewUrl}>
          {#if att.previewUrl}
            <img src={att.previewUrl} alt={att.filename} class="preview-thumb" />
          {:else}
            <!-- Leftover 2 (#3139, live test #4): same icon+filename+size
                 treatment C30 (#3128) already applied to the POST-SEND
                 in-message attachment chip (`.attachment-file`, above) —
                 that fix's own comment explicitly left this PRE-SEND box
                 untouched. Reuses `formatFileSize` (already imported/used
                 by the post-send branch) and `att.filename`/`att.size` —
                 no new formatting logic. -->
            <div class="preview-file-icon">{att.filename.split('.').pop()?.toUpperCase() || 'FILE'}</div>
            <span class="preview-file-name">{att.filename}</span>
            <span class="preview-file-size">{formatFileSize(att.size)}</span>
          {/if}
          <button class="remove-attachment" onclick={() => removeAttachment(att.id)}>x</button>
        </div>
      {/each}
    </div>
  {/if}
  <!-- Error toast (#1822) -->
  {#if errorToastMsg}
  <div class="error-toast" role="alert">
    <span class="error-toast-msg">{errorToastMsg}</span>
    <button class="error-toast-dismiss" onclick={() => { errorToastMsg = null; }}>✕</button>
  </div>
  {/if}

  {#if readOnlyState}
    <!-- ADR-0062 D5: last item in the message column, directly above the
         composer — reads as the newest event, not page chrome. -->
    <ReadOnlySessionNotice
      reason={readOnlyState.reason}
      agent={readOnlyState.agent}
      provider={readOnlyProvider}
      onStartNewSession={handleStartNewSessionFromReadOnly}
      onOpenCause={handleOpenReadOnlyCause}
    />
  {/if}

  <div class="input-area">
    {#if readOnlyState}
      <div class="input-area--off">
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3.5" y="7.5" width="9" height="6.5" rx="1.6" /><path d="M5.6 7.5V5.6a2.4 2.4 0 014.8 0v1.9" /></svg>
        <span>Composer disabled while {readOnlyState.reason === 'provider' ? `${readOnlyProvider} is disconnected` : `${readOnlyState.agent.name} is deactivated`}</span>
      </div>
    {:else}
      <button
        type="button"
        class="composer-action-btn attach-file-btn"
        onclick={handleAttachFilePicker}
        disabled={disabled}
        title="Attach a file"
        aria-label="Attach a file"
        data-testid="attach-file-picker-btn"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"/></svg>
      </button>
      <textarea
        bind:value={messageInput}
        bind:this={textareaEl}
        onkeydown={handleKeyDown}
        onpaste={handleFilePaste}
        oninput={autoResize}
        placeholder={placeholder}
        rows="3"
        class="message-input"
        disabled={disabled}
        spellcheck="true"
      ></textarea>
      <!-- §36.2.3 (#3050 PR 6, Ruling 4's composer sentence): Send/Stop
           occupy the SAME slot, toggling on `isStreaming` — not a Stop
           button that pops in and out of an otherwise-empty composer
           (the "floating button" live-test note). Idle → Send (new; the
           composer was Enter-only before this, zero buttons while idle).
           Streaming → Stop (unchanged behavior — the loop checks the
           cancel flag at the top of every iteration and before every tool
           call past the first in a batch; a tool call already in flight
           completes, its result is saved, then the turn ends with
           "Stopped by you after N tool calls." §36.2.1). -->
      {#if isStreaming && onStop}
        <button
          type="button"
          class="composer-action-btn stop-turn-btn"
          onclick={onStop}
          title="Stop this turn"
          aria-label="Stop this turn"
        >
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><rect x="3" y="3" width="10" height="10" rx="1.5"/></svg>
          Stop
        </button>
      {:else}
        <button
          type="button"
          class="composer-action-btn send-btn"
          onclick={sendMessage}
          disabled={disabled || sendDisabled || (!messageInput.trim() && pendingAttachments.length === 0)}
          title="Send message"
          aria-label="Send message"
        >
          <svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M1.5 1.5 14.5 8 1.5 14.5 4 8z"/></svg>
          Send
        </button>
      {/if}
    {/if}
  </div>
</div>

{#if saveMemoryMessage}
  <div class="save-memory-overlay" onclick={closeSaveMemory}>
    <div class="save-memory-modal" onclick={(e) => e.stopPropagation()}>
      <div class="save-memory-header">
        <span class="save-memory-title">Save to Memory</span>
        <button class="save-memory-close" onclick={closeSaveMemory}>x</button>
      </div>
      <div class="save-memory-ref">
        <span class="save-memory-ref-label">From message:</span>
        <div class="save-memory-ref-text">{saveMemoryMessage.content.slice(0, 300)}{saveMemoryMessage.content.length > 300 ? '...' : ''}</div>
      </div>
      <div class="save-memory-field">
        <label class="save-memory-label">Fact to remember</label>
        <textarea
          class="save-memory-input"
          bind:value={saveMemoryFact}
          rows="3"
          placeholder="What should be remembered..."
          onkeydown={(e) => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) confirmSaveMemory(); }}
        ></textarea>
      </div>
      <div class="save-memory-field">
        <label class="save-memory-label">Category</label>
        <select class="save-memory-select" bind:value={saveMemoryCategory}>
          {#each memoryCategories as cat}
            <option value={cat}>{cat}</option>
          {/each}
        </select>
      </div>
      <div class="save-memory-actions">
        <button class="save-memory-cancel" onclick={closeSaveMemory}>Cancel</button>
        <button class="save-memory-confirm" onclick={confirmSaveMemory}>Save</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .tool-calls-section {
    margin-top: 6px;
    background: rgba(255, 255, 255, 0.03);
    border-radius: 4px;
    padding: 4px 8px;
    font-size: 12px;
    color: #666;
  }
  .tool-calls-toggle {
    background: none;
    border: none;
    color: #666;
    font-size: 12px;
    cursor: pointer;
    padding: 2px 0;
    text-align: left;
  }
  .tool-calls-toggle:hover {
    color: #888;
  }
  .tool-calls-list {
    display: none;
    margin-top: 4px;
  }
  .tool-calls-section.expanded .tool-calls-list {
    display: block;
  }
  .tool-call-item {
    padding: 2px 0;
    font-size: 12px;
    color: #666;
    word-break: break-word;
  }
  .tool-call-name {
    color: #888;
    font-family: monospace;
  }
  .tool-call-arrow {
    color: #555;
  }
  .tool-call-result {
    color: #666;
  }
  .chat-panel {
    width: 100%;
    min-width: 0;
    height: 100%;
    background: #0a0a0f;
    display: flex;
    overflow: hidden;
    flex-direction: column;
    flex-shrink: 0;
  }

  /* §395 PR7: Old chat header styles removed — replaced by SessionDropdownHeader.
     (.chat-header / .chat-title / .chat-header-actions / reset-thread button / .header-subtitle) */

  .collapse-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: none;
    background: transparent;
    color: #666666;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .collapse-btn:hover {
    color: #888888;
  }

  .messages-area {
    flex: 1;
    overflow-y: auto;
    padding: 16px 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .empty-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: rgba(224, 224, 224, 0.4);
    font-size: 12px;
    text-align: center;
    padding: 24px;
  }

  .message {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 8px;
  }

  /* §36.2.1/§36.2.3 (#3050 PR 3): the checkpoint/continuation progress
     divider — deliberately NOT a chat bubble (no sender name, no avatar,
     no save-to-memory button) so it reads as a system note about the
     turn's own mechanics, not something the agent "said". */
  .continuation-divider {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 10px 0;
    padding: 0 4px;
  }

  .continuation-divider-line {
    flex: 1;
    height: 1px;
    background: rgba(255, 255, 255, 0.08);
  }

  .continuation-divider-text {
    flex-shrink: 0;
    font-size: 11px;
    color: var(--fg-4, #5A5A5E);
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }

  /* C32 (#3128): the job/wake dispatch brief — also NOT a chat bubble,
     same family as the continuation divider (bookkeeping, not a person)
     but visually distinct from both that AND the schedule-chip pill below,
     so a user can tell "Pane handed a brief to the agent" apart from both
     "something was scheduled" and "the agent kept working." */
  .job-brief {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin: 10px 4px;
    padding: 8px 12px;
    border-left: 2px solid rgba(255, 255, 255, 0.12);
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }

  .job-brief-label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    font-size: 10.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-4, #5A5A5E);
  }

  /* #3155 C49b: the one-deep back button, moved from its old floating
     position (`.job-session-back-button`, deleted) to the left edge of
     the job-brief strip's own label. Deliberately NOT uppercase/
     letter-spaced like the label text next to it — it's a glyph, not a
     word. */
  .job-brief-back-button {
    background: none;
    border: none;
    padding: 0;
    margin: 0;
    color: inherit;
    font-size: 12px;
    line-height: 1;
    cursor: pointer;
    text-transform: none;
    letter-spacing: normal;
  }
  .job-brief-back-button:hover {
    color: var(--fg-2, #C8C8CC);
  }

  .job-brief-text {
    font-size: 12.5px;
    color: var(--fg-3, #8A8A8E);
  }

  /* §36.3.6/§36.3.7 (#3050 PR 4): the schedule confirmation chip —
     deliberately NOT a chat bubble (no sender name, no avatar, no
     save-to-memory button), same non-bubble treatment as the continuation
     divider above, but visually distinct from it (a pill, not a line) so a
     user can tell "something was scheduled" apart from "the agent kept
     working." */
  .schedule-chip {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 8px 0;
    padding: 6px 12px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.1);
    align-self: center;
    max-width: fit-content;
  }

  .schedule-chip-icon {
    font-size: 12px;
  }

  .schedule-chip-text {
    font-size: 12px;
    color: var(--fg-3, #8A8A8E);
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }

  .message.user { align-items: flex-end; }
  .message.assistant { align-items: flex-start; }
  .message.system { align-items: center; margin: 4px 0; }

  .message-content {
    max-width: 85%;
    padding: 10px 14px;
    border-radius: 4px;
    font-size: 14px;
    line-height: 1.5;
    word-wrap: break-word;
  }

  .message-content :global(ul), .message-content :global(ol) { margin: 4px 0; padding-left: 20px; }
  .message-content :global(li) { margin: 2px 0; }
  .message-content :global(p) { margin: 4px 0; }
  .message-content :global(p:last-child) { margin-bottom: 0; }
  .message-content :global(strong) { font-weight: 600; color: #e0e0e0; }
  .message-content :global(code) { background: #2a2a2c; padding: 1px 4px; border-radius: 3px; font-size: 13px; }
  .message-content :global(pre) { background: #2a2a2c; padding: 8px; border-radius: 4px; overflow-x: auto; margin: 4px 0; }
  .message-content :global(h1), .message-content :global(h2), .message-content :global(h3) { color: #e0e0e0; margin: 8px 0 4px; font-weight: 600; }
  .message-content :global(h1) { font-size: 17px; }
  .message-content :global(h2) { font-size: 16px; }
  .message-content :global(h3) { font-size: 15px; }
  .message-content :global(a) { color: #00E4E4; text-decoration: none; }
  .message-content :global(a:hover) { text-decoration: underline; }
  .message-content :global(blockquote) { border-left: 2px solid #00E4E4; margin: 4px 0; padding-left: 12px; color: #888888; }

  .message-content :global(.status-indicator) { display: inline-flex; align-items: center; gap: 5px; font-size: 12px; color: #888; }
  .message-content :global(.status-dot) { width: 7px; height: 7px; border-radius: 50%; background: currentColor; display: inline-block; }
  .message-content :global(.status--overdue) { color: #ff4444; }
  .message-content :global(.status--due-today) { color: #ffaa00; }
  .message-content :global(.status--on-track) { color: #44cc44; }
  .message-content :global(.status--done) { color: #00E4E4; }
  .message-content :global(.status--blocked) { color: #ff6666; }
  .message-content :global(.priority-pill) { display: inline-block; padding: 1px 8px; border-radius: 10px; font-size: 11px; font-weight: 600; background: rgba(136,136,136,0.15); color: #888; text-transform: uppercase; letter-spacing: 0.5px; }
  .message-content :global(.priority--high) { background: rgba(255,68,68,0.15); color: #ff4444; }
  .message-content :global(.priority--critical) { background: rgba(255,68,68,0.2); color: #ff4444; }
  .message-content :global(.priority--medium) { background: rgba(255,170,0,0.15); color: #ffaa00; }

  /* Table Styling */
  .message-content :global(table) {
    border-collapse: collapse;
    width: 100%;
    margin: 8px 0;
    font-size: 13px;
  }
  .message-content :global(th) {
    background: rgba(255,255,255,0.08);
    border: 1px solid rgba(255,255,255,0.1);
    padding: 6px 10px;
    text-align: left;
    font-weight: 600;
    color: #e0e0e0;
  }
  .message-content :global(td) {
    border: 1px solid rgba(255,255,255,0.06);
    padding: 6px 10px;
    color: #ccc;
  }
  .message-content :global(tr:nth-child(even)) {
    background: rgba(255,255,255,0.03);
  }

  /* Task List Checkboxes */
  .message-content :global(input[type="checkbox"]) {
    appearance: none;
    -webkit-appearance: none;
    width: 14px;
    height: 14px;
    border: 1.5px solid rgba(255,255,255,0.3);
    border-radius: 3px;
    background: transparent;
    margin-right: 6px;
    vertical-align: middle;
    position: relative;
    cursor: default;
  }
  .message-content :global(input[type="checkbox"]:checked) {
    background: #00E4E4;
    border-color: #00E4E4;
  }
  .message-content :global(input[type="checkbox"]:checked::after) {
    content: '✓';
    position: absolute;
    top: -1px;
    left: 1.5px;
    font-size: 11px;
    color: #0a0a0f;
    font-weight: bold;
  }
  .message-content :global(li:has(input[type="checkbox"])) {
    list-style: none;
    margin-left: -20px;
  }

  /* Collapsible Sections */
  .message-content :global(details) {
    margin: 6px 0;
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 4px;
    padding: 0;
  }
  .message-content :global(summary) {
    cursor: pointer;
    padding: 6px 10px;
    color: #e0e0e0;
    font-weight: 500;
    user-select: none;
  }
  .message-content :global(summary:hover) {
    background: rgba(255,255,255,0.05);
  }
  .message-content :global(details[open] > summary) {
    border-bottom: 1px solid rgba(255,255,255,0.06);
  }
  .message-content :global(details > :not(summary)) {
    padding: 8px 10px;
  }

  /* Horizontal Rules */
  .message-content :global(hr) {
    border: none;
    border-top: 1px solid rgba(255,255,255,0.08);
    margin: 12px 0;
  }

  .chat-sender {
    color: #00E4E4;
    font-weight: 600;
    font-size: 12px;
    margin-bottom: 4px;
  }

  /* #2604: neutral loading treatment for the speaker label while the agent
     roster is still loading (sessionAgentsLoaded === false) — reuses the
     same thinking-pulse opacity animation already applied to
     .message-content.thinking, so the label pulses in place instead of
     showing the wrong literal 'Pane' during the load window. */
  .chat-sender--loading {
    animation: thinking-pulse 1.5s ease-in-out infinite;
  }

  .cursor {
    color: #00E4E4;
    animation: blink 0.8s step-end infinite;
    font-size: 0.9em;
  }

  @keyframes toastFadeIn {
    from { opacity: 0; transform: translateY(4px); }
    to   { opacity: 1; transform: translateY(0); }
  }

  @keyframes blink { 50% { opacity: 0; } }

  .message-content.thinking { animation: thinking-pulse 1.5s ease-in-out infinite; }

  @keyframes thinking-pulse {
    0%, 100% { opacity: 0.5; }
    50% { opacity: 1; }
  }

  .thinking-indicator {
    opacity: 0;
    animation: thinking-fade-in 0.3s ease forwards;
    animation-delay: 0.5s;
  }

  @keyframes thinking-fade-in {
    to { opacity: 1; }
  }

  .message.user .message-content {
    background: rgba(255,255,255,0.08);
    border: none;
    color: #e0e0e0;
    padding: 12px 16px;
    border-radius: 12px;
  }

  /* #1706: collapsed user message */
  .message.user .message-content.collapsed {
    max-height: 168px;
    overflow: hidden;
    position: relative;
  }

  .message.user .message-content.collapsed::after {
    content: '';
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    height: 48px;
    background: linear-gradient(to bottom, transparent, rgba(255,255,255,0.08));
    border-radius: 0 0 12px 12px;
    pointer-events: none;
  }

  .show-more-btn {
    background: none;
    border: none;
    color: #00E4E4;
    font-size: 12px;
    cursor: pointer;
    padding: 4px 0;
    margin-top: 2px;
    text-align: right;
    align-self: flex-end;
  }

  .show-more-btn:hover {
    opacity: 0.8;
  }

  .message.assistant .message-content {
    background: rgba(255,255,255,0.04);
    border: none;
    color: #888888;
    padding: 12px 16px;
    border-radius: 12px;
  }

  .message.system .message-content {
    background: rgba(0, 228, 228, 0.05);
    color: #00E4E4;
    font-size: 12px;
    font-style: italic;
    padding: 8px 12px;
    max-width: 100%;
    text-align: center;
    border-left: 2px solid #00E4E4;
    border-radius: 6px;
    padding-left: 8px;
    font-style: italic;
  }

  .message-timestamp {
    font-size: 10px;
    color: rgba(224, 224, 224, 0.3);
    margin: 0 12px;
  }

  .model-label {
    font-size: 10px;
    color: rgba(224, 224, 224, 0.25);
    margin-left: 8px;
  }

  .memory-indicator {
    padding: 8px 16px;
    font-size: 11px;
    color: #00E4E4;
    background: rgba(0, 228, 228, 0.03);
    border-top: 1px solid rgba(0, 228, 228, 0.06);
    text-align: center;
    transition: opacity 0.3s ease;
  }

  .memory-indicator.compacting { animation: pulse 1.5s ease-in-out infinite; }

  @keyframes pulse {
    0%, 100% { opacity: 0.7; }
    50% { opacity: 1; }
  }

  .input-area {
    flex-shrink: 0;
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 16px;
    border-top: none;
  }

  /* ADR-0062 D5: composer stays visible, dashed + disabled, restating the
     read-only reason — removing it would read as broken. */
  .input-area--off {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 44px;
    padding: 10px 14px;
    border: 1px dashed var(--border-strong, rgba(255, 255, 255, 0.18));
    border-radius: 12px;
    color: var(--fg-4, #5A5A5E);
    font-size: 13px;
  }

  /* §36.2.3 (#3050 PR 6) — Send/Stop share this base: ONE slot in the
     composer row (input-area is a flex row, align-items: flex-end) that
     toggles label/icon/color on `isStreaming`, never a button that pops in
     and out of an otherwise-empty composer. */
  .composer-action-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    height: 36px;
    padding: 0 14px;
    border-radius: 10px;
    font-size: 13px;
    font-weight: 500;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    cursor: pointer;
    transition: background-color 0.15s ease, border-color 0.15s ease;
  }

  /* #3050 PR 9, §36.9.2 — the picker button: icon-only, same base as
     Send/Stop (`.composer-action-btn`) but square (no label) so it reads
     as a toolbar affordance beside the textarea, not a third competing
     primary action. */
  .attach-file-btn {
    width: 36px;
    padding: 0;
    justify-content: center;
    border: 1px solid var(--border-strong, rgba(255, 255, 255, 0.18));
    background: transparent;
    color: var(--fg-3, #8A8A8E);
  }

  .attach-file-btn:hover:not(:disabled) {
    background: var(--bg-hover, rgba(255, 255, 255, 0.06));
    color: var(--fg-1, #F0F0F2);
  }

  .attach-file-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  /* §36.2.3 (#3050 PR 3) — shown only while isStreaming. */
  .stop-turn-btn {
    border: 1px solid rgba(239, 68, 68, 0.35);
    background: rgba(239, 68, 68, 0.1);
    color: #EF4444;
  }

  .stop-turn-btn:hover {
    background: rgba(239, 68, 68, 0.18);
    border-color: rgba(239, 68, 68, 0.5);
  }

  /* §36.2.3 (#3050 PR 6) — shown while idle; same slot/size as Stop,
     swapped color (cyan, matching this app's primary-action accent). */
  .send-btn {
    border: 1px solid rgba(0, 228, 228, 0.35);
    background: rgba(0, 228, 228, 0.1);
    color: #00E4E4;
  }

  .send-btn:hover:not(:disabled) {
    background: rgba(0, 228, 228, 0.18);
    border-color: rgba(0, 228, 228, 0.5);
  }

  .send-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .message-input {
    flex: 1;
    min-height: 100px;
    max-height: 300px;
    padding: 10px 14px;
    border: 1px solid rgba(0, 228, 228, 0.2);
    border-radius: 12px;
    background: transparent;
    color: #E0E0E0;
    font-size: 14px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    font-weight: 300;
    resize: none;
    outline: none;
    transition: border-color 0.15s ease;
    overflow-y: auto;
    /* C27 (#3147, live test #5, third diagnosis) — this rule set
     * overflow-y but never overflow-x. Per the CSS Overflow Module's
     * own coupling rule (CSS Overflow Module Level 3 §3): if one of
     * overflow-x/overflow-y is "visible" and the other isn't, the
     * "visible" one computes to "auto" instead — so overflow-x here
     * was silently computing to auto, not its apparent default. That
     * is a real, standard mechanism (not a WebKit-specific quirk),
     * confirmed independently by Reed's audit before this fix.
     * Explicitly setting overflow-x: hidden removes the entire CLASS
     * of horizontal-scrollbar-on-this-element failures, not just the
     * one pixel-rounding instance Matt saw at Cmd+ zoom.
     *
     * UNCONFIRMED MECHANISM, stated plainly: this is the THIRD C27
     * theory shipped this scope (after #3144's viewport-units and
     * #3145's display:contents attempts, both of which targeted
     * `.app`, not this element, and neither of which was ever the
     * cause of the composer scrollbar). Reed tested the specific
     * fractional-padding-under-zoom theory with two different
     * Playwright proxies standing in for native WKWebView `pageZoom`
     * (CSS `zoom` on `<body>`, and Chromium's fractional
     * `deviceScaleFactor` override) and could NOT reproduce the ~1px
     * discrepancy with either — see `docs/spec/ui-layout.md`'s
     * "Composer Textarea Horizontal Overflow" subsection for the full
     * writeup. The fix is shipped because it is mechanism-agnostic —
     * it closes the overflow-x-computes-to-auto class of bug
     * regardless of what triggers it — not because the zoom cause is
     * confirmed. */
    overflow-x: hidden;
  }

  .message-input:focus { border-color: rgba(0, 228, 228, 0.5); }
  .message-input::placeholder { color: rgba(224, 224, 224, 0.4); }
  .message-input:disabled { opacity: 0.5; cursor: not-allowed; }

  .messages-area::-webkit-scrollbar { width: 4px; }
  .messages-area::-webkit-scrollbar-track { background: transparent; }
  .messages-area::-webkit-scrollbar-thumb { background: rgba(0, 228, 228, 0.08); border-radius: 2px; }
  .messages-area::-webkit-scrollbar-thumb:hover { background: rgba(0, 228, 228, 0.15); }
  /* §22C.12.4: Passive reconnect indicator — amber, non-blocking (#945) */
  .reconnect-indicator {
    padding: 6px 12px;
    background: rgba(251, 191, 36, 0.08);
    border-bottom: 1px solid rgba(251, 191, 36, 0.2);
    color: #FBB024;
    font-size: 12px;
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
  .reconnect-icon {
    font-size: 14px;
    animation: spin 1.2s linear infinite;
  }
  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }

  .connection-error {
    padding: 8px 12px;
    background: rgba(239, 68, 68, 0.1);
    border-bottom: 1px solid rgba(239, 68, 68, 0.2);
    color: #EF4444;
    font-size: 12px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-shrink: 0;
  }
  .connection-error button {
    background: none;
    border: none;
    color: #EF4444;
    cursor: pointer;
    font-size: 14px;
  }
  .connection-error-actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .connection-error-actions button:first-child {
    text-decoration: underline;
    font-size: 12px;
  }

  .message-meta {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding-top: 4px;
  }

  .save-memory-btn {
    opacity: 0.15;
    /* NO transition here - instant snap back to low opacity */
    background: none;
    border: none;
    color: #666666;
    cursor: pointer;
    padding: 2px;
    display: flex;
    align-items: center;
  }
  .save-memory-btn:hover {
    opacity: 1;
    color: #00E4E4;
    transition: opacity 0.1s ease-in, color 0.1s ease-in;
  }

  .save-memory-overlay {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
    background: rgba(0,0,0,0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .save-memory-modal {
    background: #0a0a0f;
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 4px;
    padding: 20px;
    width: 420px;
    max-width: 90vw;
    max-height: 80vh;
    overflow-y: auto;
    box-shadow: 0 8px 24px rgba(0,0,0,0.5);
  }

  .save-memory-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .save-memory-title {
    font-size: 15px;
    font-weight: 600;
    color: #e0e0e0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }

  .save-memory-close {
    background: none;
    border: none;
    color: #666666;
    cursor: pointer;
    font-size: 16px;
    padding: 2px 6px;
  }

  .save-memory-close:hover {
    color: #e0e0e0;
  }

  .save-memory-ref {
    margin-bottom: 16px;
    padding: 10px 12px;
    background: rgba(255,255,255,0.03);
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 4px;
  }

  .save-memory-ref-label {
    font-size: 11px;
    color: #555555;
    display: block;
    margin-bottom: 6px;
  }

  .save-memory-ref-text {
    font-size: 12px;
    color: #999999;
    line-height: 1.5;
    max-height: 80px;
    overflow: hidden;
  }

  .save-memory-field {
    margin-bottom: 14px;
  }

  .save-memory-label {
    font-size: 12px;
    font-weight: 500;
    color: #888888;
    display: block;
    margin-bottom: 6px;
  }

  .save-memory-input {
    width: 100%;
    padding: 10px 12px;
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 4px;
    background: rgba(255,255,255,0.05);
    color: #e0e0e0;
    font-size: 13px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    outline: none;
    resize: vertical;
    box-sizing: border-box;
  }

  .save-memory-input:focus {
    border-color: rgba(0,228,228,0.4);
  }

  .save-memory-select {
    width: 100%;
    padding: 12px 14px;
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 4px;
    background: #0a0a0f;
    color: #e0e0e0;
    font-size: 14px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    outline: none;
    cursor: pointer;
    height: 44px;
  }

  .save-memory-select:focus {
    border-color: rgba(0,228,228,0.4);
  }

  .save-memory-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 16px;
  }

  .save-memory-cancel {
    padding: 8px 16px;
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 4px;
    background: transparent;
    color: #888888;
    font-size: 13px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .save-memory-cancel:hover {
    color: #e0e0e0;
  }

  .save-memory-confirm {
    padding: 8px 16px;
    border: 1px solid #00E4E4;
    border-radius: 4px;
    background: #00E4E4;
    color: #0a0a0f;
    font-size: 13px;
    font-weight: 600;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .save-memory-confirm:hover {
    background: rgba(0,228,228,0.9);
  }


  .drag-over {
    outline: 2px dashed #00E4E4;
    outline-offset: -2px;
    background: rgba(0,228,228,0.05) !important;
  }
  .message-attachments {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 6px;
    max-width: 85%;
  }
  .attachment-image {
    max-width: 300px;
    border-radius: 8px;
    cursor: pointer;
  }
  .attachment-audio {
    max-width: 300px;
  }
  .attachment-video {
    max-width: 400px;
    border-radius: 8px;
  }
  .attachment-file {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: rgba(255,255,255,0.05);
    border-radius: 6px;
    font-size: 12px;
  }
  .attachment-file-icon {
    flex-shrink: 0;
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(255,255,255,0.08);
    border-radius: 4px;
    font-size: 8.5px;
    font-weight: 700;
    color: #999;
  }
  .attachment-file-name {
    color: #e0e0e0;
  }
  .attachment-file-size {
    color: #666;
  }
  .attachment-strip {
    display: flex;
    gap: 8px;
    padding: 8px 16px;
    overflow-x: auto;
    border-top: 1px solid rgba(255,255,255,0.06);
  }
  .attachment-preview {
    position: relative;
    width: 60px;
    height: 60px;
    flex-shrink: 0;
  }
  /* Leftover 2 (#3139, live test #4): a non-image attachment's preview
     box grows to fit an icon + filename + size, matching the post-send
     `.attachment-file` row's own shape — the fixed 60x60 square stays only
     for an image thumbnail. */
  .attachment-preview.is-file {
    width: auto;
    height: 32px;
    min-width: 110px;
    max-width: 220px;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px;
    background: rgba(255,255,255,0.05);
    border-radius: 6px;
  }
  .preview-thumb {
    width: 60px;
    height: 60px;
    object-fit: cover;
    border-radius: 6px;
  }
  .preview-file-icon {
    width: 60px;
    height: 60px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(255,255,255,0.06);
    border-radius: 6px;
    font-size: 10px;
    color: #888;
    font-weight: 600;
  }
  .attachment-preview.is-file .preview-file-icon {
    width: 22px;
    height: 22px;
    flex-shrink: 0;
    font-size: 7.5px;
  }
  .preview-file-name {
    color: #e0e0e0;
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .preview-file-size {
    color: #666;
    font-size: 10px;
    flex-shrink: 0;
  }
  .remove-attachment {
    position: absolute;
    top: -4px;
    right: -4px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: #333;
    color: #ccc;
    border: none;
    font-size: 11px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    line-height: 1;
  }
  .remove-attachment:hover {
    background: #c00;
    color: #fff;
  }
  /* Error toast (#1822) */
  .error-toast {
    position: fixed;
    bottom: 24px;
    right: 24px;
    background: rgba(239,68,68,0.12);
    border: 1px solid rgba(239,68,68,0.35);
    color: #f87171;
    padding: 10px 14px;
    border-radius: 6px;
    font-size: 13px;
    z-index: 9999;
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: 380px;
    animation: errorToastIn 0.2s ease;
  }
  .error-toast-msg {
    flex: 1;
    line-height: 1.4;
  }
  .error-toast-dismiss {
    background: none;
    border: none;
    color: #f87171;
    cursor: pointer;
    font-size: 14px;
    padding: 0;
    opacity: 0.7;
    flex-shrink: 0;
  }
  .error-toast-dismiss:hover { opacity: 1; }
  @keyframes errorToastIn {
    from { opacity: 0; transform: translateY(8px); }
    to { opacity: 1; transform: translateY(0); }
  }

  @keyframes highlightFlash {
    0% { background: rgba(0,228,228,0.15); }
    100% { background: transparent; }
  }
  .highlight-flash {
    animation: highlightFlash 2s ease-out;
  }
  .tool-working {
    color: #00E4E4;
    font-style: italic;
    font-size: 13px;
  }

  /* Tool call indicators (#426) */
  .tool-calls-section {
    margin-top: 6px;
    border-radius: 8px;
    overflow: hidden;
  }

  .tool-calls-toggle {
    background: rgba(255,255,255,0.06);
    border: none;
    color: #666;
    font-size: 11px;
    font-weight: 600;
    padding: 6px 10px;
    border-radius: 6px;
    cursor: pointer;
    transition: all 0.15s ease;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    opacity: 0.6;
  }

  .tool-calls-toggle:hover {
    opacity: 1;
    background: rgba(255,255,255,0.08);
    color: #00E4E4;
  }

  .tool-calls-toggle::before {
    content: '🔧';
    font-size: 12px;
  }

  .tool-calls-list {
    max-height: 0;
    overflow: hidden;
    transition: max-height 0.2s ease;
    margin-top: 0;
  }

  .tool-calls-section.expanded .tool-calls-list {
    max-height: 400px;
    margin-top: 6px;
  }

  .tool-call-item {
    padding: 6px 10px;
    background: rgba(255,255,255,0.03);
    border-radius: 4px;
    margin-bottom: 4px;
    font-size: 11px;
    line-height: 1.4;
  }

  .tool-call-item:last-child {
    margin-bottom: 0;
  }

  .tool-call-name {
    color: #00E4E4;
    font-family: monospace;
    font-weight: 600;
    font-size: 11px;
  }

  .tool-call-arrow {
    color: #444;
    margin: 0 4px;
  }

  .tool-call-result {
    color: #888;
    font-size: 11px;
    word-break: break-word;
  }
</style>
