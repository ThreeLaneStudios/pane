<script lang="ts">
  /**
   * NewSessionSheet.svelte — the New Session sheet (§36.17.4, #3050 PR 10,
   * Captain & Crew UI). Replaces `AgentPickerPopover.svelte` at BOTH its
   * call sites (Ruling 1, Matt, 2026-10-29): `SessionDropdownHeader.svelte`'s
   * own "New Session" button and `SessionsDrawer.svelte`'s "New Session"
   * button both mount THIS component with the same props/geometry —
   * 620px, positioned over the session surface with a scrim — whichever
   * button was clicked. One component, no second layout.
   *
   * Captain list: grouped by provider (`groupAgentsByProvider`, the same
   * derivation `AgentPickerRows.svelte` already used — not reinvented),
   * search always filters both the captain list AND the crew pool
   * (Ruling 3 — no 12-agent threshold; the README's "~12" was prose, not
   * a CSS/code value). DEFAULT agent pre-selected as captain. Start is
   * always enabled (no `disabled` binding at all).
   *
   * Crew (optional): every other API agent (`runtime_type !== 'openclaw'`,
   * minus the chosen captain's own id) — "Crew pool: API agents only,
   * minus the captain. OpenClaw captains bring their own crew from the
   * gateway." (design README). Hidden entirely when the captain is an
   * OpenClaw agent, replaced by the one-line "brings its own crew" note.
   * Chip area caps at `max-height:102px` (the real value in `crew.css` —
   * pinned as a pixel value, not computed from "three rows").
   *
   * Footer reads back the choice: "<Captain> · works alone" (no crew),
   * "<Captain> + N crew" (N crew), "<Captain> · own crew" (OpenClaw) —
   * three distinct strings, not a generic template.
   *
   * 620px wide, covers the drawer/surface only, with a scrim — the exact
   * "covers the drawer only, not the whole window" geometry is a visual
   * fact best confirmed at Matt's live 4d walkthrough, not a Vitest
   * DOM-pixel assertion (Reed's scope audit, Commit 3 test guidance) —
   * this component pins the 620px width and scrim/sheet DOM structure,
   * not pixel positioning.
   */

  import type { Agent } from '$lib/types';
  import { agentInitials, groupAgentsByProvider } from '$lib/utils/agentDisplay';
  import { DEFAULT_CAPTAIN_LABEL } from '$lib/constants';
  import { matchesSearchQuery } from '$lib/utils/searchMatch';

  interface Props {
    /** Pre-filtered (connected-runtime) agent roster — same input both call sites already compute via filterConnectedAgents(). */
    agents: Agent[];
    /** Optional surface label shown under "New session" (project name for the header trigger; omitted for the Home drawer trigger). */
    projectLabel?: string;
    /** Called with the chosen captain id and the exact crew selection (always an array — empty for "works alone" or an OpenClaw captain). */
    onConfirm: (agentId: string, crewAgentIds: string[]) => void;
    onCancel: () => void;
  }

  let { agents, projectLabel, onConfirm, onCancel }: Props = $props();

  let search = $state('');
  let selectedCaptainId = $state(agents.find(a => a.is_default)?.id ?? agents[0]?.id ?? '');
  let selectedCrew = $state<Set<string>>(new Set());

  let selectedCaptain = $derived(agents.find(a => a.id === selectedCaptainId) ?? null);
  let isOpenClawCaptain = $derived(selectedCaptain?.runtime_type === 'openclaw');

  // #3166 C51/C57: delegates to the shared `matchesSearchQuery`
  // (`$lib/utils/searchMatch.ts`) — extracted FROM this exact function
  // so the image-model picker (Settings.svelte) and the New job
  // dialog's Project/Agent fields (ScheduledPanel.svelte) wire the SAME
  // substring-match logic this component established, rather than each
  // growing its own independent copy.
  function matchesSearch(agent: Agent, q: string): boolean {
    return matchesSearchQuery(agent.name, q);
  }

  // Ruling 3 (Matt, 2026-10-29): no 12-agent threshold — the search field
  // ALWAYS filters both the captain list and the crew pool whenever there
  // is text, full stop.
  let filteredCaptainAgents = $derived(agents.filter(a => matchesSearch(a, search)));
  let captainGroups = $derived(groupAgentsByProvider(filteredCaptainAgents));

  // Crew pool = API agents only, minus the captain (README: "Crew pool:
  // API agents only, minus the captain. OpenClaw captains bring their own
  // crew."). Computed off the FULL roster (not the search-filtered list)
  // so removing the captain from consideration doesn't also get
  // search-filtered twice — search is applied once, below.
  let crewCandidates = $derived(agents.filter(a => a.runtime_type !== 'openclaw' && a.id !== selectedCaptainId));
  let filteredCrewCandidates = $derived(crewCandidates.filter(a => matchesSearch(a, search)));

  function selectCaptain(id: string) {
    selectedCaptainId = id;
    // A captain can never be its own crew member — defensive, mirrors the
    // backend's own crew-pool exclusion.
    selectedCrew.delete(id);
    const agent = agents.find(a => a.id === id);
    if (agent?.runtime_type === 'openclaw') {
      // OpenClaw captains bring their own crew — any prior selection is
      // moot and must not silently resurface if the user picks an API
      // captain again without re-selecting crew.
      selectedCrew = new Set();
    }
  }

  function toggleCrew(id: string) {
    if (isOpenClawCaptain) return; // defensive — UI doesn't render toggles in this state anyway
    const next = new Set(selectedCrew);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selectedCrew = next;
  }

  let footerText = $derived.by(() => {
    if (!selectedCaptain) return '';
    if (isOpenClawCaptain) return `${selectedCaptain.name} · own crew`;
    if (selectedCrew.size === 0) return `${selectedCaptain.name} · works alone`;
    return `${selectedCaptain.name} + ${selectedCrew.size} crew`;
  });

  function handleStart() {
    if (!selectedCaptainId) return; // defensive guard only — Start has no `disabled` binding (always enabled, per design)
    const crewArr = isOpenClawCaptain ? [] : Array.from(selectedCrew);
    onConfirm(selectedCaptainId, crewArr);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onCancel();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="ns-scrim" onclick={onCancel} aria-hidden="true" data-testid="ns-scrim"></div>
<div
  class="ns-sheet"
  role="dialog"
  aria-label="New session"
  data-testid="ns-sheet"
  onclick={(e) => e.stopPropagation()}
>
  <div class="ns-h">
    <div>
      <h3>New session</h3>
      {#if projectLabel}<small>{projectLabel}</small>{/if}
    </div>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <button type="button" class="ns-close" onclick={onCancel} aria-label="Close">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M18 6 6 18" /><path d="m6 6 12 12" />
      </svg>
    </button>
  </div>

  <div class="ns-b">
    <div class="ns-cap-h">
      <span class="ns-eyb">Captain</span>
      <span class="ns-hint">Runs the session. Fixed once it starts.</span>
    </div>

    <div class="ns-search">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="11" cy="11" r="8" /><path d="m21 21-4.3-4.3" />
      </svg>
      <input
        type="text"
        placeholder="Search agents"
        bind:value={search}
        aria-label="Search agents"
        data-testid="ns-search"
      />
    </div>

    <div class="ns-aglist" role="radiogroup" aria-label="Choose a captain">
      {#each captainGroups as [label, groupAgents] (label)}
        <div class="ns-ag-g">
          {label}
          {#if label === 'OpenClaw'}<span>brings its own crew</span>{/if}
        </div>
        {#each groupAgents as agent (agent.id)}
          <button
            type="button"
            class="ns-ag"
            class:ns-ag--on={agent.id === selectedCaptainId}
            role="radio"
            aria-checked={agent.id === selectedCaptainId}
            onclick={() => selectCaptain(agent.id)}
          >
            <span class="ns-rd" aria-hidden="true"></span>
            <span class="ns-av ns-av--cap" class:ns-av--sel={agent.id === selectedCaptainId} aria-hidden="true">{agentInitials(agent.name)}</span>
            <span class="ns-ag-t">
              <b>
                {agent.name}
                {#if agent.is_default}<span class="ns-dflt">{DEFAULT_CAPTAIN_LABEL}</span>{/if}
              </b>
              {#if agent.model}<code>{agent.model}</code>{/if}
            </span>
          </button>
        {/each}
      {/each}
      {#if captainGroups.length === 0}
        <div class="ns-empty">No agents match "{search}"</div>
      {/if}
    </div>

    {#if isOpenClawCaptain}
      <div class="ns-cr-sec ns-cr-sec--oc" data-testid="ns-crew-section-oc">
        <div class="ns-cr-h"><span class="ns-eyb">Crew</span></div>
        <div class="ns-ocnote">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="12" cy="12" r="10" /><path d="M12 16v-4" /><path d="M12 8h.01" />
          </svg>
          <span>{selectedCaptain?.name} is an OpenClaw agent and brings its own crew from the gateway.</span>
        </div>
      </div>
    {:else}
      <div class="ns-cr-sec" data-testid="ns-crew-section">
        <div class="ns-cr-h">
          <span class="ns-eyb">Crew <em>(optional)</em></span>
          <span class="ns-cnt2">{selectedCrew.size === 0 ? 'None' : `${selectedCrew.size} selected`}</span>
        </div>
        <p class="ns-cr-help">Agents the captain can hand jobs to. No crew = captain works alone.</p>
        <div class="ns-ccs">
          {#each filteredCrewCandidates as agent (agent.id)}
            <button
              type="button"
              class="ns-cc"
              class:ns-cc--on={selectedCrew.has(agent.id)}
              aria-pressed={selectedCrew.has(agent.id)}
              onclick={() => toggleCrew(agent.id)}
            >
              <span class="ns-av ns-av--sm" aria-hidden="true">{agentInitials(agent.name)}</span>
              {agent.name}
              {#if selectedCrew.has(agent.id)}
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <path d="M20 6 9 17l-5-5" />
                </svg>
              {/if}
            </button>
          {/each}
          {#if filteredCrewCandidates.length === 0}
            <div class="ns-empty">No crew candidates match "{search}"</div>
          {/if}
        </div>
      </div>
    {/if}
  </div>

  <div class="ns-f">
    <span class="ns-sum2" class:ns-sum2--mut={!isOpenClawCaptain && selectedCrew.size === 0}>
      <span class="ns-cl">
        {#if selectedCaptain}<span class="ns-av ns-av--cap" aria-hidden="true">{agentInitials(selectedCaptain.name)}</span>{/if}
        {#if !isOpenClawCaptain}
          {#each Array.from(selectedCrew).slice(0, 2) as crewId (crewId)}
            {@const crewAgent = agents.find(a => a.id === crewId)}
            {#if crewAgent}<span class="ns-av ns-av--sm" aria-hidden="true">{agentInitials(crewAgent.name)}</span>{/if}
          {/each}
        {/if}
      </span>
      <span data-testid="ns-footer-text">{footerText}</span>
    </span>
    <button type="button" class="ns-btn-g" onclick={onCancel}>Cancel</button>
    <button type="button" class="ns-btn-p" onclick={handleStart} data-testid="ns-start-btn">Start session</button>
  </div>
</div>

<style>
  .ns-scrim {
    position: fixed;
    inset: 0;
    z-index: 199;
    background: rgba(5, 5, 7, 0.6);
  }

  .ns-sheet {
    position: fixed;
    z-index: 200;
    left: 50%;
    top: 44px;
    transform: translateX(-50%);
    width: 620px;
    max-height: calc(100vh - 88px);
    overflow-y: auto;
    background: var(--surface-1);
    border: 1px solid var(--border-strong);
    border-radius: 14px;
    box-shadow: var(--shadow-pop);
    display: flex;
    flex-direction: column;
  }

  .ns-h {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 16px 14px 12px 22px;
    border-bottom: 1px solid var(--border-faint, var(--border));
  }

  .ns-h > div { flex: 1; }
  .ns-h h3 { font-size: 16px; margin: 0; }
  .ns-h small { font-size: 12px; color: var(--fg-3); }

  .ns-close {
    background: transparent;
    border: none;
    color: var(--fg-3);
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
  }
  .ns-close:hover { background: var(--overlay-hover, rgba(255, 255, 255, 0.06)); color: var(--fg-1); }

  .ns-b {
    padding: 14px 22px 4px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .ns-cap-h { display: flex; align-items: baseline; gap: 10px; }
  .ns-eyb {
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--fg-4);
  }
  .ns-eyb em { font-style: normal; text-transform: none; letter-spacing: 0; font-weight: 400; color: var(--fg-4); }
  .ns-hint { font-size: 11.5px; color: var(--fg-4); }

  .ns-search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 11px;
    border: 1px solid var(--border-faint, var(--border));
    border-radius: 8px;
    color: var(--fg-4);
  }
  .ns-search input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    color: var(--fg-1);
    font-size: 13px;
  }

  .ns-aglist {
    border: 1px solid var(--border-faint, var(--border));
    border-radius: 10px;
    padding: 4px;
    background: var(--surface-2, #121216);
    max-height: 280px;
    overflow-y: auto;
  }

  .ns-ag-g {
    display: flex;
    gap: 8px;
    align-items: baseline;
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--fg-4);
    padding: 8px 10px 3px;
  }
  .ns-ag-g span { text-transform: none; letter-spacing: 0; font-weight: 400; font-size: 11px; }

  .ns-ag {
    display: flex;
    align-items: center;
    gap: 11px;
    width: 100%;
    height: 42px;
    padding: 0 10px;
    border-radius: 7px;
    background: transparent;
    border: none;
    cursor: pointer;
    text-align: left;
  }
  .ns-ag--on { background: var(--accent-soft); }
  /* C15 (#3120 PR 3): the highlighted captain row's model subtitle
     (`.ns-ag-t code`, below) stayed `var(--fg-4)` (`#444444` in the dark
     theme) even when the row's own background lightens to the
     teal-tinted `var(--accent-soft)` above — a muted-gray meant for a
     plain dark surface read as unreadable against the lighter, tinted
     one. `--fg-2` (`#A0A0A0`) is the next step up this file already uses
     for secondary-but-legible text (`.ns-ag-t b` uses `--fg-1`, the name
     itself); scoped to the selected state only — the unselected row's
     `--fg-4` is unaffected. */
  .ns-ag--on .ns-ag-t code { color: var(--fg-2); }

  .ns-rd {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 1.5px solid var(--fg-4);
    flex-shrink: 0;
    box-sizing: border-box;
  }
  .ns-ag--on .ns-rd {
    border-color: var(--accent);
    box-shadow: inset 0 0 0 3px var(--accent-soft), inset 0 0 0 7px var(--accent);
  }

  .ns-av {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: #26262b;
    color: var(--fg-2);
    font-size: 9.5px;
    font-weight: 600;
    letter-spacing: 0.02em;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  /* Captain chip: 22px, 1px teal ring with a 1.5px gap (crew.css: box-shadow
     0 0 0 1.5px <bg>, 0 0 0 2.5px var(--accent) → 2.5 - 1.5 = 1px ring). */
  .ns-av--cap.ns-av--sel,
  .ns-cl .ns-av--cap {
    box-shadow: 0 0 0 1.5px var(--surface-2, #121216), 0 0 0 2.5px var(--accent);
    color: var(--fg-1);
  }
  /* Crew chip: 18px, no ring. */
  .ns-av--sm { width: 18px; height: 18px; font-size: 8px; }

  .ns-ag-t { min-width: 0; display: flex; flex-direction: column; gap: 1px; }
  .ns-ag-t b { font-size: 13.5px; font-weight: 500; color: var(--fg-1); display: flex; align-items: center; gap: 8px; }
  .ns-ag-t code { font-family: var(--font-mono); font-size: 11px; color: var(--fg-4); }

  .ns-dflt {
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--accent);
    background: var(--accent-soft);
    border-radius: 5px;
    padding: 2px 6px;
  }

  .ns-cr-sec {
    padding-top: 12px;
    border-top: 1px solid var(--border-faint, var(--border));
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .ns-cnt2 { margin-left: auto; font-size: 11.5px; color: var(--fg-3); }
  .ns-cr-help { margin: 0; font-size: 12px; color: var(--fg-3); }

  /* Chip area cap — the real pinned value in crew.css, not "three rows"
     computed from a row count. */
  .ns-ccs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 4px;
    max-height: 102px;
    overflow-y: auto;
    align-content: flex-start;
    padding-right: 4px;
  }

  .ns-cc {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 11px 0 4px;
    border-radius: 15px;
    border: 1px solid var(--border);
    font-size: 12.5px;
    color: var(--fg-2);
    background: transparent;
    cursor: pointer;
  }
  .ns-cc--on {
    border-color: rgba(0, 228, 228, 0.6);
    background: var(--accent-soft);
    color: var(--fg-1);
  }

  .ns-ocnote {
    display: flex;
    gap: 8px;
    align-items: center;
    font-size: 12.5px;
    color: var(--fg-3);
    padding: 8px 0 4px;
  }
  .ns-ocnote svg { color: var(--fg-4); flex-shrink: 0; }

  .ns-empty { font-size: 12px; color: var(--fg-4); padding: 10px; }

  .ns-f {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 18px 16px 22px;
    border-top: 1px solid var(--border-faint, var(--border));
    margin-top: 14px;
  }
  .ns-sum2 { flex: 1; display: flex; align-items: center; gap: 10px; font-size: 12.5px; color: var(--fg-2); }
  .ns-sum2--mut { color: var(--fg-4); }
  .ns-cl { display: inline-flex; align-items: center; gap: 3px; }

  .ns-btn-g, .ns-btn-p {
    height: 30px;
    padding: 0 12px;
    font-size: 12.5px;
    border-radius: 7px;
    cursor: pointer;
  }
  .ns-btn-g { background: transparent; border: 1px solid var(--border); color: var(--fg-2); }
  .ns-btn-p { background: var(--accent); border: none; color: #04202b; font-weight: 600; }
</style>
