<script lang="ts">
  import type { Task, TaskStatus, TaskPriority } from '$lib/types';
  import * as api from '$lib/api';
  import FolderPicker from './FolderPicker.svelte';
  import { onMount } from 'svelte';

  interface Props {
    onTaskCreated?: (task: Task) => void;
    onCancel?: () => void;
    defaultStatus?: TaskStatus;
    defaultDueDate?: string;
  }

  let { onTaskCreated, onCancel, defaultStatus = 'todo', defaultDueDate = '' }: Props = $props();

  let title = $state('');
  let description = $state('');
  let status: TaskStatus = $state(defaultStatus);
  let priority: TaskPriority = $state('medium');
  let dueDate = $state(defaultDueDate);
  let tags = $state('');
  let assignee = $state('');
  let submitting = $state(false);
  let selectedFolderId = $state('');
  let selectedFolderName = $state('');
  let selectedProjectId = $state('');
  
  let titleInput: HTMLInputElement = $state(null!) as unknown as HTMLInputElement;

  // Autocomplete state
  let tagSuggestions: string[] = $state([]);
  let assigneeSuggestions: string[] = $state([]);
  let showTagSuggestions = $state(false);
  let showAssigneeSuggestions = $state(false);
  let allTags: string[] = $state([]);
  let allAssignees: string[] = $state([]);

  onMount(async () => {
    try {
      const [tagsResult, assigneesResult] = await Promise.all([
        api.listTags(),
        api.listAssigneesDistinct(),
      ]);
      allTags = tagsResult.map(t => t.name);
      allAssignees = assigneesResult;
    } catch (e) {
      console.error('Failed to load suggestions:', e);
    }
  });

  function handleTagInput() {
    const parts = tags.split(',');
    const current = parts[parts.length - 1].trim().toLowerCase();
    if (current.length > 0) {
      tagSuggestions = allTags.filter(t => t.toLowerCase().includes(current) && !parts.slice(0, -1).map(p => p.trim()).includes(t)).slice(0, 5);
      showTagSuggestions = tagSuggestions.length > 0;
    } else {
      showTagSuggestions = false;
    }
  }

  function selectTag(tag: string) {
    const parts = tags.split(',').map(p => p.trim()).filter(p => p);
    parts[parts.length - 1] = tag;
    tags = parts.join(', ') + ', ';
    showTagSuggestions = false;
  }

  function handleAssigneeInput() {
    const current = assignee.trim().toLowerCase();
    if (current.length > 0) {
      assigneeSuggestions = allAssignees.filter(a => a.toLowerCase().includes(current)).slice(0, 5);
      showAssigneeSuggestions = assigneeSuggestions.length > 0;
    } else { showAssigneeSuggestions = false; }
  }

  function selectAssignee(a: string) { assignee = a; showAssigneeSuggestions = false; }

  function handleFolderSelected(folderId: string, folderName: string, projectId: string) {
    selectedFolderId = folderId;
    selectedFolderName = folderName;
    selectedProjectId = projectId;
  }

  $effect(() => {
    if (titleInput) {
      titleInput.focus();
    }
  });

  async function handleSubmit() {
    if (!title.trim() || !selectedFolderId || submitting) return;

    submitting = true;
    try {
      const tagsArray = tags.split(',').map(t => t.trim()).filter(t => t.length > 0);
      
      const noteMeta = await api.createNoteInProject(
        selectedProjectId || '',
        selectedFolderId || undefined,
        title.trim(),
        status,
        {
          priority,
          dueDate: dueDate || undefined,
          assignee: assignee.trim() || undefined,
          tags: tagsArray.length > 0 ? tagsArray : undefined,
          content: description.trim() || undefined,
        }
      );
      const newTask = api.noteMetaToTask(noteMeta, { project: selectedFolderName || undefined });

      if (onTaskCreated) {
        onTaskCreated(newTask);
      }
      
      // Reset form
      title = '';
      description = '';
      status = defaultStatus;
      priority = 'medium';
      dueDate = defaultDueDate;
      tags = '';
      assignee = '';
      selectedFolderId = '';
      selectedFolderName = '';
      selectedProjectId = '';
    } catch (e) {
      console.error('Failed to create task:', e);
    } finally {
      submitting = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      handleSubmit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      if (onCancel) onCancel();
    }
  }

  function formatDateForInput(): string {
    const today = new Date();
    return today.toISOString().split('T')[0];
  }
</script>

<div class="quick-add-overlay" onclick={onCancel} role="button" tabindex="-1" onkeydown={(e) => { if (e.key === 'Escape') onCancel?.(); }}></div>

