<script lang="ts">
  import { onMount } from 'svelte';
  import Editor from './Editor.svelte';
  import * as api from '$lib/api';
  import type { Task, TaskStatus, TaskPriority, TagInfo, User, Project } from '$lib/types';

  interface Props {
    defaultProject?: string;
    defaultProjectId?: string;
    defaultFolderId?: string;
    onClose: () => void;
    onTaskCreated: (task: Task) => void;
  }

  let { defaultProject = '', defaultProjectId = '', defaultFolderId = '', onClose, onTaskCreated }: Props = $props();

  let saving = $state(false);
  let lastSaved = $state('');

  let title = $state('');
  let description = $state('');
  let status: TaskStatus = $state('todo');
  let priority: TaskPriority = $state('medium');
  let dueDate = $state('');
  let assignee = $state('');

  // Tags — managed as array, confirmed with Enter
  let taskTags: string[] = $state([]);
  let tagInput = $state('');
  let allTags: TagInfo[] = $state([]);
  let tagSuggestions: TagInfo[] = $state([]);
  let showTagDropdown = $state(false);
  let tagInputRef: HTMLInputElement = $state(null!) as unknown as HTMLInputElement;

  // Users for assignment
  let users: User[] = $state([]);

  // Project + Folder selectors (separate)
  let allProjects: Project[] = $state([]);
  let selectedProjectId = $state(defaultProjectId);
  let selectedFolderId = $state(defaultFolderId);

  onMount(async () => {
    try { allTags = await api.listTags(); } catch {}
    try { users = await api.listUsers(); } catch {}
    try { allProjects = await api.listProjects(); } catch {}
  });

  async function createTask() {
    if (!title.trim()) return;
    saving = true;
    try {
      const folder = allProjects.find(p => p.id === selectedFolderId);
      const projectId = selectedProjectId || folder?.parent_id || folder?.id;
      const folderName = folder?.name || defaultProject;
      
      const noteMeta = await api.createNoteInProject(
        projectId || '',
        selectedFolderId || undefined,
        title.trim(),
        status,
        {
          priority,
          dueDate: dueDate || undefined,
          assignee: assignee.trim() || undefined,
          tags: taskTags,
          content: description.trim() || undefined,
        }
      );
      const newTask = api.noteMetaToTask(noteMeta, { project: folderName || undefined });

      onTaskCreated(newTask);
      saving = false;
      lastSaved = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } catch (e) { 
      console.error('Failed to create task:', e); 
      saving = false; 
    }
  }

  function handleDescriptionUpdate(markdown: string) {
    const descContent = markdown.replace(/^#\s*.*\n\n?/, '').trim();
    description = descContent;
  }

  function handleTitleUpdate(newTitle: string) {
    title = newTitle;
  }

  // When project changes, update available folders
  function handleProjectChange() {
    // If folder doesn't belong to the new project, clear it
    if (selectedFolderId) {
      const folder = allProjects.find(p => p.id === selectedFolderId);
      if (folder && folder.parent_id !== selectedProjectId) {
        selectedFolderId = '';
      }
    }
  }

  // Tag management — Enter key to confirm
  function handleTagKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      addCurrentTag();
    } else if (e.key === 'Backspace' && tagInput === '' && taskTags.length > 0) {
      taskTags = taskTags.slice(0, -1);
    } else if (e.key === 'Escape') {
      showTagDropdown = false;
    }
  }

  function addCurrentTag() {
    const tag = tagInput.trim();
    if (tag && !taskTags.includes(tag)) {
      taskTags = [...taskTags, tag];
      tagInput = '';
      showTagDropdown = false;
    }
  }

  function removeTag(tag: string) {
    taskTags = taskTags.filter(t => t !== tag);
  }

  function handleTagInput() {
    const query = tagInput.trim().toLowerCase();
    if (query.length > 0) {
      tagSuggestions = allTags.filter(t => 
        t.name.toLowerCase().includes(query) && !taskTags.includes(t.name)
      );
      showTagDropdown = tagSuggestions.length > 0;
    } else {
      showTagDropdown = false;
    }
  }

  function selectTagSuggestion(tagName: string) {
    if (!taskTags.includes(tagName)) {
      taskTags = [...taskTags, tagName];
      tagInput = '';
      showTagDropdown = false;
    }
    if (tagInputRef) tagInputRef.focus();
  }

  function getStatusColor(s: TaskStatus): string {
    switch (s) {
      case 'cabinet': return '#888888';
      case 'todo': return '#EF4444';
      case 'in_progress': return '#3B82F6';
      case 'review': return '#F59E0B';
      case 'done': return '#10B981';
      default: return '#888888';
    }
  }

  function getPriorityColor(p: TaskPriority): string {
    switch (p) {
      case 'urgent': return '#ef4444';
      case 'high': return '#f59e0b';
      case 'medium': return '#10b981';
      case 'low': return '#888888';
      default: return '#888888';
    }
  }

  // Root projects (for Project selector)
  function getRootProjects() {
    return allProjects.filter(p => !p.parent_id);
  }

  // Folders for selected project
  function getFoldersForProject(projectId: string) {
    return allProjects.filter(p => p.parent_id === projectId);
  }
