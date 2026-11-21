# Pane Design Mockups

Self-contained HTML mockups for the Pane UI. Dark Tron aesthetic: `#0a0a0f` background, `#00E4E4` teal accent, transparent gray surfaces (`rgba(255,255,255,0.05)`).

## Mockups

### home.html
Dashboard / home screen. Greeting, recent notes, tasks due, workspace stats. Icon rail sidebar (collapsed). Right panel: AI chat (general scope).

### project-list.html
Project card view with collapsed icon rail. Shows note cards for a selected project (Passat) with filter pills (All, General, Cabinet, Tasks). Right panel: AI chat (project-scoped).

### project-list-expanded.html
Same as project-list.html but with the sidebar **expanded** (~220px). Shows full nav: Home, All Projects, All Tasks, Calendar, Trash, then a Projects section listing all project names. Selected project (Passat) highlighted with teal. Demonstrates the nav state when user clicks the Projects icon on the rail.

### note-editor.html
Full note editor with rich text toolbar. Back breadcrumb ("← Passat") above the title returns to card view. Pinned messages banner in chat. Right panel: AI chat (project-scoped). Shows diff-highlighted content from AI edits.

### kanban.html
Kanban board view for a project. Three columns: Cabinet (backlog), In Progress, Done. Cards use transparent gray surfaces with generous padding. Teal accents on active status tags, green for complete. Column separation via whitespace and subtle bottom-borders. Collapsed icon rail. Right panel: AI chat.

### sidebar-project-open.html
Expanded sidebar acting as a file tree. Project "Passat" is open showing folders (General, Cabinet, Tasks) and individual notes indented underneath. A note is selected and open in the center editor. Demonstrates the alternative navigation pattern where the sidebar replaces breadcrumb-based drilling.

### color-options.html
Color exploration mockup showing different accent/surface combinations (reference only).

## Design Tokens

| Token | Value | Usage |
|-------|-------|-------|
| Background | `#0a0a0f` | App background |
| Surface | `rgba(255,255,255,0.05)` | Cards, bubbles, inputs — barely visible |
| Accent | `#00E4E4` | Teal highlights, active states, links |
| Accent dim | `rgba(0,228,228,0.08)` | Accent backgrounds, badges |
| Text | `#e0e0e0` | Primary text |
| Muted | `#888` | Secondary text, timestamps |
| Border | `rgba(255,255,255,0.06)` | Subtle dividers |
