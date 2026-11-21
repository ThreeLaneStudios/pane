import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import type { NoteMeta, NoteContent, TagInfo, NotebookInfo, Template, TrashItem, Attachment, Task, TaskStatus, TaskPriority, Project, ProjectStats, NoteSearchResult, User, ChatThread, ChatMessage, ChatMemory, NoteContextResult, CategoryWithColor, GratitudeEntry, Session, Agent, CreateLocalAgentRequest, SheetData, SheetColumn, SheetRow, SheetCell, SheetCardSummary } from '$lib/types';
// #2972 stage 3 (§34.1.2): the one shared app-sender helper every app-
// initiated mutation below dispatches the data-changed event through,
// instead of each call site hand-rolling its own bare
// `window.dispatchEvent(new CustomEvent('pane-data-changed'))` (the
// pre-Stage-3 shape, carrying no entity_id/entity_type/reason/origin_tag
// at all — every site below replaces that bare dispatch with this call).
import { dispatchDataChanged, ENTITY_NOTE, ENTITY_SHEET, ENTITY_PROJECT, ENTITY_CATEGORY, REASON_CREATED, REASON_EDITED, REASON_DELETED, type Reason } from '$lib/refreshEvent';

// Note: "notebook" in the API maps to "project" in the UI.
// We keep the backend parameter names as-is for compatibility.

/**
 * Transform raw backend note data into NoteMeta with slug and notebook fields.
 * @param backendNote - Raw note data from Rust backend
 * @param notebook - Optional notebook string (e.g., "ProjectName" or "ProjectName/FolderName")
 * @returns NoteMeta with slug set to UUID and notebook set
 */
export function transformNoteMeta(backendNote: any, notebook?: string): NoteMeta {
  return {
    id: backendNote.id,
    title: backendNote.title,
    slug: backendNote.id,
    notebook: notebook || backendNote.notebook || '',
    preview: backendNote.content_preview || '',
    content_preview: backendNote.content_preview || '',
    created_at: backendNote.created_at,
    updated_at: backendNote.updated_at,
    pinned: backendNote.pinned || false,
    word_count: backendNote.word_count || 0,
    tags: backendNote.tags || [],
    project_id: backendNote.project_id,
    folder_id: backendNote.folder_id,
    file_path: backendNote.file_path,
    // Map backend field names to frontend field names
    task_status: (() => { const s = backendNote.status || backendNote.task_status; return (s && s !== 'cabinet') ? s : undefined; })(),
    task_priority: backendNote.priority || backendNote.task_priority || undefined,
    task_due_date: backendNote.due_date || backendNote.task_due_date || undefined,
    task_assignee: backendNote.assignee || backendNote.task_assignee || undefined,
    task_completed_at: backendNote.completed_at || backendNote.task_completed_at || undefined,
    task_sort_order: backendNote.sort_order || backendNote.task_sort_order || 0,
    note_type: backendNote.note_type || undefined,
  };
}

export async function listNotebooks(): Promise<string[]> {
  const projects: any[] = await invoke('get_projects', {});
  return projects.filter(p => !p.parent_id).map(p => p.name);
}

export async function deleteNotebook(name: string): Promise<void> {
  // Resolve name to project ID first
  const projects: any[] = await invoke('get_projects', {});
  const project = projects.find(p => p.name === name && !p.parent_id);
  if (!project) throw new Error(`Project not found: ${name}`);
  await invoke('delete_project', { projectId: project.id });
}

export async function listNotes(notebook: string): Promise<NoteMeta[]> {
  // Parse notebook: "ProjectName" or "ProjectName/FolderName"
  const parts = (notebook || '').split('/');
  const projectName = parts[0];
  const folderName = parts.length > 1 ? parts[1] : null;
  
  const projects: any[] = await invoke('get_projects', {});
  const project = projects.find(p => p.name === projectName && !p.parent_id);
  if (!project) {
    return [];
  }
  
  const params: any = { projectId: project.id };
  if (folderName) {
    const folder = projects.find(p => p.name === folderName && p.parent_id === project.id);
    if (folder) {
      params.folderId = folder.id;
    }
  }
  
  console.log('[LISTNOTES] params:', { notebook, projectName, folderName, projectId: project?.id, folderId: params.folderId });
  const notes: any[] = await invoke('get_notes', params);
  console.log('[LISTNOTES] results:', notes.length, 'notes');
  return notes.map(n => transformNoteMeta(n, notebook));
}

export async function readNote(notebook: string, slug: string): Promise<NoteContent> {
  // In v9, slug IS the note ID
  const result: any = await invoke('get_note', { noteId: slug });

  // Derive notebook from authoritative project_id + folder_id in meta (#2407).
  // The passed-in `notebook` param is FALLBACK only — used when derivation is impossible.
  let derivedNotebook: string | null = null;
  if (result.meta?.project_id) {
    const allProjects: any[] = await invoke('get_projects', {});
    const projectMap = new Map(allProjects.map((p: any) => [p.id, p]));
    const project = projectMap.get(result.meta.project_id);
    if (project) {
      const folder = result.meta.folder_id ? projectMap.get(result.meta.folder_id) : null;
      derivedNotebook = folder ? `${project.name}/${folder.name}` : project.name;
    }
  }
  if (derivedNotebook === null) {
    console.warn('[api.readNote] Could not derive notebook from meta, falling back to passed-in value:', {
      noteId: slug,
      passedNotebook: notebook,
      projectId: result.meta?.project_id,
      folderId: result.meta?.folder_id,
    });
  }

  // Transform meta for frontend compatibility
  return {
    ...result,
    meta: {
      ...result.meta,
      slug: result.meta.id,  // CRITICAL: slug = UUID
      notebook: derivedNotebook ?? notebook ?? '',
      task_status: result.meta.status,
      task_priority: result.meta.priority,
      task_due_date: result.meta.due_date,
      task_assignee: result.meta.assignee,
      task_completed_at: result.meta.completed_at,
    }
  };
}

export async function saveNote(notebook: string, slug: string, content: string, title?: string, originTag?: string): Promise<NoteMeta> {
  // In v9, slug IS the note ID (UUID). Use update_note command.
  const noteId = slug;
  console.log('[SAVE] saveNote called:', { noteId, contentLength: content?.length, titlePassed: !!title, notebook });
  
  
  const request: any = {
    note_id: noteId,
    content,
  };
  if (title) {
    request.title = title;
  }
  try {
    const result: any = await invoke('update_note', { request });
    console.log('[SAVE] saveNote success:', noteId);
    // #2972 stage 3 (§34.1.2): full entity-identified payload — origin_tag
    // forwarded from the caller when supplied (Editor.svelte registers one
    // before calling this, §34.4) so the saving editor's own echo is
    // suppressible; a fresh tag is minted when no caller cares (every
    // other site's own save still updates normally, just not self-aware).
    dispatchDataChanged(noteId, ENTITY_NOTE, REASON_EDITED, originTag);
    return result;
  } catch (e) {
    console.error('[SAVE] saveNote FAILED:', e);
    throw e;
  }
}


export async function saveSheetTitle(notebook: string, slug: string, title: string, originTag?: string): Promise<NoteMeta> {
  // Sheet title save: content field is INTENTIONALLY omitted. Sheets
  // derive body server-side (regenerate_sheet_body, sheets.rs) — sending
  // any content value here (even omitted-but-falsy) would risk tripping
  // the #2901 sheet-body guard (notes.rs:1520-1528) if a future change
  // widens that guard to check field presence rather than a truthy
  // string. Per Matt's ruling (#2874 item 17), the guard stays as-is;
  // this function's contract is: never construct a request object with
  // a `content` key for a sheet-title save.
  const noteId = slug;
  console.log('[SAVE] saveSheetTitle called:', { noteId, title, notebook });
  const request: any = { note_id: noteId, title };
  try {
    const result: any = await invoke('update_note', { request });
    console.log('[SAVE] saveSheetTitle success:', noteId);
    // #2972 stage 3 (§34.1.2) — see saveNote()'s identical comment above.
    dispatchDataChanged(noteId, ENTITY_SHEET, REASON_EDITED, originTag);
    return result;
  } catch (e) {
    console.error('[SAVE] saveSheetTitle FAILED:', e);
    throw e;
  }
}


export async function createNote(notebook: string, title: string): Promise<NoteContent> {
  // Resolve notebook name to project ID and optional folder ID
  // notebook can be "ProjectName" or "ProjectName/FolderName"
  const parts = (notebook || '').split('/');
  const projectName = parts[0];
  const folderName = parts.length > 1 ? parts[1] : null;
  
  const projects: any[] = await invoke('get_projects', {});
  const project = projects.find(p => p.name === projectName && !p.parent_id);
  if (!project) throw new Error(`Project not found: ${projectName}`);
  
  const request: any = {
    title: title || 'Untitled',
    project_id: project.id,
    status: 'cabinet',
  };
  
  // Resolve folder if specified
  if (folderName) {
    const folder = projects.find(p => p.name === folderName && p.parent_id === project.id);
    if (folder) {
      request.folder_id = folder.id;
    } else {
      console.warn('[api.createNote] folder lookup miss — folder_id not in projectMap', {
        folderId: folderName,
        projectMapSize: projects.length,
      });
    }
  }
  
  console.log('[CREATE] createNote:', { notebook, projectName, folderName, projectId: project.id, folderId: request.folder_id });
  
  const rawMeta: any = await invoke('create_note', { request });
  // #2972 stage 3 (§34.1.2): migrated from the bare, payload-less
  // `pane-note-created` CustomEvent to the single entity-identified event,
  // `reason: "created"`.
  dispatchDataChanged(rawMeta.id, ENTITY_NOTE, REASON_CREATED);
  const meta = transformNoteMeta(rawMeta, notebook);
  
  // Wrap as NoteContent for noteService compatibility
  return {
    meta,
    content: ''
  };
}

export async function createNoteInProject(
  projectId: string,
  folderId?: string,
  title?: string,
  status?: string,
  options?: {
    priority?: TaskPriority;
    dueDate?: string;
    assignee?: string;
    parentId?: string;
    tags?: string[];
    content?: string;
  }
): Promise<NoteMeta> {
  const request: any = {
    title: title || 'Untitled',
    project_id: projectId,
    status: status || 'cabinet'
  };
  if (folderId) request.folder_id = folderId;
  if (options?.priority) request.priority = options.priority;
  if (options?.dueDate) request.due_date = options.dueDate;
  if (options?.assignee) request.assignee = options.assignee;
  if (options?.parentId) request.parent_id = options.parentId;
  if (options?.tags?.length) request.tags = options.tags;
  if (options?.content) request.content = options.content;
  const rawMeta: any = await invoke('create_note', { request });
  // #2972 stage 3 (§34.1.2) — see createNote()'s identical comment above.
  dispatchDataChanged(rawMeta.id, ENTITY_NOTE, REASON_CREATED);
  
  // Get project/folder names for notebook field
  const projects: any[] = await invoke('get_projects', {});
  const project = projects.find(p => p.id === projectId);
  let notebook = project?.name || '';
  if (folderId) {
    const folder = projects.find(p => p.id === folderId);
    if (folder) {
      notebook = `${notebook}/${folder.name}`;
    } else {
      console.warn('[api.createNoteInProject] folder lookup miss — folder_id not in projectMap', {
        folderId,
        projectMapSize: projects.length,
      });
    }
  }
  
  return transformNoteMeta(rawMeta, notebook);
}

export async function deleteNote(notebook: string, slug: string): Promise<void> {
  // In v9, slug IS the note ID
  return invoke('delete_note', { noteId: slug });
}

export async function searchNotes(query: string): Promise<NoteMeta[]> {
  const notes: any[] = await invoke('search_notes', { request: { query } });
  // search_notes (Rust `SearchResult`) serializes the identifier as `note_id`,
  // not `id` — transformNoteMeta expects `.id` (NoteMeta-shaped rows, e.g. from
  // get_notes). Remap before transforming so id/slug are defined. Donor pattern:
  // QuickSwitcher.svelte:67-68 (`id: r.note_id, slug: r.note_id`). Narrow fix
  // scoped to searchNotes only — transformNoteMeta stays untouched. #2419
  return notes.map(n => transformNoteMeta({ ...n, id: n.note_id }));
}

// Tag functions
export async function listTags(): Promise<TagInfo[]> {
  return invoke('get_tags');
}

export async function addTag(noteId: string, tagName: string): Promise<void> {
  return invoke('add_note_tags', { noteId, tags: [tagName] });
}

