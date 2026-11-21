<script lang="ts">
  /**
   * SessionRow.svelte — Single session row for both Project dropdown and Home drawer.
   *
   * Props:
   *   session        — The Session object to render.
   *   compact        — Compact mode: suppress secondary "Created ..." line.
   *   onSwitch       — Called when the row is clicked to switch to this session.
   *   onHide         — Called to hide this session.
   *   onUnhide       — Called to unhide this session (shown in place of ⋯ menu).
   *   onRename       — Called with new title when rename is committed.
   *
   * Active indicator: cyan dot + left rail + soft background fill when is_active.
   * Inline rename: click name or ⋯ → Rename to enter edit mode. Enter/blur commits,
   *   Esc cancels. Empty submit calls onRename('') — backend reverts to auto-title.
   * Hover ⋯ menu: Rename + Hide (never red, never a trash can).
   * Hidden variant: shows Unhide button instead of ⋯ menu.
   *
   * §395 PR6 — spec-covered-by: #1979
   */

  import type { Session, Agent } from '$lib/types';
  import { getSessionDisplayTitle } from '$lib/stores/sessionStore';
  import { agentInitials, parseCrewAgentIds, capCrewChips } from '$lib/utils/agentDisplay';
  import { parseStoredTimestamp } from '$lib/utils/time';

  interface Props {
    session: Session;
    /**
     * §36.17.4 (#3050 PR 10): the full agent roster, used to resolve this
     * row's captain (`session.agent_id`) and crew (`session.crew_agent_ids`)
     * chips. Optional + defaults to `[]` so every pre-existing caller that
     * doesn't pass it yet still renders (no chips, same as before this PR
     * — not a breaking prop).
     */
    allAgents?: Agent[];
    compact?: boolean;
    /**
     * When true, this is the only visible (non-hidden) session on its surface.
     * The Hide menu item is disabled and shows a tooltip explaining why.
     * Prevents the backend's born-active create-if-none (§30.2.5) from spawning
     * a phantom session when the user hides the last visible session. (#395)
     */
    isLastVisible?: boolean;
    onSwitch?: (session: Session) => void;
    onHide?: (sessionId: string) => void;
    onUnhide?: (sessionId: string) => void;
    onRename?: (sessionId: string, title: string) => void;
  }

  let {
    session,
    allAgents = [],
    compact = false,
    isLastVisible = false,
    onSwitch,
    onHide,
    onUnhide,
    onRename,
  }: Props = $props();

  // ─── Derived display values ──────────────────────────────────────────────

  const isHidden = $derived(session.hidden_at !== null);
  const displayTitle = $derived(getSessionDisplayTitle(session));

  // ─── Crew chip cluster (§36.17.4, #3050 PR 10) ─────────────────────────
  // Captain 22px with a teal ring, crew 18px unringed, three max + "+N",
  // full list in the tooltip. OpenClaw captain rows show the captain chip
  // alone — crew lives on the gateway, nothing to show.

  const captainAgent = $derived(allAgents.find(a => a.id === session.agent_id) ?? null);
  const isOcCaptainRow = $derived(captainAgent?.runtime_type === 'openclaw');
  const crewAgents = $derived(
    isOcCaptainRow
      ? []
      : parseCrewAgentIds(session.crew_agent_ids)
          .map(id => allAgents.find(a => a.id === id))
          .filter((a): a is Agent => !!a)
  );
  const cappedCrew = $derived(capCrewChips(crewAgents));

  // ─── Relative time formatting ────────────────────────────────────────────

  // C44 (#3155): the parse itself (normalize + `new Date(...)`, formerly
  // this file's own `normalizeStoredTimestamp` helper) is now shared via
  // `parseStoredTimestamp` (`$lib/utils/time.ts`) — this was the last of
  // the three sites reimplementing it inline, with `normalizeStoredTimestamp`
  // itself having 3 downstream callers (all three updated below). Output
  // formatting ("Just now"/"Xm ago"/"Yesterday"/"Jul 1") is UNCHANGED —
  // this surface's vocabulary genuinely differs from every other
  // sibling's, so only the parse is unified, per the original C44 ruling.
  function formatRelativeTime(isoString: string): string {
    const now = Date.now();
    const then = parseStoredTimestamp(isoString).getTime();
    const diffMs = now - then;
    const diffSecs = Math.floor(diffMs / 1000);
    const diffMins = Math.floor(diffSecs / 60);
    const diffHours = Math.floor(diffMins / 60);
    const diffDays = Math.floor(diffHours / 24);

    if (diffMins < 1) return 'Just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays === 1) return 'Yesterday';

    // Beyond yesterday: "Jul 1", "Dec 25", etc.
    return parseStoredTimestamp(isoString).toLocaleDateString('en-US', {
      month: 'short',
      day: 'numeric',
    });
  }

  /** Same C44 fix as `formatRelativeTime` above, same reason: `isoString`
   *  (`session.created_at`) is SQLite's naive-UTC shape too — a
   *  misparse here can't manufacture a negative diff (there's no "ago"
   *  math), but it CAN shift the displayed calendar day near local
   *  midnight, the same underlying bug class. */
  function formatCreatedAt(isoString: string): string {
    return 'Created ' + parseStoredTimestamp(isoString).toLocaleDateString('en-US', {
      month: 'short',
      day: 'numeric',
    });
  }

  // ─── Rename state ────────────────────────────────────────────────────────

  let editing = $state(false);
  let editValue = $state('');
  let inputEl: HTMLInputElement | null = $state(null);
  let nameHovered = $state(false);

  function startEdit() {
    editValue = session.title; // raw title, not auto-title fallback
    editing = true;
    // Focus happens after Svelte renders the input
    setTimeout(() => inputEl?.focus(), 0);
  }

  function commitEdit() {
    if (!editing) return;
    editing = false;
    const trimmed = editValue.trim();
    // Empty/whitespace → calls onRename with '' to revert to auto-title
    onRename?.(session.id, trimmed);
  }

  function cancelEdit() {
    editing = false;
    editValue = '';
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      commitEdit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancelEdit();
    }
  }

  // ─── ⋯ menu state ────────────────────────────────────────────────────────

  let menuOpen = $state(false);
  let menuBtnEl = $state<HTMLButtonElement | null>(null);
  let menuTop = $state(0);
  let menuRight = $state(0);
  const MENU_HEIGHT_EST = 90; // ~2 items × 45px

  function updateMenuPosition() {
    if (!menuBtnEl) return;
    const rect = menuBtnEl.getBoundingClientRect();
    const spaceBelow = window.innerHeight - rect.bottom;
    menuRight = window.innerWidth - rect.right;
    if (spaceBelow < MENU_HEIGHT_EST) {
      menuTop = Math.max(4, rect.top - MENU_HEIGHT_EST);
    } else {
      menuTop = rect.bottom + 2;
    }
  }

  function toggleMenu(e: MouseEvent) {
    e.stopPropagation();
    if (!menuOpen) updateMenuPosition();
    menuOpen = !menuOpen;
  }

  function closeMenu() {
    menuOpen = false;
  }

  function handleHide(e: MouseEvent) {
    e.stopPropagation();
    if (isLastVisible) return; // guard: cannot hide the only visible session
    closeMenu();
    onHide?.(session.id);
  }

  function handleUnhide(e: MouseEvent) {
    e.stopPropagation();
    onUnhide?.(session.id);
  }

  function handleRenameFromMenu(e: MouseEvent) {
    e.stopPropagation();
    closeMenu();
    startEdit();
  }

  function handleRowClick() {
    if (editing) return;
    onSwitch?.(session);
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div
  class="session-row"
  class:is-active={session.is_active}
  class:is-hidden={isHidden}
  onclick={handleRowClick}
  role="button"
  tabindex="0"
  onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') handleRowClick(); }}
>
  <!-- Active left rail -->
  {#if session.is_active}
    <div class="active-rail" aria-hidden="true"></div>
  {/if}

  <!-- Main content -->
  <div class="row-content">
    <!-- Title row -->
    <div class="title-row">
      {#if session.is_active}
        <span class="active-dot" aria-label="Active session" title="Active"></span>
      {:else if !isHidden}
        <span class="status-dot" aria-hidden="true"></span>
      {/if}

      {#if editing}
        <!-- svelte-ignore a11y_autofocus -->
        <input
          bind:this={inputEl}
          bind:value={editValue}
          class="rename-input"
          type="text"
          placeholder="Session name"
          onclick={(e) => e.stopPropagation()}
          onblur={commitEdit}
          onkeydown={onKeydown}
          aria-label="Rename session"
          autofocus
        />
      {:else}
        <!-- svelte-ignore a11y_mouse_events_have_key_events -->
        <span
          class="session-title"
          onmouseenter={() => { nameHovered = true; }}
          onmouseleave={() => { nameHovered = false; }}
          ondblclick={(e) => { e.stopPropagation(); startEdit(); }}
          title="Double-click to rename"
          role="presentation"
        >
          {displayTitle}
          {#if nameHovered && !isHidden}
            <button
              class="pencil-btn"
              onclick={(e) => { e.stopPropagation(); startEdit(); }}
              title="Rename session"
              aria-label="Rename session"
              tabindex="-1"
            >✏</button>
          {/if}
        </span>
      {/if}

      <!-- Trailing actions -->
      <div class="row-actions">
        {#if isHidden}
          <!-- Unhide variant: eye icon + "Unhide" text button -->
          <button
            class="unhide-btn"
            onclick={handleUnhide}
            title="Unhide session"
            aria-label="Unhide session"
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/>
              <circle cx="12" cy="12" r="3"/>
            </svg>
            <span class="unhide-label">Unhide</span>
          </button>
        {:else}
          <!-- ⋯ menu trigger (hidden sessions get Unhide instead) -->
          <button
            bind:this={menuBtnEl}
            class="menu-btn"
            onclick={toggleMenu}
            title="Session options"
            aria-label="Session options"
            aria-expanded={menuOpen}
            aria-haspopup="menu"
          >⋯</button>

          {#if menuOpen}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="context-menu"
              role="menu"
              style="top:{menuTop}px; right:{menuRight}px;"
              onclick={(e) => e.stopPropagation()}
            >
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <button
                class="menu-item"
                role="menuitem"
                onclick={handleRenameFromMenu}
              >
                ✏ Rename
              </button>
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <button
                class="menu-item hide-item"
                class:hide-item--disabled={isLastVisible}
                role="menuitem"
                onclick={handleHide}
                disabled={isLastVisible}
                title={isLastVisible ? 'Cannot hide the only visible session' : 'Hide this session (reversible)'}
                aria-disabled={isLastVisible}
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <path d="M17.94 17.94A10.07 10.07 0 0112 20c-7 0-11-8-11-8a18.45 18.45 0 015.06-5.94"/>
                  <path d="M9.9 4.24A9.12 9.12 0 0112 4c7 0 11 8 11 8a18.5 18.5 0 01-2.16 3.19"/>
                  <line x1="1" y1="1" x2="23" y2="23"/>
                </svg>
                Hide — reversible
              </button>
            </div>

            <!-- Backdrop to close menu on outside click -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div class="menu-backdrop" onclick={closeMenu} aria-hidden="true"></div>
          {/if}
        {/if}
      </div>
    </div>

    <!-- Time labels -->
    <div class="meta-row">
      <span class="last-active">{formatRelativeTime(session.last_active_at)}</span>
      {#if !compact}
        <span class="created-at">{formatCreatedAt(session.created_at)}</span>
      {/if}
    </div>
  </div>

  <!-- §36.17.4 (#3050 PR 10): captain (22px, ringed) + up to 3 crew (18px,
       unringed) + "+N" overflow chip, full list in the tooltip. -->
  {#if captainAgent}
    <div class="crew-cluster" title={cappedCrew.tooltipText || undefined} data-testid="session-row-crew-cluster">
      <span class="row-av row-av--cap" aria-hidden="true">{agentInitials(captainAgent.name)}</span>
      {#each cappedCrew.shown as crewAgent (crewAgent.id)}
        <span class="row-av" aria-hidden="true">{agentInitials(crewAgent.name)}</span>
      {/each}
      {#if cappedCrew.overflowCount > 0}
        <span class="row-av row-av--more" aria-hidden="true">+{cappedCrew.overflowCount}</span>
      {/if}
    </div>
  {/if}
</div>

<style>
  /* ─── Row container ──────────────────────────────────────────────────── */

  .session-row {
    position: relative;
    display: flex;
    align-items: stretch;
    min-height: 52px;
    cursor: pointer;
    border-radius: 6px;
    background: transparent;
    transition: background 0.12s ease;
    outline: none;
    user-select: none;
    border: none;
    width: 100%;
    text-align: left;
    padding: 0;
  }

  .session-row:hover,
  .session-row:focus-visible {
    background: var(--surface-2, rgba(255, 255, 255, 0.06));
  }

  .session-row.is-active {
    background: var(--session-active-fill, rgba(0, 228, 228, 0.06));
  }

  .session-row.is-active:hover,
  .session-row.is-active:focus-visible {
    background: var(--session-active-fill-hover, rgba(0, 228, 228, 0.1));
  }

  .session-row.is-hidden {
    opacity: 0.6;
  }

  /* ─── Active left rail ───────────────────────────────────────────────── */

  .active-rail {
    width: 3px;
    border-radius: 3px 0 0 3px;
    background: var(--accent, #00E4E4);
    flex-shrink: 0;
    align-self: stretch;
  }

  /* ─── Content area ───────────────────────────────────────────────────── */

  .row-content {
    flex: 1;
    min-width: 0;
    padding: 8px 8px 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  /* ─── Title row ──────────────────────────────────────────────────────── */

  .title-row {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 22px;
  }

  /* Active cyan dot */
  .active-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent, #00E4E4);
    flex-shrink: 0;
  }

  /* Inactive status dot */
  .status-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.2);
    flex-shrink: 0;
  }

  /* Session title */
  .session-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 500;
    color: var(--fg-1, #e0e0e0);
    display: flex;
    align-items: center;
    gap: 4px;
  }

  /* Hover pencil */
  .pencil-btn {
    background: none;
    border: none;
    cursor: pointer;
    padding: 1px 3px;
    font-size: 11px;
    color: var(--fg-3, rgba(255, 255, 255, 0.4));
    border-radius: 3px;
    flex-shrink: 0;
    transition: color 0.1s;
    line-height: 1;
  }

  .pencil-btn:hover {
    color: var(--fg-1, #e0e0e0);
    background: var(--surface-3, rgba(255, 255, 255, 0.1));
  }

  /* Inline rename input */
  .rename-input {
    flex: 1;
    min-width: 0;
    background: var(--surface-2, rgba(255, 255, 255, 0.08));
    border: 1px solid var(--accent, #00E4E4);
    border-radius: 4px;
    color: var(--fg-1, #e0e0e0);
    font-size: 13px;
    font-weight: 500;
    padding: 2px 6px;
    outline: none;
  }

  /* ─── Row actions (⋯ / unhide) ───────────────────────────────────────── */

  .row-actions {
    position: relative;
    flex-shrink: 0;
  }

  .menu-btn {
    background: none;
    border: none;
    cursor: pointer;
    padding: 2px 6px;
    font-size: 16px;
    color: var(--fg-3, rgba(255, 255, 255, 0.4));
    border-radius: 4px;
    line-height: 1;
    opacity: 0;
    transition: opacity 0.1s, color 0.1s, background 0.1s;
  }

  .session-row:hover .menu-btn,
  .session-row:focus-within .menu-btn {
    opacity: 1;
  }

  .menu-btn:hover {
    color: var(--fg-1, #e0e0e0);
    background: var(--surface-3, rgba(255, 255, 255, 0.1));
  }

  /* ─── Context menu ───────────────────────────────────────────────────── */

  .context-menu {
    position: fixed;
    z-index: 300;
    background: var(--surface-2, #1a1a1a);
    border: 1px solid var(--border, rgba(255, 255, 255, 0.12));
    border-radius: 6px;
    padding: 4px;
    min-width: 160px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
  }

  .menu-item {
    display: flex;
    align-items: center;
    gap: 6px;
    background: none;
    border: none;
    cursor: pointer;
    width: 100%;
    padding: 7px 10px;
    font-size: 13px;
    color: var(--fg-1, #e0e0e0);
    border-radius: 4px;
    text-align: left;
    transition: background 0.1s;
  }

  .menu-item:hover {
    background: var(--surface-3, rgba(255, 255, 255, 0.1));
  }

  /* Hide item — NOT red, uses same fg color */
  .hide-item {
    color: var(--fg-2, rgba(255, 255, 255, 0.7));
  }

  /* Disabled hide item — greyed out, no hover highlight, not-allowed cursor */
  .hide-item--disabled,
  .hide-item--disabled:hover {
    color: var(--fg-3, rgba(255, 255, 255, 0.3));
    background: none;
    cursor: not-allowed;
    opacity: 0.5;
  }

  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 199;
  }

  /* ─── Unhide button ──────────────────────────────────────────────────── */

  .unhide-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    background: none;
    border: 1px solid var(--border, rgba(255, 255, 255, 0.15));
    cursor: pointer;
    padding: 3px 8px;
    font-size: 12px;
    color: var(--fg-2, rgba(255, 255, 255, 0.7));
    border-radius: 4px;
    transition: background 0.1s, color 0.1s;
    opacity: 0;
    white-space: nowrap;
  }

  .session-row:hover .unhide-btn,
  .session-row:focus-within .unhide-btn {
    opacity: 1;
  }

  .unhide-btn:hover {
    background: var(--surface-2, rgba(255, 255, 255, 0.08));
    color: var(--fg-1, #e0e0e0);
  }

  .unhide-label {
    font-size: 12px;
  }

  /* ─── Meta row ───────────────────────────────────────────────────────── */

  .meta-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .last-active {
    font-size: 11px;
    color: var(--fg-2, rgba(255, 255, 255, 0.55));
  }

  .created-at {
    font-size: 11px;
    color: var(--fg-3, rgba(255, 255, 255, 0.35));
  }

  /* ─── Crew chip cluster (§36.17.4, #3050 PR 10) ─────────────────────── */

  .crew-cluster {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    flex-shrink: 0;
    align-self: center;
    margin: 0 8px;
  }

  .row-av {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: #26262b;
    color: var(--fg-2, rgba(255, 255, 255, 0.7));
    font-size: 8px;
    font-weight: 600;
    letter-spacing: 0.02em;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  /* Captain chip: 22px, 1px teal ring with a 1.5px gap. */
  .row-av--cap {
    width: 22px;
    height: 22px;
    font-size: 9.5px;
    margin-right: 5px;
    box-shadow: 0 0 0 1.5px var(--bg, #0c0c11), 0 0 0 2.5px var(--accent, #00E4E4);
    color: var(--fg-1, #e0e0e0);
  }

  .row-av--more {
    background: transparent;
    border: 1px solid var(--border, rgba(255, 255, 255, 0.15));
    color: var(--fg-3, rgba(255, 255, 255, 0.4));
    width: auto;
    min-width: 18px;
    padding: 0 4px;
    border-radius: 9px;
  }
</style>
