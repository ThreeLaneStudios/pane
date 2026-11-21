<script lang="ts">
  /**
   * SessionsDrawer.svelte — Home AI sessions drawer (#395 PR8).
   *
   * Mounted as a flex-row sibling of <ChatSidebar layoutMode="centered" />
   * inside .chat-panel.home-view. When closed it collapses to a 44px toggle
   * strip; when open it expands to 320px via CSS transition. The centered chat
   * column narrows naturally (it's flex:1).
   *
   * SESSIONS:
   *   Two-call pattern (\u00a730.2.2): listHomeSessionsWithRecovered() fires
   *   listSessions('workspace', includeHidden) + listSessions('__recovered__', true)
   *   in parallel. Workspace sessions appear first (active-first, last_active_at
   *   DESC via sortSessions). A labelled separator then introduces the Recovered
   *   group. The Recovered group is always fetched include_hidden=true.
   *
   * FILTER:
   *   Filter bar appears at \u22658 total sessions (shouldShowFilter). Searches both
   *   workspace and recovered sessions. Hidden sessions surface under the footer
   *   "show hidden" toggle when showHidden is true.
   *
   * EMPTY STATE:
   *   "Just this one so far" headline appears above the list when exactly 1
   *   visible workspace session exists (the session still renders beneath it).
   *
   * SHOW HIDDEN:
   *   Footer toggle expands a dimmed "Hidden" section with per-row Unhide.
   *   No confirm on any action: hide is reversible, New Session is non-destructive.
   *
   * \u00a7395 PR8 \u2014 spec-covered-by: #1979
   */

  import { onDestroy } from 'svelte';
  import SessionRow from '$lib/components/SessionRow.svelte';
  import NewSessionSheet from '$lib/components/NewSessionSheet.svelte';
  import {
    sortSessions,
    shouldShowFilter,
    filterSessions,
    EMPTY_STATE_SINGLE,
  } from '$lib/stores/sessionStore';
  import {
    listHomeSessionsWithRecovered,
    createSessionWithCrew as apiCreateSessionWithCrew,
    switchSession as apiSwitchSession,
    hideSession as apiHideSession,
    unhideSession as apiUnhideSession,
    renameSession as apiRenameSession,
    getAgents,
    listRuntimeConnections,
  } from '$lib/api';
  import { filterConnectedAgents } from '$lib/utils/agentDisplay';
  import type { Session, Agent } from '$lib/types';

  // ─── Props ────────────────────────────────────────────────────────────────

  interface Props {
    /**
     * Called after a session switch so the parent (home surface) can reload
     * messages for the newly-active session.
     *
     * REQUIRED — must not be optional. The original bug (BUG-1, #395) was caused by
     * optional-chaining a missing callback (`onSessionSwitch?.(session)`), which silently
     * swallowed the call when the prop wasn’t wired at the mount site. Making this
     * required surfaces the missing prop as a compile-time error rather than a silent
     * runtime no-op. (#395 fix)
     */
    onSessionSwitch: (session: Session) => void;
  }

  let { onSessionSwitch }: Props = $props();

  // ─── Drawer open/close state ──────────────────────────────────────────────

  let open = $state(false);

  function toggleOpen() {
    open = !open;
  }

  // ─── Session state ────────────────────────────────────────────────────────

  let workspaceSessions = $state<Session[]>([]);
  let recoveredSessions = $state<Session[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  // §36.17.4 (#3050 PR 10): the FULL agent roster (not the New Session
  // picker's connected-runtime-filtered subset) — needed so every
  // SessionRow can resolve its captain/crew chips regardless of current
  // connectivity. Loaded eagerly on mount, alongside the session list.
  let allAgents = $state<Agent[]>([]);

  async function loadAllAgents(): Promise<void> {
    try {
      allAgents = await getAgents();
    } catch (e) {
      console.error('[SessionsDrawer] failed to load agents for crew chips:', e);
    }
  }

  // ─── List derivations ─────────────────────────────────────────────────────

  let showHidden = $state(false);
  let filterQuery = $state('');

  let sortedWorkspace = $derived(sortSessions(workspaceSessions));
  // Total count for filter threshold: all workspace + all recovered
  let totalCount = $derived(workspaceSessions.length + recoveredSessions.length);
  let showFilter = $derived(shouldShowFilter(totalCount));

  // Workspace filter split
  let splitWorkspace = $derived(filterSessions(sortedWorkspace, filterQuery));
  let visibleWorkspace = $derived(splitWorkspace.visible);
  let hiddenWorkspace = $derived(splitWorkspace.hidden);
  // Unfiltered visible count: used for isLastVisible so that an active search
  // filter never wrongly disables Hide when ≥2 non-hidden sessions exist.
  let allVisibleWorkspaceCount = $derived(sortedWorkspace.filter(s => s.hidden_at === null).length);

  // Recovered filter (all recovered rows are hidden; shown under separator)
  let filteredRecovered = $derived(
    filterQuery.trim()
      ? recoveredSessions.filter(s =>
          s.title.toLowerCase().includes(filterQuery.trim().toLowerCase())
        )
      : recoveredSessions
  );

  // Show recovered separator only when there are recovered sessions
  let hasRecovered = $derived(recoveredSessions.length > 0);

  // ─── Load sessions ────────────────────────────────────────────────────────

  async function loadAllSessions(): Promise<void> {
    loading = true;
    error = null;
    try {
      // Always fetch hidden sessions — showHidden controls UI display only, not fetching.
      const result = await listHomeSessionsWithRecovered(true);
      workspaceSessions = result.workspace;
      recoveredSessions = result.recovered;
    } catch (e: any) {
      const msg = `[SessionsDrawer] loadAllSessions failed: ${e?.message ?? e}`;
      console.error(msg);
      error = msg;
    } finally {
      loading = false;
    }
  }

  // Initial load via $effect (runs once on mount; showHidden is display-only, not a reload trigger)
  $effect(() => {
    loadAllSessions();
    loadAllAgents();
  });

  // ─── Actions ──────────────────────────────────────────────────────────────

  // §36.17.4 (#3050 PR 10): the New Session sheet — same component,
  // same geometry as SessionDropdownHeader's own trigger (Ruling 1).
  let agentPickerOpen = $state(false);
  let pickerAgents = $state<Agent[]>([]);
  let newSessionBtnEl: HTMLButtonElement | null = $state(null);

  async function handleNewSession() {
    try {
      const [agents, connections] = await Promise.all([getAgents(), listRuntimeConnections()]);
      pickerAgents = filterConnectedAgents(agents, connections);
      agentPickerOpen = true;
    } catch (e) {
      console.error('[SessionsDrawer] failed to load agents for picker:', e);
    }
  }

  async function handleAgentPicked(agentId: string, crewAgentIds: string[]) {
    agentPickerOpen = false;
    try {
      const session = await apiCreateSessionWithCrew('workspace', agentId, crewAgentIds);
      await Promise.all([loadAllSessions(), loadAllAgents()]);
      onSessionSwitch(session);  // required — no optional chain (#395 fix)
    } catch (e) {
      console.error('[SessionsDrawer] createSession failed:', e);
    }
  }

  function handleAgentPickerCancel() {
    agentPickerOpen = false;
  }

  async function handleSwitch(session: Session) {
    try {
      await apiSwitchSession(session.id);
      await loadAllSessions();
      onSessionSwitch(session);  // required — no optional chain (#395 fix)
    } catch (e) {
      console.error('[SessionsDrawer] switchSession failed:', e);
    }
  }

  async function handleHide(sessionId: string) {
    try {
      await apiHideSession(sessionId);
      await loadAllSessions();
    } catch (e) {
      console.error('[SessionsDrawer] hideSession failed:', e);
    }
  }

  async function handleUnhide(sessionId: string) {
    try {
      await apiUnhideSession(sessionId);
      await loadAllSessions();
    } catch (e) {
      console.error('[SessionsDrawer] unhideSession failed:', e);
    }
  }

  async function handleRename(sessionId: string, title: string) {
    try {
      await apiRenameSession(sessionId, title);
      await loadAllSessions();
    } catch (e) {
      console.error('[SessionsDrawer] renameSession failed:', e);
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="sd-container" class:sd-container--open={open} data-testid="sessions-drawer">
  <!-- ── Toggle strip — always rendered ──────────────────────────────────── -->
  <div class="sd-toggle-strip">
    <button
      class="sd-toggle"
      onclick={toggleOpen}
      title={open ? 'Close Sessions' : 'Open Sessions'}
      aria-label={open ? 'Close Sessions' : 'Open Sessions'}
      aria-expanded={open}
    >
      {#if !open}
        <span class="sd-toggle-icon" aria-hidden="true">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="3" y1="6" x2="21" y2="6"></line>
            <line x1="3" y1="12" x2="21" y2="12"></line>
            <line x1="3" y1="18" x2="21" y2="18"></line>
          </svg>
        </span>
      {:else}
        <span class="sd-toggle-icon" aria-hidden="true">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </span>
      {/if}
    </button>
  </div>

  <!-- ── Drawer content — only when open ─────────────────────────────────── -->
  {#if open}
    <div class="sd-panel">
      <!-- Pinned head: "Sessions" title + New Session button -->
      <div class="sd-head">
        <span class="sd-title">Sessions</span>
        <button
          class="sd-new-btn"
          bind:this={newSessionBtnEl}
          onclick={handleNewSession}
          title="New Session"
          aria-label="New Session"
        >
          <span class="sd-new-icon" aria-hidden="true">+</span>
          <span>New Session</span>
        </button>
      </div>

      {#if agentPickerOpen}
        <NewSessionSheet
          agents={pickerAgents}
          onConfirm={handleAgentPicked}
          onCancel={handleAgentPickerCancel}
        />
      {/if}

      <!-- Filter bar (threshold: ≥8 total sessions) -->
      {#if showFilter}
        <input
          class="sd-filter"
          type="text"
          placeholder="Search sessions…"
          bind:value={filterQuery}
          aria-label="Filter sessions"
        />
      {/if}

      <!-- Scrolling session list -->
      <div class="sd-list">
        {#if loading && workspaceSessions.length === 0}
          <div class="sd-status">Loading…</div>
        {:else if error}
          <div class="sd-status sd-status--error">{error}</div>
        {:else if visibleWorkspace.length === 0 && !filterQuery}
          <!-- True empty: no workspace sessions -->
          <div class="sd-empty">
            <div class="sd-empty-headline">{EMPTY_STATE_SINGLE.headline}</div>
            <div class="sd-empty-body">{EMPTY_STATE_SINGLE.body}</div>
          </div>
        {:else if visibleWorkspace.length === 0 && filterQuery}
          <div class="sd-status">No sessions match "{filterQuery}"</div>
        {:else}
          <!-- Workspace sessions -->
          {#each visibleWorkspace as session (session.id)}
            <div class="sd-row">
              <SessionRow
                {session}
                {allAgents}
                compact={false}
                isLastVisible={allVisibleWorkspaceCount === 1}
                onSwitch={handleSwitch}
                onHide={handleHide}
                onRename={handleRename}
              />
            </div>
          {/each}
        {/if}

        <!-- Recovered group (always include_hidden; shown after separator) -->
        {#if hasRecovered && filteredRecovered.length > 0}
          <div class="sd-separator" role="separator" aria-label="Recovered sessions">
            <span class="sd-separator-label">Recovered</span>
          </div>
          {#each filteredRecovered as session (session.id)}
            <div class="sd-row sd-row--recovered">
              <SessionRow
                {session}
                {allAgents}
                compact={false}
                onSwitch={handleSwitch}
                onUnhide={handleUnhide}
                onRename={handleRename}
              />
            </div>
          {/each}
        {/if}
      </div>

      <!-- Pinned footer: show-hidden toggle -->
      {#if hiddenWorkspace.length > 0}
        <div class="sd-footer">
          <button
            class="sd-hidden-toggle"
            onclick={() => { showHidden = !showHidden; }}
          >
            {showHidden ? '▴' : '▾'} Hidden ({hiddenWorkspace.length})
          </button>

          {#if showHidden}
            <div class="sd-hidden-list">
              {#each hiddenWorkspace as session (session.id)}
                <div class="sd-row sd-row--hidden">
                  <SessionRow
                    {session}
                    {allAgents}
                    compact={false}
                    onSwitch={handleSwitch}
                    onUnhide={handleUnhide}
                    onRename={handleRename}
                  />
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  /* ── Container ─────────────────────────────────────────────────────────── */

  .sd-container {
    display: flex;
    flex-direction: row;
    align-items: stretch;
    flex-shrink: 0;
    width: 44px;
    transition: width 0.2s ease;
    overflow: hidden;
    border-left: 1px solid rgba(255, 255, 255, 0.06);
    background: #0e0e14;
  }

  .sd-container--open {
    width: 320px;
  }

  /* ── Toggle strip ──────────────────────────────────────────────────────── */

  .sd-toggle-strip {
    flex-shrink: 0;
    width: 44px;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding-top: 10px;
    gap: 8px;
  }

  .sd-toggle {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    background: transparent;
    border: none;
    cursor: pointer;
    color: rgba(255, 255, 255, 0.35);
    padding: 6px;
    border-radius: 6px;
    transition: color 0.12s, background 0.12s;
    line-height: 1;
  }

  .sd-toggle:hover {
    color: rgba(255, 255, 255, 0.7);
    background: rgba(255, 255, 255, 0.05);
  }

  .sd-toggle-icon {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  /* ── Panel (visible when open) ─────────────────────────────────────────── */

  .sd-panel {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  /* ── Head ──────────────────────────────────────────────────────────────── */

  .sd-head {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    padding: 0 12px;
    height: 44px;
    min-height: 44px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    gap: 8px;
  }

  .sd-title {
    flex: 1;
    font-size: 12px;
    font-weight: 600;
    color: rgba(255, 255, 255, 0.55);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .sd-new-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    background: transparent;
    border: 1px solid rgba(0, 228, 228, 0.25);
    border-radius: 4px;
    color: rgba(0, 228, 228, 0.7);
    font-size: 11px;
    font-weight: 500;
    padding: 4px 8px;
    cursor: pointer;
    white-space: nowrap;
    transition: color 0.12s, border-color 0.12s, background 0.12s;
    flex-shrink: 0;
  }

  .sd-new-btn:hover {
    color: #00E4E4;
    border-color: rgba(0, 228, 228, 0.5);
    background: rgba(0, 228, 228, 0.05);
  }

  .sd-new-icon {
    font-size: 14px;
    line-height: 1;
  }

  /* ── Filter ────────────────────────────────────────────────────────────── */

  .sd-filter {
    flex-shrink: 0;
    display: block;
    width: 100%;
    box-sizing: border-box;
    padding: 8px 12px;
    background: rgba(255, 255, 255, 0.03);
    border: none;
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
    color: #e0e0e0;
    font-size: 12px;
    outline: none;
    font-family: inherit;
  }

  .sd-filter::placeholder {
    color: rgba(255, 255, 255, 0.2);
  }

  /* ── Scrolling list ────────────────────────────────────────────────────── */

  .sd-list {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
    padding: 4px 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .sd-list::-webkit-scrollbar {
    width: 3px;
  }

  .sd-list::-webkit-scrollbar-track {
    background: transparent;
  }

  .sd-list::-webkit-scrollbar-thumb {
    background: rgba(255, 255, 255, 0.08);
    border-radius: 2px;
  }

  .sd-row {
    /* rows have no extra padding — SessionRow handles its own */
  }

  .sd-row--hidden {
    opacity: 0.6;
  }

  .sd-row--recovered {
    opacity: 0.7;
  }

/* ── Status / empty states ─────────────────────────────────────────────── */

  .sd-status {
    padding: 16px 12px;
    font-size: 12px;
    color: rgba(255, 255, 255, 0.25);
    text-align: center;
  }

  .sd-status--error {
    color: rgba(255, 80, 80, 0.7);
  }

  .sd-empty {
    padding: 20px 12px;
    text-align: center;
  }

  .sd-empty-headline {
    font-size: 12px;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.4);
    margin-bottom: 6px;
  }

  .sd-empty-body {
    font-size: 11px;
    color: rgba(255, 255, 255, 0.2);
    line-height: 1.4;
  }

  /* ── Recovered separator ───────────────────────────────────────────────── */

  .sd-separator {
    display: flex;
    align-items: center;
    padding: 8px 12px 4px;
    gap: 8px;
  }

  .sd-separator-label {
    font-size: 10px;
    font-weight: 600;
    color: rgba(255, 255, 255, 0.25);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    white-space: nowrap;
  }

  .sd-separator::after {
    content: '';
    flex: 1;
    height: 1px;
    background: rgba(255, 255, 255, 0.06);
  }

  /* ── Footer: show-hidden toggle ───────────────────────────────────────── */

  .sd-footer {
    flex-shrink: 0;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
  }

  .sd-hidden-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 8px 12px;
    background: transparent;
    border: none;
    color: rgba(255, 255, 255, 0.22);
    font-size: 11px;
    cursor: pointer;
    text-align: left;
    transition: color 0.12s;
  }

  .sd-hidden-toggle:hover {
    color: rgba(255, 255, 255, 0.42);
  }

  .sd-hidden-list {
    border-top: 1px solid rgba(255, 255, 255, 0.04);
    opacity: 0.7;
  }
</style>