export async function removeTag(noteId: string, tagName: string): Promise<void> {
  return invoke('remove_note_tags', { noteId, tags: [tagName] });
}

export async function createTag(name: string, color?: string): Promise<TagInfo> {
  return invoke('create_tag', { name, color: color ?? null });
}

export async function getNotesByTag(tagName: string): Promise<NoteMeta[]> {
  const notes: any[] = await invoke('get_notes_by_tag', { tagName });
  return notes.map(n => transformNoteMeta(n));
}



export async function getNoteBacklinks(targetTitle: string): Promise<NoteMeta[]> {
  console.debug('getNoteBacklinks: not implemented in v9');
  return [];
}

export async function getAllNoteTitles(): Promise<string[]> {
  console.debug('getAllNoteTitles: not implemented in v9');
  return [];
}


/**
 * Raw get_note invoke — returns backend shape without meta transformation.
 * Used by navigation layer to resolve project_id when navigating to note_editor without a projectId.
 */
export async function getNoteRaw(noteId: string): Promise<{ meta: { project_id: string }; content: string }> {
  return invoke('get_note', { noteId });
}

export async function uploadAttachment(filename: string, data: string, projectId?: string): Promise<Attachment> {
  const result: any = await invoke('save_attachment', {
    projectId: projectId || '__global__',
    filename,
    mimeType: guessMimeType(filename),
    data,
  });
  return {
    id: result.id,
    filename: result.original_filename,
    mime_type: result.mime_type,
    path: result.path,
    size: result.size_bytes,
    created_at: result.created_at || '',
  };
}

/**
 * Copy an attachment from an absolute source path on disk to the project's Assets directory,
 * bypassing the base64/IPC pipeline entirely. Preferred for large files (> 50 MB).
 * Requires Tauri — `sourcePath` must be an absolute filesystem path exposed by Tauri's File API.
 */
export async function copyAttachmentFromPath(
  filename: string,
  sourcePath: string,
  projectId?: string,
): Promise<Attachment> {
  const result: any = await invoke('copy_attachment_from_path', {
    projectId: projectId || '__global__',
    sourcePath,
    filename,
    mimeType: guessMimeType(filename),
  });
  return {
    id: result.id,
    filename: result.original_filename,
    mime_type: result.mime_type,
    path: result.path,
    size: result.size_bytes,
    created_at: result.created_at || '',
  };
}

export async function getAttachment(attachmentId: string): Promise<any> {
  return await invoke('get_attachment', { attachmentId });
}

export async function getAttachmentData(attachmentId: string): Promise<any> {
  return await invoke('get_attachment_data', { attachmentId });
}

export async function deleteAttachment(attachmentId: string): Promise<void> {
  await invoke('delete_attachment', { attachmentId });
}

export async function listProjectAttachments(projectId: string): Promise<any[]> {
  return await invoke('list_project_attachments', { projectId });
}

function guessMimeType(filename: string): string {
  const ext = filename.split(".").pop()?.toLowerCase() || "";
  const map: Record<string, string> = {
    png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif",
    webp: "image/webp", svg: "image/svg+xml", bmp: "image/bmp",
    mp3: "audio/mpeg", wav: "audio/wav", ogg: "audio/ogg", m4a: "audio/mp4",
    mp4: "video/mp4", webm: "video/webm", mov: "video/quicktime",
    pdf: "application/pdf", txt: "text/plain", md: "text/markdown",
    json: "application/json", csv: "text/csv", html: "text/html",
    // #3050 PR 9, §36.9.3 — read_attachment's docx extractor dispatches on
    // file extension, not this mime type (this map had no docx entry at
    // all before — every uploaded .docx silently fell back to
    // "application/octet-stream"), but a correct mime type is still worth
    // having for display/future use.
    docx: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  };
  return map[ext] || "application/octet-stream";
}

// spec-covered-by: #2121 (§32, ADR-0058)
export async function listTemplates(): Promise<Template[]> {
  return invoke('list_templates', {});
}

export async function saveTemplate(
  name: string,
  content: string,
  checklistJson: string
): Promise<Template> {
  return invoke('save_template', { name, content, checklistJson });
}

export async function overwriteTemplate(
  id: string,
  content: string,
  checklistJson: string
): Promise<Template> {
  return invoke('overwrite_template', { id, content, checklistJson, updatedAt: new Date().toISOString() });
}

export async function renameTemplate(id: string, name: string): Promise<Template> {
  return invoke('rename_template', { id, name });
}

export async function deleteTemplate(id: string): Promise<void> {
  return invoke('delete_template', { id });
}

export async function applyTemplate(noteId: string, templateId: string): Promise<void> {
  return invoke('apply_template', { noteId, templateId });
}

export async function listTemplatesByName(name: string): Promise<Template[]> {
  return invoke('list_templates_by_name', { name });
}

export async function createNoteFromTemplate(
  notebook: string,
  templateId: string,
  title?: string
): Promise<NoteContent> {
  throw new Error('createNoteFromTemplate is not implemented (§32.6)');
}

export async function listNotebooksNested(): Promise<NotebookInfo[]> {
  const projects: any[] = await invoke('get_projects', {});
  return projects.map(p => ({
    name: p.name,
    path: p.name,
    parent: p.parent_id ? projects.find(pp => pp.id === p.parent_id)?.name : undefined,
  }));
}

export async function moveToTrash(noteId: string, originTag?: string): Promise<void> {
  await invoke('delete_note', { noteId });
  // #2972 stage 3 (§34.1.2) — see saveNote()'s identical comment above.
  dispatchDataChanged(noteId, ENTITY_NOTE, REASON_DELETED, originTag);
}

export async function listTrash(): Promise<TrashItem[]> {
  const items: any[] = await invoke('get_trash');
  return items.map(item => ({
    id: item.id,
    title: item.title || '',
    notebook: item.project_name || '',
    slug: item.id,
    deleted_at: item.deleted_at || '',
    original_path: item.file_path || '',
  }));
}

// #2874 item 1, Matt's ruling 5: the backend's `restore_note` command now
// returns `{ meta, content_warning }` — `content_warning` is set whenever
// the note's file was missing at restore time (either rebuilt from a saved
// `note_versions` snapshot, or lost entirely with no snapshot to recover
// from). This must reach the UI as an unambiguous message, never a silent
// success — see `Trash.svelte`'s `handleRestore`.
export async function restoreFromTrash(noteId: string): Promise<{ contentWarning?: string }> {
  const result = await invoke<{ meta: unknown; content_warning?: string | null }>('restore_note', { noteId });
  return { contentWarning: result.content_warning ?? undefined };
}

export async function deletePermanently(noteId: string): Promise<void> {
  return invoke('purge_note', { noteId });
}

// #2874 item 2 (#3037 follow-up, Matt's ruling): `empty_trash` now returns
// `{ purged, failed }` instead of a bare count — an item whose file
// couldn't be moved to the OS trash stays in Pane's own Trash rather than
// having its row silently erased, and the caller needs both numbers to
// tell the user how many actually emptied vs. how many didn't.
export async function emptyTrash(): Promise<{ purged: number; failed: number }> {
  return invoke('empty_trash');
}

export async function exportNoteMarkdown(notebook: string, slug: string): Promise<string> {
  console.debug('exportNoteMarkdown: not implemented in v9');
  // Fallback: get note content
  const note: any = await invoke('get_note', { noteId: slug });
  return note.content || '';
}

export async function quickCaptureNote(content: string): Promise<NoteMeta> {
  // Use create_note with default project
  const projects: any[] = await invoke('get_projects', {});
  const defaultProject = projects.find(p => !p.parent_id) || projects[0];
  if (!defaultProject) throw new Error('No projects available');
  
  const request: any = {
    title: content.substring(0, 50),
    project_id: defaultProject.id,
    status: 'cabinet',
    content,
  };
  const rawMeta: any = await invoke('create_note', { request });
  // #2972 stage 3 (§34.1.2) — see createNote()'s identical comment above.
  dispatchDataChanged(rawMeta.id, ENTITY_NOTE, REASON_CREATED);
  return transformNoteMeta(rawMeta, defaultProject.name);
}

/**
 * Convert a NoteMeta (returned by createNoteInProject) to the legacy Task shape
 * expected by task-oriented UI callbacks.
 */
export function noteMetaToTask(note: NoteMeta, overrides?: Partial<Task>): Task {
  return {
    id: note.id,
    title: note.title,
    description: note.preview || '',
    status: (note.task_status || 'todo') as TaskStatus,
    priority: (note.task_priority || 'medium') as TaskPriority,
    due_date: note.task_due_date,
    project: note.notebook?.split('/')[0] || '',
    project_id: note.project_id,
    folder_id: note.folder_id,
    tags: note.tags || [],
    assignee: note.task_assignee,
    sort_order: note.task_sort_order || 0,
    created_at: note.created_at,
    updated_at: note.updated_at,
    completed_at: note.task_completed_at,
    ...overrides,
  };
}

export async function updateTask(task: Task): Promise<Task> {
  console.debug('updateTask: using update_note in v9');
  const request: any = {
    note_id: task.id,
    title: task.title,
    content: task.description,
    status: task.status,
    priority: task.priority,
    due_date: task.due_date,
    assignee: task.assignee,
  };
  await invoke('update_note', { request });
  return task;
}

export async function deleteTask(taskId: string): Promise<void> {
  console.debug('deleteTask: using delete_note in v9');
  return invoke('delete_note', { noteId: taskId });
}

export async function getTask(taskId: string): Promise<Task | null> {
  console.debug('getTask: using get_note in v9');
  try {
    const note: any = await invoke('get_note', { noteId: taskId });
    return {
      id: note.id,
      title: note.title,
      description: note.content,
      status: note.status,
      priority: note.priority,
      due_date: note.due_date,
      assignee: note.assignee,
      tags: note.tags || [],
      sort_order: 0,
      created_at: note.created_at,
      updated_at: note.updated_at,
    } as Task;
  } catch {
    return null;
  }
}

export async function listTasks(
  status?: TaskStatus,
  project?: string,
  assignee?: string,
  limit?: number,
  offset?: number
): Promise<Task[]> {
  console.debug('listTasks: using get_tasks in v9');
  // Resolve project name to ID if provided
  let projectId = null;
  if (project) {
    const projects: any[] = await invoke('get_projects', {});
    const proj = projects.find(p => p.name === project);
    projectId = proj?.id;
  }
  
  const params: any = {};
  if (projectId) params.projectId = projectId;
  if (status) params.status = status;
  
  const notes: any[] = await invoke('get_tasks', params);
  const transformed = notes.map(n => transformNoteMeta(n));
  
  return transformed.map(n => ({
    id: n.id,
    title: n.title,
    description: n.content_preview || '',
    status: n.task_status as TaskStatus || 'todo',
    priority: n.task_priority as TaskPriority || 'medium',
    due_date: n.task_due_date,
    assignee: n.task_assignee,
    tags: [],
    sort_order: n.task_sort_order || 0,
    created_at: n.created_at,
    updated_at: n.updated_at,
    project_id: n.project_id,
    folder_id: n.folder_id,
    note_type: n.note_type || undefined,
  } as Task));
}

export async function searchTasksFts(query: string, limit?: number): Promise<Task[]> {
  console.debug('searchTasksFts: not implemented in v9, using search_notes');
  const notes: any[] = await invoke('search_notes', { request: { query } });
  const transformed = notes.map(n => transformNoteMeta(n));
  return transformed.filter(n => n.task_status !== undefined).map(n => ({
    id: n.id,
    title: n.title,
    description: n.content_preview || '',
    status: n.task_status as TaskStatus || 'todo',
    priority: n.task_priority as TaskPriority || 'medium',
    due_date: n.task_due_date,
    assignee: n.task_assignee,
    tags: [],
    sort_order: n.task_sort_order || 0,
    created_at: n.created_at,
    updated_at: n.updated_at,
    project_id: n.project_id,
    folder_id: n.folder_id,
  } as Task));
}

export async function getTasksToday(): Promise<Task[]> {
  const notes: any[] = await invoke('list_upcoming_tasks');
  const transformed = notes.map(n => transformNoteMeta(n));
  return transformed.map(n => ({
    id: n.id,
    title: n.title,
    description: n.content_preview || '',
    status: n.task_status as TaskStatus || 'todo',
    priority: n.task_priority as TaskPriority || 'medium',
    due_date: n.task_due_date,
    assignee: n.task_assignee,
    tags: [],
    sort_order: 0,
    created_at: n.created_at,
    updated_at: n.updated_at,
    project_id: n.project_id,
    folder_id: n.folder_id,
  } as Task));
}

