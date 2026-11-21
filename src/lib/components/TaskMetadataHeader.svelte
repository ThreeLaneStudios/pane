<script lang="ts">
  import type { NoteMeta, TaskStatus, TaskPriority } from '$lib/types';
  import { updateTaskFields } from '$lib/services/noteService';

  interface Props {
    note: NoteMeta;
    onUpdate?: (updatedNote: NoteMeta) => void;
  }

  let { note, onUpdate }: Props = $props();

  // Collapse by default unless it's a brand-new note (title starts with "Untitled")
  let isExpanded = $state(note.title === 'Untitled' || note.title === '');
  let isUpdating = $state(false);

  // Auto-expand when task fields change (works for both frontend and AI/backend updates)
  let taskFieldsInitialized = $state(false);
  let prevStatus = $state(note.task_status);
  let prevPriority = $state(note.task_priority);
  let prevDueDate = $state(note.task_due_date);

  $effect(() => {
    if (!taskFieldsInitialized) {
      taskFieldsInitialized = true;
      prevStatus = note.task_status;
      prevPriority = note.task_priority;
      prevDueDate = note.task_due_date;
      return;
    }
    if (note.task_status !== prevStatus || note.task_priority !== prevPriority || note.task_due_date !== prevDueDate) {
      isExpanded = true;
      prevStatus = note.task_status;
      prevPriority = note.task_priority;
      prevDueDate = note.task_due_date;
    }
  });
  const statusOptions = [
    { value: 'cabinet', label: 'Cabinet', color: '#888888' },
    { value: 'todo', label: 'To Do', color: '#EF4444' },
    { value: 'in_progress', label: 'In Progress', color: '#3B82F6' },
    { value: 'review', label: 'Review', color: '#F59E0B' },
    { value: 'done', label: 'Done', color: '#10B981' }
  ];

  const priorityOptions = [
    { value: '', label: 'No Priority', color: '#666666' },
    { value: 'low', label: 'Low', color: '#666666' },
    { value: 'medium', label: 'Medium', color: '#00E4E4' },
    { value: 'high', label: 'High', color: '#FB923C' },
    { value: 'urgent', label: 'Urgent', color: '#EF4444' }
  ];

  function formatDateForInput(dateStr?: string | null): string {
    if (!dateStr) return '';
    try {
      const d = new Date(dateStr);
      return d.toISOString().split('T')[0];
    } catch { return ''; }
  }

  let fieldsContainer: HTMLElement;

  function handleTabNav(e: KeyboardEvent) {
    if (e.key === 'Tab') {
      const current = (e.currentTarget as HTMLElement).dataset.field;
      if (!current || !fieldsContainer) return;
      const idx = parseInt(current);
      
      // If we're on the last field (3 = Priority) and not shift-tabbing, focus editor
      if (idx === 3 && !e.shiftKey) {
        e.preventDefault();
        e.stopPropagation();
        const editorContent = document.querySelector('.pane-editor') as HTMLElement;
        if (editorContent) {
          editorContent.focus();
        }
        return;
      }
      
      // If we're on the first field (0 = Status) and shift-tabbing, go to title
      if (idx === 0 && e.shiftKey) {
        e.preventDefault();
        e.stopPropagation();
        const titleInput = document.querySelector('[data-testid="note-title-input"]') as HTMLElement;
        if (titleInput) {
          titleInput.focus();
          if (titleInput instanceof HTMLInputElement) titleInput.select();
        }
        return;
      }
      
      e.preventDefault();
      e.stopPropagation();
      const nextIdx = e.shiftKey ? idx - 1 : idx + 1;
      const next = fieldsContainer.querySelector(`[data-field="${nextIdx}"]`) as HTMLElement;
      if (next) {
        next.focus();
        // Select all text in text inputs for easy overwrite
        if (next instanceof HTMLInputElement) next.select();
      }
    }
    // Enter in select moves to next field
    if (e.key === 'Enter' && e.currentTarget instanceof HTMLSelectElement) {
      e.preventDefault();
      e.stopPropagation();
      const current = (e.currentTarget as HTMLElement).dataset.field;
      if (!current || !fieldsContainer) return;
      const idx = parseInt(current);
      
      // If last field (Priority), focus editor
      if (idx === 3) {
        const editorContent = document.querySelector('.pane-editor') as HTMLElement;
        if (editorContent) editorContent.focus();
      } else {
        const next = fieldsContainer.querySelector(`[data-field="${idx + 1}"]`) as HTMLElement;
        if (next) next.focus();
      }
    }
    // Enter in text inputs moves to next field too
    if (e.key === 'Enter' && e.currentTarget instanceof HTMLInputElement) {
      e.preventDefault();
      const current = (e.currentTarget as HTMLElement).dataset.field;
      if (!current || !fieldsContainer) return;
      const idx = parseInt(current);
      const next = fieldsContainer.querySelector(`[data-field="${idx + 1}"]`) as HTMLElement;
      if (next) next.focus();
    }
  }

  function formatDateDisplay(dateStr?: string | null): string {
    if (!dateStr) return '';
    try {
      const iso = formatDateForInput(dateStr);
      if (!iso) return '';
      const [yyyy, mm, dd] = iso.split('-');
      return `${mm}/${dd}/${yyyy}`;
    } catch { return ''; }
  }

  async function handleFieldUpdate(field: string, value: string, advanceToNext = false) {
    if (isUpdating) return;
    isUpdating = true;
    
    try {
      const fields: any = {};
      if (field === 'status') fields.status = value || undefined;
      if (field === 'priority') fields.priority = value;
      if (field === 'dueDate') fields.dueDate = value || undefined;
      if (field === 'assignee') fields.assignee = value;

      await updateTaskFields(note.id, fields);

      const updatedNote = { ...note };
      if (field === 'status') updatedNote.task_status = (value || undefined) as TaskStatus | undefined;
      if (field === 'priority') updatedNote.task_priority = (value || undefined) as TaskPriority | undefined;
      if (field === 'dueDate') updatedNote.task_due_date = value || undefined;
      if (field === 'assignee') updatedNote.task_assignee = value || undefined;

      onUpdate?.(updatedNote);
      
      // Only advance to next field if explicitly requested (e.g., completed date entry)
      if (advanceToNext) {
        const fieldMap: Record<string, string> = { status: '0', dueDate: '1', assignee: '2', priority: '3' };
        const idx = parseInt(fieldMap[field] || '0');
        const nextIdx = String(idx + 1);
        requestAnimationFrame(() => {
          if (fieldsContainer) {
            const el = fieldsContainer.querySelector(`[data-field="${nextIdx}"]`) as HTMLElement;
            if (el) el.focus();
          }
        });
      }
    } catch (error) {
      console.error('Failed to update task field:', error);
    } finally {
      isUpdating = false;
    }
  }
