# Pane UI Design Brief — "Bring the Soul Screen to Everything"
*Created: Mar 8, 2026 | Requested by Matt*

## Goal
The onboarding Soul screen (see pane-soul-onboarding.png) is the best-looking screen in the app. Matt wants the rest of the app to feel this clean. Design three key states that bring that calm, focused aesthetic while keeping all functionality accessible.

## Reference Screenshots
- pane-soul-onboarding.png — THE TARGET. Clean two-panel layout, no sidebar, generous typography, content + chat only.
- pane-project-view.png — Current project view. Three columns, busy, lots of chrome.
- pane-home-view.png — Current home view. Three columns, empty center, sidebar.

## What Makes the Soul Screen Better
1. No sidebar — all attention on content + chat
2. Two panels only — document left, chat right (~55/45 split)
3. Generous typography — ~15-16px body, 1.6-1.7 line-height, 20-24px paragraph spacing
4. Minimal chrome — one progress indicator, one CTA button, nothing else
5. Purposeful whitespace — empty space feels restful, not incomplete
6. Single-purpose focus — one job per screen
7. No cards/filters/tabs competing — content dominates

## Design Constraints
- Dark Tron aesthetic: #0a0a0f background, #00E4E4 accent. No emojis in UI.
- Must keep: sidebar (projects/nav), note list, editor, AI chat panel
- The sidebar, note list, and editor toolbar are functional requirements — they can be minimized/collapsed/simplified but not removed

## Three States to Design

### State 1: Home
- User opens Pane. What do they see?
- Left: sidebar (can it collapse to icons by default?)
- Center: daily dashboard (morning briefing, modules)
- Right: AI chat (workspace-level, cross-project awareness)
- Inspiration: Soul screens breathing room with dashboard content in center

### State 2: Project View (note list)
- User clicks into a project. Sees their notes.
- Left: sidebar (collapsed or slim?)
- Center: note cards/list
- Right: AI chat (project-scoped)
- Key question: can the note list feel less like a dashboard and more like a clean index?
- Current filter pills (All, AI, General) + tabs (All, Cabinet, Tasks) feel heavy

### State 3: Note Open (editor + chat)
- User opens a note. THE most important state — this is where work happens.
- Left: sidebar (collapse?)
- Center: TipTap editor with the note content
- Right: AI chat
- THIS should feel closest to the Soul screen — two panels, content + chat, generous typography
- Note list should slide away or minimize when a note is open
- Editor typography should match Soul: bigger text, more line-height, more paragraph spacing
- When AI edits the note, green diff highlights appear (already working)

## Specific Design Explorations

### Sidebar
- Explore: collapsible to icon-only rail (~48px), expands on hover or click
- Explore: auto-hide when note is open, show on hover at left edge
- Current: always visible at ~170px, takes space even when not needed

### Note List to Editor Transition
- When no note is selected: show note list (State 2)
- When note is selected: note list slides away, editor takes full center width (State 3)
- Back button or breadcrumb to return to note list
- This is the key to matching the Soul screens two-panel feel

### Chat Panel
- Reduce header chrome — "AI CHAT → Passat" could be subtler (just project name, smaller)
- More padding in messages (match onboarding: 16px/20px, 12px gap)
- Pinned messages: thin teal banner at top of chat showing count, expandable on click
- "Pane" sender label on assistant messages (already there)

### Editor
- Match Soul screen typography: ~15-16px body, 1.6-1.7 line-height
- More padding around content area
- Toolbar: minimal, maybe hidden by default, appears on text selection (floating toolbar)
- Title: large, bold, clean — like the Soul screens title

## Color Palette (existing)
- Background: #0a0a0f
- Surface/cards: #1a1a2e
- Accent: #00E4E4 (teal/cyan)
- Text primary: #e0e0e0
- Text muted: #888
- Pinned highlight: teal left border + subtle teal tint

## Deliverables
1. Mockup: Home (State 1)
2. Mockup: Project view with note list (State 2)
3. Mockup: Note open with editor + chat (State 3)
4. Mockup: Pinned messages UI in chat
5. Optional: sidebar collapsed vs expanded states

## Format
HTML mockups preferred (renderable in browser). Dark theme mandatory. Use screenshots as reference for current state.
