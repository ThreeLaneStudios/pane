// Vitest setup file - mocks Tauri API for unit tests
// Based on tests/e2e/tauri-mock.ts but adapted for vitest

/// <reference types="vitest/globals" />

// Mock data store
const mockData = {
  notebooks: ['Inbox', 'Test Notebook'],
  notes: {} as Record<string, any[]>,
  noteContents: {} as Record<string, string>,
  noteJsons: {} as Record<string, any>,
  tags: [],
  subtasks: [] as any[],
  projects: [
    { id: 'project-inbox', name: 'Inbox', parent_id: null, created_at: new Date().toISOString(), updated_at: new Date().toISOString() },
    { id: 'project-test', name: 'Test Notebook', parent_id: null, created_at: new Date().toISOString(), updated_at: new Date().toISOString() },
  ] as any[],
  tasks: [] as any[],
  trash: [],
  nextId: 1,
  // #2874 (Kanban column-collapse persistence, item "Kanban" PR 5): generic
  // in-memory KV store backing the real `get_ai_setting`/`set_ai_setting`
  // Tauri commands (src-tauri/src/notes.rs) these tests now exercise for
  // real persist-across-reload coverage, instead of only asserting on a
  // fire-and-forget mock call. Previously unmocked (fell through to
  // `default: return null`), which happened to be harmless for the one
  // prior caller (NotesBrowser's #2252 collapse rails, tested via logic
  // extraction, not a real invoke) but gave no real persistence to assert
  // against for a component-render test.
  aiSettings: {} as Record<string, string>,
};

// Initialize some test data
mockData.notes['Inbox'] = [];
mockData.notes['Test Notebook'] = [];

// #2972 stage 2: minimal Tauri event-plugin registry so tests can simulate
// a real backend `app.emit(...)` reaching a component's `listen(...)`
// callback — needed to exercise the new per-screen `pane-data-changed`
// listeners (KanbanBoard/Editor/SheetGrid/Sidebar/CalendarView/
// AllTasksView/NotesBrowser) without a real Tauri runtime. Previously
// `plugin:event|listen`/`plugin:event|unlisten` fell through to this file's
// `default: return null` case — `listen()` would resolve (to a no-op
// unlisten), but there was no way to actually invoke a registered handler,
// so no existing test exercised this path at all (confirmed before adding
// this: zero `pane-note-changed`/`pane-project-changed` test hits anywhere
// in `src/`). This registry is additive only — it does not change the
// return shape of either command for any existing caller.
const tauriEventListeners: Record<string, { eventId: number; handlerId: string }[]> = {};
let nextTauriEventId = 1;

/**
 * Simulates a backend `app.emit(eventName, payload)` reaching every
 * frontend `listen(eventName, ...)` registration made since the test
 * started (or since the last `vi.resetModules()`/full page mock reset).
 * Mirrors the real `Event<T>` shape passed to a `listen()` callback
 * (`{ event, payload, id }`).
 */
export function simulateTauriEvent(eventName: string, payload: unknown): void {
  const listeners = tauriEventListeners[eventName] || [];
  for (const { eventId, handlerId } of listeners) {
    const handler = (globalThis as any)['_' + handlerId];
    if (typeof handler === 'function') {
      handler({ event: eventName, payload, id: eventId });
    }
  }
}