export async function updateTaskStatus(taskId: string, status: TaskStatus): Promise<void> {
  console.debug('updateTaskStatus: using update_note in v9');
  return invoke('update_note', { request: { note_id: taskId, status } });
}

// Move note between notebooks/projects
export async function moveNote(notebook: string, slug: string, targetNotebook: string): Promise<NoteMeta> {
  // In v9, slug IS the note ID. Parse targetNotebook to get project_id and folder_id
  const noteId = slug;
  
  // Parse targetNotebook: "ProjectName" or "ProjectName/FolderName"
  const parts = targetNotebook.split('/');
  const projectName = parts[0];
  const folderName = parts.length > 1 ? parts[1] : null;
  
  // Get all projects to find IDs
  const allProjects: Project[] = await invoke('get_projects', {});
  
  // Find project
  const project = allProjects.find(p => p.name === projectName && !p.parent_id);
  if (!project) {
    throw new Error(`Project not found: ${projectName}`);
  }
  
  let folderId: string | undefined = undefined;
  if (folderName) {
    const folder = allProjects.find(p => p.name === folderName && p.parent_id === project.id);
    if (!folder) {
      throw new Error(`Folder not found: ${folderName}`);
    }
    folderId = folder.id;
  }
  
  // Use update_note to move the note
  const request: any = {
    note_id: noteId,
    project_id: project.id,
  };
  if (folderId) {
    request.folder_id = folderId;
  }
  
  const result: any = await invoke('update_note', { request });
  return result;
}

// Get attachment file path
export async function getAttachmentPath(attachmentId: string): Promise<string> {
  console.debug('getAttachmentPath: not implemented in v9');
  return '';
}

export async function getAttachmentBase64(attachmentId: string): Promise<string> {
  try {
    const result: any = await invoke('get_attachment_data', { attachmentId });
    // Backend returns { id, mime_type, original_filename, data }
    // where data is base64 encoded
    const dataUrl = `data:${result.mime_type};base64,${result.data}`;
    return dataUrl;
  } catch (e) {
    console.error('Failed to get attachment data:', e);
    return '';
  }
}

/**
 * Open an attachment in the OS default application for that file type.
 *
 * Delegates to `open_attachment_externally` Tauri command (#401 PR1):
 * resolves the attachment path from DB, then calls the OS shell open
 * (macOS: `open`, Windows: `start`, Linux: `xdg-open`).
 *
 * @param attachmentId - The attachment UUID
 * @throws if the attachment is not found or the OS refuses to open the file
 */
export async function openAttachmentExternally(attachmentId: string): Promise<void> {
  await invoke('open_attachment_externally', { attachmentId });
}

/**
 * Resolve an attachment to a loadable asset:// URL via Tauri's asset protocol.
 *
 * Wired into the render path via `addNodeView()` (AttachmentNodeView, #1777 PR2b).
 * Returns a runtime src URL that the WebView loads directly via the asset: protocol
 * without base64 encoding.
 *
 * @param attachmentId - The attachment UUID
 * @returns A `asset://localhost/...` URL safe to use as img/video/audio src,
 *          or empty string on failure.
 */
export async function resolveAttachmentSrc(attachmentId: string): Promise<string> {
  try {
    const absolutePath: string = await invoke('resolve_attachment_path', { attachmentId });
    return convertFileSrc(absolutePath);
  } catch (e) {
    console.error('[resolveAttachmentSrc] Failed to resolve attachment path:', e);
    return '';
  }
}

// Distinct projects and assignees for autocomplete
export async function listProjectsDistinct(): Promise<string[]> {
  // v9: no distinct command. Get project names from get_projects.
  const projects: any[] = await invoke('get_projects', {});
  return [...new Set(projects.filter(p => !p.parent_id).map(p => p.name))];
}

export async function listAssigneesDistinct(): Promise<string[]> {
  // v9: no distinct assignee command. Return empty for now.
  return [];
}

// Projects CRUD
// NOTE: no createProject wrapper here — ADR-0062 D2 made agent_id a required
// field on CreateProjectRequest (born-active first session). The one live
// caller (ProjectSettings.svelte) builds the request object and invokes
// 'create_project' directly so it can include agent_id from the New Project
// agent picker; a thin wrapper without that field would silently regress
// project creation to a null/self-heal-pending session.

export async function listProjects(): Promise<Project[]> {
  const projects: any[] = await invoke('get_projects', {});
  return projects.map(p => ({
    id: p.id,
    name: p.name,
    description: p.description,
    color: p.color,
    parent_id: p.parent_id,
    sort_order: p.sort_order || 0,
    created_at: p.created_at,
    updated_at: p.updated_at,
    category: p.category || undefined,
    is_system: p.is_system || false,
    is_favorite: p.is_favorite ?? false,
    default_folder_id: p.default_folder_id ?? null
  }));
}

export async function getProjectNoteCounts(): Promise<Map<string, number>> {
  const counts: [string, number][] = await invoke('get_project_note_counts', {});
  return new Map(counts);
}

export async function deleteProject(projectId: string): Promise<void> {
  return invoke('delete_project', { projectId });
}

export async function syncProjectsWithNotebooks(): Promise<Project[]> {
  // v9: sync_projects_with_notebooks doesn't exist. Use get_projects instead.
  const projects: any[] = await invoke('get_projects', {});
  return projects.map(p => ({
    id: p.id,
    name: p.name,
    description: p.description,
    color: p.color,
    parent_id: p.parent_id,
    sort_order: p.sort_order || 0,
    created_at: p.created_at,
    updated_at: p.updated_at,
    category: p.category || undefined,
    is_system: p.is_system || false,
    is_favorite: p.is_favorite ?? false,
    default_folder_id: p.default_folder_id ?? null
  }));
}

export async function reorderProjects(projectIds: string[]): Promise<void> {
  console.debug('reorderProjects: not implemented in v9');
}

export async function getProjectStats(projectName: string): Promise<ProjectStats> {
  console.debug('getProjectStats: not implemented in v9');
  return {
    project_name: projectName,
    note_count: 0,
    task_count: 0,
  };
}

export async function moveProject(projectId: string, parentId?: string): Promise<Project> {
  console.debug('moveProject: not implemented in v9');
  // Fallback: return unchanged
  const projects: any[] = await invoke('get_projects', {});
  const project = projects.find(p => p.id === projectId);
  return project;
}

// Wiki Links functions
//
// #2419 round-2 delta-audit fix (2026-08-05, Reed's audit — Option 3): PR
// #2435 originally built `title:"word"*` FTS5 syntax HERE, frontend-side, and
// sent the pre-built string as `request.query`. That shipped a
// launch-blocking regression: `search_notes_internal`'s general-purpose
// re-quote path (notes.rs) unconditionally wraps every incoming query in an
// extra layer of quoting for injection-safety, and it doesn't know how to
// leave already-FTS5-syntax strings alone — `title:"De"*` got re-wrapped into
// the literal phrase `"title:De*"`, destroying both column-targeting and
// prefix matching. Every wiki-link-picker query returned zero results.
//
// Fix: query construction moves entirely server-side
// (`search_notes_internal`, notes.rs), gated by a `title_prefix` flag on
// `SearchNotesRequest`. This function now sends the RAW query string plus
// `title_prefix: true` — no FTS5 syntax is ever built frontend-side. There is
// exactly one place in the codebase that constructs the final MATCH string.
//
// The Tauri command (`search_notes`) itself still routes through
// `search_notes_internal` unchanged for every OTHER caller — `api.searchNotes`
// and QuickSwitcher never set `title_prefix`, so the flag defaults to
// false/absent there and the pre-existing general re-quote path runs
// byte-identical to before.
//
// searchNotesByTitle has exactly ONE consumer: WikiLink.ts:36. Does not affect
// QuickSwitcher or api.searchNotes, which both keep full-content semantics.
export async function searchNotesByTitle(query: string): Promise<NoteSearchResult[]> {
  if (!query || query.trim().length === 0) return [];
  return invoke('search_notes', { request: { query, title_prefix: true } });
}

export async function rebuildNoteLinks(noteId: string): Promise<void> {
  console.debug('rebuildNoteLinks: not implemented in v9');
}

export async function getBacklinks(noteId: string): Promise<NoteSearchResult[]> {
  console.debug('getBacklinks: not implemented in v9');
  return [];
}

export async function createNoteFromLink(title: string, projectId: string): Promise<NoteContent> {
  console.debug('createNoteFromLink: not implemented in v9');
  // Fallback: create regular note
  const request: any = {
    title,
    project_id: projectId,
    status: 'cabinet',
  };
  const rawMeta: any = await invoke('create_note', { request });
  // #2972 stage 3 (§34.1.2) — see createNote()'s identical comment above.
  dispatchDataChanged(rawMeta.id, ENTITY_NOTE, REASON_CREATED);
  
  // Get project name for notebook field
  const projects: any[] = await invoke('get_projects', {});
  const project = projects.find(p => p.id === projectId);
  const notebook = project?.name || '';
  
  return {
    meta: transformNoteMeta(rawMeta, notebook),
    content: ''
  };
}

// Tag management
export async function deleteTag(tagId: number): Promise<void> {
  return invoke('delete_tag', { tagId });
}

export async function updateTag(tagId: number, name: string, color: string): Promise<void> {
  return invoke('update_tag', { tagId, name, color });
}

// Task queries by folder/project
export async function listTasksByFolder(folderId: string): Promise<Task[]> {
  console.debug('listTasksByFolder: using get_tasks in v9');
  const notes: any[] = await invoke('get_tasks', { projectId: folderId });
  const transformed = notes.map(n => transformNoteMeta(n));
  return transformed.map(n => ({
    id: n.id,
    title: n.title,
    description: n.content_preview || '',
    status: n.task_status as TaskStatus || 'todo',
    priority: n.task_priority as TaskPriority || 'medium',
    due_date: n.task_due_date,
    assignee: n.task_assignee,
    tags: [],
    sort_order: n.task_sort_order || 0,
    created_at: n.created_at,
    updated_at: n.updated_at,
    project_id: n.project_id,
    folder_id: n.folder_id,
  } as Task));
}

export async function listTasksByNote(noteId: string): Promise<Task[]> {
  console.debug('listTasksByNote: not implemented in v9');
  return [];
}

export async function listTasksByProject(projectId: string): Promise<Task[]> {
  console.debug('listTasksByProject: using get_tasks in v9');
  const notes: any[] = await invoke('get_tasks', { projectId });
  const transformed = notes.map(n => transformNoteMeta(n));
  return transformed.map(n => ({
    id: n.id,
    title: n.title,
    description: n.content_preview || '',
    status: n.task_status as TaskStatus || 'todo',
    priority: n.task_priority as TaskPriority || 'medium',
    due_date: n.task_due_date,
    assignee: n.task_assignee,
    tags: [],
    sort_order: n.task_sort_order || 0,
    created_at: n.created_at,
    updated_at: n.updated_at,
    project_id: n.project_id,
    folder_id: n.folder_id,
  } as Task));
}

/**
 * Kanban-only data source: loads ALL non-deleted notes for a project (including
 * cabinet status) via `get_project_notes_for_kanban`. Cabinet notes are
 * preserved as `status: 'cabinet'` — no `|| 'todo'` fallback is applied on
 * this path. Do NOT use for All-Tasks views or task counts.
 */
export async function listNotesForKanban(projectId: string): Promise<Task[]> {
  const notes: any[] = await invoke('get_project_notes_for_kanban', { projectId });
  return notes.map(n => {
    // Determine status: preserve 'cabinet' explicitly; fallback to 'cabinet'
    // (not 'todo') for notes with no status, since new notes default to cabinet.
    const rawStatus = n.status || n.task_status;
    const status: TaskStatus = (rawStatus as TaskStatus) || 'cabinet';
    return {
      id: n.id,
      title: n.title,
      description: n.content_preview || '',
      status,
      priority: (n.priority || n.task_priority || 'medium') as TaskPriority,
      due_date: n.due_date || n.task_due_date,
      assignee: n.assignee || n.task_assignee,
      tags: [],
      sort_order: n.sort_order || n.task_sort_order || 0,
      created_at: n.created_at,
      updated_at: n.updated_at,
      project_id: n.project_id,
      folder_id: n.folder_id,
      note_type: n.note_type || undefined,
    } as Task;
  });
}

