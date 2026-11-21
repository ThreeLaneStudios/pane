/**
 * Injects a mock Tauri API into the browser context so the SvelteKit app
 * can run without the actual Tauri runtime. This enables E2E testing
 * against the plain dev server.
 */
export function getTauriMockScript(): string {
  return `
    // In-memory storage for mock data (exposed for test seeding)
    const mockData = {
      notebooks: [],
      notes: {},       // notebook -> NoteMeta[]
      noteContents: {}, // "notebook/slug" -> content
      noteJsons: {},    // "notebook/slug" -> json
      tags: [],
      projects: [],
      trash: [],
      nextId: 1,
    };

    // ADR-0070 (#3195 PR B): account/sync IPC recorder for
    // \`assertLocalFirstBoot\` (home-fixtures.ts). Every command that
    // touches session storage, the master key, or Supabase — the exact
    // set \`require_account_enabled()\` guards in \`sync/commands.rs\` — is
    // recorded here, regardless of whether this mock answers it or falls
    // through. An empty array after boot is the mocked-harness half of
    // "no network call in the launch path."
    //
    // #3218 item 9: this list is a MANUALLY KEPT duplicate of the Rust-side
    // \`E2E_ACCOUNT_SYNC_RECORDER_COMMANDS\` constant in
    // \`src-tauri/src/sync/commands.rs\`'s \`account_enabled_guard_tests\`
    // module, which source-derives the real guarded set and asserts it
    // against that constant. If you change either list, change both, or
    // the Rust test goes red. (get_auth_session and is_encryption_ready
    // were removed here 2026-11 — confirmed ungated, drift fix for #3218.)
    window.__PANE_ACCOUNT_SYNC_CALLS__ = [];
    const ACCOUNT_SYNC_CMD_PATTERN = /^(auth_sign_in|auth_sign_out|auth_refresh|auth_request_password_reset|auth_change_password|register_device_cmd|save_auth_session|clear_auth_session|init_encryption|clear_encryption|test_encryption|sync_push|sync_pull|sync_configure|sync_files_push|sync_files_pull|start_auto_sync|check_cleanup_pending|write_cleanup_flag|clear_cleanup_flag)$/;

    // Mock the Tauri invoke system
    window.__TAURI_INTERNALS__ = {
      invoke: async (cmd, args) => {
        // console.log('[TAURI MOCK]', cmd, args);
        if (ACCOUNT_SYNC_CMD_PATTERN.test(cmd)) {
          window.__PANE_ACCOUNT_SYNC_CALLS__.push(cmd);
        }
        switch (cmd) {
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

          case 'delete_notebook':
            mockData.notebooks = mockData.notebooks.filter(n => n !== args.name);
            delete mockData.notes[args.name];
            return null;

          case 'list_notes': {
            const nb = args.notebook;
            // Support both flat ("Notes") and path-style ("Test Project/Notes") lookups
            if (mockData.notes[nb]) return mockData.notes[nb];
            // Try the last segment as fallback
            const lastSeg = nb.split('/').pop();
            if (lastSeg && mockData.notes[lastSeg]) return mockData.notes[lastSeg];
            return [];
          }

          case 'create_note': {
            let nb = args.notebook;
            // Normalize notebook: find the full path version if a short name was used
            // e.g., 'Notes' -> 'Test Project/Notes' if that key exists
            if (mockData.notes[nb] === undefined || nb.indexOf('/') === -1) {
              for (const key of Object.keys(mockData.notes)) {
                if (key.endsWith('/' + nb) && key !== nb) {
                  nb = key;
                  break;
                }
              }
            }
            const title = args.title || 'Untitled';
            const baseSlug = title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
            const slug = baseSlug ? baseSlug + '-' + mockData.nextId++ : 'note-' + mockData.nextId++;
            const id = nb + '/' + slug;
            const now = new Date().toISOString();
            if (!mockData.notes[nb]) mockData.notes[nb] = [];
            const meta = {
              id, title, slug, notebook: nb,
              content_preview: '', word_count: 0,
              tags: [], is_pinned: false,
              created_at: now, updated_at: now,
              file_path: '',
              task_status: null, task_priority: null, task_due_date: null,
            };
            mockData.notes[nb].push(meta);
            mockData.noteContents[id] = '';
            return { meta, content: '' };
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
            // Update meta
            const notes = mockData.notes[args.notebook] || [];
            const note = notes.find(n => n.slug === args.slug);
            if (note) {
              note.title = args.title || note.title;
              note.updated_at = new Date().toISOString();
              note.content_preview = (args.content || '').substring(0, 200);
              note.word_count = (args.content || '').split(/\\s+/).length;
            }
            return null;
          }

          case 'save_note_json': {
            const key = args.notebook + '/' + args.slug;
            mockData.noteJsons[key] = args.jsonContent;
            return null;
          }

          case 'delete_note': {
            const nb2 = args.notebook;
            mockData.notes[nb2] = (mockData.notes[nb2] || []).filter(n => n.slug !== args.slug);
            delete mockData.noteContents[nb2 + '/' + args.slug];
            delete mockData.noteJsons[nb2 + '/' + args.slug];
            return null;
          }

          case 'search_notes':
            return [];

          case 'list_tags':
            return mockData.tags;

          case 'create_tag': {
            const tag = { id: mockData.nextId++, name: args.name, color: args.color || '#6B7280' };
            mockData.tags.push(tag);
            return tag;
          }

          case 'add_tag':
          case 'remove_tag':
          case 'toggle_pin_note':
            return null;

          case 'get_notes_by_tag':
            return [];

          case 'get_note_backlinks':
            return [];

          case 'get_all_note_titles': {
            const allTitles = [];
            for (const nb of Object.keys(mockData.notes)) {
              for (const n of mockData.notes[nb]) {
                allTitles.push(n.title);
              }
            }
            return allTitles;
          }

          case 'upload_attachment':
            return 'mock-attachment-url';

          case 'list_templates':
            return [];

          case 'create_note_from_template':
            return null;

          case 'list_notebooks_nested':
            return mockData.notebooks.map(n => ({ name: n, children: [] }));

          case 'move_to_trash': {
            // Find note by ID across all notebooks
            for (const nb of Object.keys(mockData.notes)) {
              const idx = mockData.notes[nb].findIndex(n => n.id === args.noteId);
              if (idx !== -1) {
                const [note] = mockData.notes[nb].splice(idx, 1);
                mockData.trash.push(note);
                break;
              }
            }
            return null;
          }

          case 'list_trash':
            return mockData.trash;

          case 'restore_from_trash':
          case 'delete_permanently':
          case 'empty_trash':
            return null;

          case 'export_note_markdown':
            return mockData.noteContents[args.notebook + '/' + args.slug] || '';

          case 'quick_capture_note':
            return null;

          // Project commands
          case 'list_projects':
            return mockData.projects;

          case 'create_project': {
            const now = new Date().toISOString();
            const proj = {
              id: 'proj_' + mockData.nextId++,
              name: args.name,
              description: null,
              color: null,
              parent_id: args.parentId || null,
              sort_order: mockData.projects.length,
              created_at: now,
              updated_at: now,
              archived_at: null,
            };
            mockData.projects.push(proj);
            // Also create a notebook for this project
            if (!mockData.notebooks.includes(args.name)) {
              mockData.notebooks.push(args.name);
              mockData.notes[args.name] = [];
            }
            return proj;
          }

          case 'update_project':
          case 'delete_project':
            return null;

          case 'get_project_notes': {
            const projNotes = [];
            // Return all notes from all notebooks for now
            for (const nb of Object.keys(mockData.notes)) {
              projNotes.push(...mockData.notes[nb]);
            }
            return projNotes;
          }

          case 'get_folder_contents': {
            const folderId = args.folderId || args.folder_id;
            const proj = mockData.projects.find(p => p.id === folderId);
            const folderName = proj ? proj.name : '';
            return {
              notes: mockData.notes[folderName] || [],
              subfolders: [],
              tasks: [],
            };
          }

          case 'list_tasks':
          case 'get_tasks_today':
          case 'list_tasks_by_folder':
          case 'list_task_notes':
          case 'list_task_notes_by_folder':
            return [];

          case 'list_notes_by_folder': {
            // Return notes from all notebooks (simplified)
            const allNotes = [];
            for (const nb of Object.keys(mockData.notes)) {
              allNotes.push(...mockData.notes[nb]);
            }
            return allNotes;
          }

          case 'get_project_stats':
            return { note_count: 0, task_count: 0, word_count: 0 };

          case 'create_task':
          case 'update_task':
          case 'delete_task':
            return null;

          case 'sync_projects_with_notebooks':
            return mockData.projects;

          case 'discover_peers':
            return [];

          case 'list_categories':
            return [];

          case 'get_or_create_thread':
            return { id: 'thread_' + mockData.nextId++, title: 'Chat', project_id: args?.projectId || null, created_at: new Date().toISOString(), updated_at: new Date().toISOString() };

          case 'get_ai_setting':
            return null;

          case 'set_ai_setting':
            return null;

          case 'ai_chat':
            return 'Mock AI response';

          case 'list_chat_threads':
            return [];

          case 'create_chat_thread':
            return { id: 'thread_' + mockData.nextId++, title: 'Chat', messages: [] };

          case 'get_chat_messages':
            return [];

          case 'send_chat_message':
            return { id: 'msg_' + mockData.nextId++, role: 'assistant', content: 'Mock response' };

          case 'get_project_category':
            return null;

          case 'set_project_category':
            return null;

          case 'get_all_project_categories':
            return {};

          case 'reorder_projects':
            return null;

          default:
            console.warn('[TAURI MOCK] Unhandled command:', cmd, args);
            return null;
        }
      },
      convertFileSrc: (path) => path,
      transformCallback: (callback) => {
        const id = Math.random().toString(36).substr(2, 9);
        window['_' + id] = callback;
        return id;
      },
    };

    // Expose mock data for test seeding
    window.__TAURI_INTERNALS__._mockData = mockData;

    // Also mock the plugin:event system
    window.__TAURI_INTERNALS__.metadata = {
      currentWindow: { label: 'main' },
      currentWebview: { label: 'main' },
    };
  `;
}