</script>

<div class="task-create-page">
  <!-- Top bar -->
  <div class="task-meta-bar">
    <div class="meta-left">
      <button class="back-btn" onclick={onClose} title="Cancel">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 12H5M12 19l-7-7 7-7"/>
        </svg>
      </button>
      <span class="task-label">New Task</span>
    </div>
    <div class="meta-right">
      <div class="save-status">
        {#if saving}<span class="saving">Creating...</span>
        {:else if lastSaved}<span class="saved">Created {lastSaved}</span>{/if}
      </div>
      <button class="create-btn" onclick={createTask} disabled={!title.trim() || saving}>
        {saving ? 'Creating...' : 'Create Task'}
      </button>
    </div>
  </div>

  <!-- Metadata strip -->
  <div class="task-metadata">
    <div class="meta-grid">
      <div class="meta-field">
        <label>Status</label>
        <div class="status-select-wrapper">
          <span class="status-dot" style:background={getStatusColor(status)}></span>
          <select bind:value={status}>
            <option value="cabinet">Cabinet</option>
            <option value="todo">To Do</option>
            <option value="in_progress">In Progress</option>
            <option value="review">Review</option>
            <option value="done">Done</option>
          </select>
        </div>
      </div>
      <div class="meta-field">
        <label>Priority</label>
        <div class="priority-select-wrapper">
          <span class="priority-dot" style:background={getPriorityColor(priority)}></span>
          <select bind:value={priority}>
            <option value="low">Low</option>
            <option value="medium">Medium</option>
            <option value="high">High</option>
            <option value="urgent">Urgent</option>
          </select>
        </div>
      </div>
      <div class="meta-field">
        <label>Due Date</label>
        <input type="date" bind:value={dueDate} />
      </div>
      <div class="meta-field">
        <label>Assignee</label>
        <select bind:value={assignee}>
          <option value="">Unassigned</option>
          {#each users as user (user.id)}
            <option value={user.name}>{user.name}</option>
          {/each}
        </select>
      </div>
      <div class="meta-field">
        <label>Project</label>
        <select bind:value={selectedProjectId} onchange={handleProjectChange}>
          <option value="">None</option>
          {#each getRootProjects() as project (project.id)}
            <option value={project.id}>{project.name}</option>
          {/each}
        </select>
      </div>
      <div class="meta-field">
        <label>Notebook</label>
        <select bind:value={selectedFolderId}>
          <option value="">None</option>
          {#if selectedProjectId}
            {#each getFoldersForProject(selectedProjectId) as folder (folder.id)}
              <option value={folder.id}>{folder.name}</option>
            {/each}
          {:else}
            {#each getRootProjects() as project (project.id)}
              <optgroup label={project.name}>
                {#each getFoldersForProject(project.id) as folder (folder.id)}
                  <option value={folder.id}>{folder.name}</option>
                {/each}
              </optgroup>
            {/each}
          {/if}
        </select>
      </div>
    </div>
  </div>

  <!-- Editor area (title + description) -->
  <div class="task-editor-area">
    <Editor
      content={title ? `# ${title}\n\n${description}` : ''}
      title={title || 'New Task'}
      onUpdate={handleDescriptionUpdate}
      onTitleUpdate={handleTitleUpdate}
      placeholder="Add description, checklists, notes..."
    />
  </div>

  <!-- Bottom: Tags -->
  <div class="task-bottom">
    <!-- Tags -->
    <div class="bottom-section">
      <label class="bottom-label">Tags</label>
      <div class="tags-area">
        {#each taskTags as tag}
          <span class="tag-chip">
            {tag}
            <button class="tag-remove" onclick={() => removeTag(tag)}>×</button>
          </span>
        {/each}
        <div class="tag-input-wrapper">
          <input
            bind:this={tagInputRef}
            bind:value={tagInput}
            oninput={handleTagInput}
            onkeydown={handleTagKeydown}
            onblur={() => setTimeout(() => showTagDropdown = false, 200)}
            onfocus={handleTagInput}
            placeholder={taskTags.length === 0 ? "Add tags (Enter to confirm)" : ""}
            class="tag-inline-input"
          />
          {#if showTagDropdown}
            <div class="tag-dropdown">
              {#each tagSuggestions as tag (tag.id)}
                <button class="tag-option" onmousedown={() => selectTagSuggestion(tag.name)}>
                  <span class="tag-color" style:background={tag.color}></span>
                  {tag.name}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      </div>
    </div>
  </div>
</div>

<style>
  .task-create-page {
    flex: 1; display: flex; flex-direction: column; height: 100%;
    background: var(--bg-primary);
  }

  .task-meta-bar {
    display: flex; align-items: center; justify-content: space-between;
    padding: 6px 16px; border-bottom: 1px solid var(--border-color);
    flex-shrink: 0; background: var(--bg-primary);
    height: 40px;
  }
  .meta-left { display: flex; align-items: center; gap: 6px; }
  .back-btn {
    display: flex; align-items: center; justify-content: center;
    width: 28px; height: 28px; border: none; border-radius: 6px;
    background: transparent; color: var(--text-tertiary); cursor: pointer;
    transition: all 0.15s;
  }
  .back-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
  .task-label {
    font-size: 11px; font-weight: 500; letter-spacing: 0.02em;
    color: var(--text-tertiary);
    padding: 2px 0;
  }
  .meta-right { display: flex; align-items: center; gap: 6px; }
  .save-status { font-size: 11px; }
  .saving { color: var(--accent); }
  .saved { color: var(--text-tertiary); opacity: 0.6; }

  .create-btn {
    padding: 6px 12px; border: none; border-radius: 6px;
    background: var(--accent); color: white; cursor: pointer;
    font-size: 12px; font-weight: 500; font-family: inherit; transition: all 0.15s;
  }
  .create-btn:hover:not(:disabled) { background: var(--accent-hover); }
  .create-btn:disabled { opacity: 0.5; cursor: not-allowed; }

  .task-metadata {
    padding: 12px 48px; border-bottom: 1px solid var(--border-color); flex-shrink: 0;
    background: var(--bg-primary);
  }
  .meta-grid {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 16px;
    align-items: center;
  }
  .meta-field { display: flex; align-items: center; gap: 6px; }

  .meta-field label {
    font-size: 11px; font-weight: 500;
    color: var(--text-tertiary); white-space: nowrap;
    min-width: fit-content;
  }

  .meta-field select,
  .meta-field input {
    padding: 4px 8px; height: 28px;
    border: 1px solid transparent; border-radius: 6px;
    background: var(--bg-secondary); color: var(--text-primary);
    font-size: 12px; font-family: inherit; outline: none;
    transition: all 0.15s;
  }
  .meta-field select:hover,
  .meta-field input:hover { border-color: var(--border-color); }
  .meta-field select:focus,
  .meta-field input:focus { border-color: var(--accent); background: var(--bg-primary); }

  .status-select-wrapper,
  .priority-select-wrapper {
    display: flex; align-items: center; gap: 8px; position: relative;
  }
  .status-dot, .priority-dot {
    width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0;
    pointer-events: none;
  }

  .task-editor-area { flex: 1; min-height: 0; overflow: hidden; }

  /* Bottom section: Tags */
  .task-bottom {
    flex-shrink: 0;
    border-top: 1px solid var(--border-color);
    padding: 12px 48px 16px;
    background: var(--bg-primary);
  }

  .bottom-section {
    display: flex;
    align-items: flex-start;
    gap: 12px;
  }

  .bottom-label {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-tertiary);
    padding-top: 6px;
    min-width: 50px;
  }

  .tags-area {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
    flex: 1;
  }

  .tag-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 10px;
    background: var(--accent-light);
    color: var(--accent);
    border-radius: 12px;
    font-size: 12px;
    font-weight: 500;
  }

  .tag-remove {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    border: none;
    background: transparent;
    color: var(--accent);
    cursor: pointer;
    font-size: 12px;
    border-radius: 50%;
    transition: all 0.15s;
  }

  .tag-remove:hover {
    background: var(--accent);
    color: white;
  }

  .tag-input-wrapper {
    position: relative;
    flex: 1;
    min-width: 120px;
  }

  .tag-inline-input {
    width: 100%;
    padding: 4px 8px;
    border: none;
    background: transparent;
    color: var(--text-primary);
    font-size: 13px;
    font-family: inherit;
    outline: none;
  }

  .tag-inline-input::placeholder {
    color: var(--text-tertiary);
  }

  .tag-dropdown {
    position: absolute; top: 100%; left: 0; right: 0; margin-top: 4px;
    background: var(--bg-secondary); border: 1px solid var(--border-color);
    border-radius: 8px; box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
    z-index: 100; max-height: 160px; overflow-y: auto; padding: 4px;
  }
  .tag-option {
    display: flex; align-items: center; gap: 8px; width: 100%;
    padding: 6px 10px; border: none; border-radius: 4px;
    background: transparent; color: var(--text-primary); cursor: pointer;
    font-size: 13px; font-family: inherit; text-align: left;
  }
  .tag-option:hover { background: var(--bg-hover); }
  .tag-color { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
</style>