// Note queries by folder/project
// Ensure notes from DB queries have slug populated (slug = UUID, not filename)
function ensureSlugs(notes: NoteMeta[]): NoteMeta[] {
  return notes.map(n => {
    if (!n.slug && n.id) {
      // CRITICAL: slug MUST be the UUID (same as id) for saveNote to work
      n.slug = n.id;
    }
    if (!n.notebook) {
      // Derive notebook from project/folder if available
      n.notebook = n.notebook || '';
    }
    return n;
  });
}

export async function listNotesByFolder(folderId: string, projectId?: string): Promise<NoteMeta[]> {
  const params: any = { folderId };
  if (projectId) params.projectId = projectId;
  const notes: any[] = await invoke('get_notes', params);

  // Build notebook string so breadcrumb renders correctly in card views.
  // get_projects returns both projects and folders (folders have parent_id set).
  let notebookPrefix = '';
  if (projectId) {
    const allProjects: any[] = await invoke('get_projects', {});
    const projectMap = new Map(allProjects.map((p: any) => [p.id, p]));
    const project = projectMap.get(projectId);
    if (project) {
      const folder = projectMap.get(folderId);
      if (folder) {
        notebookPrefix = `${project.name}/${folder.name}`;
      } else {
        console.warn('[api.listNotesByFolder] folder lookup miss — folder_id not in projectMap', {
          folderId,
          projectMapSize: projectMap.size,
        });
        notebookPrefix = project.name;
      }
    }
  }

  return notes.map(n => transformNoteMeta(n, notebookPrefix || undefined));
}

export async function listNotesByProject(projectId: string): Promise<NoteMeta[]> {
  const notes: any[] = await invoke('get_notes', { projectId });
  // Get all projects (includes folders as child projects) to build full notebook paths.
  const allProjects: any[] = await invoke('get_projects', {});
  const projectMap = new Map(allProjects.map((p: any) => [p.id, p]));
  const project = projectMap.get(projectId);
  const projectName = project?.name || '';

  return notes.map(n => {
    // If the note is in a subfolder, include folder name: "ProjectName/FolderName"
    let notebook = projectName;
    if (n.folder_id && projectName) {
      const folder = projectMap.get(n.folder_id);
      if (folder) {
        notebook = `${projectName}/${folder.name}`;
      } else {
        console.warn('[api.listNotesByProject] folder lookup miss — folder_id not in projectMap', {
          folderId: n.folder_id,
          projectMapSize: projectMap.size,
          noteId: n.id,
        });
      }
    }
    return transformNoteMeta(n, notebook);
  });
}

// Folder operations (folders are projects with parent_id)
export async function listFolders(projectId: string): Promise<Project[]> {
  return invoke('get_folders', { projectId });
}

// #2972 stage 2 (§34.5 WARN closure, PR #3016): single-entity-by-id read for
// a project OR a folder (a folder is a row in the same `projects` table—
// backend's `get_project_or_folder` command). Lets Sidebar, CalendarView and
// NotesBrowser patch the one changed project/folder in place on a
// `pane-data-changed` event instead of calling `get_projects()`/
// `syncProjectsWithNotebooks()`/`listFolders()` to refetch a whole list.
// Mapping mirrors `listProjects()`/`syncProjectsWithNotebooks()` field-for-
// field, so a patched entity has the identical shape a list fetch would
// have produced for the same row.
export async function getProjectOrFolder(id: string): Promise<Project> {
  const p: any = await invoke('get_project_or_folder', { id });
  return {
    id: p.id,
    name: p.name,
    description: p.description,
    color: p.color,
    parent_id: p.parent_id,
    sort_order: p.sort_order || 0,
    created_at: p.created_at,
    updated_at: p.updated_at,
    category: p.category || undefined,
    is_system: p.is_system || false,
    is_favorite: p.is_favorite ?? false,
    default_folder_id: p.default_folder_id ?? null
  };
}

// #2972 stage 3 (§34.1.4/item 5): the backend `create_folder` command
// (projects.rs) already emits the single data-changed event itself
// (unconditionally, since Stage 1) — this wrapper deliberately dispatches
// NOTHING on the app side. A second, app-side dispatch here would be a
// duplicate of the backend's own emit for the exact same mutation,
// violating §34.1's "one event per change" (every mutating command emits
// exactly once, from exactly one side).
export async function createFolder(projectId: string, name: string, description?: string, color?: string): Promise<Project> {
  return invoke('create_folder', { request: { project_id: projectId, name, parent_id: null } });
}

export async function updateFolder(folderId: string, name: string): Promise<Project> {
  return invoke('update_folder', { folderId, name });
}

export async function deleteFolder(folderId: string): Promise<void> {
  return invoke('delete_folder', { folderId });
}

// #2874 item 65 ruling 4: the delete-folder confirmation dialog must say
// where content goes (counts + the default folder's REAL, current name),
// read fresh at display time — never cached, never hardcoded ("General").
export interface FolderDeletePreview {
  note_count: number;
  sheet_count: number;
  default_folder_name: string;
}

export async function getFolderDeletePreview(folderId: string): Promise<FolderDeletePreview> {
  return invoke('get_folder_delete_preview', { folderId });
}

export async function toggleFolderHidden(folderId: string, hidden: boolean): Promise<void> {
  return invoke('toggle_folder_hidden', { folderId, hidden });
}

export async function getHiddenFolderIds(): Promise<string[]> {
  return invoke('get_hidden_folder_ids') as Promise<string[]>;
}

// User functions
export async function listUsers(): Promise<User[]> {
  console.debug('listUsers: not implemented in v9');
  return [];
}

export async function createUser(name: string, email?: string): Promise<User> {
  console.debug('createUser: not implemented in v9');
  return { id: '', name, email: email || '' } as User;
}

// Archive functions
export async function archiveTask(taskId: string): Promise<void> {
  console.debug('archiveTask: not implemented in v9');
}

export async function unarchiveTask(taskId: string): Promise<void> {
  console.debug('unarchiveTask: not implemented in v9');
}

export async function archiveProject(projectId: string): Promise<void> {
  console.debug('archiveProject: not implemented in v9');
}

export async function unarchiveProject(projectId: string): Promise<void> {
  console.debug('unarchiveProject: not implemented in v9');
}

export async function listArchivedTasks(): Promise<Task[]> {
  console.debug('listArchivedTasks: not implemented in v9');
  return [];
}

export async function listArchivedProjects(): Promise<Project[]> {
  console.debug('listArchivedProjects: not implemented in v9');
  return [];
}

// Chat functions

// === Session Commands (§30.2, #395) ===

/**
 * Create a new session for a surface and make it active. Returns the full
 * Session record.
 *
 * ADR-0062 D1/D2: `agentId` binds the session permanently at creation —
 * required, chosen via the New Session agent picker (default preselected).
 */
export async function createSession(projectId: string, agentId: string): Promise<Session> {
  return invoke('create_session', { projectId, agentId }) as Promise<Session>;
}

/**
 * §36.17.4 (#3050 PR 10): the New Session sheet's write path for a captain
 * chosen WITH a crew. Additive sibling of `createSession` above — zero
 * change to that function or its callers. `crewAgentIds` is always an
 * array (possibly empty); the backend normalizes empty → `NULL` and
 * forces `NULL` outright for an OpenClaw captain regardless of what's
 * sent (`create_session_with_crew`, `ai/chat.rs`).
 */
export async function createSessionWithCrew(projectId: string, agentId: string, crewAgentIds: string[]): Promise<Session> {
  return invoke('create_session_with_crew', { projectId, agentId, crewAgentIds }) as Promise<Session>;
}

/**
 * §36.17.4 (#3050 PR 10): the edit-crew popover's write path ("Save
 * crew"). The only production write to `chat_sessions.crew_agent_ids`
 * after session creation. Captain is never part of this call — there is
 * no `agentId` parameter, matching the backend command's own shape
 * (`update_session_crew`, `ai/chat.rs`): the captain is immutable, only
 * the crew list is editable.
 */
export async function updateSessionCrew(sessionId: string, crewAgentIds: string[]): Promise<Session> {
  return invoke('update_session_crew', { sessionId, crewAgentIds }) as Promise<Session>;
}

/** List sessions for a surface, newest first. Excludes hidden by default. */
export async function listSessions(projectId: string, includeHidden?: boolean): Promise<Session[]> {
  return invoke('list_sessions', { projectId, includeHidden: includeHidden ?? false }) as Promise<Session[]>;
}

/**
 * Switch to a different session. One transaction: demote current, promote
 * target — UNLESS the target is a job-origin (`spawn`/`schedule`) session,
 * in which case the backend's own "open without promote" guard (Ruling 2,
 * #3050 PR 6, `demote_and_promote`) no-ops the mutation and just resolves
 * the session for display. Safe to call on ANY session id for that reason
 * — a notification's click-to-open (§36.7) and `navigateTo`'s own
 * `sessionId` override both rely on this.
 */
export async function switchSession(sessionId: string): Promise<Session> {
  return invoke('switch_session', { sessionId }) as Promise<Session>;
}

/** Hide a session. If it was active, promotes the next visible session (or creates one). */
export async function hideSession(sessionId: string): Promise<void> {
  return invoke('hide_session', { sessionId }) as Promise<void>;
}

/** Unhide a session. Does not change active status. */
export async function unhideSession(sessionId: string): Promise<void> {
  return invoke('unhide_session', { sessionId }) as Promise<void>;
}

/** Rename a session. Empty/whitespace resets title to '' (reverts to auto-title behavior). */
export async function renameSession(sessionId: string, title: string): Promise<void> {
  return invoke('rename_session', { sessionId, title }) as Promise<void>;
}

/**
 * Get the active session for a surface. Lazy safety net: creates one if none exists.
 * Replaces getOrCreateThread. Never returns null.
 */
export async function getActiveSession(projectId: string): Promise<Session> {
  return invoke('get_active_session', { projectId }) as Promise<Session>;
}

/**
 * §36.7 (#3050 PR 6): tell the backend which session is currently on
 * screen — the single write path `is_session_open()` reads to gate
 * `agent_message` notifications. Call with the resolved session id on
 * every navigation that changes the visible chat (`navigateTo`'s own
 * thread resolution, the sessions-drawer switch), and with `null` on
 * chat-view teardown (e.g. navigating to Settings, where ChatSidebar
 * unmounts entirely). Fire-and-forget from the caller's perspective is
 * NOT safe here — unlike `pane-data-changed`, a dropped call leaves the
 * backend's notion of "what's open" stale, so callers `await` this.
 */
export async function setViewingSession(sessionId: string | null): Promise<void> {
  return invoke('set_viewing_session', { sessionId }) as Promise<void>;
}

/** List chat sessions for a surface (real implementation of the former stub). */
export async function listChatThreads(projectId?: string): Promise<Session[]> {
  const surface = projectId || 'workspace';
  return listSessions(surface);
}

/**
 * Home AI dual-call helper (§30.2.2): loads workspace sessions plus sessions
 * recovered from the `__recovered__` pseudo-project in a single await.
 *
 * The two calls are required because list_sessions accepts one project_id per
 * invocation. Recovered rows are always fetched include_hidden=true (all rows
 * in that bucket are hidden by the v71→v72 migration). Workspace sessions
 * respect the caller's includeHidden flag.
 *
 * Returns { workspace, recovered } — callers group the two arrays with a
 * separator. No backend change needed.
 */
export interface HomeSessionsResult {
  workspace: Session[];
  recovered: Session[];
}

export async function listHomeSessionsWithRecovered(
  includeHidden = false
): Promise<HomeSessionsResult> {
  const [workspace, recovered] = await Promise.all([
    listSessions('workspace', includeHidden),
    listSessions('__recovered__', true), // always include_hidden for recovered bucket
  ]);
  return { workspace, recovered };
}


export async function getChatMessages(threadId: string, limit: number = 50, offset?: number): Promise<ChatMessage[]> {
  const sessionId = threadId || 'workspace';
  return invoke('load_chat_messages', { sessionId, limit, offset }) as Promise<ChatMessage[]>;
}

export async function sendChatMessage(threadId: string, projectId: string, content: string, id?: string, attachmentIds?: string): Promise<string> {
  const surface = projectId || 'workspace';
  const sessionId = threadId || 'workspace';
  const messageId = id || `msg-${Date.now()}-${Math.random().toString(36).slice(2, 11)}`;
  await invoke('save_chat_message', { 
    id: messageId, 
    projectId: surface, 
    threadId: sessionId,
    role: 'user', 
    content,
    attachmentIds: attachmentIds || null,
    toolCalls: null,
    toolResults: null,
    model: null,
    tokenCount: null,
  });
  return messageId;
}

