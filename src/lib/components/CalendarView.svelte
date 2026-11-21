<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { Task, Project } from '$lib/types';
  import * as api from '$lib/api';
  import { noteMetaToTask } from '$lib/utils/cardNormalizers';
  import FolderPicker from './FolderPicker.svelte';
  import { createCalendarTask } from './calendarCreation';
  import {
    ENTITY_NOTE,
    ENTITY_SHEET,
    ENTITY_FOLDER,
    ENTITY_PROJECT,
    REASON_DELETED,
    subscribeDataChanged,
    type DataChangedPayload,
  } from '$lib/refreshEvent';

  interface Props {
    onTaskSelect?: (task: Task) => void;
    refreshKey?: number;
  }

  let { onTaskSelect, refreshKey = 0 }: Props = $props();

  let tasks: Task[] = $state([]);
  let projects: Project[] = $state([]);
  let loading = $state(false);
  let currentDate = $state(new Date());
  let selectedDate: string | null = $state(null);
  let showTaskModal = $state(false);

  // Picker state — set when user clicks a date cell
  let pickerDate: string | null = $state(null);
  let pickerProjectId = $state('');
  let pickerFolderId = $state('');
  let pickerSubmitting = $state(false);

  // Calendar state
  let currentMonth = $derived(currentDate.getMonth());
  let currentYear = $derived(currentDate.getFullYear());
  let monthName = $derived(currentDate.toLocaleDateString([], { month: 'long', year: 'numeric' }));
  
  // Get first day of month and number of days
  let firstDay = $derived(new Date(currentYear, currentMonth, 1).getDay());
  let daysInMonth = $derived(new Date(currentYear, currentMonth + 1, 0).getDate());
  
  // Create calendar grid
  let calendarDays = $derived(() => {
    const days = [];
    
    // Add empty cells for days before month starts
    for (let i = 0; i < firstDay; i++) {
      days.push(null);
    }
    
    // Add actual days
    for (let day = 1; day <= daysInMonth; day++) {
      days.push(day);
    }
    
    return days;
  });

  // #2972 stage 2 (§34.5 WARN closure, PR #3016 / Reed's 2026-10-11 audit —
  // Matt's sign-off: "fix before merge"): CalendarView has zero listeners
  // today (confirmed at spec time — no `pane-*` registration anywhere in
  // this file). Gains a single pane-data-changed handler: a note/sheet
  // change patches that note's calendar card in place if it has a due date
  // shown here (same discipline as the board); a folder/project change
  // patches, inserts or removes just the ONE changed project/folder in the
  // `projects` lookup table `getProjectColor()` reads, via the new
  // single-entity read (`api.getProjectOrFolder`) — no more
  // `listProjects()`/`get_projects()` round-trip for every event.
  async function handleDataChangedEvent(payload: DataChangedPayload) {
    const { entity_id, entity_type, reason } = payload;

    if (entity_type === ENTITY_FOLDER || entity_type === ENTITY_PROJECT) {
      if (reason === REASON_DELETED) {
        // A delete removes it locally with no read at all.
        projects = projects.filter(p => p.id !== entity_id);
        return;
      }
      try {
        const entity = await api.getProjectOrFolder(entity_id);
        const idx = projects.findIndex(p => p.id === entity_id);
        if (idx === -1) {
          projects = [...projects, entity];
        } else {
          projects = projects.map(p => (p.id === entity_id ? { ...p, ...entity } : p));
        }
      } catch (e) {
        console.error('[CALENDAR] Failed to patch project/folder for pane-data-changed event:', entity_id, e);
      }
      return;
    }

    if (entity_type !== ENTITY_NOTE && entity_type !== ENTITY_SHEET) return;

    if (reason === REASON_DELETED) {
      tasks = tasks.filter(t => t.id !== entity_id);
      return;
    }

    try {
      // `api.readNote()`, not `entityService.getNote()` — only `readNote()`
      // remaps the raw backend `due_date` field onto `task_due_date`, the
      // field this handler (and `noteMetaToTask()`) reads.
      const noteData = await api.readNote('', entity_id);
      const idx = tasks.findIndex(t => t.id === entity_id);
      if (!noteData.meta.task_due_date) {
        // No due date — this note has no calendar card. Remove if one
        // existed from a prior due date that was just cleared.
        if (idx !== -1) tasks = tasks.filter(t => t.id !== entity_id);
        return;
      }
      const patched = noteMetaToTask(noteData.meta);
      if (idx === -1) {
        tasks = [...tasks, patched];
      } else {
        tasks = tasks.map(t => (t.id === entity_id ? { ...t, ...patched } : t));
      }
    } catch (e) {
      console.error('[CALENDAR] Failed to patch/insert task for pane-data-changed event:', entity_id, e);
    }
  }

  let unlistenDataChanged: Promise<() => void> | null = null;

  onMount(() => {
    loadData();
    // #2972 stage 3 (item 4): one shared helper hearing BOTH channels —
    // backend `listen()` AND the app `CustomEvent`.
    unlistenDataChanged = subscribeDataChanged((payload) => {
      handleDataChangedEvent(payload);
    });
  });

  onDestroy(() => {
    unlistenDataChanged?.then(fn => fn());
  });

  $effect(() => {
    if (refreshKey > 0) {
      loadData();
    }
  });

  async function loadData() {
    loading = true;
    try {
      const [allTasks, allProjects] = await Promise.all([
        api.listTasks(),
        api.listProjects()
      ]);
      tasks = allTasks;
      projects = allProjects;
    } catch (e) {
      console.error('Failed to load calendar data:', e);
      tasks = [];
      projects = [];
    } finally {
      loading = false;
    }
  }

  function getTasksForDate(day: number): Task[] {
    if (!day) return [];
    
    const dateStr = `${currentYear}-${String(currentMonth + 1).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
    
    return tasks.filter(task => {
      if (!task.due_date) return false;
      const taskDate = task.due_date.split('T')[0]; // Handle datetime format
      return taskDate === dateStr;
    });
  }

  function getProjectColor(task: Task): string {
    if (!task.project_id && !task.folder_id) return '#00E4E4'; // Default teal
    
    const project = projects.find(p => p.id === task.project_id || p.id === task.folder_id);
    return project?.color || '#00E4E4';
  }

  function goToPrevMonth() {
    currentDate = new Date(currentYear, currentMonth - 1, 1);
  }

  function goToNextMonth() {
    currentDate = new Date(currentYear, currentMonth + 1, 1);
  }

  function goToToday() {
    currentDate = new Date();
  }

  function isToday(day: number): boolean {
    if (!day) return false;
    const today = new Date();
    return day === today.getDate() && 
           currentMonth === today.getMonth() && 
           currentYear === today.getFullYear();
  }

  function isOverdue(task: Task): boolean {
    if (!task.due_date) return false;
    const today = new Date().toISOString().split('T')[0];
    return task.due_date.split('T')[0] < today && task.status !== 'done';
  }

  function handleDateClick(day: number) {
    if (!day) return;
    const dateStr = `${currentYear}-${String(currentMonth + 1).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
    selectedDate = dateStr;
    // Open the project/folder picker before creating anything
    pickerDate = dateStr;
    pickerProjectId = '';
    pickerFolderId = '';
    pickerSubmitting = false;
  }

  function handleFolderSelected(folderId: string, _folderName: string, projectId: string) {
    pickerFolderId = folderId;
    pickerProjectId = projectId;
  }

  function cancelPicker() {
    pickerDate = null;
    pickerProjectId = '';
    pickerFolderId = '';
  }

  async function confirmCreateTask() {
    // Guard: no selection → no creation (enforced inside createCalendarTask)
    if (!pickerProjectId || !pickerFolderId || !pickerDate) return;
    pickerSubmitting = true;
    try {
      const task = await createCalendarTask(pickerProjectId, pickerFolderId, pickerDate);
      if (task) {
        tasks = [...tasks, task];
        onTaskSelect?.(task);
      }
    } catch (e) {
      console.error('Failed to create task:', e);
    } finally {
      pickerSubmitting = false;
      pickerDate = null;
      pickerProjectId = '';
      pickerFolderId = '';
    }
  }

  async function toggleTaskStatus(task: Task) {
    const newStatus = task.status === 'done' ? 'todo' : 'done';
    try {
      const updated = await api.updateTask({ 
        ...task, 
        status: newStatus,
        completed_at: newStatus === 'done' ? new Date().toISOString() : undefined,
        updated_at: new Date().toISOString()
      });
      tasks = tasks.map(t => t.id === updated.id ? updated : t);
    } catch (e) {
      console.error('Failed to toggle task status:', e);
    }
  }
</script>

<div class="calendar-container">
  <div class="calendar-document">
    <!-- Header -->
    <div class="calendar-header">
      <h1 class="document-title">Calendar</h1>
      <div class="title-line"></div>
      <div class="calendar-nav">
        <button class="nav-btn" onclick={goToPrevMonth}>‹</button>
        <button class="month-title" onclick={goToToday}>{monthName}</button>
        <button class="nav-btn" onclick={goToNextMonth}>›</button>
      </div>
    </div>

    <!-- Calendar Grid -->
    <section class="calendar-section">
      {#if loading}
        <div class="loading-state">Loading...</div>
      {:else}
        <!-- Day headers -->
        <div class="calendar-grid">
          <div class="day-headers">
            <div class="day-header">SUN</div>
            <div class="day-header">MON</div>
            <div class="day-header">TUE</div>
            <div class="day-header">WED</div>
            <div class="day-header">THU</div>
            <div class="day-header">FRI</div>
            <div class="day-header">SAT</div>
          </div>
          
          <!-- Calendar days -->
          <div class="calendar-days">
            {#each calendarDays() as day}
              <div 
                class="calendar-day"
                class:today={isToday(day || 0)}
                class:empty={!day}
                onclick={() => handleDateClick(day || 0)}
              >
                {#if day}
                  <div class="day-number">{day}</div>
                  
                  <div class="day-tasks">
                    {#each getTasksForDate(day) as task (task.id)}
                      <div 
                        class="task-item"
                        class:overdue={isOverdue(task)}
                        class:completed={task.status === 'done'}
                        style:border-left-color={getProjectColor(task)}
                        onclick={(e) => { e.stopPropagation(); onTaskSelect?.(task); }}
                      >
                        <button 
                          class="task-checkbox"
                          class:checked={task.status === 'done'}
                          onclick={(e) => { e.stopPropagation(); toggleTaskStatus(task); }}
                        >
                          {#if task.status === 'done'}✓{/if}
                        </button>
                        <span class="task-title">{task.title}</span>
                      </div>
                    {/each}
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        </div>
      {/if}
    </section>
  </div>
{#if pickerDate}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="picker-overlay" onclick={cancelPicker} onkeydown={(e) => { if (e.key === 'Escape') cancelPicker(); }}></div>
  <div class="picker-modal" role="dialog" aria-modal="true" aria-label="Choose project and folder">
    <div class="picker-header">
      <h3>Create Task for {pickerDate}</h3>
      <button class="picker-close" onclick={cancelPicker} title="Cancel (Esc)">×</button>
    </div>
    <div class="picker-body">
      <label class="picker-label" for="calendar-folder-picker">Project &amp; Folder <span class="required">*</span></label>
      <FolderPicker
        placeholder="Choose a folder…"
        onFolderSelected={handleFolderSelected}
        required
      />
    </div>
    <div class="picker-actions">
      <button class="picker-cancel-btn" onclick={cancelPicker}>Cancel</button>
      <button
        class="picker-confirm-btn"
        disabled={!pickerProjectId || !pickerFolderId || pickerSubmitting}
        onclick={confirmCreateTask}
      >
        {pickerSubmitting ? 'Creating…' : 'Create Task'}
      </button>
    </div>
  </div>
{/if}
</div>

<style>
  .calendar-container {
    height: 100%;
    background: #0a0a0f;
    color: #E0E0E0;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
    -webkit-font-smoothing: antialiased;
    font-weight: 300;
    display: flex;
    justify-content: center;
    overflow: hidden;
  }

  .calendar-document {
    width: 100%;
    max-width: 1200px;
    height: 100%;
    display: flex;
    flex-direction: column;
    padding: 60px 30px 24px;
  }

  /* Header */
  .calendar-header {
    text-align: center;
    margin-bottom: 40px;
    flex-shrink: 0;
  }

  .document-title {
    font-size: 24px;
    font-weight: 600;
    letter-spacing: 0;
    color: #e0e0e0;
    margin-bottom: 20px;
    line-height: 1.2;
  }

  .title-line {
    width: 100px;
    height: 1px;
    background: linear-gradient(90deg, transparent, #00E4E4, transparent);
    margin: 0 auto 20px;
  }

  .calendar-nav {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 20px;
    margin-top: 20px;
  }

  .nav-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: 1px solid rgba(0, 228, 228, 0.3);
    border-radius: 6px;
    background: transparent;
    color: #00E4E4;
    cursor: pointer;
    font-size: 18px;
    font-weight: 300;
    transition: all 0.15s ease;
  }

  .nav-btn:hover {
    background: rgba(0, 228, 228, 0.1);
    border-color: rgba(0, 228, 228, 0.6);
    box-shadow: 0 0 12px rgba(0, 228, 228, 0.3);
  }

  .month-title {
    font-size: 16px;
    font-weight: 300;
    letter-spacing: 3px;
    color: #00E4E4;
    background: transparent;
    border: none;
    cursor: pointer;
    transition: all 0.15s ease;
    text-transform: uppercase;
    min-width: 200px;
  }

  .month-title:hover {
    color: #FFFFFF;
    text-shadow: 0 0 8px #00E4E4;
  }

  /* Calendar */
  .calendar-section {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    min-height: 0;
  }

  .loading-state {
    text-align: center;
    font-size: 13px;
    color: rgba(0, 228, 228, 0.5);
    padding: 60px 0;
    text-transform: uppercase;
    letter-spacing: 2px;
  }

  .calendar-grid {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .day-headers {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 1px;
    margin-bottom: 10px;
    flex-shrink: 0;
  }

  .day-header {
    text-align: center;
    font-size: 10px;
    font-weight: 400;
    letter-spacing: 2px;
    color: rgba(0, 228, 228, 0.6);
    text-transform: uppercase;
    padding: 8px 0;
  }

  .calendar-days {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    grid-auto-rows: minmax(120px, auto);
    gap: 1px;
    flex: 1;
    background: rgba(0, 228, 228, 0.1);
  }

  .calendar-day {
    background: rgba(11, 11, 15, 0.8);
    border: 1px solid rgba(0, 228, 228, 0.1);
    border-radius: 4px;
    padding: 8px;
    cursor: pointer;
    transition: all 0.15s ease;
    display: flex;
    flex-direction: column;
    position: relative;
    min-height: 120px;
  }

  .calendar-day:hover {
    background: rgba(0, 228, 228, 0.05);
    border-color: rgba(0, 228, 228, 0.3);
  }

  .calendar-day.today {
    background: rgba(0, 228, 228, 0.1);
    border-color: #00E4E4;
    box-shadow: 0 0 12px rgba(0, 228, 228, 0.3);
  }

  .calendar-day.today .day-number {
    color: #FFFFFF;
    font-weight: 400;
  }

  .calendar-day.empty {
    background: transparent;
    border-color: transparent;
    cursor: default;
  }

  .day-number {
    font-size: 12px;
    font-weight: 300;
    color: rgba(0, 228, 228, 0.7);
    margin-bottom: 6px;
    flex-shrink: 0;
  }

  .day-tasks {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow-y: auto;
  }

  .task-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 6px;
    border-radius: 2px;
    background: rgba(0, 228, 228, 0.1);
    border-left: 2px solid #00E4E4;
    cursor: pointer;
    transition: all 0.15s ease;
    min-height: 20px;
  }

  .task-item:hover {
    background: rgba(0, 228, 228, 0.2);
    transform: translateY(-1px);
    box-shadow: 0 2px 8px rgba(0, 228, 228, 0.2);
  }

  .task-item.overdue {
    border-left-color: #FF6B6B;
    background: rgba(255, 107, 107, 0.1);
  }

  .task-item.completed {
    opacity: 0.5;
    background: rgba(0, 228, 228, 0.05);
  }

  .task-checkbox {
    width: 12px;
    height: 12px;
    border: 1px solid rgba(0, 228, 228, 0.5);
    border-radius: 2px;
    background: transparent;
    color: #00E4E4;
    cursor: pointer;
    transition: all 0.15s ease;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 8px;
    padding: 0;
  }

  .task-checkbox:hover {
    border-color: #00E4E4;
    box-shadow: 0 0 4px rgba(0, 228, 228, 0.3);
  }

  .task-checkbox.checked {
    background: #00E4E4;
    border-color: #00E4E4;
    color: #0a0a0f;
  }

  .task-title {
    font-size: 10px;
    font-weight: 300;
    color: #FFFFFF;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1.3;
  }

  .task-item.completed .task-title {
    text-decoration: line-through;
    opacity: 0.6;
  }

  /* Responsive design */
  @media (max-width: 768px) {
    .calendar-document {
      padding: 40px 20px 20px;
    }

    .document-title {
      font-size: 28px;
      letter-spacing: 6px;
    }

    .calendar-days {
      grid-auto-rows: minmax(100px, auto);
    }

    .calendar-day {
      min-height: 100px;
      padding: 6px;
    }

    .task-item {
      padding: 2px 4px;
    }

    .task-title {
      font-size: 9px;
    }
  }

  @media (max-width: 480px) {
    .calendar-document {
      padding: 30px 15px 15px;
    }

    .document-title {
      font-size: 24px;
      letter-spacing: 4px;
    }

    .calendar-days {
      grid-auto-rows: minmax(80px, auto);
    }

    .calendar-day {
      min-height: 80px;
      padding: 4px;
    }
  }

  /* Picker modal */
  .picker-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    z-index: 1000;
  }

  .picker-modal {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 400px;
    max-width: calc(100vw - 32px);
    background: #111118;
    border: 1px solid rgba(0, 228, 228, 0.3);
    border-radius: 12px;
    box-shadow: 0 20px 40px rgba(0, 0, 0, 0.5), 0 0 20px rgba(0, 228, 228, 0.1);
    z-index: 1001;
    display: flex;
    flex-direction: column;
  }

  .picker-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 20px 24px;
    border-bottom: 1px solid rgba(0, 228, 228, 0.15);
  }

  .picker-header h3 {
    font-size: 15px;
    font-weight: 500;
    color: #e0e0e0;
    margin: 0;
    letter-spacing: 0.5px;
  }

  .picker-close {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: rgba(224, 224, 224, 0.5);
    cursor: pointer;
    font-size: 18px;
    transition: all 0.15s ease;
  }

  .picker-close:hover {
    background: rgba(0, 228, 228, 0.1);
    color: #e0e0e0;
  }

  .picker-body {
    padding: 20px 24px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .picker-label {
    font-size: 11px;
    font-weight: 600;
    color: rgba(0, 228, 228, 0.7);
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  .required {
    color: #FF6B6B;
  }

  .picker-actions {
    display: flex;
    gap: 12px;
    justify-content: flex-end;
    padding: 16px 24px 20px;
    border-top: 1px solid rgba(0, 228, 228, 0.15);
  }

  .picker-cancel-btn,
  .picker-confirm-btn {
    padding: 9px 18px;
    border: none;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    font-family: inherit;
  }

  .picker-cancel-btn {
    background: rgba(224, 224, 224, 0.08);
    color: rgba(224, 224, 224, 0.7);
  }

  .picker-cancel-btn:hover {
    background: rgba(224, 224, 224, 0.15);
    color: #e0e0e0;
  }

  .picker-confirm-btn {
    background: #00E4E4;
    color: #0a0a0f;
  }

  .picker-confirm-btn:hover:not(:disabled) {
    background: #00ffff;
    box-shadow: 0 0 12px rgba(0, 228, 228, 0.4);
  }

  .picker-confirm-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  /* Scrollbar */
  .calendar-section::-webkit-scrollbar {
    width: 2px;
  }

  .calendar-section::-webkit-scrollbar-track {
    background: transparent;
  }

  .calendar-section::-webkit-scrollbar-thumb {
    background: rgba(0, 228, 228, 0.3);
    border-radius: 1px;
  }

  .day-tasks::-webkit-scrollbar {
    width: 1px;
  }

  .day-tasks::-webkit-scrollbar-track {
    background: transparent;
  }

  .day-tasks::-webkit-scrollbar-thumb {
    background: rgba(0, 228, 228, 0.2);
    border-radius: 1px;
  }
</style>