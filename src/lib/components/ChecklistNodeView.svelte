<script lang="ts">
  /**
   * ChecklistNodeView.svelte — Svelte NodeView for the Checklist TipTap extension.
   *
   * Renders the parent note's subtasks (from the `subtasks` table via Tauri).
   * Data source: `$activeNote?.meta?.id` — mirrors the pattern in AttachmentNodeView.svelte:141.
   * Using activeNote (not canvasState.context.noteId) covers all flows: canvas navigation,
   * new note creation, and All Tasks panel inserts where canvasState.context.noteId is null.
   *
   * §31.3 Advanced Checklist UI (PR C).
   * §24.2 Visual conformance to locked note-blocks design (design conformance PR).
   * RISK 6 null-guard: if noteId is null/undefined, renders an explicit error state.
   *
   * Visual spec: ~/Shared/ExampleApp/docs/design/note-blocks/
   *   - note-blocks.css       → pixel-perfect CSS reference
   *   - note-blocks-screens.jsx → reference React frames
   *   - colors_and_type.css   → design system tokens
   * Locked decisions:
   *   - Dates ALWAYS absolute (e.g. "Jul 21"). Never relative.
   *   - Urgency tokens: amber = due today, red = overdue.
   *   - Checkbox tap ≠ body tap (separate hit areas).
   *   - Shared date control = full calendar popover (no quick chips, no OS picker).
   */

  import { onMount } from 'svelte';
  import { activeNote } from '$lib/stores/app';
  import {
    listChecklistItems,
    createChecklistItem,
    updateChecklistItem,
    deleteChecklistItem,
    type ChecklistItem,
  } from '$lib/api';

  // ── State ─────────────────────────────────────────────────────────────────

  let items = $state<ChecklistItem[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let newItemText = $state('');
  let addingItem = $state(false);

  // Per-item edit state
  let editingId = $state<string | null>(null);
  let editingText = $state('');

  // Drag state
  let dragSourceId = $state<string | null>(null);
  let dragOverId = $state<string | null>(null);

  // Highlight (notification-routed) — triggered externally via highlightItem()
  let highlightId = $state<string | null>(null);

  // Block element ref for calendar popover positioning
  let blockEl = $state<HTMLElement | null>(null);

  // ── Calendar state ────────────────────────────────────────────────────────

  const CAL_MONTHS = [
    'January','February','March','April','May','June',
    'July','August','September','October','November','December',
  ];

  let calOpenId   = $state<string | null>(null);
  let calViewYear = $state(new Date().getFullYear());
  let calViewMonth = $state(new Date().getMonth()); // 0-indexed
  let calPopTop   = $state(60);
  let calPopRight = $state(0);

  let calMonthLabel = $derived(CAL_MONTHS[calViewMonth] + ' ' + calViewYear);

  // Days from the previous month needed to fill the first row
  let calLeadDays = $derived((() => {
    const firstDow = new Date(calViewYear, calViewMonth, 1).getDay(); // 0=Sun
    if (firstDow === 0) return [] as number[];
    const prevMonthDays = new Date(calViewYear, calViewMonth, 0).getDate();
    return Array.from({ length: firstDow }, (_, i) => prevMonthDays - firstDow + 1 + i);
  })());

  // Days of the current month
  let calDays = $derived((() => {
    const count = new Date(calViewYear, calViewMonth + 1, 0).getDate();
    return Array.from({ length: count }, (_, i) => i + 1);
  })());

  // Trailing days from next month to fill the last row
  let calTrailDays = $derived((() => {
    const total = calLeadDays.length + calDays.length;
    const needed = Math.ceil(total / 7) * 7 - total;
    return Array.from({ length: needed }, (_, i) => i + 1);
  })());

  // ── Derived ───────────────────────────────────────────────────────────────

  // Reactively read noteId from activeNote — same source as AttachmentNodeView.svelte:141.
  let noteId = $derived($activeNote?.meta?.id ?? null);

  // Count of completed items for progress bar
  let doneCount = $derived(items.filter(i => i.done).length);

  // ── Lifecycle ─────────────────────────────────────────────────────────────

  onMount(() => {
    loadItems();

    // Item 2 (Reed #2116 audit): wire pane-highlight-subtask → highlightItem
    // Connects notifications panel's window.dispatchEvent(...) to our highlightItem().
    const handleHighlight = (e: CustomEvent) => {
      if (e.detail?.subtaskId) highlightItem(e.detail.subtaskId);
    };
    window.addEventListener('pane-highlight-subtask', handleHighlight as EventListener);

    return () => {
      window.removeEventListener('pane-highlight-subtask', handleHighlight as EventListener);
    };
  });

  // Reload when noteId changes (e.g. user navigates to a different note without
  // fully re-mounting the editor).
  $effect(() => {
    const id = noteId; // track
    void id;
    loadItems();
  });

  // ── Data helpers ──────────────────────────────────────────────────────────

  async function loadItems() {
    if (!noteId) {
      loading = false;
      items = [];
      return;
    }
    try {
      loading = true;
      error = null;
      items = await listChecklistItems(noteId);
    } catch (e: any) {
      error = e?.message ?? 'Failed to load checklist items';
    } finally {
      loading = false;
    }
  }

  // ── Date helpers ──────────────────────────────────────────────────────────

  function todayStr(): string {
    return new Date().toISOString().slice(0, 10);
  }

  /**
   * Returns the urgency state for a checklist item's due chip.
   * 'today' → amber; 'overdue' → red; 'later' → neutral; 'done' → muted/strike;
   * 'none' → no date (hover reveals Add date affordance).
   * Locked design decision: absolute dates, no relative copy.
   */
  function dueKind(item: ChecklistItem): 'today' | 'overdue' | 'later' | 'done' | 'none' {
    if (!item.due_date) return 'none';
    if (item.done) return 'done';
    const today = todayStr();
    if (item.due_date < today) return 'overdue';
    if (item.due_date === today) return 'today';
    return 'later';
  }

  /**
   * Formats "2026-08-01" → "Aug 1".
   * LOCKED: always absolute; never "Today", "Tomorrow", or "3 days" — these lie once the
   * note ages. See locked-design README.
   */
  function formatDueDate(dateStr: string): string {
    const d = new Date(dateStr + 'T00:00:00');
    return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
  }

  // ── Interaction handlers ──────────────────────────────────────────────────

  async function toggleDone(item: ChecklistItem) {
    if (!noteId) return;
    try {
      const updated = await updateChecklistItem(item.id, { done: !item.done });
      items = items.map(i => (i.id === item.id ? updated : i));
    } catch (e: any) {
      console.error('[Checklist] toggleDone failed:', e);
    }
  }

  function startEdit(item: ChecklistItem) {
    editingId = item.id;
    editingText = item.text;
  }

  async function commitEdit(item: ChecklistItem) {
    if (!noteId || editingId !== item.id) return;
    const trimmed = editingText.trim();
    editingId = null;
    if (!trimmed || trimmed === item.text) return;
    try {
      const updated = await updateChecklistItem(item.id, { text: trimmed });
      items = items.map(i => (i.id === item.id ? updated : i));
    } catch (e: any) {
      console.error('[Checklist] commitEdit failed:', e);
    }
  }

  async function updateDueDate(item: ChecklistItem, value: string) {
    if (!noteId) return;
    const dueDate = value || null;
    try {
      const updated = await updateChecklistItem(item.id, { dueDate });
      items = items.map(i => (i.id === item.id ? updated : i));
    } catch (e: any) {
      console.error('[Checklist] updateDueDate failed:', e);
    }
  }

  async function deleteItem(id: string) {
    if (!noteId) return;
    try {
      await deleteChecklistItem(id);
      items = items.filter(i => i.id !== id);
    } catch (e: any) {
      console.error('[Checklist] deleteItem failed:', e);
    }
  }

  async function addItem() {
    const trimmed = newItemText.trim();
    if (!trimmed || !noteId) return;
    addingItem = true;
    try {
      const maxOrder = items.length > 0 ? Math.max(...items.map(i => i.sort_order)) : 0;
      const created = await createChecklistItem(noteId, trimmed, undefined, maxOrder + 1);
      items = [...items, created];
      newItemText = '';
    } catch (e: any) {
      console.error('[Checklist] addItem failed:', e);
    } finally {
      addingItem = false;
    }
  }

  function handleAddKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      addItem();
    }
  }

  function handleEditKeydown(e: KeyboardEvent, item: ChecklistItem) {
    if (e.key === 'Enter') {
      e.preventDefault();
      commitEdit(item);
    }
    if (e.key === 'Escape') {
      editingId = null;
    }
  }

  // ── Drag-reorder ──────────────────────────────────────────────────────────

  function onDragStart(e: DragEvent, id: string) {
    dragSourceId = id;
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
  }

  function onDragOver(e: DragEvent, id: string) {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
    dragOverId = id;
  }

  function onDragLeave() {
    dragOverId = null;
  }

  async function onDrop(e: DragEvent, targetId: string) {
    e.preventDefault();
    dragOverId = null;
    if (!dragSourceId || dragSourceId === targetId) {
      dragSourceId = null;
      return;
    }
    const sourceId = dragSourceId;
    dragSourceId = null;

    // Reorder locally
    const sourceIdx = items.findIndex(i => i.id === sourceId);
    const targetIdx = items.findIndex(i => i.id === targetId);
    if (sourceIdx === -1 || targetIdx === -1) return;

    const reordered = [...items];
    const [moved] = reordered.splice(sourceIdx, 1);
    reordered.splice(targetIdx, 0, moved);

    // Assign new sort_orders
    const withOrders = reordered.map((item, idx) => ({ ...item, sort_order: idx + 1 }));
    items = withOrders;

    // Persist new sort_orders
    for (const item of withOrders) {
      try {
        await updateChecklistItem(item.id, { sortOrder: item.sort_order });
      } catch (e: any) {
        console.error('[Checklist] onDrop persist failed:', e);
      }
    }
  }

  function onDragEnd() {
    dragSourceId = null;
    dragOverId = null;
  }

  // ── Highlight (notification routing) ─────────────────────────────────────

  export function highlightItem(id: string) {
    highlightId = id;
    setTimeout(() => {
      highlightId = null;
    }, 1800);
  }

  // ── Calendar helpers ──────────────────────────────────────────────────────

  /** Convert a day number in the current calendar view to an ISO date string. */
  function calDayToStr(day: number): string {
    const m = String(calViewMonth + 1).padStart(2, '0');
    const d = String(day).padStart(2, '0');
    return `${calViewYear}-${m}-${d}`;
  }

  function calIsToday(day: number): boolean {
    return calDayToStr(day) === todayStr();
  }

  function calIsSelected(day: number): boolean {
    if (!calOpenId) return false;
    const item = items.find(i => i.id === calOpenId);
    return item?.due_date === calDayToStr(day);
  }

  /**
   * Open the calendar popover for a checklist item.
   * Positions the popover below the trigger element, right-aligned to the card edge.
   */
  function openCalendar(item: ChecklistItem, event: MouseEvent) {
    calOpenId = item.id;
    // Navigate calendar to item's current due date (or today if unset)
    const d = item.due_date ? new Date(item.due_date + 'T00:00:00') : new Date();
    calViewYear  = d.getFullYear();
    calViewMonth = d.getMonth();
    // Compute position relative to the .nb-block container
    if (blockEl && event.currentTarget) {
      const triggerRect = (event.currentTarget as HTMLElement).getBoundingClientRect();
      const blockRect   = blockEl.getBoundingClientRect();
      calPopTop   = triggerRect.bottom - blockRect.top + 6;
      calPopRight = blockRect.right - triggerRect.right;
    }
  }

  function closeCalendar() {
    calOpenId = null;
  }

  function calPrevMonth() {
    if (calViewMonth === 0) { calViewMonth = 11; calViewYear--; }
    else { calViewMonth--; }
  }

  function calNextMonth() {
    if (calViewMonth === 11) { calViewMonth = 0; calViewYear++; }
    else { calViewMonth++; }
  }

  async function calSelectDay(day: number) {
    if (!calOpenId) return;
    const item = items.find(i => i.id === calOpenId);
    if (!item) return;
    await updateDueDate(item, calDayToStr(day));
    closeCalendar();
  }

  async function calClearDate() {
    if (!calOpenId) return;
    const item = items.find(i => i.id === calOpenId);
    if (!item) return;
    await updateDueDate(item, '');
    closeCalendar();
  }
