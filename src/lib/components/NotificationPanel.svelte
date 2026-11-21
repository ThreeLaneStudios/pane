<!-- spec-skip: frontend-only notification panel per §19.6; no CORE_BEHAVIORAL files touched -->
<!-- Spec §19.6, §19.6.1, §19.6.2 | #1811 PR5 -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { sidebarCollapsed, activeChatThreadId } from '$lib/stores/app';
  import { navigateTo } from '$lib/services/navigation';
  import { recordJobSessionEntry } from '$lib/services/jobSessionBack';
  import { sessionStore } from '$lib/stores/sessionStore';
  import { parseStoredTimestamp } from '$lib/utils/time';
  import { get } from 'svelte/store';
  import type { Session } from '$lib/types';
  // #3178: the bell preview rendered notif.body raw — a notification
  // built from an agent reply containing an image embed showed literal
  // `![alt](...)` markdown syntax instead of readable text. Same shared
  // utility `AllTasksView.svelte`'s own card preview already uses, not
  // a new one.
  import { stripMarkdown } from '$lib/utils/stripMarkdown';

  // ── Props ──────────────────────────────────────────────────────────────────
  let {
    panelOpen = $bindable(),
    onRefreshBadge
  }: {
    panelOpen: boolean;
    onRefreshBadge: () => void;
  } = $props();

  // ── Types ──────────────────────────────────────────────────────────────────
  interface NotificationRow {
    id: string;
    type: string;
    title: string;
    body: string;
    source: string;
    read: number;          // 0 | 1
    created_at: string;
    target_kind: string | null;
    target_id: string | null;
  }

  // ── State ──────────────────────────────────────────────────────────────────
  let notifications = $state<NotificationRow[]>([]);
  let loading = $state(true);
  let dismissingIds = $state(new Set<string>());

  // ── Data loading ───────────────────────────────────────────────────────────
  async function loadNotifications() {
    try {
      notifications = await invoke<NotificationRow[]>('get_notifications', { unreadOnly: false });
    } catch (e) {
      console.error('[NOTIF] Failed to load notifications:', e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    loadNotifications();
    // Re-query when panel is open and a new notification arrives
    const unlistenPromise = listen('pane-notification', () => {
      loadNotifications();
    });
    return () => {
      unlistenPromise.then(fn => fn());
    };
  });

  // ── Handlers ───────────────────────────────────────────────────────────────

  // #3151 PR1 Commit F: exported (same convention `ChatSidebar.svelte`'s
  // `handleSessionSwitch`/`navigateToNoteById` already use) so the back-
  // target recording on the bell's session-click path has a real,
  // directly-callable Vitest surface — not asserted only by proxy
  // through `jobSessionBack.test.ts`'s own pure-function tests.
  export async function handleRowClick(notif: NotificationRow) {
    // Mark read optimistically
    notifications = notifications.map(n =>
      n.id === notif.id ? { ...n, read: 1 } : n
    );

    // Fire-and-forget mark_notification_read
    invoke('mark_notification_read', { notificationId: notif.id }).catch(() => {});

    // Refresh badge
    onRefreshBadge();

    // Navigate if target exists
    if (notif.target_kind && notif.target_id) {
      if (notif.target_kind === 'note') {
        panelOpen = false;
        navigateTo({ view: 'note_editor', noteId: notif.target_id });
      } else if (notif.target_kind === 'subtask') {
        // §31.2.2 subtask routing: resolve parent note, navigate there,
        // then highlight the specific subtask row (cyan ring + fading flash).
        try {
          const parentNoteId = await invoke<string | null>('get_subtask_parent_note_id', { id: notif.target_id });
          if (parentNoteId) {
            panelOpen = false;
            navigateTo({ view: 'note_editor', noteId: parentNoteId });
            // Brief delay for navigation to settle, then emit highlight event.
            await new Promise<void>(resolve => setTimeout(resolve, 150));
            window.dispatchEvent(new CustomEvent('pane-highlight-subtask', { detail: { subtaskId: notif.target_id } }));
          } else {
            // §19.5.1d: subtask no longer exists — no-op (mark read only).
            console.warn('[NOTIF] Subtask no longer exists, routing no-op:', notif.target_id);
          }
        } catch (routingErr) {
          console.error('[NOTIF] Subtask routing failed:', routingErr);
        }
      } else if (notif.target_kind === 'session') {
        // §36.7 (#3050 PR 6): the first `target_kind = 'session'` — opens
        // the project (or Home) and lands on `target_id` as the active
        // session. Safe for a job-origin (hidden spawn/schedule) session:
        // `switchSession`'s own backend guard (Ruling 2) resolves and
        // returns it WITHOUT promoting it or demoting the project's real
        // active interactive session — no dead click, no destructive click.
        try {
          // #3151 PR1 Commit F: the bell is the SECOND job-session entry
          // point — read the current session BEFORE switching, same as
          // the Jobs card's own `openScheduledJobSession` (`scheduledPanel.ts`).
          const previousSessionId = get(activeChatThreadId);
          const session = await invoke<Session>('switch_session', { sessionId: notif.target_id });
          // #3166 C58: the bell is a job-session entry point that
          // bypasses `SessionDropdownHeader`'s own `switchSession`/
          // `loadSessions` — without this, `sessionStore.activeSession`
          // stays pinned on the project's ordinary interactive session
          // (its `$effect` only re-fires on a `projectId` change, which
          // this isn't, when the job session shares the user's current
          // project) while the message body correctly shows the job
          // session's content. Reuses the response `switch_session`
          // ALREADY returned — no second round trip.
          sessionStore.setActiveSessionFromSwitch(session);
          panelOpen = false;
          const isHome = !session.project_id || session.project_id === 'workspace';
          // Two literal-`view` call sites, not a ternary on `view` itself —
          // `userReachableScan.ts`'s static analyzer requires `view` to be
          // a literal string at every `navigateTo` call site so the
          // user-reachable-view set stays derivable without running the
          // app (see that module's own doc comment).
          if (isHome) {
            await navigateTo({ view: 'home', sessionId: notif.target_id });
          } else {
            await navigateTo({ view: 'kanban', projectId: session.project_id, sessionId: notif.target_id });
          }
          recordJobSessionEntry(previousSessionId, notif.target_id);
        } catch (routingErr) {
          console.error('[NOTIF] Session routing failed:', routingErr);
        }
      } else {
        // Unknown target_kind: treat as null — mark read, no navigate, panel stays open
        console.warn('[NOTIF] Unknown target_kind:', notif.target_kind);
      }
    }
    // If target_kind or target_id is null: mark read, panel stays open, no navigate. No error.
  }

  async function handleDismiss(notif: NotificationRow) {
    // Start CSS animation
    dismissingIds = new Set([...dismissingIds, notif.id]);

    // Wait for --dur-norm (250ms dismiss animation)
    await new Promise<void>(resolve => setTimeout(resolve, 250));

    // Invoke dismiss; clean up in finally so the row doesn't stay stuck faded on error
    try {
      await invoke('dismiss_notification', { notificationId: notif.id });
      // Remove from local list
      notifications = notifications.filter(n => n.id !== notif.id);
      // Refresh badge
      onRefreshBadge();
    } catch (e) {
      console.error('[NOTIF] dismiss_notification failed:', e);
    } finally {
      dismissingIds = new Set([...dismissingIds].filter(id => id !== notif.id));
    }
  }

  async function handleMarkAllRead() {
    await invoke('mark_all_notifications_read');
    notifications = notifications.map(n => ({ ...n, read: 1 }));
    onRefreshBadge();
  }

  // ── Utilities ──────────────────────────────────────────────────────────────

  /** Inline relative time: "now" (<2min), "Xm", "Xh", "Xd"
   *  #3151 PR3/C44: `created_at` is SQLite's naive-UTC `datetime('now')`
   *  shape ("YYYY-MM-DD HH:MM:SS", no `T`/`Z`/offset) — handing it
   *  straight to `new Date()` parses it as LOCAL wall-clock time, not
   *  UTC. In any timezone behind UTC that makes this diff negative,
   *  which satisfies `mins < 2` unconditionally: every notification
   *  showed "now" forever, regardless of real age. Same fix pattern
   *  already used elsewhere (`ChatPanel.svelte`, `scheduledPanel.ts`,
   *  `jobStatusChip.ts`, `utils/time.ts`'s own sibling fix): append `Z`
   *  (after swapping the space for `T`) so `Date` treats it as UTC.
   *  Kept as its own local function (NOT collapsed into
   *  `utils/time.ts`'s `formatRelativeTime`) — the two have genuinely
   *  different output vocabularies ("5m"/"3h" compact chip text here
   *  vs. "5 minutes ago"/"3 hours ago" verbose text there), suited to
   *  this panel's narrow bell dropdown; unifying them would be a
   *  visible format change on one surface or the other, out of scope
   *  for a parsing-bug fix. */
  function formatRelativeTime(created_at: string): string {
    // C44 (#3155): the parse itself (normalize + `new Date(...)`) is now
    // shared via `parseStoredTimestamp` (`$lib/utils/time.ts`) — this was
    // the last inline reimplementation of it. Output formatting below
    // ('now'/'Xm'/'Xh'/'Xd') is UNCHANGED — this panel's compact
    // vocabulary genuinely differs from `formatRelativeTime`'s own
    // sibling in `utils/time.ts` ('X minutes ago'/etc.), so only the
    // parse is unified, per the original C44 ruling.
    const diffMs = Date.now() - parseStoredTimestamp(created_at).getTime();
    const mins = Math.floor(diffMs / 60000);
    if (mins < 2) return 'now';
    if (mins < 60) return `${mins}m`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h`;
    const days = Math.floor(hours / 24);
    return `${days}d`;
  }

  // Derived: unread count for header badge
  let unreadCount = $derived(notifications.filter(n => n.read === 0).length);
  let allRead = $derived(notifications.every(n => n.read === 1));
</script>

<!-- Panel floats from the sidebar bell. Anchor position driven by $sidebarCollapsed store. -->
<!-- Virtualization deferred: LIMIT 100 replaces §19.6.1's '>100 virtualization' at launch. -->
<div
  class="nt-pop nt-panel"
  style={`left: ${$sidebarCollapsed ? 52 : 224}px; bottom: 6px;`}
  role="dialog"
  aria-label="Notifications"
>
  <!-- Header -->
  <div class="nt-head">
    <h3 class="nt-head__title">Notifications</h3>
    {#if unreadCount > 0}
      <span class="nt-count">{unreadCount}</span>
    {/if}
    <span class="nt-head__spacer"></span>
    <button
      class="nt-markall"
      disabled={!allRead ? undefined : true}
      onclick={handleMarkAllRead}
    >
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <path d="m2 12 5 5M9 12l5 5L22 6M13 6l-1 1"/>
      </svg>
      Mark all read
    </button>
  </div>

  <!-- Notification list -->
  <div class="nt-list">
    {#if loading}
      <!-- loading state: empty, no spinner (panel opens fast) -->
    {:else if notifications.length === 0}
      <!-- Empty state: calm bell in cyan glow — never red/alert -->
      <div class="nt-empty">
        <span class="nt-empty__art">
          <span class="nt-empty__ring"></span>
          <span class="nt-empty__bell">
            <svg width="34" height="34" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
              <path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9"/>
              <path d="M13.7 21a2 2 0 0 1-3.4 0"/>
            </svg>
          </span>
        </span>
        <p class="nt-empty__title">You're all caught up</p>
        <p class="nt-empty__sub">No notifications. You'll see things here when something needs your attention.</p>
      </div>
    {:else}
      {#each notifications as notif (notif.id)}
        <div
          class="nt-row"
          class:is-read={notif.read === 1}
          class:is-dismissing={dismissingIds.has(notif.id)}
          role="button"
          tabindex="0"
          onclick={() => handleRowClick(notif)}
          onkeydown={(e) => { if (e.key === 'Enter' || (e.key === ' ' && (e.preventDefault(), true))) handleRowClick(notif); }}
        >
          <!-- Status dot: cyan = unread, grey = read -->
          <span class="nt-status-dot"></span>

          <!-- Body -->
          <span class="nt-body">
            <span class="nt-row__top">
              <span class="nt-title">{notif.title}</span>
              <span class="nt-time">{formatRelativeTime(notif.created_at)}</span>
            </span>
            <p class="nt-preview">{stripMarkdown(notif.body)}</p>
            {#if notif.target_kind === 'subtask'}
              <!-- §31.2.7: Subtask badge + metadata line in drawer -->
              <span class="nt-tag nt-tag--subtask">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
                  <path d="M14 2v6h6"/>
                </svg>
                Subtask
              </span>
            {/if}
          </span>

          <!-- Dismiss × — hover-revealed, stopPropagation so row click doesn't fire -->
          <button
            class="nt-x"
            aria-label="Dismiss"
            onclick={(e) => { e.stopPropagation(); handleDismiss(notif); }}
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
              <path d="M18 6 6 18M6 6l12 12"/>
            </svg>
          </button>
        </div>
      {/each}
    {/if}
  </div>

  <!-- Footer: "See all" — visual only, disabled, no click handler (post-launch page view) -->
  <div class="nt-foot">
    <button class="nt-seeall" disabled>
      See all
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
        <path d="M5 12h14M13 6l6 6-6 6"/>
      </svg>
    </button>
  </div>
</div>

<style>
  /* ── Panel shell ─────────────────────────────────────────────────────────── */
  /* C25 (#3128): root cause — `.chat-panel`'s own z-index was raised to
     1001 by C10 (#3125) to escape in-project toolbar stacking, but
     `.nt-pop` (this panel's own fixed-position root) was left at its old
     30, so it now paints BEHIND `.chat-panel` instead of over it. Raised
     above 1001, same reasoning as C10's own fix. */
  .nt-pop {
    position: fixed;
    z-index: 1002;
  }

  .nt-panel {
    width: 372px;
    box-sizing: border-box;
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: var(--shadow-pop);
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  /* ── Header ──────────────────────────────────────────────────────────────── */
  .nt-head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 15px 12px;
    border-bottom: 1px solid var(--border-faint);
  }

  .nt-head__title {
    font-size: 14px;
    font-weight: 600;
    color: var(--fg-1);
    letter-spacing: -0.01em;
    margin: 0;
  }

  .nt-count {
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    box-sizing: border-box;
    border-radius: 9px;
    background: var(--accent);
    color: var(--fg-on-accent);
    font-size: 11px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .nt-head__spacer {
    flex: 1;
  }

  .nt-markall {
    background: transparent;
    border: 0;
    cursor: pointer;
    font-family: var(--font-sans);
    font-size: 12.5px;
    font-weight: 500;
    color: var(--fg-3);
    padding: 5px 9px;
    border-radius: 7px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    transition: background var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease);
  }

  .nt-markall:hover {
    background: var(--overlay-hover);
    color: var(--accent);
  }

  .nt-markall[disabled] {
    color: var(--fg-4);
    cursor: default;
  }

  .nt-markall[disabled]:hover {
    background: transparent;
    color: var(--fg-4);
  }

  /* ── List ────────────────────────────────────────────────────────────────── */
  .nt-list {
    display: flex;
    flex-direction: column;
    max-height: 428px;
    overflow-y: auto;
    padding: 5px;
  }

  /* ── Row ─────────────────────────────────────────────────────────────────── */
  .nt-row {
    position: relative;
    display: flex;
    gap: 11px;
    padding: 11px 10px 12px;
    border-radius: 9px;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease);
  }

  .nt-row:hover {
    background: var(--overlay-hover);
  }

  .nt-row + .nt-row {
    box-shadow: 0 -1px 0 var(--border-faint);
  }

  .nt-row:hover,
  .nt-row:hover + .nt-row {
    box-shadow: none;
  }

  /* Status dot: replaces both the old left-edge unread dot and the icon tile */
  .nt-status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    align-self: center;
    background: var(--accent);
    transition:
      background var(--dur-norm) var(--ease),
      opacity var(--dur-norm) var(--ease);
  }

  .nt-row.is-read .nt-status-dot {
    background: var(--fg-4);
  }

  /* Body */
  .nt-body {
    flex: 1;
    min-width: 0;
  }

  .nt-row__top {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .nt-title {
    flex: 1;
    min-width: 0;
    font-size: 13.5px;
    font-weight: 500;
    color: var(--fg-1);
    line-height: 1.4;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .nt-row.is-read .nt-title {
    color: var(--fg-2);
    font-weight: 400;
  }

  .nt-time {
    flex-shrink: 0;
    font-size: 11.5px;
    color: var(--fg-4);
    font-variant-numeric: tabular-nums;
  }

  .nt-preview {
    font-size: 12.5px;
    color: var(--fg-3);
    line-height: 1.45;
    margin-top: 3px;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  /* Dismiss × overlap fix: give body space on hover so timestamp doesn't slide under × button */
  .nt-row:hover .nt-body {
    padding-right: 26px;
  }

  /* Dismiss × — hover-revealed */
  .nt-x {
    position: absolute;
    top: 8px;
    right: 8px;
    width: 24px;
    height: 24px;
    border-radius: 6px;
    border: 0;
    background: transparent;
    color: var(--fg-4);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transition:
      opacity var(--dur-fast) var(--ease),
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }

  .nt-row:hover .nt-x {
    opacity: 1;
  }

  .nt-x:hover {
    background: var(--overlay-press);
    color: var(--fg-1);
  }

  /* Dismissing: row collapsing out */
  .nt-row.is-dismissing {
    background: transparent;
    opacity: 0.35;
    transform: translateX(14px);
    transition:
      opacity var(--dur-norm) var(--ease),
      transform var(--dur-norm) var(--ease);
  }

  /* ── Empty state (calm, not an alert) ───────────────────────────────────── */
  .nt-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    padding: 46px 34px 44px;
  }

  .nt-empty__art {
    width: 76px;
    height: 76px;
    margin-bottom: 18px;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .nt-empty__ring {
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background: radial-gradient(circle at 50% 42%, color-mix(in srgb, var(--accent) 10%, transparent), transparent 68%);
  }

  .nt-empty__bell {
    color: var(--fg-4);
    position: relative;
  }

  .nt-empty__title {
    font-size: 14px;
    font-weight: 600;
    color: var(--fg-2);
    margin: 0 0 6px;
  }

  .nt-empty__sub {
    font-size: 12.5px;
    color: var(--fg-4);
    line-height: 1.55;
    margin: 0;
    max-width: 250px;
  }

  /* ── Footer ──────────────────────────────────────────────────────────────── */
  .nt-foot {
    border-top: 1px solid var(--border-faint);
    padding: 4px;
  }

  /* See all — visual only, disabled, no click handler (post-launch) */
  .nt-seeall {
    width: 100%;
    background: transparent;
    border: 0;
    cursor: not-allowed;
    font-family: var(--font-sans);
    font-size: 12.5px;
    font-weight: 500;
    color: var(--fg-4);
    padding: 10px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    opacity: 0.5;
  }

  .nt-seeall[disabled] {
    pointer-events: none;
  }

  /* §31.2.7 Subtask badge in drawer rows */
  .nt-tag {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-top: 5px;
    font-size: 11px;
    font-weight: 500;
    color: var(--fg-3);
    background: var(--surface-2);
    padding: 2px 7px;
    border-radius: 5px;
    line-height: 1.4;
  }

  .nt-tag--subtask {
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 0.02em;
  }

  .nt-tag--subtask svg {
    color: var(--fg-4);
  }

  /* Hover ×: timestamp shifts left to make room (matches design spec) */
  .nt-time {
    transition: margin-right var(--dur-fast) var(--ease);
  }

  .nt-row:hover .nt-time {
    margin-right: 22px;
  }
</style>
