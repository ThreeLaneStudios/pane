import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import * as taskService from './taskService';
import { tasks, activeTask, activeProject, activeFolder } from '$lib/stores/app';
import { mockData } from '../../../vitest-setup';
import type { Task, Folder } from '$lib/types';

// NOTE: createTask was removed from taskService (it called api.createTask which hardcoded
// status: 'task', a value not in the spec vocabulary). Task creation is now done via
// api.createNoteInProject + api.noteMetaToTask at each call site.
// The tests below cover the remaining taskService functions.

describe('taskService', () => {
  beforeEach(() => {
    // Reset stores
    tasks.set([]);
    activeTask.set(null);
    activeProject.set(null);
    activeFolder.set(null);

    // Reset mock data
    mockData.tasks = [];
    mockData.nextId = 1;
  });

  describe('updateTask create/update cycle', () => {
    it('updateTask updates store and active task', async () => {
      // Seed a task directly in the store
      const seedTask: Task = {
        id: 'note-1',
        title: 'Initial Task',
        description: '',
        status: 'todo',
        priority: 'medium',
        due_date: undefined,
        project: undefined,
        project_id: undefined,
        folder_id: undefined,
        tags: [],
        assignee: undefined,
        parent_task_id: undefined,
        sort_order: 0,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        completed_at: undefined,
        note_id: undefined,
      };
      tasks.set([seedTask]);
      activeTask.set(seedTask);
      mockData.tasks = [{ ...seedTask }];

      // Update the task
      const updatedTask = await taskService.updateTask({
        ...seedTask,
        title: 'Updated Task',
        status: 'in_progress',
        priority: 'high',
      } as Task);

      expect(updatedTask).toMatchObject({
        id: seedTask.id,
        title: 'Updated Task',
        status: 'in_progress',
        priority: 'high',
        updated_at: expect.any(String),
      });

      // Check store was updated
      const currentTasks = get(tasks);
      expect(currentTasks).toHaveLength(1);
      expect(currentTasks[0]).toMatchObject(updatedTask);

      // Check activeTask was updated
      const currentActiveTask = get(activeTask);
      expect(currentActiveTask).toMatchObject(updatedTask);
    });

    it('updateTaskStatus updates both store and active task', async () => {
      const task: Task = {
        id: 'note-status-test',
        title: 'Status Test Task',
        description: '',
        status: 'todo',
        priority: 'medium',
        due_date: undefined,
        project: undefined,
        project_id: undefined,
        folder_id: undefined,
        tags: [],
        assignee: undefined,
        parent_task_id: undefined,
        sort_order: 0,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        completed_at: undefined,
        note_id: undefined,
      };
      tasks.set([task]);
      activeTask.set(task);
      mockData.tasks = [{ ...task }];
      const originalUpdatedAt = task.updated_at;

      // Small delay to ensure updated_at changes
      await new Promise(r => setTimeout(r, 10));

      await taskService.updateTaskStatus(task.id, 'done');

      const currentTasks = get(tasks);
      expect(currentTasks[0]).toMatchObject({
        id: task.id,
        status: 'done',
        updated_at: expect.not.stringMatching(originalUpdatedAt),
      });

      const currentActiveTask = get(activeTask);
      expect(currentActiveTask).toMatchObject({
        id: task.id,
        status: 'done',
      });
    });
  });

  describe('loadTasks', () => {
    it('merges without overwriting just-created tasks', async () => {
      // Add a just-created task to the store
      const justCreated: Task = {
        id: 'temp-task-1',
        title: 'Just Created',
        description: '',
        status: 'todo',
        priority: 'medium',
        due_date: undefined,
        project: undefined,
        project_id: undefined,
        folder_id: undefined,
        tags: [],
        assignee: undefined,
        parent_task_id: undefined,
        sort_order: 0,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        completed_at: undefined,
        note_id: undefined,
      };
      tasks.set([justCreated]);

      // Add different tasks to mock backend
      const loadedTask: Task = {
        id: 'task_1',
        title: 'Loaded Task',
        description: '',
        status: 'todo',
        priority: 'medium',
        due_date: undefined,
        project: undefined,
        project_id: undefined,
        folder_id: undefined,
        tags: [],
        assignee: undefined,
        parent_task_id: undefined,
        sort_order: 1,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        completed_at: undefined,
        note_id: undefined,
      };
      mockData.tasks = [loadedTask];

      await taskService.loadTasks();

      const currentTasks = get(tasks);
      expect(currentTasks).toHaveLength(2);
      expect(currentTasks).toEqual(expect.arrayContaining([
        justCreated, // Should keep just-created task
        loadedTask,  // Should add loaded task
      ]));
    });

    it('filters by project when projectId provided', async () => {
      const projectTask: Task = {
        id: 'project-task-1',
        title: 'Project Task',
        description: '',
        status: 'todo',
        priority: 'medium',
        due_date: undefined,
        project: 'Test Project',
        project_id: 'project-1',
        folder_id: undefined,
        tags: [],
        assignee: undefined,
        parent_task_id: undefined,
        sort_order: 0,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
        completed_at: undefined,
        note_id: undefined,
      };
      mockData.tasks = [projectTask];

      await taskService.loadTasks({ projectId: 'project-1' });

      const currentTasks = get(tasks);
      expect(currentTasks).toHaveLength(1);
      expect(currentTasks[0]).toMatchObject({
        project_id: 'project-1',
      });
    });
  });
});