<div class="quick-add-modal">
  <div class="quick-add-header">
    <h3>New Task for Today</h3>
    <button class="close-btn" onclick={onCancel} title="Close (Esc)">×</button>
  </div>
  
  <form class="quick-add-form" onsubmit={(e) => { e.preventDefault(); handleSubmit(); }}>
    <div class="form-group">
      <label for="task-title">Title *</label>
      <input
        id="task-title"
        bind:this={titleInput}
        bind:value={title}
        onkeydown={handleKeydown}
        placeholder="What needs to be done?"
        required
      />
    </div>

    <div class="form-group">
      <label for="task-folder">Folder *</label>
      <FolderPicker
        placeholder="Choose a folder..."
        onFolderSelected={handleFolderSelected}
        required
      />
      <small class="field-hint">Select which folder this task belongs to</small>
    </div>

    <div class="form-group">
      <label for="task-description">Description</label>
      <textarea
        id="task-description"
        bind:value={description}
        onkeydown={handleKeydown}
        placeholder="Additional details..."
        rows="3"
      ></textarea>
    </div>

    <div class="form-row">
      <div class="form-group">
        <label for="task-status">Status / Lane</label>
        <select id="task-status" bind:value={status}>
          <option value="cabinet">Cabinet</option>
          <option value="todo">Todo</option>
          <option value="in_progress">In Progress</option>
          <option value="done">Done</option>
        </select>
      </div>

      <div class="form-group">
        <label for="task-priority">Priority</label>
        <select id="task-priority" bind:value={priority}>
          <option value="low">Low</option>
          <option value="medium">Medium</option>
          <option value="high">High</option>
          <option value="urgent">Urgent</option>
        </select>
      </div>
    </div>

    <div class="form-row">
      <div class="form-group">
        <label for="task-due-date">Due Date</label>
        <input
          id="task-due-date"
          type="date"
          bind:value={dueDate}
          min={formatDateForInput()}
          onchange={() => {
            // Force close the date picker on mobile by blurring
            const target = event?.target as HTMLInputElement;
            if (target) target.blur();
          }}
        />
      </div>

      <div class="form-group autocomplete-wrapper">
        <label for="task-assignee">Assignee</label>
        <input
          id="task-assignee"
          bind:value={assignee}
          oninput={handleAssigneeInput}
          onfocus={handleAssigneeInput}
          onblur={() => setTimeout(() => showAssigneeSuggestions = false, 200)}
          placeholder="Who's responsible?"
        />
        {#if showAssigneeSuggestions}
          <ul class="suggestions">
            {#each assigneeSuggestions as suggestion}
              <li><button onmousedown={() => selectAssignee(suggestion)}>{suggestion}</button></li>
            {/each}
          </ul>
        {/if}
      </div>
    </div>

    <div class="form-group autocomplete-wrapper">
      <label for="task-tags">Tags</label>
      <input
        id="task-tags"
        bind:value={tags}
        oninput={handleTagInput}
        onfocus={handleTagInput}
        onblur={() => setTimeout(() => showTagSuggestions = false, 200)}
        placeholder="Comma-separated tags..."
      />
      {#if showTagSuggestions}
        <ul class="suggestions">
          {#each tagSuggestions as suggestion}
            <li><button onmousedown={() => selectTag(suggestion)}>{suggestion}</button></li>
          {/each}
        </ul>
      {/if}
    </div>

    <div class="form-actions">
      <button type="button" class="cancel-btn" onclick={onCancel}>
        Cancel
      </button>
      <button type="submit" class="submit-btn" disabled={!title.trim() || !selectedFolderId || submitting}>
        {submitting ? 'Creating...' : 'Create Task'}
      </button>
    </div>
  </form>
</div>

<style>
  .quick-add-overlay {
    position: fixed;
    top: 0;
    left: 0;
    width: 100vw;
    height: 100vh;
    background: rgba(0, 0, 0, 0.5);
    z-index: 1000;
  }

  .quick-add-modal {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 500px;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 64px);
    background: var(--bg-primary);
    border: 1px solid var(--border-color);
    border-radius: 12px;
    box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.1), 0 10px 10px -5px rgba(0, 0, 0, 0.04);
    z-index: 1001;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .quick-add-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 20px 24px;
    border-bottom: 1px solid var(--border-color);
  }

  .quick-add-header h3 {
    font-size: 18px;
    font-weight: 600;
    color: var(--text-primary);
    margin: 0;
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-tertiary);
    cursor: pointer;
    font-size: 20px;
    line-height: 1;
    transition: all 0.15s ease;
  }

  .close-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .quick-add-form {
    padding: 24px;
    overflow-y: auto;
    flex: 1;
    min-height: 0;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 16px;
  }

  .form-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
    margin-bottom: 16px;
  }

  .form-row .form-group {
    margin-bottom: 0;
  }

  label {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .field-hint {
    font-size: 11px;
    color: var(--text-tertiary);
    font-style: italic;
  }

  input, textarea, select {
    padding: 8px 12px;
    border: 1px solid var(--border-color);
    border-radius: 6px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-size: 14px;
    font-family: inherit;
    outline: none;
    transition: all 0.15s ease;
  }

  input:focus, textarea:focus, select:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.1);
  }

  textarea {
    resize: vertical;
    min-height: 60px;
  }

  select {
    cursor: pointer;
  }

  .form-actions {
    display: flex;
    gap: 12px;
    justify-content: flex-end;
    margin-top: 24px;
    padding-top: 20px;
    border-top: 1px solid var(--border-color);
  }

  .cancel-btn, .submit-btn {
    padding: 10px 20px;
    border: none;
    border-radius: 6px;
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .cancel-btn {
    background: var(--bg-secondary);
    color: var(--text-secondary);
  }

  .cancel-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .submit-btn {
    background: var(--accent);
    color: white;
  }

  .submit-btn:hover:not(:disabled) {
    background: var(--accent-hover);
    transform: translateY(-1px);
  }

  .submit-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
    transform: none;
  }

  .autocomplete-wrapper { position: relative; }

  .suggestions {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    list-style: none;
    margin: 4px 0 0;
    padding: 4px 0;
    z-index: 10;
    max-height: 160px;
    overflow-y: auto;
  }

  .suggestions button {
    display: block;
    width: 100%;
    padding: 6px 12px;
    border: none;
    background: transparent;
    color: var(--text-primary);
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    font-family: inherit;
  }

  .suggestions button:hover { background: var(--bg-hover); }
</style>