<script lang="ts">
  /**
   * SessionDropdownHeader.svelte — Project AI session dropdown header (#395 PR7).
   *
   * Replaces the entire .chat-header div in ChatPanel.svelte (§395 audit attach-point).
   *
   * Header bar: project name (top line) + active session name (bottom line), live cyan
   * dot, and ▾/▴ chevron. The WHOLE bar is the click target. Dot and chevron are always
   * visible regardless of width.
   *
   * OVERFLOW WATCH ITEM (§395 audit watch-item #1):
   *   The .chat-panel ancestor has `overflow: hidden`, which would clip any
   *   position:absolute dropdown. The dropdown panel uses `position: fixed` and is
   *   repositioned via getBoundingClientRect() on every open, so it visually anchors
   *   below the header bar while remaining unclipped by any ancestor overflow setting.
   *   This mirrors the NotificationPanel pattern.
   *
   * INLINE-RENAME CLIPPING WATCH ITEM (§395 audit watch-item #2):
   *   The rename input lives inside the fixed dropdown panel, which is outside the
   *   overflow:hidden ancestor, so there is no clipping risk.
   *
   * Z-INDEX WATCH ITEM (§395 audit watch-item #3):
   *   z-index 200 — above chat content (.chat-panel z-index:1), below modals (9999).
   *
   * RESPONSIVE:
   *   Responsive off the header bar's own rendered width via ResizeObserver.
   *   Threshold 380px (the dblclick-reset default for chatWidth).
   *   · Wide (≥380px): + button visible in header bar; compact=false on SessionRow.
   *   · Narrow (<380px): + button moves into dropdown; name truncates; compact=true.
   *
   * §395 PR7 — spec-covered-by: #1979
   */

  import { onMount, onDestroy } from 'svelte';
  import SessionRow from '$lib/components/SessionRow.svelte';
  import NewSessionSheet from '$lib/components/NewSessionSheet.svelte';
  import EditCrewPopover from '$lib/components/EditCrewPopover.svelte';
  import {
    sessionStore,
    sortSessions,
    shouldShowFilter,
    filterSessions,
    getSessionDisplayTitle,
    EMPTY_STATE_SINGLE,
  } from '$lib/stores/sessionStore';
  import { getAgents, listRuntimeConnections } from '$lib/api';
  import type { RuntimeConnectionInfo } from '$lib/api';
  import { filterConnectedAgents, agentProviderLabel, parseCrewAgentIds, capCrewChips, agentInitials } from '$lib/utils/agentDisplay';
  import { resolveBoundAgent, computeBindingUnavailableReason } from '$lib/services/sessionReadOnlyCheck';
  import { isJobSessionOrigin } from '$lib/services/jobSessionBack';
  import { activeSessionOrigin } from '$lib/stores/app';
  import type { Agent, Session } from '$lib/types';

  // #2682: gates the [2604-PROBE] console diagnostics below. Off by default —
  // set localStorage.pane-debug=1 to enable (same runtime flag already used
  // by ChatSidebar.svelte, Editor.svelte, KanbanBoard.svelte, etc. for
  // verbose diagnostic logging). Was previously unconditional, polluting
  // production console output on every session mount.
  const DEBUG = typeof localStorage !== 'undefined' && localStorage.getItem('pane-debug') === '1';

  // ─── Props ────────────────────────────────────────────────────────────────

  interface Props {
    /** Surface identifier (project UUID). */
    projectId: string;
    /** Display name for the project (shown on the top line of the header bar). */
    projectName: string;
    /** Optional collapse callback passed through from ChatPanel. */
    onClose?: () => void;
    /**
     * Called after a session switch so ChatSidebar can reload messages for the
     * newly-active session. Receives the just-switched-to Session object.
     */
    onSessionSwitch?: (session: Session) => void;
    /**
     * ADR-0062 D5: fires whenever the active session's read-only state
     * changes (bound agent deactivated, or outside the active provider
     * class). `null` = available. ChatPanel uses this to disable the
     * composer and render the read-only notice.
     */
    onReadOnlyChange?: (state: { reason: 'agent' | 'provider'; agent: Agent } | null) => void;
    /**
     * #2604 RC-B: fires whenever the active session's OWN bound agent
     * (`activeSession.agent_id` → `allAgents.find(...)`, same `boundAgent`
     * derivation the binding line already uses below) changes identity or
     * becomes known/unknown. ChatPanel uses this as its speaker-label
     * fallback source — it must NEVER use the app-wide default agent name
     * for this (see docs/audits/2026-08-20-c8-session-binding-investigation.md,
     * Root Cause B).
     */
    onBoundAgentChange?: (name: string | null) => void;
    /**
     * #2604: fires once, the first time `allAgents` finishes its initial
     * `getAgents()` fetch (success or failure) for THIS component instance.
     * ChatPanel uses this to distinguish "agent roster hasn't loaded yet"
     * (render the neutral thinking treatment) from "agent roster loaded,
     * genuinely no bound agent" (render the 'Pane' literal, ADR-0059 D5).
     * Before this signal existed, both states looked identical to ChatPanel
     * (`sessionBoundAgentName === null`), which is what let the first-message
     * -after-session-open race render 'Pane' during the load window.
     */
    onAgentsLoadedChange?: (loaded: boolean) => void;
  }

  let { projectId, projectName, onClose, onSessionSwitch, onReadOnlyChange, onBoundAgentChange, onAgentsLoadedChange }: Props = $props();

  // ─── Store subscription ───────────────────────────────────────────────────

  let sessions = $state<Session[]>([]);
  let activeSession = $state<Session | null>(null);
  let storeLoading = $state(false);
  let storeError = $state<string | null>(null);

  const unsubStore = sessionStore.subscribe(s => {
    sessions = s.sessions;
    activeSession = s.activeSession;
    storeLoading = s.loading;
    storeError = s.error;
  });

  // ─── Width tracking (ResizeObserver on the wrapper) ───────────────────────

  let wrapperEl: HTMLElement | null = $state(null);
  let containerWidth = $state(420); // start wide to avoid narrow flash
  const NARROW_THRESHOLD = 380;

  let resizeObs: ResizeObserver | null = null;

  // ─── Dropdown position (fixed, so it escapes overflow:hidden ancestors) ───

  let dropdownPanelEl: HTMLElement | null = $state(null);
  let dropdownTop = $state(0);
  let dropdownLeft = $state(0);
  let dropdownWidth = $state(0);

  function updateDropdownCoords() {
    if (!wrapperEl) return;
    const rect = wrapperEl.getBoundingClientRect();
    dropdownTop = rect.bottom;
    dropdownLeft = rect.left;
    dropdownWidth = rect.width;
  }

  // ─── Dropdown open/close state ────────────────────────────────────────────

  let dropdownOpen = $state(false);
  let showHidden = $state(false);
  let filterQuery = $state('');

  // ─── New Session sheet (§36.17.4, #3050 PR 10 — replaces the old
  // ADR-0062 D2 split-control popover at this call site, Ruling 1) ────────

  let agentPickerOpen = $state(false);
  let pickerAgents = $state<Agent[]>([]);
  let newSessionBtnEl: HTMLButtonElement | null = $state(null);

  // ─── Edit-crew popover (§36.17.4, #3050 PR 10) ─────────────────────────

  let editCrewOpen = $state(false);
  let editCrewTop = $state(0);
  let editCrewLeft = $state(0);
  let crewBtnEl: HTMLButtonElement | null = $state(null);

  function openDropdown() {
    updateDropdownCoords();
    dropdownOpen = true;
  }

  function closeDropdown() {
    dropdownOpen = false;
    filterQuery = '';
    showHidden = false;
  }

  function toggleDropdown() {
    if (dropdownOpen) {
      closeDropdown();
    } else {
      openDropdown();
    }
  }

  // ─── Derived: session list ────────────────────────────────────────────────

  let allSessionCount = $derived(sessions.length);
  let showFilter = $derived(shouldShowFilter(allSessionCount));
  // Apply sortSessions before filterSessions so active-first ordering is
  // guaranteed even when the store was seeded without going through sortSessions
  // (e.g., in tests via direct __set calls).
  let sortedSessions = $derived(sortSessions(sessions));
  let splitSessions = $derived(filterSessions(sortedSessions, filterQuery));
  let visibleSessions = $derived(splitSessions.visible);
  let hiddenSessions = $derived(splitSessions.hidden);
  // Unfiltered visible count: used for isLastVisible so that an active search
  // filter never wrongly disables Hide when ≥2 non-hidden sessions exist.
  let allVisibleCount = $derived(sortedSessions.filter(s => s.hidden_at === null).length);

  let activeDisplayTitle = $derived(
    activeSession ? getSessionDisplayTitle(activeSession) : 'Loading…'
  );

  // #3166 C58: a persistent "Job session · <name>" cue, driven off the
  // SAME `$activeSessionOrigin ∈ {spawn, schedule}` check #3157's `←`
  // button (`ChatPanel.svelte`'s `showBackButton`/`shouldShowJobSession
  // BackButton`) already uses — `isJobSessionOrigin` is the shared half
  // both now call, so the two can never independently drift about what
  // counts as "a job session." Deliberately NOT gated on a back target
  // existing (unlike the `←` button): the cue's job is to say what this
  // session IS, which is true even with nothing to go back to (e.g. a
  // deep link straight into a job session). Lives HERE — in the header,
  // which is always on screen — rather than only inside the JOB BRIEF
  // message strip (`ChatPanel.svelte`), which scrolls out of view the
  // moment the conversation continues past the first message.
  let isJobSession = $derived(isJobSessionOrigin($activeSessionOrigin));

  let isNarrow = $derived(containerWidth < NARROW_THRESHOLD);

  // ─── Session header binding line (ADR-0062 D9, design pc-a.jsx PcSessionHeader) ─
  // Agent name + provider + model + "bound" lock cue, below the session name.
  // Same agent/connections fetch backs both the binding line and the New
  // Session picker (handleNewSession), so it's cached at component scope
  // and refreshed on mount + pane-agent-switched (D6: the default-agent
  // display refresh signal survives; session headers themselves don't listen
  // for it — this cache refresh keeps the *binding line's own* agent facts
  // like current is_active/model current, not which agent the session is bound to).
  //
  // #2563 B2 fix: previously fetched `aiLoadConfig()` for the now-retired
  // `ai_settings.provider` global scalar. Replaced with
  // `listRuntimeConnections()` — ADR-0063 D6's per-agent connectivity check.
  let allAgents = $state<Agent[]>([]);
  let runtimeConnections = $state<RuntimeConnectionInfo[]>([]);
  // #2604: tracks whether getAgents() has resolved (success OR failure) at
  // least once for this component instance. `boundAgent` is $derived off
  // `activeSession`/`allAgents` (line ~205 below) — on the very first render
  // after mount, `activeSession` can resolve (session store hydrates) on a
  // faster tick than `allAgents` finishes its async fetch, so `boundAgent`
  // reads as null NOT because the session has no bound agent, but because
  // the agent roster simply hasn't arrived yet. Without this flag, the
  // onBoundAgentChange effect below can't tell those two nulls apart and
  // emits the false one, which is what produced the 'Pane' flash on the
  // first message of a freshly-opened session (#2604).
  let agentsLoaded = $state(false);

  async function loadAgentContext() {
    try {
      const [agents, connections] = await Promise.all([getAgents(), listRuntimeConnections()]);
      allAgents = agents;
      runtimeConnections = connections;
    } catch (e) {
      console.error('[SessionDropdownHeader] failed to load agent context:', e);
    } finally {
      // #2604 probe: fires once per mount, marks the point after which
      // boundAgent's null/non-null state can be trusted (fetch attempted,
      // success or failure — either way allAgents now reflects reality).
      if (!agentsLoaded) {
        agentsLoaded = true;
        if (DEBUG) {
          console.log('[2604-PROBE] SessionDropdownHeader agentsLoaded=true (getAgents resolved)', {
            agentCount: allAgents.length,
            activeSessionId: activeSession?.id ?? null,
            ts: performance.now(),
          });
        }
      }
    }
  }

  // D22 (#2649 family): delegates to the shared sessionReadOnlyCheck helper so
  // this component and ChatSidebar (Home) can never disagree on the lookup.
  let boundAgent = $derived(resolveBoundAgent(activeSession, allAgents));

  // ─── Crew derivations (§36.17.4, #3050 PR 10) ──────────────────────────

  let isOcCaptain = $derived(boundAgent?.runtime_type === 'openclaw');
  let activeCrewIds = $derived(parseCrewAgentIds(activeSession?.crew_agent_ids));
  let activeCrewAgents = $derived(
    activeCrewIds.map(id => allAgents.find(a => a.id === id)).filter((a): a is Agent => !!a)
  );
  // Crew pool for the edit popover: API agents only, minus the captain —
  // same predicate the New Session sheet's own crew section uses.
  let crewEditPool = $derived(
    boundAgent ? allAgents.filter(a => a.runtime_type !== 'openclaw' && a.id !== boundAgent!.id) : []
  );

  // #2604 RC-B: surface the bound agent's name to ChatPanel via callback,
  // scoped to THIS session only — does not react to pane-agent-switched
  // (the default-agent change event), only to activeSession/allAgents
  // changing, which is exactly the session-scoped signal ChatPanel needs.
  //
  // #2604 load-order fix: `boundAgent` reads null in two DISTINCT cases —
  // (1) the session genuinely has no bound agent, and (2) `allAgents` just
  // hasn't finished its first `getAgents()` fetch yet (activeSession can
  // resolve on a faster tick than the agent roster on session-open). Case
  // (2) previously caused this effect to emit `null` to ChatPanel, which
  // fell all the way through to the hardcoded 'Pane' literal for the first
  // stream token — self-correcting only once `allAgents` landed (coincidentally
  // near stream completion). Gate the emission on `agentsLoaded` so we only
  // ever emit null for the genuine case (1).
  $effect(() => {
    if (!agentsLoaded) {
      // #2604 probe: this is the exact null-emission window that used to leak
      // through to ChatPanel and render 'Pane'. Suppressed here — logged so
      // Matt/Reed can confirm the window's timing from a live repro.
      if (DEBUG) {
        console.log('[2604-PROBE] SessionDropdownHeader effect: suppressed null emission (agentsLoaded=false)', {
          activeSessionId: activeSession?.id ?? null,
          ts: performance.now(),
        });
      }
      return;
    }
    onBoundAgentChange?.(boundAgent?.name ?? null);
  });

  // #2604: mirror agentsLoaded out to ChatPanel so it can distinguish
  // "still loading" from "genuinely no bound agent" for its own fallback
  // render (the literal 'Pane' vs. the neutral thinking treatment).
  $effect(() => {
    onAgentsLoadedChange?.(agentsLoaded);
  });

  // ADR-0063 D6 ruling: read-only fires ONLY when the bound agent is inactive,
  // OR its runtime is genuinely disconnected — replaces the retired D5
  // "out-of-class XOR" (`runtime_ref != null !== (provider === 'openclaw')`),
  // which checked the bound agent's identity against the global
  // `ai_settings.provider` scalar instead of actual runtime connectivity.
  // D22 (#2649 family): delegates to the shared sessionReadOnlyCheck helper —
  // see that file's docstring for why both surfaces must call the same logic.
  let bindingUnavailableReason = $derived(
    computeBindingUnavailableReason(boundAgent, runtimeConnections)
  );

  $effect(() => {
    if (bindingUnavailableReason && boundAgent) {
      onReadOnlyChange?.({ reason: bindingUnavailableReason, agent: boundAgent });
    } else {
      onReadOnlyChange?.(null);
    }
  });

  // ─── Lifecycle ────────────────────────────────────────────────────────────

  // Reactive reload when projectId changes (e.g. user switches project without
  // component remounting). Also covers initial load so the onMount call below
  // is the fallback for non-Svelte-5 rune contexts.
  $effect(() => {
    const pid = projectId; // captured for reactivity
    if (pid) {
      sessionStore.reset();
      // Always fetch hidden sessions — showHidden controls UI display only, not fetching.
      sessionStore.loadSessions(pid, true);
    }
  });

  onMount(() => {
    // NOTE: initial session load is handled by the $effect above.
    // A duplicate call that previously lived here caused every mount to fire the
    // backend list command twice, with a momentary empty-state flash between them.
    // Removed as part of BUG-2 fix (#395).

    // ResizeObserver tracks the header's own rendered width for narrow/wide mode
    if (wrapperEl) {
      resizeObs = new ResizeObserver(entries => {
        for (const entry of entries) {
          containerWidth = entry.contentRect.width;
        }
      });
      resizeObs.observe(wrapperEl);
    }

    // Outside-click closes the dropdown (capture phase so it fires before
    // child stopPropagation)
    const handleOutsideClick = (e: MouseEvent) => {
      if (!dropdownOpen) return;
      const target = e.target as Node | null;
      if (!target) return;
      const inWrapper = wrapperEl?.contains(target) ?? false;
      const inDropdown = dropdownPanelEl?.contains(target) ?? false;
      if (!inWrapper && !inDropdown) {
        closeDropdown();
      }
    };

    // Escape key closes dropdown
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && dropdownOpen) {
        closeDropdown();
      }
    };

    window.addEventListener('click', handleOutsideClick, true);
    window.addEventListener('keydown', handleKeyDown);

    // ADR-0062 D6: session headers derive from the session's own binding and
    // do not listen for 'pane-agent-switched' to determine *which* agent is
    // bound (that never changes post-creation). The event remains the
    // refresh signal for default-agent display facts — here, re-fetching
    // agent rows so is_active/model changes on the *already-bound* agent
    // (e.g. it gets deactivated) are reflected without a full remount.
    loadAgentContext();
    const handleAgentSwitched = () => { loadAgentContext(); };
    window.addEventListener('pane-agent-switched', handleAgentSwitched);

    return () => {
      window.removeEventListener('click', handleOutsideClick, true);
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('pane-agent-switched', handleAgentSwitched);
    };
  });

  onDestroy(() => {
    resizeObs?.disconnect();
    unsubStore();
  });

  // ─── Callbacks ────────────────────────────────────────────────────────────

  async function handleSwitch(session: Session) {
    try {
      await sessionStore.switchSession(session.id, projectId);
      closeDropdown();
      onSessionSwitch?.(session);
    } catch (e) {
      console.error('[SessionDropdownHeader] switchSession failed:', e);
    }
  }

  // ADR-0062 D2: New Session opens the agent picker (default preselected)
  // rather than creating a session directly — the binding is permanent, so
  // it is shown at the moment of creation, every time.
  async function handleNewSession() {
    try {
      const [agents, connections] = await Promise.all([getAgents(), listRuntimeConnections()]);
      pickerAgents = filterConnectedAgents(agents, connections);
    } catch (e) {
      console.error('[SessionDropdownHeader] failed to load agents for picker:', e);
      pickerAgents = [];
    }
    if (pickerAgents.length === 0) {
      console.error('[SessionDropdownHeader] no connected-runtime agents available for New Session');
      return;
    }
    agentPickerOpen = true;
  }

  async function handleAgentPicked(agentId: string, crewAgentIds: string[]) {
    agentPickerOpen = false;
    try {
      const newSession = await sessionStore.createSession(projectId, agentId, crewAgentIds);
      closeDropdown();
      onSessionSwitch?.(newSession);
    } catch (e) {
      console.error('[SessionDropdownHeader] createSession failed:', e);
    }
  }

  function handleAgentPickerCancel() {
    agentPickerOpen = false;
  }

  // ─── Edit-crew popover handlers (§36.17.4, #3050 PR 10) ────────────────

  function openEditCrew() {
    if (!crewBtnEl || isOcCaptain) return; // "own crew" is never clickable (design: no onclick/role=button at all)
    const rect = crewBtnEl.getBoundingClientRect();
    editCrewTop = rect.bottom + 4;
    editCrewLeft = rect.left;
    editCrewOpen = true;
  }

  function closeEditCrew() {
    editCrewOpen = false;
  }

  async function handleSaveCrew(crewAgentIds: string[]) {
    editCrewOpen = false;
    if (!activeSession) return;
    try {
      await sessionStore.updateSessionCrew(activeSession.id, crewAgentIds, projectId);
    } catch (e) {
      console.error('[SessionDropdownHeader] updateSessionCrew failed:', e);
    }
  }

  function handleHide(sessionId: string) {
    sessionStore.hideSession(sessionId, projectId).catch(e => {
      console.error('[SessionDropdownHeader] hideSession failed:', e);
    });
  }

  function handleUnhide(sessionId: string) {
    sessionStore.unhideSession(sessionId, projectId).catch(e => {
      console.error('[SessionDropdownHeader] unhideSession failed:', e);
    });
  }

  function handleRename(sessionId: string, title: string) {
    sessionStore.renameSession(sessionId, title, projectId).catch(e => {
      console.error('[SessionDropdownHeader] renameSession failed:', e);
    });
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="sdh-wrapper" bind:this={wrapperEl}>
  <!-- ── Header bar (always visible, the click target) ──────────────────── -->
  <div
    class="sdh-bar"
    class:sdh-bar--narrow={isNarrow}
    onclick={toggleDropdown}
    role="button"
    tabindex="0"
    onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') toggleDropdown(); }}
    aria-expanded={dropdownOpen}
    aria-haspopup="listbox"
  >
    <!-- Live cyan dot — always present -->
    <span class="sdh-dot" aria-hidden="true"></span>

    <!-- Text block: project name + session name -->
    <div class="sdh-text">
      <span class="sdh-project">{projectName}</span>
      <span class="sdh-session" title={activeDisplayTitle}>{activeDisplayTitle}</span>
      {#if isJobSession}
        <!-- #3166 C58: persistent cue — always on screen, unlike the
             JOB BRIEF strip's own label which scrolls away. -->
        <span class="sdh-job-cue" data-testid="sdh-job-session-cue" title="This is a job's own dedicated session, not the project's regular chat">
          Job session · {activeDisplayTitle}
        </span>
      {/if}
    </div>

    <!-- Chevron — always visible -->
    <span class="sdh-chevron" aria-hidden="true">{dropdownOpen ? '▴' : '▾'}</span>

    <!-- Collapse (close chat panel) button -->
    {#if onClose}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <button
        class="sdh-close-btn"
        onclick={(e) => { e.stopPropagation(); onClose?.(); }}
        title="Close"
        aria-label="Close chat panel"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="9,18 15,12 9,6"></polyline>
        </svg>
      </button>
    {/if}
  </div>

  <!-- ── Binding line (ADR-0062 D9): agent · provider · model, permanent,
       with a "bound" lock cue. Not part of the dropdown click target —
       nothing here can be changed, so nothing responds to a click. ── -->
  {#if boundAgent}
    <div class="sdh-bind">
      <b class="sdh-bind__agent">{boundAgent.name}</b>
      <!-- §36.17.4 (#3050 PR 10): crew chips after the captain name, as a
           button (opens the edit-crew popover) — "+ Crew" muted when none;
           OpenClaw captains show "own crew", not clickable. -->
      {#if isOcCaptain}
        <span class="sdh-ownc" data-testid="sdh-own-crew">own crew</span>
      {:else if activeCrewAgents.length === 0}
        <button type="button" class="sdh-crewbtn sdh-crewbtn--add" bind:this={crewBtnEl} onclick={openEditCrew} data-testid="sdh-crew-add-btn">
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
          Crew
        </button>
      {:else}
        {@const capped = capCrewChips(activeCrewAgents)}
        <button type="button" class="sdh-crewbtn" bind:this={crewBtnEl} onclick={openEditCrew} title={capped.tooltipText} data-testid="sdh-crew-btn">
          {#each capped.shown as crewAgent (crewAgent.id)}
            <span class="sdh-av" aria-hidden="true">{agentInitials(crewAgent.name)}</span>
          {/each}
          {#if capped.overflowCount > 0}<span class="sdh-av sdh-av--more" aria-hidden="true">+{capped.overflowCount}</span>{/if}
        </button>
      {/if}
      {#if editCrewOpen && boundAgent}
        <EditCrewPopover
          captain={boundAgent}
          crew={activeCrewAgents}
          crewPool={crewEditPool}
          top={editCrewTop}
          left={editCrewLeft}
          onSave={handleSaveCrew}
          onCancel={closeEditCrew}
        />
      {/if}
      <span class="sdh-bind__sep">·</span>
      <span class="sdh-bind__meta">{agentProviderLabel(boundAgent)}</span>
      <span class="sdh-bind__sep">·</span>
      <span class="sdh-bind__meta">{boundAgent.model || 'no model'}</span>
      <span class="sdh-bind__lock" title="This session's agent is permanent">
        <svg width="11" height="11" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <rect x="3.5" y="7.5" width="9" height="6.5" rx="1.6" />
          <path d="M5.6 7.5V5.6a2.4 2.4 0 014.8 0v1.9" />
        </svg>
        bound
      </span>
    </div>
  {/if}

  <!-- ── Dropdown panel (position:fixed to escape overflow:hidden) ─────── -->
  {#if dropdownOpen}
    <div
      class="sdh-dropdown"
      bind:this={dropdownPanelEl}
      style="top:{dropdownTop}px; left:{dropdownLeft}px; width:{dropdownWidth}px;"
      role="listbox"
      aria-label="Session list"
    >
      <!-- Pinned top: New Session + filter -->
      <div class="sdh-pinned-top">
        <!-- W1 fix: single unconditional button; both isNarrow branches were identical -->
        <button class="sdh-new-session-btn" bind:this={newSessionBtnEl} onclick={handleNewSession}>
          <span class="sdh-new-icon">+</span>
          <span>New Session</span>
        </button>

        {#if agentPickerOpen}
          <NewSessionSheet
            agents={pickerAgents}
            projectLabel={projectName}
            onConfirm={handleAgentPicked}
            onCancel={handleAgentPickerCancel}
          />
        {/if}

        {#if showFilter}
          <input
            class="sdh-filter"
            type="text"
            placeholder="Search sessions…"
            bind:value={filterQuery}
            aria-label="Filter sessions"
          />
        {/if}
      </div>

      <!-- Scrolling session list -->
      <div class="sdh-list">
        {#if visibleSessions.length === 0 && !filterQuery}
          <div class="sdh-empty">
            <div class="sdh-empty-headline">{EMPTY_STATE_SINGLE.headline}</div>
            <div class="sdh-empty-body">{EMPTY_STATE_SINGLE.body}</div>
          </div>
        {:else if visibleSessions.length === 0 && filterQuery}
          <div class="sdh-empty">No sessions match "{filterQuery}"</div>
        {:else}
          {#each visibleSessions as session (session.id)}
            <div class="sdh-session-row">
              <SessionRow
                {session}
                compact={isNarrow}
                isLastVisible={allVisibleCount === 1}
                onSwitch={handleSwitch}
                onHide={handleHide}
                onRename={handleRename}
              />
            </div>
          {/each}
        {/if}
      </div>

      <!-- Pinned footer: hidden sessions toggle -->
      {#if hiddenSessions.length > 0}
        <div class="sdh-pinned-foot">
          <button
            class="sdh-hidden-toggle"
            onclick={() => { showHidden = !showHidden; }}
          >
            {showHidden ? '▴' : '▾'} Hidden ({hiddenSessions.length})
          </button>

          {#if showHidden}
            <div class="sdh-hidden-list">
              {#each hiddenSessions as session (session.id)}
                <div class="sdh-session-row sdh-session-row--hidden">
                  <SessionRow
                    {session}
                    compact={isNarrow}
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
  /* ── Wrapper ──────────────────────────────────────────────────────────── */
  .sdh-wrapper {
    position: relative;
    width: 100%;
    flex-shrink: 0;
  }

  /* ── Header bar ───────────────────────────────────────────────────────── */
  /* #3171 PR 2 (C58): was `height`/`max-height: 48px` (a hard ceiling,
     with no `overflow` guard on this element) — sized for exactly the
     original 2-row text block (`.sdh-project` + `.sdh-session`). The
     `.sdh-job-cue` 3rd row (below) pushed `.sdh-text`'s natural content
     height past that fixed box, and since `align-items: center` (not
     `stretch`) centers the overflow, the excess rendered both above AND
     below the bar's own border — reading as the "Job session · <name>"
     pill floating over the header's right edge rather than sitting
     inside it. `min-height` (only) keeps the exact same 48px height for
     every session that ISN'T a job session (the common case: 2 rows of
     text is well under 48px, so `min-height` is the dominant constraint,
     identical rendering to before) while letting the bar grow to fit a
     3rd row when one is actually present — no JS, no toggle state. */
  .sdh-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    min-height: 48px;
    box-sizing: border-box;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    background: transparent;
    cursor: pointer;
    user-select: none;
  }

  .sdh-bar:hover {
    background: rgba(255,255,255,0.02);
  }

  /* Live cyan dot — always visible */
  .sdh-dot {
    flex-shrink: 0;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #00E4E4;
    box-shadow: 0 0 4px rgba(0, 228, 228, 0.5);
  }

  /* Text block */
  .sdh-text {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    gap: 1px;
  }

  .sdh-project {
    font-size: 11px;
    font-weight: 500;
    color: rgba(255,255,255,0.4);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    line-height: 1.2;
  }

  .sdh-session {
    font-size: 12px;
    font-weight: 400;
    color: rgba(255,255,255,0.7);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    line-height: 1.3;
  }

  /* #3166 C58: persistent job-session cue — always visible in the
     header, not only inside the (scrollable) JOB BRIEF message strip. */
  .sdh-job-cue {
    display: inline-block;
    margin-top: 2px;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: #00E4E4;
    background: rgba(0, 228, 228, 0.1);
    border: 1px solid rgba(0, 228, 228, 0.25);
    border-radius: 4px;
    padding: 1px 6px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
    box-sizing: border-box;
  }

  /* Chevron — always visible */
  .sdh-chevron {
    flex-shrink: 0;
    font-size: 11px;
    color: rgba(255,255,255,0.3);
  }

  /* Collapse button */
  .sdh-close-btn {
    flex-shrink: 0;
    width: 24px;
    height: 24px;
    border: none;
    background: transparent;
    color: #666;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: color 0.15s;
    padding: 0;
  }

  .sdh-close-btn:hover {
    color: #888;
  }

  /* ── Binding line (ADR-0062 D9) ──────────────────────────────────────── */
  .sdh-bind {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 12px 8px;
    font-size: 11.5px;
    color: var(--fg-4, #5A5A5E);
    overflow: hidden;
  }

  .sdh-bind__agent {
    font-weight: 600;
    color: var(--fg-1, #F0EDE8);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    /* Agent name never truncates before provider/model per design spec —
       give it room to keep its full width and let siblings shrink first. */
    flex-shrink: 0;
  }

  .sdh-bind__sep {
    color: var(--fg-4, #5A5A5E);
    flex-shrink: 0;
  }

  /* §36.17.4 (#3050 PR 10): crew chips after the captain name, header line. */
  .sdh-crewbtn {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    height: 24px;
    padding: 0 4px;
    border-radius: 6px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--fg-3, #8A8A8F);
    cursor: pointer;
    flex-shrink: 0;
  }
  .sdh-crewbtn:hover {
    border-color: var(--border, rgba(255, 255, 255, 0.12));
    background: var(--surface-2, rgba(255, 255, 255, 0.06));
  }
  .sdh-crewbtn--add {
    font-size: 11.5px;
    gap: 4px;
    padding: 0 7px;
    color: var(--fg-4, #5A5A5E);
  }
  .sdh-crewbtn--add:hover { color: var(--fg-2, rgba(255, 255, 255, 0.7)); }

  .sdh-ownc {
    font-size: 11px;
    color: var(--fg-4, #5A5A5E);
    border: 1px solid var(--border-faint, rgba(255, 255, 255, 0.08));
    border-radius: 5px;
    padding: 1px 6px;
    flex-shrink: 0;
  }

  .sdh-av {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: #26262b;
    color: var(--fg-2, rgba(255, 255, 255, 0.7));
    font-size: 8px;
    font-weight: 600;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .sdh-av--more {
    background: transparent;
    border: 1px solid var(--border, rgba(255, 255, 255, 0.12));
    color: var(--fg-3, #8A8A8F);
    width: auto;
    min-width: 18px;
    padding: 0 4px;
    border-radius: 9px;
  }

  .sdh-bind__meta {
    font-family: var(--font-mono, monospace);
    color: var(--fg-4, #5A5A5E);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }

  .sdh-bind__lock {
    display: flex;
    align-items: center;
    gap: 3px;
    margin-left: 2px;
    color: var(--fg-4, #5A5A5E);
    flex-shrink: 0;
  }

  /* ── Dropdown panel (position:fixed) ─────────────────────────────────── */
  .sdh-dropdown {
    position: fixed;
    z-index: 200;
    background: #111116;
    border: 1px solid rgba(255,255,255,0.1);
    border-top: none;
    box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    display: flex;
    flex-direction: column;
    max-height: 400px;
    overflow: hidden;
  }

  /* ── Pinned top: New Session + filter ───────────────────────────────── */
  .sdh-pinned-top {
    flex-shrink: 0;
    border-bottom: 1px solid rgba(255,255,255,0.06);
  }

  .sdh-new-session-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 14px;
    background: transparent;
    border: none;
    color: rgba(0,228,228,0.8);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    text-align: left;
    transition: background 0.12s, color 0.12s;
  }

  .sdh-new-session-btn:hover {
    background: rgba(0,228,228,0.05);
    color: #00E4E4;
  }

  .sdh-new-icon {
    font-size: 16px;
    line-height: 1;
    color: inherit;
  }

  .sdh-filter {
    display: block;
    width: 100%;
    box-sizing: border-box;
    padding: 8px 14px;
    background: rgba(255,255,255,0.03);
    border: none;
    border-top: 1px solid rgba(255,255,255,0.05);
    color: #e0e0e0;
    font-size: 12px;
    outline: none;
    font-family: inherit;
  }

  .sdh-filter::placeholder {
    color: rgba(255,255,255,0.25);
  }

  /* ── Scrolling session list ─────────────────────────────────────────── */
  .sdh-list {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 4px 0;
  }

  .sdh-list::-webkit-scrollbar {
    width: 3px;
  }

  .sdh-list::-webkit-scrollbar-track {
    background: transparent;
  }

  .sdh-list::-webkit-scrollbar-thumb {
    background: rgba(255,255,255,0.1);
    border-radius: 2px;
  }

  /* Empty state */
  .sdh-empty {
    padding: 20px 14px;
    color: rgba(255,255,255,0.3);
    font-size: 12px;
    text-align: center;
  }

  .sdh-empty-headline {
    font-weight: 500;
    margin-bottom: 6px;
    color: rgba(255,255,255,0.4);
  }

  .sdh-empty-body {
    font-size: 11px;
    color: rgba(255,255,255,0.25);
    line-height: 1.4;
  }

  /* ── Pinned footer: hidden toggle ───────────────────────────────────── */
  .sdh-pinned-foot {
    flex-shrink: 0;
    border-top: 1px solid rgba(255,255,255,0.06);
  }

  .sdh-hidden-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 8px 14px;
    background: transparent;
    border: none;
    color: rgba(255,255,255,0.25);
    font-size: 11px;
    cursor: pointer;
    text-align: left;
    transition: color 0.12s;
  }

  .sdh-hidden-toggle:hover {
    color: rgba(255,255,255,0.45);
  }

  .sdh-hidden-list {
    border-top: 1px solid rgba(255,255,255,0.04);
    opacity: 0.7;
  }
</style>