export async function saveAssistantMessage(
  threadId: string,
  projectId: string,
  content: string, 
  id?: string,
  toolCalls?: string | null,
  toolResults?: string | null,
  model?: string | null,
  // #3161 PR 1 (§39.5): generate_image attachment ids for THIS assistant
  // row, comma-joined (same shape the user-message upload path already
  // uses — see saveUserMessage above). Hardcoded `null` before this PR —
  // `save_chat_message`'s own `attachment_ids` parameter already existed
  // and already persists whatever is passed; nobody on the assistant
  // side ever passed a non-null value in.
  attachmentIds?: string | null,
): Promise<string> {
  const surface = projectId || 'workspace';
  const sessionId = threadId || 'workspace';
  const messageId = id || `msg-${Date.now()}-${Math.random().toString(36).slice(2, 11)}`;
  await invoke('save_chat_message', { 
    id: messageId, 
    projectId: surface, 
    threadId: sessionId,
    role: 'assistant', 
    content,
    attachmentIds: attachmentIds || null,
    toolCalls: toolCalls || null,
    toolResults: toolResults || null,
    model: model || null,
    tokenCount: null,
  });
  return messageId;
}

// AI Settings functions
export async function getAISetting(key: string): Promise<string | null> {
  console.debug('getAISetting: not implemented in v9');
  return null;
}

export async function setAISetting(key: string, value: string): Promise<void> {
  console.debug('setAISetting: not implemented in v9');
}

export async function getProjectTasksSummary(projectId?: string): Promise<Task[]> {
  console.debug('getProjectTasksSummary: not implemented in v9');
  return [];
}

// AI Chat response type
export interface AiChatResponse {
  text: string;
  model: string;
  input_tokens: number;
  output_tokens: number;
}

// AI Configuration functions
export async function aiConfigure(provider: string, apiKey?: string, model?: string, endpoint?: string): Promise<string> {
  return invoke('ai_configure', { 
    provider, 
    apiKey: apiKey || null, 
    model: model || null, 
    endpoint: endpoint || null 
  }) as Promise<string>;
}

export async function aiGetConfig(): Promise<any> {
  return invoke('ai_get_config');
}

export async function aiCheckOllama(): Promise<string[]> {
  return invoke('ai_check_ollama') as Promise<string[]>;
}

export async function aiSaveConfig(provider: string, apiKey?: string, model?: string, endpoint?: string): Promise<void> {
  return invoke('ai_save_config', {
    provider,
    apiKey: apiKey || null,
    model: model || null,
    endpoint: endpoint || null
  });
}

export async function aiLoadConfig(): Promise<any> {
  return invoke('ai_load_config');
}

// AI Chat function
// === UNIFIED NOTE/TASK API FUNCTIONS (v7.0) ===

// promoteNoteToTask removed — it hardcoded status: 'task'. Use updateTaskStatus(id, status) instead.

export async function demoteNoteFromTask(id: string): Promise<void> {
  console.debug('demoteNoteFromTask: using update_note in v9');
  return invoke('update_note', { request: { note_id: id, status: 'cabinet' } });
}

export async function updateNoteTaskFields(
  id: string,
  status?: string,
  priority?: string,
  dueDate?: string,
  assignee?: string,
  tags?: string
): Promise<void> {
  console.log('[API] update_note_task_fields:', { id, status, priority, due_date: dueDate, assignee });
  const request: any = { note_id: id };
  if (status !== undefined) request.status = status;
  if (priority !== undefined) request.priority = priority;
  if (dueDate !== undefined) request.due_date = dueDate;
  if (assignee !== undefined) request.assignee = assignee;
  return invoke('update_note', { request });
}

export async function listTaskNotes(
  status?: string,
  priority?: string,
  folderId?: string
): Promise<NoteMeta[]> {
  console.debug('listTaskNotes: using get_tasks in v9');
  const params: any = {};
  if (folderId) params.projectId = folderId;
  if (status) params.status = status;
  
  const notes: any[] = await invoke('get_tasks', params);
  
  // Load projects to map project_id and folder_id to notebook paths
  const projects: any[] = await invoke('get_projects', {});
  const projectMap = new Map(projects.map(p => [p.id, p]));
  
  return notes.map(n => {
    // Construct notebook path from project_id and folder_id
    let notebook = '';
    if (n.project_id) {
      const project = projectMap.get(n.project_id);
      if (project) {
        notebook = project.name;
        if (n.folder_id) {
          const folder = projectMap.get(n.folder_id);
          if (folder) {
            notebook = `${project.name}/${folder.name}`;
          } else {
            console.warn('[api.listTaskNotes] folder lookup miss — folder_id not in projectMap', {
              folderId: n.folder_id,
              projectMapSize: projectMap.size,
              noteId: n.id,
            });
          }
        }
      }
    }
    return transformNoteMeta(n, notebook);
  });
}

export async function listTaskNotesByFolder(folderId: string): Promise<NoteMeta[]> {
  console.debug('listTaskNotesByFolder: using get_tasks in v9');
  const notes: any[] = await invoke('get_tasks', { projectId: folderId });
  
  // Load projects to map project_id and folder_id to notebook paths
  const projects: any[] = await invoke('get_projects', {});
  const projectMap = new Map(projects.map(p => [p.id, p]));
  
  return notes.map(n => {
    // Construct notebook path from project_id and folder_id
    let notebook = '';
    if (n.project_id) {
      const project = projectMap.get(n.project_id);
      if (project) {
        notebook = project.name;
        if (n.folder_id) {
          const folder = projectMap.get(n.folder_id);
          if (folder) {
            notebook = `${project.name}/${folder.name}`;
          } else {
            console.warn('[api.listTaskNotesByFolder] folder lookup miss — folder_id not in projectMap', {
              folderId: n.folder_id,
              projectMapSize: projectMap.size,
              noteId: n.id,
            });
          }
        }
      }
    }
    return transformNoteMeta(n, notebook);
  });
}

// Chat Memory functions
export async function getChatMemory(projectId?: string): Promise<string | null> {
  console.debug('getChatMemory: not implemented in v9');
  return null;
}

export async function saveChatMemory(projectId: string | undefined, content: string): Promise<void> {
  console.debug('saveChatMemory: not implemented in v9');
}

export async function getChatMessageCount(threadId: string): Promise<number> {
  console.debug('getChatMessageCount: not implemented in v9');
  return 0;
}

export async function deleteOldChatMessages(threadId: string, keepLast: number): Promise<void> {
  console.debug('deleteOldChatMessages: not implemented in v9');
}

// Subtask functions (v7.2.0)
export async function updateSubtask(noteId: string, fields: {
  title?: string,
  contentPreview?: string,
  status?: string,
  priority?: string,
  dueDate?: string,
  assignee?: string,
}): Promise<void> {
  console.debug('updateSubtask: using update_note in v9');
  const request: any = { note_id: noteId };
  if (fields.title !== undefined) request.title = fields.title;
  if (fields.status !== undefined) request.status = fields.status;
  if (fields.priority !== undefined) request.priority = fields.priority;
  if (fields.dueDate !== undefined) request.due_date = fields.dueDate;
  if (fields.assignee !== undefined) request.assignee = fields.assignee;
  if (fields.contentPreview) request.content = fields.contentPreview;
  return invoke('update_note', { request });
}

export async function listSubtasks(parentId: string): Promise<NoteMeta[]> {
  if (!parentId) return [];
  const results: any[] = await invoke('get_subtasks', { parentId });
  return results.map(r => transformNoteMeta(r));
}

export async function createSubtask(parentId: string, title: string): Promise<any> {
  // Get parent note to find project_id and folder_id
  const parent: any = await invoke('get_note', { noteId: parentId });
  const result: any = await invoke('create_note', {
    request: {
      title,
      project_id: parent.meta.project_id,
      folder_id: parent.meta.folder_id || null,
      parent_id: parentId,
      status: 'todo'
    }
  });
  return transformNoteMeta(result);
}

export async function getSubtaskCounts(noteIds: string[]): Promise<Record<string, number>> {
  const counts: Record<string, number> = {};
  for (const id of noteIds) {
    try {
      const subtasks: any[] = await invoke('get_subtasks', { parentId: id });
      if (subtasks.length > 0) counts[id] = subtasks.length;
    } catch { /* ignore */ }
  }
  return counts;
}

// Category functions
export async function listCategories(): Promise<CategoryWithColor[]> {
  return invoke('list_categories');
}

export async function setProjectCategory(projectId: string, category: string | null, originTag?: string): Promise<void> {
  console.log('[CATEGORY] Setting category:', { projectId, category });
  const result = await invoke('set_project_category', { projectId, category: category ?? null });
  console.log('[CATEGORY] Result:', result);
  // #2972 stage 3 (§34.1.2) — see saveNote()'s identical comment above.
  dispatchDataChanged(projectId, ENTITY_PROJECT, REASON_EDITED, originTag);
}

// #3018 (§34.1/§34.1.2): category is a fifth entity_type, entity_id = the
// category name. This one function backs both Settings.svelte call sites —
// `addCategory()` (brand-new category, `reason: "created"`) and
// `updateCategory()`'s color-only branch (existing category, `reason:
// "edited"`) — so the reason can't be hardcoded here; callers pass it.
// Defaults to "edited" since the color-only-update call site is the
// pre-existing/more common path and `renameCategory`'s own rename case
// below has its own dedicated call.
export async function setCategoryColor(
  category: string,
  color: string,
  reason: Reason = REASON_EDITED,
  originTag?: string,
): Promise<void> {
  await invoke('set_category_color', { category, color });
  dispatchDataChanged(category, ENTITY_CATEGORY, reason, originTag);
}

/**
 * Rename a category atomically.
 * Updates both `category_colors` and all `projects.category` references in one transaction.
 * Use this whenever the category name changes; use setCategoryColor for color-only updates.
 */
// #3018 (§34.1/§34.2): category is a fifth entity_type — rename/recolor is
// `reason: "edited"` on the category itself (entity_id = the NEW name;
// listeners re-read the whole category list on any category event rather
// than patching by id, §34.2, so which name is reported doesn't change
// listener behavior, but the new name is the one that still exists).
//
// The backend `rename_category` command (`projects.rs`) also updates
// `projects.category` in bulk for every project that carried the old name,
// and returns those projects' IDs from the same transaction (not worked
// out from frontend state). One `project`/`edited` event is dispatched per
// returned ID — zero if the rename affected no projects — so every screen
// that groups/colors projects by category regroups without a restart.
export async function renameCategory(oldCategory: string, newCategory: string, color: string): Promise<void> {
  const affectedProjectIds: string[] = await invoke('rename_category', { oldCategory, newCategory, color });
  dispatchDataChanged(newCategory, ENTITY_CATEGORY, REASON_EDITED);
  for (const projectId of affectedProjectIds) {
    dispatchDataChanged(projectId, ENTITY_PROJECT, REASON_EDITED);
  }
}

// #3018 — same rationale as renameCategory() immediately above: the
// backend `delete_category` command also clears `projects.category` in
// bulk and returns the affected project IDs from the same transaction. One
// category/`deleted` event plus one project/`edited` event per affected
// project (zero if none) — the project itself isn't deleted, only its
// category field changed to NULL.
export async function deleteCategory(category: string): Promise<void> {
  const affectedProjectIds: string[] = await invoke('delete_category', { category });
  dispatchDataChanged(category, ENTITY_CATEGORY, REASON_DELETED);
  for (const projectId of affectedProjectIds) {
    dispatchDataChanged(projectId, ENTITY_PROJECT, REASON_EDITED);
  }
}

export async function rebuildIndex(): Promise<string> {
  return invoke('rebuild_index');
}

// === DASHBOARD MODULE MANAGEMENT ===

export async function toggleModuleVisibility(moduleId: string, visible: boolean): Promise<void> {
  return invoke('toggle_dashboard_module', { moduleId, enabled: visible });
}

export async function removeModule(moduleId: string): Promise<void> {
  return invoke('delete_dashboard_module', { moduleId });
}

export async function addDashboardModule(moduleType: string, sortOrder?: number, configJson?: string): Promise<any> {
  return invoke('create_dashboard_module', { moduleType, configJson: configJson ?? null, sortOrder: sortOrder ?? null });
}

export async function reorderModules(moduleOrders: [string, number][]): Promise<void> {
  return invoke('reorder_dashboard_modules', { moduleOrders });
}

export async function updateModuleContent(moduleId: string, content: string): Promise<void> {
  // content goes into config_json as { content: "..." }
  const configJson = JSON.stringify({ content });
  return invoke('update_dashboard_module', { moduleId, configJson });
}

