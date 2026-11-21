# Pane Design Implementation Spec
*Created: Mar 8, 2026 | Source of truth for the design pass (Sprint 1, Item 3)*

## Approved Designs
Reference mockups: ~/Shared/ExampleApp/docs/design/mockups/
- project-list-expanded.html — APPROVED (full sidebar)
- note-editor.html — APPROVED (back breadcrumb, toolbar, transparent gray)
- kanban.html — APPROVED
- home.html — APPROVED (placeholder modules, will iterate)
- sidebar-project-open.html — REJECTED (no file tree in sidebar)

## Color Palette (LOCKED)
- Background: #0a0a0f
- Surface/cards/bubbles: rgba(255,255,255,0.05) — whisper gray, barely visible
- Accent: #00E4E4 (teal/cyan)
- Text primary: #e0e0e0
- Text muted: #888888
- Hover surface: rgba(255,255,255,0.08)
- Active/selected: teal highlight or teal left border
- No emojis in UI

## Layout Architecture

### Sidebar
- Default: expanded (~220px) with project list visible
- Collapsible to icon rail (~48px) via toggle button or Cmd+B
- Icons: Home, All Projects, All Tasks, Calendar, Trash, Settings
- Projects listed under PROJECTS header with teal highlight on active
- Collapse/expand: 200ms ease transition
- Remember state in SQLite settings

### Three App States

**State 1: Home**
- Sidebar (expanded) | Dashboard modules (center) | AI Chat (right)
- Dashboard: placeholder grid for modules, "Add Module" button
- Chat: workspace-scoped, cross-project awareness
- Chat closeable via toggle (Cmd+\)

**State 2: Project View (cards)**
- Sidebar (expanded) | Note cards (center) | AI Chat (right)
- Filter row: folder pills (All, AI, General, etc.)
- Tab row: All, Cabinet, Tasks
- Kanban toggle button in top-right of card area
- Chat: project-scoped
- Chat closeable

**State 2b: Kanban View**
- Sidebar (expanded) | Kanban columns (center, horizontally scrollable) | AI Chat (right)
- Columns: Cabinet, In Progress, Done (mapped to note status)
- Cards: transparent gray, title + metadata
- Horizontal scroll independent of chat panel
- Chat overlays right side, kanban scrolls underneath
- Chat closeable — when closed, kanban gets full width

**State 3: Note Editor**
- Sidebar (expanded or collapsed) | Editor (center) | AI Chat (right)
- "← Project Name" breadcrumb above title
- Note cards disappear when editor opens
- Editor typography: 15-16px body, 1.6-1.7 line-height, generous padding
- Slim toolbar under title/metadata, above body
- Escape or breadcrumb click returns to card view
- Chat: project-scoped, note context loaded

### Transitions Between States
- Home → Project: sidebar highlights project, center swaps to cards (200ms fade)
- Project → Note: cards slide out left, editor slides in (250ms ease)
- Note → Project: editor slides out, cards slide in (250ms ease)
- All transitions: smooth, not instant

## Resizable Panels
- Thin drag handle (4px, visible on hover as subtle teal line) between:
  - Sidebar and center content
  - Center content and AI chat
- Drag to resize. Min widths: sidebar 48px (collapsed), center 400px, chat 280px
- Store user-preferred widths in SQLite settings (persist across sessions)
- Double-click handle to reset to default widths

## Chat Panel
- Closeable/toggleable via button or Cmd+\
- When closed: center content gets full remaining width
- When open: default ~380px right panel
- Header: subtle — just "Pane" + project name in small text
- Pinned messages: thin teal banner at top with count, expandable
- Messages: 16px/20px padding, 12px gap between messages
- "Pane" sender label on assistant messages in teal

## Responsive Behavior
- Window width >= 1200px: sidebar + center + chat (three columns)
- Window width 900-1199px: sidebar auto-collapses to icon rail, center + chat
- Window width < 900px: icon rail + center only, chat becomes toggle overlay panel
- Sidebar collapse saves space for content on smaller screens

## Keyboard Shortcuts

### Global
| Shortcut | Action |
|----------|--------|
| Cmd+B | Toggle sidebar collapse/expand |
| Cmd+\ | Toggle AI chat panel |
| Cmd+P | Quick Switcher (search) |
| Cmd+N | New note (in current project) |
| Cmd+Shift+N | New project |
| Cmd+K | Command palette (future) |
| Cmd+, | Settings |
| Cmd+1 | Go to Home |
| Cmd+2 | Go to All Projects |
| Cmd+3 | Go to All Tasks |
| Escape | Back (editor → cards, close modal, close search) |

### Note Editor
| Shortcut | Action |
|----------|--------|
| Cmd+B | Bold (when editor focused) |
| Cmd+I | Italic |
| Cmd+U | Underline |
| Cmd+Shift+X | Strikethrough |
| Cmd+Shift+7 | Numbered list |
| Cmd+Shift+8 | Bullet list |
| Cmd+Shift+9 | Toggle task/checkbox |
| Cmd+E | Inline code |
| Cmd+Shift+E | Code block |
| Cmd+K | Insert link (when editor focused) |
| Cmd+Enter | Send to AI chat (focus jumps to chat input) |
| Tab | Indent list item |
| Shift+Tab | Outdent list item |
| Cmd+Z | Undo |
| Cmd+Shift+Z | Redo |

### AI Chat
| Shortcut | Action |
|----------|--------|
| Enter | Send message |
| Shift+Enter | New line in message |
| Cmd+/ | Focus chat input from anywhere |
| Up arrow | Edit last message (when input empty) |

### Kanban
| Shortcut | Action |
|----------|--------|
| Left/Right arrow | Move between columns (when card selected) |
| Enter | Open selected card in editor |

## AI Navigation Tools (new/updated)
The AI must be able to navigate to all views. Required tools:

| Tool | Action |
|------|--------|
| navigate_home | Go to Home view |
| navigate_project | Go to project card view (by name or ID) |
| navigate_note | Open a specific note in editor (already exists as open_note) |
| navigate_all_projects | Go to All Projects view |
| navigate_all_tasks | Go to All Tasks view |
| navigate_kanban | Switch to kanban view for current project |
| set_filter | Set card view filter (folder: All/AI/General, type: All/Cabinet/Tasks) |
| create_project | Create a new project (already exists) |
| star_project | Toggle star/favorite on a project |

## Editor Toolbar
- Position: fixed, under title + metadata, above body
- Height: ~36px, slim
- Background: transparent (blends with editor background)
- Buttons: text-based icons, muted color (#666), brighten on hover (#e0e0e0)
- Dividers: thin vertical lines between button groups
- Groups: [B I S] | [H1 H2 H3] | [UL OL Task] | [Link Code CodeBlock] | [Undo Redo]
- Active state: teal color on active formatting (e.g., bold text selected, B button is teal)

## Implementation Order
1. Color change (transparent gray surfaces) — quick CSS pass
2. Sidebar expand/collapse with animation
3. Resizable panels with drag handles
4. Card → editor transition (slide animation, breadcrumb)
5. Editor typography (match Soul screen)
6. Chat panel toggle (Cmd+\)
7. Keyboard shortcuts
8. Kanban horizontal scroll + chat overlay
9. Editor toolbar (slim, under title)
10. AI navigation tools
11. Responsive breakpoints