</script>

<!--
  §24.2 Checklist block — conformed to locked note-blocks design.
  Structure mirrors Checklist() in note-blocks-screens.jsx exactly:
    .nb-block > .nb-card > .nb-head + .nb-list(.nb-row*) + .nb-foot
  Plus a shared dark calendar popover (.nb-cal) on .nb-pop.
-->

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="nb-block" bind:this={blockEl}>
  <div class="nb-card">

    <!-- ── Card header ───────────────────────────────────────────────────── -->
    <div class="nb-head">
      <span class="nb-head__ico" aria-hidden="true">
        <!-- List icon: matches I.list in note-blocks-screens.jsx -->
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none"
          stroke="currentColor" stroke-width="1.7"
          stroke-linecap="round" stroke-linejoin="round">
          <path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/>
        </svg>
      </span>
      <span class="nb-head__txt">
        <span class="nb-head__title">Checklist</span>
        <span class="nb-head__sub">{items.length} items</span>
      </span>
    </div>

    <!-- ── Content: loading / error / null-noteId / items ───────────────── -->

    {#if !noteId}
      <!-- RISK 6 null-guard: never attempt writes without a note ID -->
      <div class="nb-state-msg">Checklist unavailable — no note context</div>

    {:else if loading}
      <div class="nb-state-msg">Loading…</div>

    {:else if error}
      <div class="nb-state-msg nb-state-msg--error">{error}</div>

    {:else}

      <!-- ── Item list ─────────────────────────────────────────────────── -->
      <div class="nb-list">
        {#each items as item (item.id)}
          {@const kind = dueKind(item)}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <div
            class="nb-row"
            class:is-today={kind === 'today'}
            class:is-overdue={kind === 'overdue'}
            class:is-done={item.done}
            class:is-routed={highlightId === item.id}
            class:is-editing={editingId === item.id}
            class:is-drag-over={dragOverId === item.id}
            draggable="true"
            ondragstart={(e) => onDragStart(e, item.id)}
            ondragover={(e) => onDragOver(e, item.id)}
            ondragleave={onDragLeave}
            ondrop={(e) => onDrop(e, item.id)}
            ondragend={onDragEnd}
          >
            <!-- Drag grip: zero-opacity at rest, fades in on row hover -->
            <span
              class="nb-grip"
              class:is-forced={dragSourceId === item.id}
              aria-hidden="true"
            >
              <!-- 6-dot grip: matches I.grip in note-blocks-screens.jsx -->
              <svg width="14" height="14" viewBox="0 0 24 24"
                fill="currentColor" stroke="none">
                <circle cx="9" cy="6" r="1.4"/><circle cx="15" cy="6" r="1.4"/>
                <circle cx="9" cy="12" r="1.4"/><circle cx="15" cy="12" r="1.4"/>
                <circle cx="9" cy="18" r="1.4"/><circle cx="15" cy="18" r="1.4"/>
              </svg>
            </span>

            <!-- Checkbox — own hit area, never opens edit or calendar -->
            <button
              class="nb-check"
              class:is-done={item.done}
              onclick={(e) => { e.stopPropagation(); toggleDone(item); }}
              aria-label={item.done ? 'Mark incomplete' : 'Mark complete'}
            >
              {#if item.done}
                <!-- Check tick: matches I.check in note-blocks-screens.jsx -->
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none"
                  stroke="currentColor" stroke-width="1.7"
                  stroke-linecap="round" stroke-linejoin="round"
                  aria-hidden="true">
                  <path d="M4 12.5 9 17l11-11"/>
                </svg>
              {/if}
            </button>

            <!-- Item text: inline edit or display -->
            {#if editingId === item.id}
              <!-- svelte-ignore a11y_autofocus -->
              <input
                class="nb-itemtext"
                bind:value={editingText}
                autofocus
                onblur={() => commitEdit(item)}
                onkeydown={(e) => handleEditKeydown(e, item)}
                onclick={(e) => e.stopPropagation()}
              />
            {:else}
              <!-- Body: clickable to enter edit. NEVER toggles done. -->
              <span
                class="nb-rowbody"
                role="button"
                tabindex="0"
                onclick={() => startEdit(item)}
                onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') startEdit(item); }}
              >
                <span class="nb-rowtop">
                  <span class="nb-rowtitle">{item.text}</span>
                </span>
              </span>
            {/if}

            <!-- Due chip or "Add date" ghost affordance -->
            {#if kind === 'none'}
              <!-- Dashed ghost: hidden at rest, revealed on row hover -->
              <button
                class="nb-adddate"
                onclick={(e) => { e.stopPropagation(); openCalendar(item, e); }}
                title="Set due date"
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                  stroke="currentColor" stroke-width="1.7"
                  stroke-linecap="round" stroke-linejoin="round"
                  aria-hidden="true">
                  <rect x="3" y="4.5" width="18" height="16" rx="2"/>
                  <path d="M3 9h18M8 2.5v4M16 2.5v4"/>
                </svg>
                Add date
              </button>
            {:else}
              <!-- Solid due chip: amber=today, red=overdue, neutral=later, muted=done -->
              <button
                class="nb-due"
                class:nb-due--today={kind === 'today'}
                class:nb-due--overdue={kind === 'overdue'}
                class:nb-due--later={kind === 'later'}
                class:nb-due--done={kind === 'done'}
                onclick={(e) => { e.stopPropagation(); openCalendar(item, e); }}
                title="Change due date"
              >
                {#if kind === 'today'}
                  <!-- Clock icon for due-today -->
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                    stroke="currentColor" stroke-width="1.7"
                    stroke-linecap="round" stroke-linejoin="round"
                    aria-hidden="true">
                    <circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>
                  </svg>
                {:else if kind === 'overdue'}
                  <!-- Triangle-warning icon for overdue -->
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                    stroke="currentColor" stroke-width="1.7"
                    stroke-linecap="round" stroke-linejoin="round"
                    aria-hidden="true">
                    <path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/>
                    <path d="M12 9v4M12 17h.01"/>
                  </svg>
                {:else}
                  <!-- Calendar icon for later / done -->
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                    stroke="currentColor" stroke-width="1.7"
                    stroke-linecap="round" stroke-linejoin="round"
                    aria-hidden="true">
                    <rect x="3" y="4.5" width="18" height="16" rx="2"/>
                    <path d="M3 9h18M8 2.5v4M16 2.5v4"/>
                  </svg>
                {/if}
                {item.due_date ? formatDueDate(item.due_date) : ''}
              </button>
            {/if}

            <!-- Delete: zero-width at rest, expands on row hover (due chip shifts left) -->
            <button
              class="nb-del"
              onclick={(e) => { e.stopPropagation(); deleteItem(item.id); }}
              aria-label="Delete item"
              title="Delete item"
            >
              <!-- Trash icon: matches I.trash in note-blocks-screens.jsx -->
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none"
                stroke="currentColor" stroke-width="1.7"
                stroke-linecap="round" stroke-linejoin="round"
                aria-hidden="true">
                <path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6M10 11v6M14 11v6"/>
              </svg>
            </button>
          </div>
        {/each}

        <!-- Add item row: persistent at bottom of list -->
        <div class="nb-add">
          <span class="nb-add__box" aria-hidden="true">
            <!-- Plus icon -->
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none"
              stroke="currentColor" stroke-width="1.7"
              stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 5v14M5 12h14"/>
            </svg>
          </span>
          <input
            class="nb-add__input"
            bind:value={newItemText}
            placeholder="Add item…"
            disabled={addingItem}
            onkeydown={handleAddKeydown}
            aria-label="New checklist item"
          />
        </div>
      </div>

      <!-- ── Footer: progress bar ────────────────────────────────────────── -->
      <div class="nb-foot">
        <span class="nb-progress">
          <span class="nb-progress__track">
            <span
              class="nb-progress__fill"
              style="width: {items.length > 0 ? (doneCount / items.length) * 100 : 0}%"
            ></span>
          </span>
          <span class="nb-progress__label">{doneCount} of {items.length} done</span>
        </span>
      </div>

    {/if}
  </div>

  <!-- ── Shared dark calendar popover ──────────────────────────────────── -->
  <!-- Full calendar, no quick chips. Same control for all date picking
       surfaces (§24.2 locked decision). "Clear date" + "Done" footer only. -->
  {#if calOpenId}
    <!-- Lightweight backdrop: click-away closes the popover -->
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
        <!-- Calendar header: month nav -->
        <div class="nb-cal__head">
          <button
            class="nb-cal__nav"
            onclick={calPrevMonth}
            aria-label="Previous month"
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none"
              stroke="currentColor" stroke-width="1.7"
              stroke-linecap="round" stroke-linejoin="round"
              aria-hidden="true">
              <path d="M15 6l-6 6 6 6"/>
            </svg>
          </button>
          <span class="nb-cal__month">{calMonthLabel}</span>
          <button
            class="nb-cal__nav"
            onclick={calNextMonth}
            aria-label="Next month"
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none"
              stroke="currentColor" stroke-width="1.7"
              stroke-linecap="round" stroke-linejoin="round"
              aria-hidden="true">
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

        <!-- Calendar footer: Clear date + Done -->
        <div class="nb-cal__foot">
          <button class="nb-cal__clear" onclick={calClearDate}>
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none"
              stroke="currentColor" stroke-width="1.7"
              stroke-linecap="round" stroke-linejoin="round"
              aria-hidden="true">
              <path d="M18 6 6 18M6 6l12 12"/>
            </svg>
            Clear date
          </button>
          <span class="nb-cal__spacer"></span>
          <button class="nb-cal__done" onclick={closeCalendar}>Done</button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  /* ==========================================================================
     ChecklistNodeView — §24.2 design conformance
     CSS mirrors note-blocks.css exactly (Pane locked design).
     Tokens consumed from global app.css / colors_and_type.css:
       --accent, --accent-soft, --border, --border-faint, --border-strong,
       --surface-2, --surface-3, --fg-1..--fg-4, --fg-on-accent,
       --overlay-hover, --overlay-press, --shadow-pop, --shadow-input-focus,
       --status-warning, --status-danger,
       --radius-sm, --radius-md, --radius-lg, --radius-pill,
       --dur-fast, --dur-norm, --ease
     ========================================================================== */

  /* ── Block shell ─────────────────────────────────────────────────────────── */

  .nb-block {
    position: relative;
    margin: 8px 0;
  }

  /* Suppress global ::selection teal wash (src/app.css:220) within the block.
     Fixes #2111: atom: true caused NodeSelection on insert → browser-native
     selection range applied rgba(0,228,228,0.35) fill over the whole component.
     selectable:false (Checklist.ts) prevents ProseMirror NodeSelection;
     this guard catches any residual browser ::selection from adjacent clicks. */
  .nb-block *::selection {
    background: transparent;
  }

  /* ── Card container (#161618 bg, --border, 12px radius) ─────────────────── */

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

  /* Icon tile: cyan-soft background, accent icon */
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

  /* ── State messages (loading / error / no-context) ──────────────────────── */

  .nb-state-msg {
    padding: 18px 16px;
    font-size: 13px;
    color: var(--fg-4);
    text-align: center;
  }

  .nb-state-msg--error {
    color: var(--status-danger);
  }

  /* ── List container ──────────────────────────────────────────────────────── */

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

  /* Hairline separator between rows */
  .nb-row + .nb-row {
    box-shadow: 0 -1px 0 var(--border-faint);
  }

  /* Remove separator when either adjacent row is hovered (avoids double-line) */
  .nb-row:hover,
  .nb-row:hover + .nb-row {
    box-shadow: none;
  }

  .nb-row:hover {
    background: var(--overlay-hover);
  }

  /* Drag-over: subtle cyan hint */
  .nb-row.is-drag-over {
    background: var(--accent-soft);
    box-shadow: none;
  }

  /* Urgency tint the checkbox border before the user reads the label */
  .nb-row.is-today .nb-check:not(.is-done) {
    border-color: var(--status-warning);
  }

  .nb-row.is-overdue .nb-check:not(.is-done) {
    border-color: var(--status-danger);
  }

  /* Done rows recede */
  .nb-row.is-done .nb-rowtitle {
    color: var(--fg-4);
    text-decoration: line-through;
    font-weight: 400;
  }

  /* Editing row: inset cyan ring */
  .nb-row.is-editing {
    background: var(--overlay-hover);
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  /* Routed highlight: cyan ring flash (fades via the animation below) */
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

  /* ── Drag grip ───────────────────────────────────────────────────────────── */

  .nb-grip {
    flex-shrink: 0;
    width: 16px;
    align-self: center;
    margin: 0 -4px 0 -4px;
    color: var(--fg-4);
    cursor: grab;
    display: flex;
    justify-content: center;
    opacity: 0;
    transition: opacity var(--dur-fast) var(--ease);
  }

  .nb-row:hover .nb-grip,
  .nb-grip.is-forced {
    opacity: 1;
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

  /* Done: cyan-soft fill + accent tick */
  .nb-check.is-done {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }

  /* ── Row body (clickable text area → edit) ───────────────────────────────── */

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

  /* ── Inline edit input ───────────────────────────────────────────────────── */

  .nb-itemtext {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: 0;
    outline: 0;
    padding: 0;
    font-family: var(--font-sans);
    font-size: 14px;
    font-weight: 500;
    color: var(--fg-1);
    line-height: 1.4;
  }

  .nb-itemtext::placeholder {
    color: var(--fg-4);
  }

  /* ── Due chip ────────────────────────────────────────────────────────────── */

  .nb-due {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 9px;
    border-radius: var(--radius-pill);
    font-size: 12px;
    font-weight: 500;
    font-variant-numeric: tabular-nums;
    border: 1px solid transparent;
    cursor: pointer;
    white-space: nowrap;
    transition:
      background    var(--dur-fast) var(--ease),
      border-color  var(--dur-fast) var(--ease);
  }

  .nb-due svg {
    width: 12px;
    height: 12px;
    flex-shrink: 0;
  }

  /* Neutral: future date */
  .nb-due--later {
    color: var(--fg-2);
    background: var(--surface-2);
    border-color: var(--border);
  }

  /* Amber: due today */
  .nb-due--today {
    color: var(--status-warning);
    background: rgba(245, 158, 11, 0.12);
    border-color: rgba(245, 158, 11, 0.28);
  }

  /* Red: overdue */
  .nb-due--overdue {
    color: var(--status-danger);
    background: rgba(239, 68, 68, 0.10);
    border-color: rgba(239, 68, 68, 0.30);
  }

  /* Muted + strikethrough: done */
  .nb-due--done {
    color: var(--fg-4);
    background: var(--surface-2);
    border-color: transparent;
    text-decoration: line-through;
  }

  /* Hover intensify */
  .nb-due:hover                { border-color: var(--border-strong); }
  .nb-due--today:hover         { border-color: var(--status-warning); }
  .nb-due--overdue:hover       { border-color: var(--status-danger); }

  /* ── "Add date" ghost affordance ─────────────────────────────────────────── */

  .nb-adddate {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 9px;
    border-radius: var(--radius-pill);
    font-size: 12px;
    font-weight: 500;
    color: var(--fg-4);
    background: transparent;
    border: 1px dashed var(--border-strong);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity      var(--dur-fast) var(--ease),
      color        var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
  }

  /* Reveal on row hover */
  .nb-row:hover .nb-adddate,
  .nb-adddate.is-forced {
    opacity: 1;
  }

  .nb-adddate:hover {
    color: var(--accent);
    border-color: var(--accent);
  }

  /* ── Delete button ───────────────────────────────────────────────────────── */
  /*
   * Collapsed to zero width at rest (margin-left cancels the flex gap) so the
   * due chip sits flush right. On row hover it expands and the chip shifts left
   * to make room — the same reveal pattern as the drawer's hover-dismiss ×.
   * Neutral color at rest; turns status-danger on hover.
   */

  .nb-del {
    flex-shrink: 0;
    width: 0;
    height: 26px;
    margin-left: -11px;
    padding: 0;
    overflow: hidden;
    border-radius: var(--radius-sm);
    background: transparent;
    border: 0;
    cursor: pointer;
    color: var(--fg-4);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transition:
      width        var(--dur-norm) var(--ease),
      margin-left  var(--dur-norm) var(--ease),
      opacity      var(--dur-fast) var(--ease),
      background   var(--dur-fast) var(--ease),
      color        var(--dur-fast) var(--ease);
  }

  .nb-row:hover .nb-del,
  .nb-del.is-forced {
    width: 26px;
    margin-left: 0;
    opacity: 1;
  }

  .nb-del:hover {
    background: var(--overlay-press);
    color: var(--status-danger);
  }

  /* ── Add item row ────────────────────────────────────────────────────────── */

  .nb-add {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 11px 12px;
    border-radius: 8px;
    color: var(--fg-4);
    transition:
      background var(--dur-fast) var(--ease),
      color      var(--dur-fast) var(--ease);
  }

  .nb-add:hover,
  .nb-add:focus-within {
    background: var(--overlay-hover);
    color: var(--fg-3);
  }

  /* Dashed box mirrors an empty checkbox affordance */
  .nb-add__box {
    width: 20px;
    height: 20px;
    border-radius: var(--radius-sm);
    border: 1.5px dashed var(--border-strong);
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--fg-4);
  }

  .nb-add__input {
    flex: 1;
    background: transparent;
    border: 0;
    outline: 0;
    padding: 0;
    font-family: var(--font-sans);
    font-size: 14px;
    color: var(--fg-2);
    line-height: 1.4;
  }

  .nb-add__input::placeholder {
    color: var(--fg-4);
  }

  .nb-add__input:disabled {
    opacity: 0.5;
  }

  /* ── Footer: progress bar ────────────────────────────────────────────────── */

  .nb-foot {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    border-top: 1px solid var(--border-faint);
    font-size: 12.5px;
    color: var(--fg-3);
  }

  .nb-progress {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 0;
  }

  .nb-progress__track {
    flex: 1;
    max-width: 140px;
    height: 4px;
    border-radius: 2px;
    background: var(--surface-3);
    overflow: hidden;
  }

  .nb-progress__fill {
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transition: width var(--dur-norm) var(--ease);
  }

  .nb-progress__label {
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  /* ── Calendar popover ────────────────────────────────────────────────────── */

  /* Fullscreen transparent backdrop — click-away closes the calendar */
  .nb-cal-backdrop {
    position: fixed;
    inset: 0;
    z-index: 29;
    background: transparent;
    border: none;
    cursor: default;
  }

  /* Popover anchor: absolute within .nb-block (position: relative) */
  .nb-pop {
    position: absolute;
    z-index: 30;
  }

  /* Dark calendar shell */
  .nb-cal {
    width: 268px;
    padding: 12px;
    box-sizing: border-box;
    background: #1c1c20;
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    box-shadow: var(--shadow-pop);
  }

  /* Calendar header: prev / month label / next */
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
    transition:
      background var(--dur-fast) var(--ease),
      color      var(--dur-fast) var(--ease);
  }

  .nb-cal__nav:hover {
    background: var(--overlay-hover);
    color: var(--fg-1);
  }

  /* 7-column day grid */
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
    transition:
      background var(--dur-fast) var(--ease),
      color      var(--dur-fast) var(--ease);
  }

  .nb-cal__day:hover {
    background: var(--overlay-hover);
    color: var(--fg-1);
  }

  /* Previous / next month days: muted */
  .nb-cal__day.is-muted {
    color: var(--fg-4);
    cursor: default;
  }

  .nb-cal__day.is-muted:hover {
    background: transparent;
    color: var(--fg-4);
  }

  /* Today: amber ring + weight (only when not selected) */
  .nb-cal__day.is-today {
    color: var(--status-warning);
    font-weight: 600;
    box-shadow: inset 0 0 0 1px rgba(245, 158, 11, 0.4);
  }

  /* Selected: solid accent fill */
  .nb-cal__day.is-selected {
    background: var(--accent);
    color: var(--fg-on-accent);
    font-weight: 600;
  }

  .nb-cal__day.is-selected:hover {
    background: var(--accent-hover);
  }

  /* Calendar footer: Clear date (left) + Done (right) */
  .nb-cal__foot {
    display: flex;
    align-items: center;
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid var(--border-faint);
  }

  .nb-cal__clear {
    background: transparent;
    border: 0;
    cursor: pointer;
    color: var(--fg-3);
    font-family: var(--font-sans);
    font-size: 12.5px;
    font-weight: 500;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 8px;
    border-radius: var(--radius-sm);
    transition:
      background var(--dur-fast) var(--ease),
      color      var(--dur-fast) var(--ease);
  }

  .nb-cal__clear:hover {
    background: var(--overlay-hover);
    color: var(--status-danger);
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