export async function resetOnboarding(): Promise<any> {
  // Delete all modules and re-run onboarding
  const [config, modules] = await invoke('get_dashboard_config') as [any, any[]];
  for (const m of modules) {
    await invoke('delete_dashboard_module', { moduleId: m.id });
  }
  return { success: true };
}

// === MODULE ITEMS ===

export async function getModuleItems(moduleId: string): Promise<any[]> {
  return invoke('get_module_items_cmd', { moduleId });
}

export async function addModuleItem(
  moduleId: string, 
  itemType: string, 
  label: string, 
  description?: string,
  emoji?: string,
  configJson?: string
): Promise<any> {
  return invoke('create_module_item_cmd', { 
    moduleId, itemType, label, 
    description: description ?? null,
    emoji: emoji ?? null,
    configJson: configJson ?? null
  });
}

export async function updateModuleItem(
  itemId: string, 
  label?: string, 
  description?: string,
  emoji?: string,
  configJson?: string
): Promise<any> {
  return invoke('update_module_item_cmd', { 
    itemId, 
    label: label ?? null,
    description: description ?? null,
    emoji: emoji ?? null,
    configJson: configJson ?? null
  });
}

export async function removeModuleItem(itemId: string): Promise<void> {
  return invoke('delete_module_item_cmd', { itemId });
}

// === GRATITUDE ===

export async function getGratitudeEntries(date: string): Promise<GratitudeEntry[]> {
  return invoke('get_gratitude_entries_cmd', { entryDate: date });
}

export async function addGratitudeEntry(date: string, text: string): Promise<GratitudeEntry> {
  return invoke('save_gratitude_entry_cmd', { entryDate: date, text });
}

export async function deleteGratitudeEntry(entryId: string): Promise<void> {
  return invoke('delete_gratitude_entry_cmd', { entryId });
}

// === HABITS ===

export async function getHabitsToday(): Promise<any[]> {
  const today = new Date().toISOString().split('T')[0];
  return invoke('get_habit_entries_cmd', { entryDate: today });
}

export async function toggleHabit(itemId: string, date: string, completed: boolean): Promise<any> {
  return invoke('save_habit_entry_cmd', { itemId, entryDate: date, completed });
}

// === REFLECTION ===

export async function getReflectionEntries(date: string): Promise<any[]> {
  return invoke('get_reflection_entries_cmd', { entryDate: date });
}

export async function saveReflectionEntry(
  questionId: string,
  date: string,
  valueNumeric: number | null,
  valueBool: boolean | null,
  valueText: string | null
): Promise<any> {
  return invoke('save_reflection_entry_cmd', { 
    questionId, 
    entryDate: date, 
    valueNumeric: valueNumeric ?? null,
    valueBool: valueBool ?? null,
    valueText: valueText ?? null
  });
}

// === DAILY ENTRIES ===

export async function getDailyEntries(date: string): Promise<any[]> {
  return invoke('get_daily_entries_cmd', { entryDate: date });
}

export async function saveDailyEntry(moduleId: string, date: string, dataJson: string): Promise<any> {
  return invoke('save_daily_entry_cmd', { moduleId, entryDate: date, dataJson });
}

// === TASKS (additional helpers) ===

export async function listTasksDueToday(): Promise<any[]> {
  return getTasksToday();
}

// === MISSING STUBS FOR v9 ===

export type FolderInfo = Project;
export type HomeModule = any;

export async function getNoteTaskFields(noteId: string): Promise<Record<string, string | null>> {
  const result: any = await invoke('get_note', { noteId: noteId });
  const meta = result.meta || result;
  return {
    title: meta.title || null,
    task_status: meta.status || null,
    task_priority: meta.priority || null,
    task_due_date: meta.due_date || null,
    task_assignee: meta.assignee || null,
  };
}

export async function listAllFolders(): Promise<Project[]> {
  console.debug('listAllFolders: using get_projects in v9');
  const projects: any[] = await invoke('get_projects', {});
  return projects.filter(p => p.parent_id);
}

export async function repairNoteMappings(): Promise<number> {
  console.debug('repairNoteMappings: not implemented in v9');
  return 0;
}

export async function listNotesByProjectName(projectName: string): Promise<NoteMeta[]> {
  console.debug('listNotesByProjectName: using get_projects + get_notes in v9');
  const projects: any[] = await invoke('get_projects', {});
  const project = projects.find(p => p.name === projectName && !p.parent_id);
  if (!project) return [];
  const notes: any[] = await invoke('get_notes', { projectId: project.id });
  return notes.map(n => transformNoteMeta(n, projectName));
}

export async function saveOnboarding(configId: string | null, modules: any[]): Promise<void> {
  console.debug('saveOnboarding: using dashboard module functions in v10');
  
  // Create modules one by one
  for (let i = 0; i < modules.length; i++) {
    const m = modules[i];
    try {
      const configJson = m.configJson || (m.config ? JSON.stringify(m.config) : null);
      
      // Create the module
      const module: any = await invoke('create_dashboard_module', { 
        moduleType: m.moduleType, 
        configJson, 
        sortOrder: i 
      });
      
      // Save static content if present
      if (m.staticContent && typeof m.staticContent === 'string' && m.staticContent.trim()) {
        await invoke('update_module_content_cmd', { 
          moduleId: module.id, 
          content: m.staticContent 
        });
      }
      
      // Create items if present
      if (m.items && Array.isArray(m.items) && m.items.length > 0) {
        for (const item of m.items) {
          await invoke('create_module_item_cmd', {
            moduleId: module.id,
            itemType: m.moduleType,
            label: item.label,
            description: item.description ?? null,
            emoji: item.emoji ?? null,
            configJson: item.configJson ?? null,
          });
        }
      }
    } catch (e) {
      console.error(`Failed to create module ${m.moduleType}:`, e);
    }
  }
  
  // Always ensure tasks module exists
  const hasTasksModule = modules.some(m => m.moduleType === 'tasks');
  if (!hasTasksModule) {
    try {
      await invoke('create_dashboard_module', {
        moduleType: 'tasks',
        configJson: '{}',
        sortOrder: modules.length
      });
    } catch (e) {
      console.error('Failed to create default tasks module:', e);
    }
  }
}

export async function listHomeModules(): Promise<any[]> {
  console.debug('listHomeModules: using get_dashboard_config in v9');
  const [config, modules] = await invoke('get_dashboard_config') as [any, any[]];
  return modules.map(m => ({
    id: m.id,
    module_type: m.module_type,
    static_content: m.config_json ? JSON.parse(m.config_json).content : '',
    enabled: m.enabled,
    sort_order: m.sort_order,
  }));
}

// ========================================
// Auth & Device Registration (v9.1.2)
// ========================================

export async function authSignUp(email: string, password: string) {
    return invoke('auth_sign_up', { email, password });
}

export async function authSignIn(email: string, password: string) {
    return invoke('auth_sign_in', { email, password });
}

export async function authSignOut(accessToken: string) {
    return invoke('auth_sign_out', { accessToken });
}

export async function authRefresh(refreshToken: string) {
    return invoke('auth_refresh', { refreshToken });
}

export async function registerDevice(accessToken: string, userId: string, deviceName: string) {
    return invoke('register_device_cmd', { accessToken, userId, deviceName });
}

export async function getAuthSession() {
    return invoke('get_auth_session');
}

// ADR-0070 Decision 3 (#3195 PR A): generic ai_settings reader. Used by
// initAuth()/startAutoSync() to read the `account_enabled` flag before
// deciding whether to touch session storage / start auto-sync at all.
export async function getAiSetting(key: string): Promise<string | null> {
    return invoke('get_ai_setting', { key }) as Promise<string | null>;
}

export async function saveAuthSession(session: any) {
    return invoke('save_auth_session', { session });
}

export async function clearAuthSession() {
    return invoke('clear_auth_session');
}

// Encryption commands
export async function initEncryption(email: string, password: string) {
    return invoke('init_encryption', { email, password });
}

export async function isEncryptionReady(): Promise<boolean> {
    return invoke('is_encryption_ready') as Promise<boolean>;
}

export async function clearEncryption() {
    return invoke('clear_encryption');
}

export async function testEncryption(): Promise<string> {
    return invoke('test_encryption') as Promise<string>;
}

// Sync Engine commands
export async function syncPush(): Promise<string> {
    return invoke('sync_push') as Promise<string>;
}

export async function syncPull(): Promise<string> {
    return invoke('sync_pull') as Promise<string>;
}

export async function syncConfigure(userId: string, deviceId: string, accessToken: string) {
    return invoke('sync_configure', { userId, deviceId, accessToken });
}

export async function getSyncInfo() {
    return invoke('get_sync_info');
}

export async function syncFilesPush(): Promise<string> {
    return invoke('sync_files_push') as Promise<string>;
}

export async function syncFilesPull(): Promise<string> {
    return invoke('sync_files_pull') as Promise<string>;
}

// Manual Sync Now (#2522, ADR-0063 Stage 7a)
export interface SyncNowResult {
    success: boolean;
    timestamp: string;
}

export async function syncNow(): Promise<SyncNowResult> {
    return invoke('sync_now') as Promise<SyncNowResult>;
}

// Cleanup Pending Flag (v0.65.32 — #1079)
export async function checkCleanupPending() {
    return invoke('check_cleanup_pending');
}

export async function writeCleanupFlag(cleanupEncryption: boolean, cleanupAuthSession: boolean, attempts?: number) {
    return invoke('write_cleanup_flag', { cleanupEncryption, cleanupAuthSession, attempts: attempts || 1 });
}

export async function clearCleanupFlag() {
    return invoke('clear_cleanup_flag');
}

// ─── Sheets (#2339 PR 9/PR 10, §33.3) — frontend wrappers ───────────────────
// PR 9 wired the two read/create commands below. PR 10 (typed-column
// editing) adds the four mutating wrappers that follow — `updateSheetCell`,
// `createSheetRow`, `createSheetColumn`, `updateSheetColumn` — each a thin
// `invoke(...)` call following the same convention as every wrapper in this
// file, param names matching the Tauri commands' camelCase-mapped-from-
// snake_case argument names (confirmed against sheets.rs's real signatures).

export interface CreateSheetRequest {
  project_id: string;
  folder_id?: string;
  title: string;
  tags?: string[];
  status?: string;
  priority?: string;
  due_date?: string;
}

/** Wraps the `create_sheet` Tauri command (backend: sheets.rs) — creates a new
 *  sheet note (atomically note_type='sheet', file_path='', 5 default text
 *  columns seeded) and returns the full grid so the caller can render
 *  immediately without a second round-trip. */
export async function createSheet(request: CreateSheetRequest): Promise<SheetData> {
  const raw: any = await invoke('create_sheet', { request });
  // #2972 stage 3 (§34.1.2): migrated from the bare `pane-note-created`
  // CustomEvent to the single entity-identified event, `entity_type:
  // "sheet"`, `reason: "created"`.
  dispatchDataChanged(raw.note_meta.id, ENTITY_SHEET, REASON_CREATED);
  return raw as SheetData;
}

/** Wraps the `get_sheet_data` Tauri command (backend: sheets.rs) — full grid
 *  read (columns/rows/cells + note_meta), ordered by sort_order, with
 *  computed_value populated for formula cells that resolved on this read. */
export async function getSheetData(noteId: string): Promise<SheetData> {
  const raw: any = await invoke('get_sheet_data', { noteId });
  return raw as SheetData;
}

/** Wraps the `restore_sheet_snapshot` Tauri command (backend:
 *  `sheets_snapshots.rs`) — the SHEET-NATIVE restore path (#2339 PR
 *  15b-beta). Diff-applies a past snapshot envelope against the live
 *  sheet's columns/rows/cells, after first capturing a `pre_restore`
 *  snapshot of the CURRENT state (so a restore is itself always
 *  undoable). Returns the freshly-recomputed `SheetData` for the caller
 *  to render immediately, without a separate `getSheetData` round-trip.
 *
 *  This is a HARD fork from the generic `restore_note_version` command —
 *  calling that command against a sheet note instead of this one would
 *  silently corrupt the sheet (it overwrites `notes.body` with the raw
 *  JSON envelope text as if it were markdown). `ShVersionHistoryPanel`'s
 *  sheet-restore path calls ONLY this function; see
 *  `ShVersionHistoryPanel.test.ts`'s explicit regression test asserting
 *  `restore_note_version` is never invoked from that path. */
