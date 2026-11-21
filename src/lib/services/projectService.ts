// Pane v9.0.0 — Project & Folder Service

import { invoke } from '@tauri-apps/api/core';

// === Types ===

export interface Project {
  id: string;
  name: string;
  description?: string;
  color?: string;
  parent_id?: string;
  sort_order: number;
  owner_id?: string;
  created_at: string;
  updated_at: string;
  is_favorite?: boolean;
  is_system?: boolean;
  // #2874 item 9: only meaningfully set on a top-level project row.
  default_folder_id?: string | null;
}

export type Folder = Project; // Folders are projects with parent_id set

export interface CreateFolderRequest {
  project_id: string;
  name: string;
  parent_id?: string;
}

export interface UpdateProjectRequest {
  project_id: string;
  name?: string;
  description?: string;
  color?: string;
  // #2874 item 9: re-points this project's default folder; absent/undefined
  // leaves it untouched, same "don't touch" convention as the fields above.
  default_folder_id?: string;
}

// === Project Operations ===
// NOTE: no createProject wrapper here — ADR-0062 D2 made agent_id a required
// field on the backend's CreateProjectRequest (born-active first session).
// The live caller (ProjectSettings.svelte) invokes 'create_project' directly
// with agent_id from the New Project agent picker.

export async function getProjects(): Promise<Project[]> {
  return await invoke('get_projects');
}

export async function updateProject(request: UpdateProjectRequest): Promise<Project> {
  return await invoke('update_project', { request });
}

export async function deleteProject(projectId: string): Promise<void> {
  await invoke('delete_project', { projectId });
}

// === Folder Operations ===

export async function createFolder(request: CreateFolderRequest): Promise<Folder> {
  return await invoke('create_folder', { request });
}

export async function getFolders(projectId: string): Promise<Folder[]> {
  return await invoke('get_folders', { projectId });
}

export async function updateFolder(folderId: string, name: string): Promise<Folder> {
  return await invoke('update_folder', { folderId, name });
}

export async function deleteFolder(folderId: string): Promise<void> {
  await invoke('delete_folder', { folderId });
}

// === Helper Functions ===

export function isFolder(project: Project): boolean {
  return project.parent_id !== undefined && project.parent_id !== null;
}

export function isTopLevelProject(project: Project): boolean {
  return !isFolder(project);
}

export function filterTopLevelProjects(projects: Project[]): Project[] {
  return projects.filter(isTopLevelProject);
}

export function filterFolders(projects: Project[], projectId: string): Folder[] {
  return projects.filter(p => p.parent_id === projectId) as Folder[];
}

export function buildProjectTree(projects: Project[]): ProjectNode[] {
  const map = new Map<string, ProjectNode>();
  const roots: ProjectNode[] = [];

  // Create nodes
  projects.forEach(project => {
    map.set(project.id, { project, children: [] });
  });

  // Build tree
  projects.forEach(project => {
    const node = map.get(project.id)!;
    if (project.parent_id) {
      const parent = map.get(project.parent_id);
      if (parent) {
        parent.children.push(node);
      } else {
        roots.push(node);
      }
    } else {
      roots.push(node);
    }
  });

  return roots;
}

export interface ProjectNode {
  project: Project;
  children: ProjectNode[];
}
