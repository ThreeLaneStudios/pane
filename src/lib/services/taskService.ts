import * as api from '$lib/api';
import { tasks, activeTask, activeProject, activeFolder } from '$lib/stores/app';
import { get } from 'svelte/store';
import type { Task, TaskStatus, TaskPriority } from '$lib/types';

// createTask removed — it called api.createTask which hardcoded status: 'task'.
// Create tasks via api.createNoteInProject + api.noteMetaToTask at call sites.

export async function selectTask(task: Task): Promise<void> {
  // Clear current task selection
  activeTask.set(null);
  // Small delay for any cleanup
  await new Promise(r => setTimeout(r, 20));
  
  try {
    const taskData = await api.getTask(task.id);
    if (taskData) {
      activeTask.set(taskData);
    }
  } catch (e) {
    console.error('Failed to load task:', e);
  }
}

export async function deleteTask(task: Task): Promise<void> {
  const current = get(activeTask);
  tasks.update(t => t.filter(x => x.id !== task.id));
  if (current?.id === task.id) {
    activeTask.set(null);
  }
  await api.deleteTask(task.id);
}

export async function loadTasks(
  options?: {
    status?: TaskStatus;
    project?: string;
    assignee?: string;
    projectId?: string;
    folderId?: string;
  }
): Promise<void> {
  let loaded: Task[];
  
  if (options?.projectId) {
    loaded = await api.listTasksByProject(options.projectId);
  } else if (options?.folderId) {
    loaded = await api.listTasksByFolder(options.folderId);
  } else {
    loaded = await api.listTasks(
      options?.status,
      options?.project,
      options?.assignee
    );
  }
  
  // Load and merge — never overwrite just-created tasks
  tasks.update(current => {
    const loadedIds = new Set(loaded.map(t => t.id));
    const extras = current.filter(t => !loadedIds.has(t.id));
    return [...extras, ...loaded];
  });
}

export async function updateTaskStatus(taskId: string, status: TaskStatus): Promise<void> {
  await api.updateTaskStatus(taskId, status);
  
  // Update the task in local store
  tasks.update(current => 
    current.map(t => 
      t.id === taskId 
        ? { ...t, status: status as any, updated_at: new Date().toISOString() }
        : t
    )
  );
  
  // Update active task if it matches
  const activeTaskValue = get(activeTask);
  if (activeTaskValue && activeTaskValue.id === taskId) {
    activeTask.set({
      ...activeTaskValue, 
      status: status as any,
      updated_at: new Date().toISOString()
    });
  }
}

export async function updateTask(task: Task): Promise<Task> {
  const updated = await api.updateTask(task);
  
  // Update in local store
  tasks.update(current => 
    current.map(t => t.id === task.id ? updated : t)
  );
  
  // Update active task if it matches
  const activeTaskValue = get(activeTask);
  if (activeTaskValue && activeTaskValue.id === task.id) {
    activeTask.set(updated);
  }
  
  return updated;
}

export async function loadTasksToday(): Promise<void> {
  const loaded = await api.getTasksToday();
  tasks.update(current => {
    const loadedIds = new Set(loaded.map(t => t.id));
    const extras = current.filter(t => !loadedIds.has(t.id));
    return [...extras, ...loaded];
  });
}