export async function restoreSheetSnapshot(noteId: string, versionId: string): Promise<SheetData> {
  const raw: any = await invoke('restore_sheet_snapshot', { noteId, versionId });
  return raw as SheetData;
}

/** Wraps the `get_sheet_snapshot_computed_values` Tauri command (backend:
 *  `sheets_snapshots.rs`) — recomputes a historical version-history
 *  snapshot's formula cells for DISPLAY ONLY (item 32, #2874). Never
 *  written back to `note_versions.content` (ADR-0066 D4 preserved).
 *  Returns a map whose keys are `"{column_id}:{row_id}"` and whose
 *  values are the cell's computed display string — a plain
 *  number/text/boolean rendering, or an error string like `#DIV/0!`/
 *  `#REF!`/`#CYCLE!`/`#PARSE!` — for every formula cell in the snapshot;
 *  non-formula cells are excluded entirely (nothing to compute). A
 *  formula cell absent from this map means "no computed entry yet"
 *  (in-flight fetch, fetch failure, or genuinely missing) — callers must
 *  NEVER fall back to the raw stored formula string in that case (Matt's
 *  ruling 2026-09-15: users must never see internal IDs under any
 *  circumstance, and the raw stored string can itself carry
 *  `{column_id:row_id}` internal references). */
export async function getSheetSnapshotComputedValues(versionId: string): Promise<Record<string, string>> {
  const raw: any = await invoke('get_sheet_snapshot_computed_values', { versionId });
  return raw as Record<string, string>;
}

/** Wraps the `get_sheet_card_summary` Tauri command (backend: sheets.rs) —
 *  read-only counts for a single sheet's card face (col_count, row_count,
 *  formula_count, col_types). Much lighter than `getSheetData`: no cell
 *  values, no recompute pass. Rejects if `noteId` isn't a sheet note. */
export async function getSheetCardSummary(noteId: string): Promise<SheetCardSummary> {
  const raw: any = await invoke('get_sheet_card_summary', { noteId });
  return raw as SheetCardSummary;
}

/** Wraps the `get_sheet_card_summaries` Tauri command (backend: sheets.rs) —
 *  batched card summaries for a visible set of note IDs (a Kanban column, an
 *  All Tasks page, a folder view). Non-sheet/unknown IDs are silently
 *  omitted from the result rather than raising — callers pass a mixed list
 *  of note IDs and only want the sheet subset back. One DB round-trip
 *  instead of N. */
export async function getSheetCardSummaries(noteIds: string[]): Promise<SheetCardSummary[]> {
  if (noteIds.length === 0) return [];
  const raw: any[] = await invoke('get_sheet_card_summaries', { noteIds });
  return raw as SheetCardSummary[];
}

/** Wraps the `update_sheet_cell` Tauri command (backend: sheets.rs:1334) —
 *  upserts (or sparse-deletes, on an empty value) one cell. `styleJson` is
 *  always omitted by PR 10's callers (styling is untouched this PR); passing
 *  it through as an explicit param anyway matches the backend's real shape
 *  and keeps this wrapper forward-compatible with a future styling PR.
 *  Rejects (rejected Promise) if `value` starts with `=` and fails formula
 *  parse — callers must catch and keep the editor open on failure. */
export async function updateSheetCell(
  rowId: string,
  columnId: string,
  value: string | null,
  styleJson?: string | null
): Promise<SheetCell> {
  const raw: any = await invoke('update_sheet_cell', {
    rowId,
    columnId,
    value,
    styleJson: styleJson ?? null,
  });
  return raw as SheetCell;
}

/** Wraps the `update_sheet_cells_style_batch` Tauri command (backend:
 *  sheets.rs, #2874 item 51) — batches a selection's style-only writes
 *  into ONE backend transaction (one snapshot, one recompute for the
 *  whole batch, no body regeneration — style-only, same as today's
 *  single-cell path). Replaces `SheetGrid.svelte`'s old per-cell
 *  `updateSheetCell` loop in `applyStyleToSelection`/
 *  `clearSelectedStyling` — a 50-cell style apply used to be 50 separate
 *  `update_sheet_cell` round-trips (each running its own full-sheet
 *  recompute); it is now ONE round-trip. Every returned `SheetCell`
 *  still carries its readable A1 formula and live computed value (item
 *  47, unchanged) — the reply shape is identical to what N
 *  `updateSheetCell` calls would have returned, just batched into one
 *  array. `entries` order is not guaranteed to match the returned array
 *  order — callers should key off `row_id`/`column_id`, not index. */
export async function updateSheetCellsStyleBatch(
  noteId: string,
  entries: { rowId: string; columnId: string; styleJson: string }[]
): Promise<SheetCell[]> {
  if (entries.length === 0) return [];
  const raw: any[] = await invoke('update_sheet_cells_style_batch', {
    noteId,
    entries: entries.map((e) => ({ row_id: e.rowId, column_id: e.columnId, style_json: e.styleJson })),
  });
  return raw as SheetCell[];
}

/** Wraps the `create_sheet_row` Tauri command (backend: sheets.rs) —
 *  appends a new sparse row at the end of the sheet (`sort_order = max +
 *  1000`) when `referenceRowId`/`position` are both omitted. Used by
 *  ghost-row-commit (#2339 PR 10, §4) and the toolbar's insert-row button
 *  in append mode. §33.3.4 positional insert: pass `referenceRowId` +
 *  `position: 'before'` to insert immediately above that row instead —
 *  used by the sheet context menu's "Insert row above" (only `'before'`
 *  is wired anywhere in the UI; `'after'` is accepted by the backend but
 *  unused). Both args must be supplied together or both omitted — the
 *  backend rejects a mismatched pair. */
export async function createSheetRow(
  noteId: string,
  referenceRowId?: string,
  position?: 'before' | 'after'
): Promise<SheetRow> {
  const raw: any = await invoke('create_sheet_row', {
    noteId,
    referenceId: referenceRowId ?? null,
    position: position ?? null,
  });
  return raw as SheetRow;
}

/** Wraps the `create_sheet_rows` Tauri command (backend: sheets.rs,
 *  item 23 / #2874) — creates `count` new sparse rows in ONE backend
 *  transaction (one snapshot, one recompute+regen for the whole batch),
 *  each appended at `sort_order = max + (i+1) * 1000`. Used by ghost-row
 *  commit (`SheetGrid.svelte`'s `beginGhostCommit`) when the user types
 *  into a ghost row N rows past the last real row — the typed value is
 *  then written into the LAST row in the returned array
 *  (`rows[rows.length - 1].id`); every row before it is an intentionally
 *  empty intermediate row. Unlike `createSheetRow`, this has no
 *  positional-insert (`referenceRowId`/`position`) support — it is
 *  strictly an append-N-at-the-end operation. */
export async function createSheetRowsBatch(noteId: string, count: number): Promise<SheetRow[]> {
  const raw: any = await invoke('create_sheet_rows', { noteId, count });
  return raw as SheetRow[];
}

/** Wraps the `update_sheet_row` Tauri command (#2339 PR 13, §33.3.3) —
 *  height-only partial update. Unlike `updateSheetColumn`, this does NOT
 *  trigger a body/formula regen on the backend (row height has zero
 *  computed-value or body-serialization impact — see `sheets.rs`'s
 *  `update_sheet_row_internal` doc comment). Used by `SheetGrid.svelte`'s
 *  row-resize drag commit (mouseup, non-zero delta only). */
export async function updateSheetRow(rowId: string, height: number): Promise<SheetRow> {
  return invoke<SheetRow>('update_sheet_row', { rowId, height });
}

/** Wraps the `create_sheet_column` Tauri command (backend: sheets.rs) —
 *  appends a new column when `referenceColumnId`/`position` are both
 *  omitted. Used by ghost-column-commit (#2339 PR 10, §4, always called
 *  with `name: ''`, `colType: 'text'`, `optionsJson: null`) and the
 *  toolbar's insert-column button (same defaults) in append mode.
 *  §33.3.4 positional insert: pass `referenceColumnId` + `position:
 *  'before'` to insert immediately left of that column instead — used
 *  by the sheet context menu's "Insert column left" (only `'before'` is
 *  wired anywhere in the UI). Both args must be supplied together or
 *  both omitted — the backend rejects a mismatched pair. */
export async function createSheetColumn(
  noteId: string,
  name: string,
  colType: string,
  optionsJson?: string | null,
  referenceColumnId?: string,
  position?: 'before' | 'after'
): Promise<SheetColumn> {
  const raw: any = await invoke('create_sheet_column', {
    noteId,
    name,
    colType,
    optionsJson: optionsJson ?? null,
    referenceId: referenceColumnId ?? null,
    position: position ?? null,
  });
  return raw as SheetColumn;
}

/** Wraps the `create_sheet_columns` Tauri command (backend: sheets.rs,
 *  item 23 / #2874) — creates `count` new columns (`name: ''`,
 *  `colType: 'text'`, matching `createSheetColumn`'s ghost-commit
 *  defaults) in ONE backend transaction (one snapshot, one recompute+
 *  regen for the whole batch), each appended at `sort_order = max +
 *  (i+1) * 1000`. Used by ghost-column commit (`SheetGrid.svelte`'s
 *  `beginGhostCommit`) when the user types into a ghost column N
 *  columns past the last real column — the typed value is then written
 *  into the LAST column in the returned array
 *  (`cols[cols.length - 1].id`); every column before it is an
 *  intentionally empty intermediate column. Unlike `createSheetColumn`,
 *  this has no positional-insert support — strictly append-N-at-the-end. */
export async function createSheetColumnsBatch(noteId: string, count: number): Promise<SheetColumn[]> {
  const raw: any = await invoke('create_sheet_columns', { noteId, count });
  return raw as SheetColumn[];
}

/** Wraps the `update_sheet_column` Tauri command (backend: sheets.rs:822) —
 *  partial update, only provided fields are written. PR 10 uses this ONLY
 *  for the inline header-rename-on-ghost-column-commit flow (`name` only,
 *  every other field omitted/undefined so the backend preserves them). */
export async function updateSheetColumn(
  columnId: string,
  fields: { name?: string; colType?: string; optionsJson?: string; width?: number }
): Promise<SheetColumn> {
  const raw: any = await invoke('update_sheet_column', {
    columnId,
    name: fields.name ?? null,
    colType: fields.colType ?? null,
    optionsJson: fields.optionsJson ?? null,
    width: fields.width ?? null,
  });
  return raw as SheetColumn;
}

/** Wraps the `delete_sheet_column` Tauri command (backend: sheets.rs:891) —
 *  cascade-deletes the column's `sheet_cells` and the column row itself in
 *  one backend transaction. Used by `ShHeaderMenu.svelte`'s "Delete
 *  column" row (#2339 PR 11, §33.7). Returns nothing (the backend command
 *  itself returns `Result<(), String>`) — callers refetch `get_sheet_data`
 *  afterward to pick up the new column list, matching every other
 *  structural mutation's established refetch pattern. */
export async function deleteSheetColumn(columnId: string): Promise<void> {
  await invoke('delete_sheet_column', { columnId });
}

// ─── og: link preview (#1780, ADR-0049 Decision 2) ──────────────────────────

export interface OgData {
  url: string;
  og_title: string | null;
  og_description: string | null;
  og_image: string | null;
  status: 'ok' | 'timeout' | 'blocked' | 'no_og' | 'error';
  fetched_at: number;
}

/**
 * Fetch og: metadata for a URL. Guarded server-side (SSRF guard, redirect cap,
 * body cap, content-type gate). Returns null when link_previews_enabled=false.
 * Call fire-and-forget; cache is populated as a side-effect.
 */
export async function fetchLinkPreview(url: string): Promise<OgData | null> {
  return invoke('fetch_link_preview', { url }) as Promise<OgData | null>;
}

/**
 * Return cached og: data for a URL without triggering a new fetch.
 * Returns null on cache miss or stale entry.
 */
export async function getCachedLinkPreview(url: string): Promise<OgData | null> {
  return invoke('get_cached_link_preview', { url }) as Promise<OgData | null>;
}

// ─── Checklist Item API (§31.3, ADR-0057 subtasks table) ─────────────────────
// These wrap the Tauri subtask commands introduced in PR #2099.
// Named distinctly from the legacy child-notes stubs above (listSubtasks,
// createSubtask etc.) to avoid any collision.

export interface ChecklistItem {
  id: string;
  note_id: string;
  text: string;
  due_date: string | null;
  done: boolean;
  completed_at: string | null;
  sort_order: number;
  created_at: string;
  updated_at: string;
}