</script>

<div class="task-metadata-header">
  {#if !isExpanded}
    <div class="collapsed-header" onclick={() => isExpanded = true}>
      <div class="task-indicator">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <polyline points="9,11 12,14 22,4"></polyline>
          <path d="M21,12v7a2,2 0,0 1,-2,2H5a2,2 0,0 1,-2,-2V5a2,2 0,0 1,2,-2h11"></path>
        </svg>
        <span>Details</span>
        {#if note.task_status}
          <span class="status-badge" style="color: {statusOptions.find(s => s.value === note.task_status)?.color || '#666666'}">{statusOptions.find(s => s.value === note.task_status)?.label || ''}</span>
        {/if}
        <svg class="expand-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <polyline points="6,9 12,15 18,9"></polyline>
        </svg>
      </div>
    </div>
  {:else}
    <div class="expanded-header">
      <div class="header-row" onclick={() => isExpanded = false} style="cursor: pointer">
        <div class="task-indicator-expanded">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polyline points="9,11 12,14 22,4"></polyline>
            <path d="M21,12v7a2,2 0,0 1,-2,2H5a2,2 0,0 1,-2,-2V5a2,2 0,0 1,2,-2h11"></path>
          </svg>
          <span>Details</span>
        </div>
        <button class="collapse-btn" onclick={(e) => { e.stopPropagation(); isExpanded = false; }} title="Collapse">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polyline points="18,15 12,9 6,15"></polyline>
          </svg>
        </button>
      </div>

      <div class="metadata-fields" bind:this={fieldsContainer}>
        <div class="field-row">
          <div class="field-group">
            <label>Status</label>
            <select 
              data-field="0"
              value={note.task_status || ''}
              onchange={(e) => {
                handleFieldUpdate('status', e.currentTarget.value);
                // Auto-advance to Due Date
                setTimeout(() => {
                  const next = fieldsContainer?.querySelector('[data-field="1"]') as HTMLElement;
                  if (next) next.focus();
                }, 50);
              }}
              onkeydown={handleTabNav}
              disabled={isUpdating}
            >
              {#each statusOptions as option}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          </div>

          <div class="field-group">
            <label>Due Date</label>
            <input 
              data-field="1"
              type="date"
              value={formatDateForInput(note.task_due_date)}
              onchange={(e) => {
                const val = e.currentTarget.value;
                if (val) {
                  handleFieldUpdate('dueDate', val);
                } else {
                  handleFieldUpdate('dueDate', '');
                }
              }}
              onkeydown={handleTabNav}
              disabled={isUpdating}
            />
          </div>
        </div>

        <div class="field-row">
          <div class="field-group">
            <label>Assignee</label>
            <input 
              data-field="2"
              type="text"
              value={note.task_assignee || ''}
              placeholder="Assign to..."
              onchange={(e) => handleFieldUpdate('assignee', e.currentTarget.value)}
              onkeydown={handleTabNav}
              disabled={isUpdating}
            />
          </div>

          <div class="field-group">
            <label>Priority</label>
            <select 
              data-field="3"
              value={note.task_priority || ''}
              onchange={(e) => {
                handleFieldUpdate('priority', e.currentTarget.value);
                // Auto-advance to editor body
                setTimeout(() => {
                  const editor = document.querySelector('.pane-editor') as HTMLElement;
                  if (editor) editor.focus();
                }, 50);
              }}
              onkeydown={handleTabNav}
              disabled={isUpdating}
            >
              {#each priorityOptions as option}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          </div>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .task-metadata-header {
    background: rgba(255,255,255,0.05);
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 8px;
    margin: 12px 16px;
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Segoe UI', sans-serif;
  }

  .collapsed-header {
    padding: 10px 14px;
    cursor: pointer;
    transition: background 0.15s ease;
    border-radius: 8px;
  }

  .collapsed-header:hover {
    background: rgba(255,255,255,0.03);
  }

  .task-indicator {
    display: flex;
    align-items: center;
    gap: 8px;
    color: #888888;
    font-size: 13px;
    font-weight: 500;
  }

  .task-indicator svg {
    color: #00E4E4;
  }

  .status-badge {
    font-size: 12px;
    font-weight: 500;
    margin-left: 4px;
  }

  .expand-icon {
    margin-left: auto;
    color: #666666;
  }

  .expanded-header {
    padding: 14px;
  }

  .header-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 14px;
  }

  .task-indicator-expanded {
    display: flex;
    align-items: center;
    gap: 8px;
    color: #e0e0e0;
    font-size: 14px;
    font-weight: 600;
  }

  .task-indicator-expanded svg {
    color: #00E4E4;
  }

  .collapse-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: #666666;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .collapse-btn:hover {
    background: rgba(255,255,255,0.06);
    color: #888888;
  }

  .metadata-fields {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .field-row {
    display: flex;
    gap: 12px;
  }

  .field-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
  }

  .field-group label {
    font-size: 11px;
    color: #666666;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .field-group select,
  .field-group input {
    padding: 6px 10px;
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 4px;
    background: #0a0a0f;
    color: #e0e0e0;
    font-size: 13px;
    font-family: inherit;
    height: 32px;
    transition: border-color 0.15s ease;
  }

  .field-group select:hover,
  .field-group input:hover {
    border-color: rgba(0,228,228,0.3);
  }

  .field-group select:focus,
  .field-group input:focus {
    outline: none;
    border-color: #00E4E4;
  }

  .field-group select:disabled,
  .field-group input:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .field-group input[type="text"]::placeholder {
    color: #666666;
  }

  .field-group input[type="date"] {
    color-scheme: dark;
  }
</style>