// jsdom does not implement ResizeObserver. Several components (Editor.svelte's
// toolbar-overflow effect, Card.svelte, KanbanBoard.svelte, NotesBrowser.svelte,
// SessionDropdownHeader.svelte, etc.) attach one in onMount. Individually-owned
// no-op stubs were duplicated across a couple dozen test files before this
// (#2950) — defined once here, globally, for any test that mounts a real
// component with a ResizeObserver in its mount path. Only defined if a real
// implementation isn't already present, so a real browser-like test env (or a
// future jsdom version that adds one) is never shadowed.
if (typeof (globalThis as any).ResizeObserver !== 'function') {
  (globalThis as any).ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

// Mock localStorage
const localStorageMock = (() => {
  let store: Record<string, string> = {};
  return {
    getItem: (key: string) => store[key] || null,
    setItem: (key: string, value: string) => { store[key] = value; },
    removeItem: (key: string) => { delete store[key]; },
    clear: () => { store = {}; },
  };
})();

// Add localStorage to global scope
Object.defineProperty(globalThis, 'localStorage', {
  value: localStorageMock,
  writable: true,
  configurable: true,
});

// Mock window.__TAURI_INTERNALS__ for Vitest  
Object.defineProperty(globalThis, 'window', {
  value: {
    localStorage: localStorageMock,
    dispatchEvent: (event: Event) => {
      // Mock window.dispatchEvent for tests
      return true;
    },
    // #2972 stage 2: `@tauri-apps/api/event`'s `listen()`-returned unlisten
    // fn calls `window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener`
    // synchronously before its own `invoke('plugin:event|unlisten', ...)`
    // call — with no stub here, every component that registers a listen()
    // and later unmounts (now most per-screen components, #2972 stage 2)
    // threw an unhandled-rejection TypeError on cleanup. No-op is correct:
    // this mock's own `plugin:event|unlisten` case (below) is the thing
    // that actually keeps the listener registry in sync for tests.
    __TAURI_EVENT_PLUGIN_INTERNALS__: {
      unregisterListener: (_event: string, _eventId: number) => {},
    },
    __TAURI_INTERNALS__: {
      invoke: async (cmd: string, args: any = {}) => {
        // console.log('[TAURI MOCK]', cmd, args);
        switch (cmd) {
          // V9 API: Projects/Folders
          case 'get_projects':
            return mockData.projects;

          // Folders are projects with parent_id set (api.ts's listFolders/get_folders
          // convention). No prior mock case existed for this command — any
          // caller of api.listFolders (KanbanBoard.loadTasks, NotesBrowser,
          // etc.) received `undefined` back and crashed on `.map()`.
          case 'get_folders': {
            const projectId = args.projectId;
            return mockData.projects.filter((p: any) => p.parent_id === projectId);
          }

          // #2874: generic ai_settings KV store — real backend behavior is
          // `Option<String>` (null for a never-written key), mirrored here.
          case 'get_ai_setting': {
            const key = args.key;
            return Object.prototype.hasOwnProperty.call(mockData.aiSettings, key)
              ? mockData.aiSettings[key]
              : null;
          }
          case 'set_ai_setting': {
            mockData.aiSettings[args.key] = args.value;
            return null;
          }

          case 'create_project': {
            const req = args.request || args;
            const project = {
              id: 'project-' + mockData.nextId++,
              name: req.name,
              description: req.description || null,
              color: req.color || null,
              parent_id: req.parent_id || null,
              sort_order: mockData.projects.length,
              created_at: new Date().toISOString(),
              updated_at: new Date().toISOString(),
            };
            mockData.projects.push(project);
            return project;
          }

          // V9 API: Notes (unified note/task)
          case 'create_note': {
            const req = args.request || args;
            const now = new Date().toISOString();
            const id = 'note-' + mockData.nextId++;
            const note = {
              id,
              title: req.title || 'Untitled',
              content: req.content || '',
              status: req.status || 'cabinet',
              priority: req.priority || 'medium',
              due_date: req.due_date || null,
              assignee: req.assignee || null,
              project_id: req.project_id || null,
              folder_id: req.folder_id || null,
              parent_id: req.parent_id || null,
              content_preview: (req.content || '').substring(0, 200),
              word_count: (req.content || '').split(/\s+/).filter(Boolean).length,
              tags: [],
              created_at: now,
              updated_at: now,
            };
            mockData.tasks.push(note);
            mockData.noteContents[id] = note.content;
            return note;
          }

          // Item 15 fix #4 test support: thin mock mirroring 'create_note'
          // above (no backend auto-assign-to-General replicated here — see
          // KanbanBoard.folder-assignment.test.ts's own comment on why the
          // "call shape" is what's asserted on, not simulated backend state).
          case 'create_sheet': {
            const req = args.request || args;
            const now = new Date().toISOString();
            const id = 'sheet-note-' + mockData.nextId++;
            const noteMeta = {
              id,
              title: req.title || 'Untitled Sheet',
              status: req.status || 'cabinet',
              priority: req.priority || 'medium',
              due_date: req.due_date || null,
              project_id: req.project_id || null,
              folder_id: req.folder_id || null,
              file_path: '',
              tags: req.tags || [],
              pinned: false,
              created_at: now,
              updated_at: now,
              note_type: 'sheet',
            };
            mockData.tasks.push(noteMeta);
            return {
              note_meta: noteMeta,
              columns: [],
              rows: [],
              cells: [],
              formula_count: 0,
              formulas_paused: false,
            };
          }

          case 'get_note': {
            const noteId = args.noteId;
            let note = mockData.tasks.find(t => t.id === noteId);
            // Fallback: check legacy note storage for test compatibility
            if (!note) {
              for (const nb in mockData.notes) {
                note = mockData.notes[nb].find((n: any) => n.id === noteId || n.slug === noteId);
                if (note) break;
              }
            }
            if (!note) {
              throw new Error(`Note not found: ${noteId}`);
            }
            return {
              meta: note,
              content: mockData.noteContents[noteId] || note.content || '',
            };
          }

          case 'get_notes': {
            const projectId = args.projectId;
            const folderId = args.folderId;
            let filtered = mockData.tasks;
            if (projectId) {
              filtered = filtered.filter(t => t.project_id === projectId);
            }
            if (folderId) {
              filtered = filtered.filter(t => t.folder_id === folderId);
            }
            return filtered;
          }

          case 'update_note': {
            const req = args.request || args;
            const noteId = req.note_id;
            const noteIndex = mockData.tasks.findIndex(t => t.id === noteId);
            if (noteIndex !== -1) {
              const note = mockData.tasks[noteIndex];
              if (req.title !== undefined) note.title = req.title;
              if (req.content !== undefined) {
                note.content = req.content;
                mockData.noteContents[noteId] = req.content;
                note.content_preview = req.content.substring(0, 200);
                note.word_count = req.content.split(/\s+/).filter(Boolean).length;
              }
              if (req.status !== undefined) note.status = req.status;
              if (req.priority !== undefined) note.priority = req.priority;
              if (req.due_date !== undefined) note.due_date = req.due_date;
              if (req.assignee !== undefined) note.assignee = req.assignee;
              if (req.project_id !== undefined) note.project_id = req.project_id;
              if (req.folder_id !== undefined) note.folder_id = req.folder_id;
              note.updated_at = new Date().toISOString();
              return note;
            }
            throw new Error(`Note not found: ${noteId}`);
          }

          case 'delete_note': {
            const noteId = args.noteId;
            mockData.tasks = mockData.tasks.filter(t => t.id !== noteId);
            delete mockData.noteContents[noteId];
            return null;
          }

          case 'get_tasks': {
            const projectId = args.projectId;
            const status = args.status;
            let filtered = mockData.tasks.filter(t => t.status === 'task' || t.status === 'todo' || t.status === 'in_progress' || t.status === 'done');
            if (projectId) {
              filtered = filtered.filter(t => t.project_id === projectId);
            }
            if (status) {
              filtered = filtered.filter(t => t.status === status);
            }
            return filtered;
          }

          // Kanban board data source: all notes including cabinet, no status filter.
          // Mirrors the get_project_notes_for_kanban Rust command (notes.rs).
          // #2874 item 15: the real backend's KANBAN_NOTES_QUERY predicate is
          // `note_type IS NULL OR note_type = 'sheet'` — sheets ARE kanban
          // cards (they render via Card.svelte's sheet-card face). The prior
          // `!t.note_type` blanket exclusion here diverged from that and
          // silently hid sheet notes from every Kanban-render test using this
          // mock. Fixed to mirror the backend predicate exactly; non-sheet
          // note_types (daily, etc.) are still excluded, matching the SQL.
          case 'get_project_notes_for_kanban': {
            const projectId = args.projectId;
            let filtered = mockData.tasks.filter(
              (t: any) => !t.deleted_at && (!t.parent_id || t.parent_id === '') && (!t.note_type || t.note_type === 'sheet')
            );
            if (projectId) {
              filtered = filtered.filter((t: any) => t.project_id === projectId);
            }
            return filtered;
          }

          case 'get_note_task_fields': {
            const noteId = args.noteId;
            const note = mockData.tasks.find(t => t.id === noteId);
            if (!note) {
              return { title: null, task_status: null, task_priority: null, task_due_date: null, task_assignee: null };
            }
            return {
              title: note.title,
              task_status: note.status,
              task_priority: note.priority,
              task_due_date: note.due_date,
              task_assignee: note.assignee,
            };
          }

          // Legacy/compatibility handlers
          case 'list_notebooks':
            return mockData.notebooks;

          case 'create_notebook': {
            const name = args.name;
            if (!mockData.notebooks.includes(name)) {
              mockData.notebooks.push(name);
              mockData.notes[name] = [];
            }
            return name;
          }

          case 'list_notes': {
            const nb = args.notebook;
            if (mockData.notes[nb]) return mockData.notes[nb];
            const lastSeg = nb.split('/').pop();
            if (lastSeg && mockData.notes[lastSeg]) return mockData.notes[lastSeg];
            return [];
          }

          case 'read_note': {
            const readKey = args.notebook + '/' + args.slug;
            const readNotes = mockData.notes[args.notebook] || [];
            const readMeta = readNotes.find(n => n.slug === args.slug);
            return {
              meta: readMeta || { id: readKey, title: '', slug: args.slug, notebook: args.notebook },
              content: mockData.noteContents[readKey] || '',
            };
          }

          case 'read_note_json':
            return mockData.noteJsons[args.notebook + '/' + args.slug] || null;

          case 'save_note': {
            const key = args.notebook + '/' + args.slug;
            mockData.noteContents[key] = args.content || '';
            const notes = mockData.notes[args.notebook] || [];
            const note = notes.find(n => n.slug === args.slug);
            if (note) {
              note.title = args.title || note.title;
              note.updated_at = new Date().toISOString();
              note.content_preview = (args.content || '').substring(0, 200);
              note.word_count = (args.content || '').split(/\s+/).length;
            }
            return null;
          }

          case 'save_note_json': {
            const key = args.notebook + '/' + args.slug;
            mockData.noteJsons[key] = args.jsonContent;
            return null;
          }

          case 'list_tasks_by_project':
            return mockData.tasks.filter(t => t.project_id === args.projectId);

          case 'list_tasks_by_folder':
            return mockData.tasks.filter(t => t.folder_id === args.folderId);

          case 'update_note_task_fields':
            return null; // Just acknowledge — store update is optimistic

          case 'resolve_attachment_path': {
            const id = args.attachmentId || args.attachment_id || '';
            return `/mock/pane/Projects/TestProject/Assets/${id}.bin`;
          }

          // ── Checklist / Subtask commands (§31.3, ADR-0057) ──────────────
          case 'create_subtask': {
            const now = new Date().toISOString();
            const id = 'subtask-' + mockData.nextId++;
            const item = {
              id,
              note_id: args.noteId,
              text: args.text,
              due_date: args.dueDate ?? null,
              done: false,
              completed_at: null,
              sort_order: args.sortOrder ?? 0,
              created_at: now,
              updated_at: now,
            };
            if (!mockData.subtasks) mockData.subtasks = [];
            (mockData.subtasks as any[]).push(item);
            return item;
          }

          case 'get_subtasks_for_note': {
            const subtasks: any[] = mockData.subtasks ?? [];
            return subtasks
              .filter((s: any) => s.note_id === args.noteId)
              .sort((a: any, b: any) => a.sort_order - b.sort_order);
          }

          case 'update_subtask': {
            const subtasks: any[] = mockData.subtasks ?? [];
            const idx = subtasks.findIndex((s: any) => s.id === args.id);
            if (idx === -1) throw new Error(`Subtask not found: ${args.id}`);
            const current = subtasks[idx];
            const now = new Date().toISOString();
            const doneBool = args.done !== null && args.done !== undefined ? args.done : current.done;
            const updated = {
              ...current,
              text: args.text !== null && args.text !== undefined ? args.text : current.text,
              due_date: args.dueDate !== undefined ? args.dueDate : current.due_date,
              done: doneBool,
              completed_at: doneBool && !current.done ? now : (!doneBool && current.done ? null : current.completed_at),
              sort_order: args.sortOrder !== null && args.sortOrder !== undefined ? args.sortOrder : current.sort_order,
              updated_at: now,
            };
            subtasks[idx] = updated;
            return updated;
          }

          case 'delete_subtask': {
            if (!mockData.subtasks) mockData.subtasks = [];
            mockData.subtasks = (mockData.subtasks as any[]).filter((s: any) => s.id !== args.id);
            return null;
          }

          // #2972 stage 2: see `tauriEventListeners`/`simulateTauriEvent`
          // above — registers a `listen(eventName, handler)` call so a
          // test can later fire it via `simulateTauriEvent`.
          case 'plugin:event|listen': {
            const eventName = args.event;
            const handlerId = args.handler;
            const eventId = nextTauriEventId++;
            if (!tauriEventListeners[eventName]) tauriEventListeners[eventName] = [];
            tauriEventListeners[eventName].push({ eventId, handlerId });
            return eventId;
          }

          case 'plugin:event|unlisten': {
            const eventName = args.event;
            const eventId = args.eventId;
            if (tauriEventListeners[eventName]) {
              tauriEventListeners[eventName] = tauriEventListeners[eventName].filter(
                (l) => l.eventId !== eventId
              );
            }
            return null;
          }

          default:
            // console.warn('[TAURI MOCK] Unhandled command:', cmd, args);
            return null;
        }
      },
      convertFileSrc: (path: string) => path,
      transformCallback: (callback: any) => {
        const id = Math.random().toString(36).substr(2, 9);
        (globalThis as any)['_' + id] = callback;
        return id;
      },
      _mockData: mockData, // Expose for test manipulation
    },
  },
  writable: true,
  configurable: true,
});

// Mock Tauri API modules
if (typeof vi !== 'undefined') {
  vi.mock('@tauri-apps/api/core', () => ({
    invoke: (globalThis.window as any)?.__TAURI_INTERNALS__?.invoke || vi.fn(),
    convertFileSrc: (path: string) => `asset://localhost/${path}`,
  }));
}

// Export mock data for tests
export { mockData };