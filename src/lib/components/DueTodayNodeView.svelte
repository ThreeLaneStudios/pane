<script lang="ts">
  /**
   * DueTodayNodeView.svelte — Svelte NodeView for the DueToday TipTap extension.
   *
   * Renders a snapshot of all tasks and subtasks due on a given date.
   * Date is stored on the TipTap node as `data-date` (YYYY-MM-DD).
   *
   * §31.2 Due Today block spec:
   *   §31.2.1  done-inclusive query (tasks + subtasks)
   *   §31.2.2  Row routing: body tap → navigate to source note; subtask → parent note
   *            + cyan ring highlight (`.nb-row.is-routed`)
   *   §31.2.3  Anchored-not-live: date is stamped at insert, never auto-rolls.
   *            Null date → error state, never query with today().
   *   §31.2.4  Check-off: task tick → update_note status; subtask tick → update_subtask.
   *   §31.2.5  Done items appear in the block (done-inclusive display).
   *   §31.2.6  Sort order: open tasks → open subtasks → done tasks → done subtasks (A-Z each).
   *   §31.2.7  Metadata lines: task: "in {Project}"; subtask: "{Parent} · {Project}",
   *            note icon on parent-note ref only, middot separator.
   *
   * Visual spec: ~/Shared/ExampleApp/docs/design/note-blocks/
   */

  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { navigateTo } from '$lib/services/navigation';

  // ── Props ─────────────────────────────────────────────────────────────────

  let {
    date: initialDate = null,
    getPos,
    editor,
  }: {
    date: string | null;
    getPos?: () => number | undefined;
    editor?: any;
  } = $props();

  // ── Types ─────────────────────────────────────────────────────────────────

  interface DueTodayItem {
    id: string;
    title: string;
    done: boolean;
    kind: 'task' | 'subtask';
    project_name: string | null;
    parent_note_title: string | null;
    parent_note_id: string | null;
    note_id: string;
  }

  // ── State ─────────────────────────────────────────────────────────────────

  let date = $state<string | null>(initialDate);
  let items = $state<DueTodayItem[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  // Highlight (routing flash) — subtask id or note id being highlighted
  let highlightId = $state<string | null>(null);

  // Block element ref for calendar popover positioning
  let blockEl = $state<HTMLElement | null>(null);

  // ── Calendar state ────────────────────────────────────────────────────────

  const CAL_MONTHS = [
    'January','February','March','April','May','June',
    'July','August','September','October','November','December',
  ];

  let calOpen    = $state(false);
  let calViewYear  = $state(new Date().getFullYear());
  let calViewMonth = $state(new Date().getMonth()); // 0-indexed
  let calPopTop  = $state(60);
  let calPopRight = $state(0);

  let calMonthLabel = $derived(CAL_MONTHS[calViewMonth] + ' ' + calViewYear);

  let calLeadDays = $derived((() => {
    const firstDow = new Date(calViewYear, calViewMonth, 1).getDay();
    if (firstDow === 0) return [] as number[];
    const prevMonthDays = new Date(calViewYear, calViewMonth, 0).getDate();
    return Array.from({ length: firstDow }, (_, i) => prevMonthDays - firstDow + 1 + i);
  })());

  let calDays = $derived((() => {
    const count = new Date(calViewYear, calViewMonth + 1, 0).getDate();
    return Array.from({ length: count }, (_, i) => i + 1);
  })());

  let calTrailDays = $derived((() => {
    const total = calLeadDays.length + calDays.length;
    const needed = Math.ceil(total / 7) * 7 - total;
    return Array.from({ length: needed }, (_, i) => i + 1);
  })());

  // ── Lifecycle ─────────────────────────────────────────────────────────────

  onMount(() => {
    loadItems();

    // Item 1 (Reed #2116 audit): wire pane-highlight-subtask → highlightRow
    // Connects notifications panel's window.dispatchEvent(...) to our highlightRow().
    const handleHighlight = (e: CustomEvent) => {
      if (e.detail?.subtaskId) highlightRow(e.detail.subtaskId);
    };
    window.addEventListener('pane-highlight-subtask', handleHighlight as EventListener);

    return () => {
      window.removeEventListener('pane-highlight-subtask', handleHighlight as EventListener);
    };
  });

  // Reload when date changes
  $effect(() => {
    const d = date; // track
    void d;
    loadItems();
  });

  // ── Data helpers ──────────────────────────────────────────────────────────

  async function loadItems() {
    // §31.2.3: null date → error state, never query with today's date.
    if (!date) {
      loading = false;
      items = [];
      return;
    }
    try {
      loading = true;
      error = null;
      items = await invoke<DueTodayItem[]>('get_due_today_items', { date });
    } catch (e: any) {
      error = e?.message ?? 'Failed to load due today items';
    } finally {
      loading = false;
    }
  }

  // ── Date helpers ──────────────────────────────────────────────────────────

  function todayStr(): string {
    return new Date().toLocaleDateString('en-CA'); // YYYY-MM-DD local
  }

  /**
   * Format YYYY-MM-DD → "Jul 20" (absolute; never relative per locked design).
   */
  function formatDate(dateStr: string): string {
    const d = new Date(dateStr + 'T00:00:00');
    return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
  }

  // ── Check-off handlers ─────────────────────────────────────────────────────

  /**
   * §31.2.4 Task check-off: toggle note status done ↔ todo.
   */
  async function toggleTask(item: DueTodayItem) {
    try {
      const newStatus = item.done ? 'todo' : 'done';
      await invoke('update_note', {
        request: { note_id: item.id, status: newStatus },
      });
      items = items.map(i =>
        i.id === item.id ? { ...i, done: !i.done } : i
      );
    } catch (e: any) {
      console.error('[DueToday] toggleTask failed:', e);
    }
  }

  /**
   * §31.2.4 Subtask check-off: toggle subtask done field.
   */
  async function toggleSubtask(item: DueTodayItem) {
    try {
      await invoke('update_subtask', {
        id: item.id,
        done: !item.done,
        text: null,
        due_date: null,
        sort_order: null,
      });
      items = items.map(i =>
        i.id === item.id ? { ...i, done: !i.done } : i
      );
    } catch (e: any) {
      console.error('[DueToday] toggleSubtask failed:', e);
    }
  }

  function handleToggle(item: DueTodayItem) {
    if (item.kind === 'task') {
      toggleTask(item);
    } else {
      toggleSubtask(item);
    }
  }

  // ── Row routing §31.2.2 ──────────────────────────────────────────────────

  /**
   * Body tap → navigate to the source note.
   * Task: navigate to the task note. Subtask: navigate to parent note + highlight.
   */
  async function handleRowClick(item: DueTodayItem) {
    // Navigate to the note that contains this item.
    navigateTo({ view: 'note_editor', noteId: item.note_id });

    // For subtasks: highlight the specific subtask after navigation.
    if (item.kind === 'subtask') {
      // Brief delay to allow navigation to complete, then broadcast highlight.
      await new Promise(resolve => setTimeout(resolve, 150));
      // Dispatch custom event for the ChecklistNodeView to pick up (if it exposes highlightItem).
      // Also apply local highlight if the block shows this subtask (same note).
      window.dispatchEvent(new CustomEvent('pane-highlight-subtask', { detail: { subtaskId: item.id } }));
    }
  }

  // ── Date chip + calendar ──────────────────────────────────────────────────

  function openCalendar(event: MouseEvent) {
    calOpen = true;
    const d = date ? new Date(date + 'T00:00:00') : new Date();
    calViewYear  = d.getFullYear();
    calViewMonth = d.getMonth();
    if (blockEl && event.currentTarget) {
      const triggerRect = (event.currentTarget as HTMLElement).getBoundingClientRect();
      const blockRect   = blockEl.getBoundingClientRect();
      calPopTop   = triggerRect.bottom - blockRect.top + 6;
      calPopRight = blockRect.right - triggerRect.right;
    }
  }

  function closeCalendar() {
    calOpen = false;
  }

  function calPrevMonth() {
    if (calViewMonth === 0) { calViewMonth = 11; calViewYear--; }
    else { calViewMonth--; }
  }

  function calNextMonth() {
    if (calViewMonth === 11) { calViewMonth = 0; calViewYear++; }
    else { calViewMonth++; }
  }

  function calDayToStr(day: number): string {
    const m = String(calViewMonth + 1).padStart(2, '0');
    const d = String(day).padStart(2, '0');
    return `${calViewYear}-${m}-${d}`;
  }

  function calIsToday(day: number): boolean {
    return calDayToStr(day) === todayStr();
  }

  function calIsSelected(day: number): boolean {
    return date === calDayToStr(day);
  }

  /**
   * Changing the block's date updates the TipTap node attribute and re-queries.
   * §31.2.3: user explicitly changes date; no auto-roll.
   */
  function calSelectDay(day: number) {
    const newDate = calDayToStr(day);
    closeCalendar();
    // Update TipTap node attr so the change persists in markdown.
    if (editor && getPos) {
      const pos = getPos();
      if (pos != null) {
        editor.chain().command(({ tr }: any) => {
          tr.setNodeMarkup(pos, undefined, { component: 'dueToday', date: newDate });
          return true;
        }).run();
      }
    }
    // Update local state and reload.
    date = newDate;
  }

  // ── External API (called by NodeView update) ──────────────────────────────

  export function setDate(newDate: string | null) {
    if (newDate !== date) {
      date = newDate;
    }
  }

  // ── Highlight (routing flash) ─────────────────────────────────────────────

  export function highlightRow(id: string) {
    highlightId = id;
    setTimeout(() => { highlightId = null; }, 1800);
  }
</script>

<!--
  §31.2 Due Today block — conformed to locked note-blocks design.
  Structure: .nb-block > .nb-card > .nb-head + .nb-list(.nb-row*) / .nb-empty
-->

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="nb-block" bind:this={blockEl}>
  <div class="nb-card">

    <!-- ── Card header ───────────────────────────────────────────────────── -->
    <div class="nb-head">
      <span class="nb-head__ico" aria-hidden="true">
        <!-- Calendar check icon — represents "Due Today" block -->
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none"
          stroke="currentColor" stroke-width="1.7"
          stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="4.5" width="18" height="16" rx="2"/>
          <path d="M3 9h18M8 2.5v4M16 2.5v4M9 14l2 2 4-4"/>
        </svg>
      </span>
      <span class="nb-head__txt">
        <span class="nb-head__title">Due Today</span>
        {#if date}
          <span class="nb-head__sub">{items.length} item{items.length !== 1 ? 's' : ''}</span>
        {/if}
      </span>
      <span class="nb-head__spacer"></span>

      <!-- Block date chip: shows block's date + opens date picker -->
      {#if date}
        <button
          class="nb-datechip"
          class:is-open={calOpen}
          onclick={openCalendar}
          title="Change block date"
          aria-label="Block date: {formatDate(date)}. Click to change."
        >
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
            stroke="currentColor" stroke-width="1.7"
            stroke-linecap="round" stroke-linejoin="round"
            aria-hidden="true">
            <rect x="3" y="4.5" width="18" height="16" rx="2"/>
            <path d="M3 9h18M8 2.5v4M16 2.5v4"/>
          </svg>
          {formatDate(date)}
        </button>
      {:else}
        <!-- Null date: date chip is absent; error state rendered below -->
        <span class="nb-datechip nb-datechip--error" aria-label="No date set">
          No date
        </span>
      {/if}
    </div>

    <!-- ── Content ─────────────────────────────────────────────────────── -->

    {#if !date}
      <!-- §31.2.3: null date → error state, never fall back to today -->
      <div class="nb-state-msg nb-state-msg--error">
        Due Today block has no date. Please delete and re-insert this block.
      </div>

    {:else if loading}
      <div class="nb-state-msg">Loading…</div>

    {:else if error}
      <div class="nb-state-msg nb-state-msg--error">{error}</div>

    {:else if items.length === 0}
      <!-- §31.2 Empty state: "Nothing was due on {date}" per .nb-empty in CSS spec -->
      <div class="nb-empty">
        <span class="nb-empty__art">
          <span class="nb-empty__ring"></span>
          <span class="nb-empty__ico">
            <svg width="28" height="28" viewBox="0 0 24 24" fill="none"
              stroke="currentColor" stroke-width="1.5"
              stroke-linecap="round" stroke-linejoin="round"
              aria-hidden="true">
              <rect x="3" y="4.5" width="18" height="16" rx="2"/>
              <path d="M3 9h18M8 2.5v4M16 2.5v4M9 14l2 2 4-4"/>
            </svg>
          </span>
        </span>
        <p class="nb-empty__title">Nothing was due on {formatDate(date)}</p>
        <p class="nb-empty__sub">No tasks or subtasks were due on this date.</p>
      </div>

    {:else}
      <!-- ── Item list ──────────────────────────────────────────────────── -->
      <div class="nb-list">
        {#each items as item (item.id)}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <div
            class="nb-row"
            class:is-done={item.done}
            class:is-routed={highlightId === item.id}
          >
            <!-- Checkbox — own hit area, §31.2.4 check-off logic -->
            <button
              class="nb-check"
              class:is-done={item.done}
              onclick={(e) => { e.stopPropagation(); handleToggle(item); }}
              aria-label={item.done ? 'Mark incomplete' : 'Mark complete'}
            >
              {#if item.done}
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none"
                  stroke="currentColor" stroke-width="1.7"
                  stroke-linecap="round" stroke-linejoin="round"
                  aria-hidden="true">
                  <path d="M4 12.5 9 17l11-11"/>
                </svg>
              {/if}
            </button>

            <!-- Body: clickable → navigate to source. NEVER toggles done. -->
            <span
              class="nb-rowbody"
              role="button"
              tabindex="0"
              onclick={() => handleRowClick(item)}
              onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') handleRowClick(item); }}
            >
              <span class="nb-rowtop">
                <span class="nb-rowtitle">{item.title}</span>

                <!-- §31.2.7: Subtask tag -->
                {#if item.kind === 'subtask'}
                  <span class="nb-subtag">
                    <!-- Note icon on subtask tag -->
                    <svg width="10" height="10" viewBox="0 0 24 24" fill="none"
                      stroke="currentColor" stroke-width="1.7"
                      stroke-linecap="round" stroke-linejoin="round"
                      aria-hidden="true">
                      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
                      <path d="M14 2v6h6M10 13h4M10 17h4M10 9h1"/>
                    </svg>
                    subtask
                  </span>
                {/if}
              </span>

              <!-- §31.2.7 Metadata line -->
              {#if item.kind === 'task' && item.project_name}
                <!-- Task: "in {Project}" — no icons -->
                <span class="nb-ctx">
                  <span class="nb-ctx__seg">
                    <span class="nb-ctx__t">in <b>{item.project_name}</b></span>
                  </span>
                </span>
              {:else if item.kind === 'subtask'}
                <!-- Subtask: "{Parent note} · {Project}" — note icon on parent ref only -->
                <span class="nb-ctx">
                  {#if item.parent_note_title}
                    <span class="nb-ctx__seg">
                      <!-- Note icon on parent-note ref only (§31.2.7) -->
                      <svg width="11" height="11" viewBox="0 0 24 24" fill="none"
                        stroke="currentColor" stroke-width="1.7"
                        stroke-linecap="round" stroke-linejoin="round"
                        aria-hidden="true">
                        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
                        <path d="M14 2v6h6"/>
                      </svg>
                      <span class="nb-ctx__t">{item.parent_note_title}</span>
                    </span>
                  {/if}
                  {#if item.parent_note_title && item.project_name}
                    <!-- Middot separator (§31.2.7) -->
                    <span class="nb-ctx__sep" aria-hidden="true">·</span>
                  {/if}
                  {#if item.project_name}
                    <span class="nb-ctx__seg">
                      <span class="nb-ctx__t">in <b>{item.project_name}</b></span>
                    </span>
                  {/if}
                </span>
              {/if}
            </span>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <!-- ── Shared dark calendar popover ──────────────────────────────────── -->
  {#if calOpen}
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div
      class="nb-cal-backdrop"
      onclick={closeCalendar}
      aria-hidden="true"
    ></div>
    <div
      class="nb-pop"
      style="top: {calPopTop}px; right: {calPopRight}px;"
      role="dialog"
      aria-label="Date picker"
    >
      <div class="nb-cal">
        <!-- Calendar header -->
        <div class="nb-cal__head">
          <button class="nb-cal__nav" onclick={calPrevMonth} aria-label="Previous month">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none"
              stroke="currentColor" stroke-width="1.7"
              stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M15 6l-6 6 6 6"/>
            </svg>
          </button>
          <span class="nb-cal__month">{calMonthLabel}</span>
          <button class="nb-cal__nav" onclick={calNextMonth} aria-label="Next month">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none"
              stroke="currentColor" stroke-width="1.7"
              stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M9 6l6 6-6 6"/>
            </svg>
          </button>
        </div>

        <!-- Day-of-week headers -->
        <div class="nb-cal__grid">
          {#each ['S','M','T','W','T','F','S'] as dow, i (i)}
            <span class="nb-cal__dow">{dow}</span>
          {/each}

          <!-- Leading days (prev month, muted) -->
          {#each calLeadDays as d, i (i)}
            <button class="nb-cal__day is-muted" aria-hidden="true" tabindex="-1">{d}</button>
          {/each}

          <!-- Days of current month -->
          {#each calDays as d (d)}
            <button
              class="nb-cal__day"
              class:is-today={calIsToday(d) && !calIsSelected(d)}
              class:is-selected={calIsSelected(d)}
              onclick={() => calSelectDay(d)}
              aria-label="{CAL_MONTHS[calViewMonth]} {d}, {calViewYear}"
              aria-pressed={calIsSelected(d)}
            >{d}</button>
          {/each}

          <!-- Trailing days (next month, muted) -->
          {#each calTrailDays as d, i (i)}
            <button class="nb-cal__day is-muted" aria-hidden="true" tabindex="-1">{d}</button>
          {/each}
        </div>

        <!-- Calendar footer -->
        <div class="nb-cal__foot">
          <span class="nb-cal__spacer"></span>
          <button class="nb-cal__done" onclick={closeCalendar}>Done</button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  /* ==========================================================================
     DueTodayNodeView — §31.2 design conformance
     CSS mirrors note-blocks.css exactly. Tokens from global app.css.
     ========================================================================== */

  /* ── Block shell ─────────────────────────────────────────────────────────── */
  .nb-block {
    position: relative;
    margin: 8px 0;
  }

  /* Suppress global ::selection teal wash within the block */
  .nb-block *::selection {
    background: transparent;
  }

  /* ── Card ────────────────────────────────────────────────────────────────── */
  .nb-card {
    background: #161618;
    border: 1px solid var(--border);
    border-radius: 12px;
    overflow: hidden;
    transition: border-color var(--dur-fast) var(--ease);
  }

  .nb-block:hover .nb-card {
    border-color: var(--border-strong);
  }

  /* ── Card header ─────────────────────────────────────────────────────────── */
  .nb-head {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 13px 14px;
    border-bottom: 1px solid var(--border-faint);
  }

  .nb-head__ico {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    flex-shrink: 0;
    background: var(--accent-soft);
    color: var(--accent);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .nb-head__txt {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .nb-head__title {
    font-size: 14px;
    font-weight: 600;
    color: var(--fg-1);
    letter-spacing: -0.01em;
  }

  .nb-head__sub {
    font-size: 12px;
    color: var(--fg-3);
    margin-top: 1px;
  }

  .nb-head__spacer {
    flex: 1;
  }

  /* ── Block date chip ─────────────────────────────────────────────────────── */
  .nb-datechip {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 5px 10px;
    border-radius: var(--radius-pill);
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--fg-1);
    font-size: 12.5px;
    font-weight: 500;
    cursor: pointer;
    font-variant-numeric: tabular-nums;
    transition:
      background var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
  }

  .nb-datechip:hover {
    background: var(--surface-3);
    border-color: var(--border-strong);
  }

  .nb-datechip svg {
    color: var(--fg-3);
  }

  .nb-datechip.is-open {
    border-color: var(--accent);
    box-shadow: var(--shadow-input-focus);
  }

  .nb-datechip--error {
    cursor: default;
    color: var(--status-danger);
    border-color: var(--status-danger);
    opacity: 0.7;
  }

  /* ── State messages ──────────────────────────────────────────────────────── */
  .nb-state-msg {
    padding: 18px 16px;
    font-size: 13px;
    color: var(--fg-4);
    text-align: center;
  }

  .nb-state-msg--error {
    color: var(--status-danger);
  }

  /* ── List ────────────────────────────────────────────────────────────────── */
  .nb-list {
    padding: 5px;
    display: flex;
    flex-direction: column;
  }

  /* ── Row ─────────────────────────────────────────────────────────────────── */
  .nb-row {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: 11px;
    padding: 11px 12px;
    border-radius: 8px;
    transition: background var(--dur-fast) var(--ease);
  }

  .nb-row + .nb-row {
    box-shadow: 0 -1px 0 var(--border-faint);
  }

  .nb-row:hover {
    background: var(--overlay-hover);
  }

  .nb-row:hover,
  .nb-row:hover + .nb-row {
    box-shadow: none;
  }

  /* Done rows recede + strike */
  .nb-row.is-done .nb-rowtitle {
    color: var(--fg-4);
    text-decoration: line-through;
    font-weight: 400;
  }

  .nb-row.is-done .nb-ctx {
    color: var(--fg-4);
  }

  .nb-row.is-done .nb-ctx__seg,
  .nb-row.is-done .nb-ctx__seg b,
  .nb-row.is-done .nb-ctx__seg svg {
    color: var(--fg-4);
  }

  /* Routed highlight: cyan ring + fading flash */
  .nb-row.is-routed {
    background: var(--accent-soft);
    box-shadow: 0 0 0 1px var(--accent), 0 0 0 4px var(--accent-soft);
    border-radius: 8px;
    animation: nb-highlight-fade 1.8s ease-out forwards;
  }

  @keyframes nb-highlight-fade {
    0%   { box-shadow: 0 0 0 1px var(--accent), 0 0 0 4px var(--accent-soft); background: var(--accent-soft); }
    70%  { box-shadow: 0 0 0 1px rgba(0,228,228,0.3); background: rgba(0,228,228,0.04); }
    100% { box-shadow: none; background: transparent; }
  }

  /* ── Checkbox ────────────────────────────────────────────────────────────── */
  .nb-check {
    flex-shrink: 0;
    width: 20px;
    height: 20px;
    margin-top: 1px;
    border-radius: var(--radius-sm);
    border: 1.5px solid var(--fg-4);
    background: transparent;
    cursor: pointer;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: transparent;
    transition:
      border-color var(--dur-fast) var(--ease),
      background   var(--dur-fast) var(--ease),
      color        var(--dur-fast) var(--ease);
  }

  .nb-check:hover {
    border-color: var(--accent);
  }

  .nb-check.is-done {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }

  /* ── Row body ────────────────────────────────────────────────────────────── */
  .nb-rowbody {
    flex: 1;
    min-width: 0;
    cursor: pointer;
    outline: none;
  }

  .nb-rowtop {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .nb-rowtitle {
    flex: 1;
    min-width: 0;
    font-size: 14px;
    font-weight: 500;
    color: var(--fg-1);
    line-height: 1.4;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .nb-rowbody:hover .nb-rowtitle {
    color: var(--accent);
  }

  /* ── Subtask tag ─────────────────────────────────────────────────────────── */
  .nb-subtag {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 0.02em;
    color: var(--fg-3);
    background: var(--surface-2);
    padding: 2px 7px;
    border-radius: 5px;
    line-height: 1.4;
  }

  .nb-subtag svg {
    color: var(--fg-4);
  }

  /* ── Metadata (context) line ─────────────────────────────────────────────── */
  .nb-ctx {
    display: flex;
    align-items: center;
    gap: 6px 8px;
    margin-top: 3px;
    min-width: 0;
    flex-wrap: wrap;
  }

  .nb-ctx__seg {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    font-size: 12px;
    color: var(--fg-3);
    max-width: 100%;
  }

  .nb-ctx__seg svg {
    color: var(--fg-4);
    flex-shrink: 0;
  }

  .nb-ctx__t {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .nb-ctx__seg b {
    color: var(--fg-2);
    font-weight: 500;
  }

  /* Middot separator between parent-note and project */
  .nb-ctx__sep {
    color: var(--fg-4);
    font-size: 12px;
    flex-shrink: 0;
  }

  /* ── Empty state ─────────────────────────────────────────────────────────── */
  .nb-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    padding: 30px 26px 32px;
  }

  .nb-empty__art {
    width: 56px;
    height: 56px;
    margin-bottom: 12px;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .nb-empty__ring {
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background: radial-gradient(circle at 50% 42%, rgba(0,228,228,0.10), transparent 68%);
  }

  .nb-empty__ico {
    color: var(--fg-4);
    position: relative;
  }

  .nb-empty__title {
    font-size: 13.5px;
    font-weight: 600;
    color: var(--fg-2);
    margin: 0 0 4px;
  }

  .nb-empty__sub {
    font-size: 12.5px;
    color: var(--fg-4);
    line-height: 1.5;
    margin: 0;
    max-width: 240px;
  }

  /* ── Calendar popover ────────────────────────────────────────────────────── */
  .nb-cal-backdrop {
    position: fixed;
    inset: 0;
    z-index: 29;
    background: transparent;
    border: none;
    cursor: default;
  }

  .nb-pop {
    position: absolute;
    z-index: 30;
  }

  .nb-cal {
    width: 268px;
    padding: 12px;
    box-sizing: border-box;
    background: #1c1c20;
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    box-shadow: var(--shadow-pop);
  }

  .nb-cal__head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
  }

  .nb-cal__month {
    flex: 1;
    font-size: 13.5px;
    font-weight: 600;
    color: var(--fg-1);
    text-align: center;
  }

  .nb-cal__nav {
    width: 26px;
    height: 26px;
    border-radius: var(--radius-sm);
    border: 0;
    background: transparent;
    color: var(--fg-3);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease);
  }

  .nb-cal__nav:hover {
    background: var(--overlay-hover);
    color: var(--fg-1);
  }

  .nb-cal__grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 2px;
  }

  .nb-cal__dow {
    font-size: 10.5px;
    font-weight: 600;
    color: var(--fg-4);
    text-align: center;
    padding: 4px 0;
    letter-spacing: 0.02em;
  }

  .nb-cal__day {
    aspect-ratio: 1;
    border: 0;
    background: transparent;
    border-radius: var(--radius-sm);
    color: var(--fg-2);
    font-family: var(--font-sans);
    font-size: 12.5px;
    cursor: pointer;
    font-variant-numeric: tabular-nums;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background var(--dur-fast) var(--ease), color var(--dur-fast) var(--ease);
  }

  .nb-cal__day:hover {
    background: var(--overlay-hover);
    color: var(--fg-1);
  }

  .nb-cal__day.is-muted {
    color: var(--fg-4);
    cursor: default;
  }

  .nb-cal__day.is-muted:hover {
    background: transparent;
    color: var(--fg-4);
  }

  .nb-cal__day.is-today {
    color: var(--status-warning);
    font-weight: 600;
    box-shadow: inset 0 0 0 1px rgba(245,158,11,0.4);
  }

  .nb-cal__day.is-selected {
    background: var(--accent);
    color: var(--fg-on-accent);
    font-weight: 600;
  }

  .nb-cal__day.is-selected:hover {
    background: var(--accent-hover);
  }

  .nb-cal__foot {
    display: flex;
    align-items: center;
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid var(--border-faint);
  }

  .nb-cal__spacer {
    flex: 1;
  }

  .nb-cal__done {
    background: var(--surface-2);
    border: 1px solid var(--border);
    cursor: pointer;
    color: var(--fg-1);
    font-family: var(--font-sans);
    font-size: 12.5px;
    font-weight: 500;
    padding: 5px 12px;
    border-radius: var(--radius-sm);
    transition: background var(--dur-fast) var(--ease);
  }

  .nb-cal__done:hover {
    background: var(--surface-3);
  }
</style>