/**
 * Create a new checklist item attached to a note.
 * @param noteId   - UUID of the parent note
 * @param text     - Item text
 * @param dueDate  - ISO date string (YYYY-MM-DD) or undefined
 * @param sortOrder - Explicit sort position; omit to use backend default (0)
 */
export async function createChecklistItem(
  noteId: string,
  text: string,
  dueDate?: string,
  sortOrder?: number,
): Promise<ChecklistItem> {
  return invoke('create_subtask', {
    noteId,
    text,
    dueDate: dueDate ?? null,
    sortOrder: sortOrder ?? 0,
  }) as Promise<ChecklistItem>;
}

/**
 * Fetch all non-deleted checklist items for a note, ordered by sort_order ASC.
 */
export async function listChecklistItems(noteId: string): Promise<ChecklistItem[]> {
  if (!noteId) return [];
  return invoke('get_subtasks_for_note', { noteId }) as Promise<ChecklistItem[]>;
}

/**
 * Update one or more fields on an existing checklist item.
 * Only the provided fields are written; omitted fields are unchanged.
 */
export async function updateChecklistItem(
  id: string,
  updates: {
    text?: string;
    dueDate?: string | null;
    done?: boolean;
    sortOrder?: number;
  },
): Promise<ChecklistItem> {
  return invoke('update_subtask', {
    id,
    text: updates.text ?? null,
    dueDate: updates.dueDate !== undefined ? updates.dueDate : undefined,
    done: updates.done ?? null,
    sortOrder: updates.sortOrder ?? null,
  }) as Promise<ChecklistItem>;
}

/**
 * Hard-delete a checklist item and its associated notification rows.
 */
export async function deleteChecklistItem(id: string): Promise<void> {
  return invoke('delete_subtask', { id }) as Promise<void>;
}

// ============================================================
// Agent API — §24, ADR-0059 (#2030 PR B)
// ============================================================

/** Get all agents ordered by is_default DESC, name ASC. */
export async function getAgents(): Promise<Agent[]> {
  return invoke<Agent[]>('get_agents');
}

/**
 * Create a new locally-sourced agent (ADR-0059 D4).
 * Performs: filesystem write + single demote-then-promote transaction.
 * Returns the newly created Agent (is_default=1).
 */
export async function createLocalAgent(request: CreateLocalAgentRequest): Promise<Agent> {
  return invoke<Agent>('create_local_agent', { request });
}

/**
 * Promote an agent to main (ADR-0059 D2).
 * Single transaction: demote current main, promote target.
 * Caller must dispatch 'pane-agent-switched' on success (§24.7B).
 */
export async function setMainAgent(agentId: string): Promise<Agent> {
  return invoke<Agent>('set_main_agent', { agentId });
}

/**
 * Check if a given name would collide with any existing agent slug.
 * Checks all agents including inactive and OC-imported (§24.10.1).
 * Returns true if name is taken, false if available.
 */
export async function checkNameCollision(name: string): Promise<boolean> {
  return invoke<boolean>('check_name_collision', { name });
}

/**
 * ADR-0063 D4.3 / #2522 Stage 7c: legacy-pin write. The ONE way to write a
 * `runtime_type` onto a pre-migration agent whose `runtime_type` is NULL.
 *
 * C3 (#2587): its only caller, `LegacyProviderPrompt`'s "Set provider"
 * action, was removed — the `runtime_type IS NULL` population it existed to
 * rescue is closed (only `migrate_v79`'s D4.3 backfill ever created one) and
 * is now deactivated at rest by `migrate_v83`. This function and its backend
 * command (`update_agent` / `set_legacy_runtime_type_impl` in agents.rs) are
 * intentionally left in place — out of scope for C3's cleanup, which only
 * covers the rescue UI and its wiring, not the underlying write path — but
 * currently have no caller in this codebase. Backend still rejects this once
 * the row already has a non-NULL runtime_type; it is not a general-purpose
 * provider-change command.
 */
export async function setLegacyAgentRuntimeType(agentId: string, runtimeType: string): Promise<Agent> {
  return invoke<Agent>('update_agent', {
    request: { agent_id: agentId, runtime_type: runtimeType },
  });
}

// ============================================================
// Provider Connection Cards — ADR-0063 D8, #2522 Stage 5
// NET-NEW commands (`ai_list_runtime_connections`, `ai_disconnect_runtime`) — see
// src-tauri/src/ai/commands.rs module comment and the PR body for the full flag.
// ============================================================

export interface RuntimeRosterAgent {
  id: string;
  name: string;
  model: string | null;
  is_default: boolean;
}

export interface RuntimeConnectionInfo {
  runtime_type: 'openclaw' | 'openrouter' | 'anthropic' | 'ollama' | 'venice';
  connected: boolean;
  masked_credential: string | null;
  endpoint: string | null;
  error: string | null;
  /** #2641 D14 three-state color fix: true only when the user explicitly saved this
   *  runtime's credentials THIS install (Save path writes `{runtime}_configured_at`;
   *  openclaw uses the existing `gateway_paired_at`). Distinct from `connected`, which
   *  is a presence check that can be true from a stale keychain leftover (#2631) even
   *  though the user never configured this provider this install. */
  configured_this_install: boolean;
  agents: RuntimeRosterAgent[];
}

/** List per-runtime connection status + read-only agent roster for all four provider cards. */
export async function listRuntimeConnections(): Promise<RuntimeConnectionInfo[]> {
  return invoke<RuntimeConnectionInfo[]>('ai_list_runtime_connections');
}

/** Disconnect (clear stored credentials for) a single runtime type. */
export async function disconnectRuntime(runtimeType: string): Promise<void> {
  return invoke('ai_disconnect_runtime', { runtimeType });
}

// ─── Job Status (§36.4/§36.4.5, #3050 PR 5 — Matt's "chips show live
// status" addition) ───────────────────────────────────────────────────────

export interface JobStatusInfo {
  status: string;
  started_at: string | null;
  finished_at: string | null;
  /** live-test #1b fix PR, Ruling 3: non-NULL when the last firing of a
   *  recurring/heartbeat job failed transport (status stays 'pending' by
   *  design — this is the only observable signal of that failure). */
  last_error: string | null;
  /** C37 (#3139, live test #4): the existing `agent_jobs.next_run_at`
   *  column, newly SELECTed here so the chip's "next" clause can be
   *  reformatted at render time (local timezone) instead of a baked-UTC
   *  string frozen into the chip's stored content at creation time. */
  next_run_at: string | null;
}

/** Read a single job's live status/timestamps by id, for the spawn/schedule
 *  chip's own polling loop (`ChatPanel.svelte`). `null` for an unknown
 *  job_id (defensive — never throws for this case). */
export async function getJobStatus(jobId: string): Promise<JobStatusInfo | null> {
  return invoke('get_job_status', { jobId }) as Promise<JobStatusInfo | null>;
}

// ─── Home → Scheduled panel (§36.3.8, #3050 PR 8) ─────────────────────────

/** Every `agent_jobs` row except `kind = 'spawn'`, across every project —
 *  the panel's one query (`ai::scheduler::list_scheduled_jobs`, a bare
 *  command, not the `list_jobs` AI tool). Tab partition/sort is done
 *  client-side by `scheduledPanel.ts`. */
export async function listScheduledJobs(): Promise<import('./scheduledPanel').ScheduledJobRow[]> {
  return invoke('list_scheduled_jobs') as Promise<import('./scheduledPanel').ScheduledJobRow[]>;
}

/** Makes a pending/paused job eligible for the NEXT real scheduler tick
 *  (does not bypass `claim_due_jobs`). Disabled in the UI while the row
 *  is already `running` (§36.3.8/Notes: "Run now is disabled while a run
 *  is in progress"). */
export async function runJobNow(jobId: string): Promise<void> {
  return invoke('run_job_now', { jobId });
}

/** §36.3.7 carry-over (#3050 PR 9) — Pause/Delete on a RUNNING row call
 *  this FIRST, then the normal pause/delete command once it resolves.
 *  Never an optimistic row flip while the job is still really running.
 *  API-runtime: sets the same cancel flag Stop uses, waits for the row
 *  to leave 'running'. OC-runtime: no cancel checkpoint exists — waits
 *  on the dispatch's own completion instead. */
export async function waitForRunningJobToStop(jobId: string): Promise<void> {
  return invoke('wait_for_running_job_to_stop', { jobId });
}

/** Thin wrapper over `execute_pause_job` — the exact same handler the
 *  `pause_job` AI tool dispatches to. */
export async function pauseScheduledJob(jobId: string): Promise<void> {
  return invoke('pause_job_command', { jobId });
}

/** Thin wrapper over `execute_resume_job`. */
export async function resumeScheduledJob(jobId: string): Promise<void> {
  return invoke('resume_job_command', { jobId });
}

/** "Delete" in the panel — thin wrapper over `execute_cancel_job`
 *  (`status = 'cancelled'`, never a hard delete; the row stays visible
 *  in the Done tab, per §36.3.8/Notes). */
export async function deleteScheduledJob(jobId: string): Promise<void> {
  return invoke('cancel_job_command', { jobId });
}

/** §3151 PR2/C24 — "Clear done": archives every done/failed/cancelled
 *  row (shared `archive_terminal_jobs` mechanism, unconditional
 *  trigger). Returns the count archived, for a confirmation toast —
 *  `ScheduledPanel.svelte` doesn't currently show one, but the backend
 *  count is there if a future PR wants it. */
export async function clearDoneJobs(): Promise<number> {
  return invoke('clear_done_jobs_command');
}

/** §3151 PR2/C24 — "Delete" on a Done-tab card: a HARD delete of the job
 *  row AND its own session (never shared since PR1). NOT the same
 *  command as `deleteScheduledJob` above (which is the soft-cancel
 *  Active-tab "Delete") — only reachable for `done`/`failed`/`cancelled`
 *  rows, enforced server-side. */
export async function deleteArchivedJob(jobId: string): Promise<void> {
  return invoke('delete_archived_job_command', { jobId });
}

export interface CreateScheduledJobParams {
  brief: string;
  agent: string;
  projectId?: string;
  title?: string;
  runAt?: string;
  cron?: string;
  /** Only 'heartbeat' is a real override (the Check-in preset); omit for
   *  a plain once/recurring schedule — `create_scheduled_job`'s own
   *  run_at/cron inference covers those. */
  kind?: 'heartbeat';
}

/** The ONE create path for the panel's preset cards AND the plain "New
 *  schedule" form — thin wrapper over `execute_schedule_job`, the SAME
 *  function the `schedule_job` AI tool calls (task item 5: "the chip, the
 *  job session and the status flips are shared, not reimplemented"). */
export async function createScheduledJob(
  params: CreateScheduledJobParams
): Promise<{ job_id: string; next_run_at: string }> {
  return invoke('create_scheduled_job', {
    brief: params.brief,
    agent: params.agent,
    projectId: params.projectId ?? null,
    title: params.title ?? null,
    runAt: params.runAt ?? null,
    cron: params.cron ?? null,
    kind: params.kind ?? null,
  }) as Promise<{ job_id: string; next_run_at: string }>;
}

export interface UpdateScheduledJobParams {
  brief: string;
  agent: string;
  projectId?: string;
  title?: string;
  runAt?: string;
  cron?: string;
}

/** #3134 C26 — Edit action: thin wrapper over `update_job`/
 *  `execute_update_job`, mirroring `createScheduledJob`'s shape above.
 *  Rewrites the SAME `agent_jobs` row (`jobId`) — never creates a new
 *  one. No `kind` param: `update_job` only ever writes `'once'` or
 *  `'recurring'` (whichever `runAt`/`cron` implies), matching Edit's own
 *  gating (never offered for `heartbeat`/`reminder` rows). No dedicated
 *  test for this one-line pass-through — the mutation that matters
 *  (wrong command name, wrong argument shape) is caught for real by
 *  `ScheduledPanel.svelte`'s own
 *  "submit in edit mode calls updateScheduledJob, not createScheduledJob"
 *  test, which exercises this wrapper through its actual call site; a
 *  standalone test here would only restate the wrapper's own body. */
export async function updateScheduledJob(
  jobId: string,
  params: UpdateScheduledJobParams
): Promise<{ job_id: string; next_run_at: string }> {
  return invoke('update_job', {
    jobId,
    brief: params.brief,
    agent: params.agent,
    projectId: params.projectId ?? null,
    title: params.title ?? null,
    runAt: params.runAt ?? null,
    cron: params.cron ?? null,
  }) as Promise<{ job_id: string; next_run_at: string }>;
}
