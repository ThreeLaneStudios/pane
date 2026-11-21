// Agent types — §24, ADR-0059
export interface Agent {
  id: string;
  name: string;
  slug: string;
  role: string;
  description?: string | null;
  model?: string | null;
  is_default: boolean;
  is_active: boolean;
  workspace_path: string;
  soul_hash?: string | null;
  memory_hash?: string | null;
  config_json: string;
  owner_id?: string | null;
  runtime_ref?: string | null;  // ADR-0063 D1 (was oc_agent_id): null = locally created; non-null = OC-imported
  runtime_type?: string | null; // ADR-0063 D1: 'anthropic' | 'openrouter' | 'ollama' | 'openclaw'; NULL only as v79 migration legacy-unpinned state (D4.3)
  source: string;
  created_at: string;
  updated_at: string;
}

export interface CreateLocalAgentRequest {
  name: string;
  model?: string;
  description?: string;
  /** ADR-0063 D1 / #2522 Stage 6a: the runtime this new local agent is created under.
   *  One of 'openclaw' | 'openrouter' | 'anthropic' | 'venice' | 'ollama'. Caller must resolve this
   *  before calling createLocalAgent — the backend no longer picks it internally. */
  runtime_type: string;
}

export interface NoteMeta {
  id: string;
  title: string;
  notebook: string;
  slug: string;
  created_at: string;
  updated_at: string;
  is_system?: boolean;
  preview: string;
  content_preview?: string; // Alternative naming used in some parts
  tags?: string[];
  pinned?: boolean;
  word_count?: number;
  reading_time?: number;
  project_id?: string;
  folder_id?: string;
  file_path?: string;
  
  // Task fields - all optional, undefined = not a task
  task_status?: 'cabinet' | 'todo' | 'in_progress' | 'review' | 'done';
  task_priority?: 'low' | 'medium' | 'high' | 'urgent';
  task_due_date?: string;
  task_assignee?: string;
  task_completed_at?: string;
  task_sort_order?: number;
  task_tags?: string;
  note_type?: string;
}

export interface NoteContent {
  meta: NoteMeta;
  content: string;
  jsonContent?: any;
}

export interface TagInfo {
  id: number;
  name: string;
  color: string;
}

export interface CategoryWithColor {
  category: string;
  color: string;
}

export interface NotebookInfo {
  name: string;
  path: string;
  parent?: string;
  children?: NotebookInfo[];
}

export interface Template {
  id: string;
  name: string;
  content: string;
  checklist_json: string;
  created_at: string;
  updated_at: string;
}

export interface TrashItem {
  id: string;
  title: string;
  notebook: string;
  slug: string;
  deleted_at: string;
  original_path: string;
}

export interface Attachment {
  id: string;
  filename: string;
  mime_type: string;
  size: number;
  path: string;
  note_id?: string;
  created_at: string;
}

export type TaskStatus = 'cabinet' | 'todo' | 'in_progress' | 'review' | 'done';
export type TaskPriority = 'low' | 'medium' | 'high' | 'urgent';

export interface Task {
  id: string;
  title: string;
  description?: string;
  status: TaskStatus;
  priority: TaskPriority;
  due_date?: string;
  project?: string;
  project_id?: string;
  folder_id?: string;
  tags: string[];
  assignee?: string;
  parent_task_id?: string;
  sort_order: number;
  created_at: string;
  updated_at: string;
  is_system?: boolean;
  completed_at?: string;
  note_id?: string;
  note_type?: string;
}

export interface TaskDueToday {
  id: string;
  title: string;
  status: string;
  priority?: string;
  due_date: string;
  notebook?: string;
  content_preview?: string;
  task_assignee?: string;
}

export interface Project {
  id: string;
  name: string;
  description?: string;
  color?: string;
  parent_id?: string;
  category?: string | null;
  sort_order: number;
  created_at: string;
  updated_at: string;
  is_system?: boolean;
  is_favorite?: boolean;
  // #2874 item 9: only meaningfully set on a top-level project row — points
  // at one of that project's own folder ids. A folder row itself always
  // has this as undefined/null.
  default_folder_id?: string | null;
}

export interface ProjectStats {
  project_name: string;
  note_count: number;
  task_count: number;
}

export interface NoteSearchResult {
  id: string;
  title: string;
  project: string;
}

// Alias for clarity - folders are projects with parent_id
export type Folder = Project;

export interface User {
  id: string;
  name: string;
  email?: string;
  avatar_url?: string;
  created_at: string;
}

/// First-class chat session (§30.2, #395, ADR-0052).
export interface Session {
  id: string;
  project_id: string;
  title: string;
  // #3155 C49b: widened from 'user' | 'migrated' | 'recovered' — those
  // were the only values `list_sessions` could ever return (job-kind
  // sessions are deliberately excluded from that query, §36.11), but
  // `switch_session`/`get_active_session` (the direct-fetch commands used
  // to OPEN a specific session, e.g. via the Scheduled panel card or the
  // notification bell) can return a job's own dedicated session, whose
  // real backend `origin` column value is `'spawn'` or `'schedule'`
  // (`ai::scheduler::create_job`). The Rust `Session.origin` field
  // (`ai/chat.rs`) is a plain `String`, so these values always arrived
  // over the wire — this type was simply inaccurate for that case.
  origin: 'user' | 'migrated' | 'recovered' | 'spawn' | 'schedule';
  created_at: string;
  last_active_at: string;
  hidden_at: string | null;
  is_active: boolean;
  /**
   * ADR-0062 D1/D3: the agent this session is permanently bound to at
   * creation. `null` only in the narrow pre-migration/self-heal-pending
   * window on the backend (see `commands.rs` Session doc comment) — by the
   * time a session reaches the frontend via `load_session_by_id`/`list_sessions`
   * it has already been self-healed, so in practice this is always set.
   */
  agent_id: string | null;
  /**
   * §36.17.4 (#3050 PR 10, Captain & Crew UI): JSON array of `Agent.id`
   * this session's captain may spawn onto, raw text exactly as stored by
   * the backend (`ai_agents.id` strings) — `null` = no crew. Deliberately
   * NOT pre-parsed into `string[]` here, matching this same interface's
   * own `tool_calls`/`tool_results`-on-`ChatMessage` convention (raw JSON
   * text on the wire type, parsed at the point of use) — see
   * `parseCrewAgentIds` in `$lib/utils/agentDisplay`, the one parse site.
   */
  crew_agent_ids: string | null;
}

export interface ChatThread {
  id: string;
  project_id?: string;
  title: string;
  created_at: string;
  updated_at: string;
  is_system?: boolean;
}

export interface ChatMessage {
  id: string;
  thread_id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  created_at: string;
  attachment_ids?: string;
  tool_calls?: string;
  tool_results?: string;
  model?: string;
  token_count?: number;
  // spec-covered-by: #2030 (ADR-0059 D5, §24.8) — per-message agent attribution
  agent_id?: string | null;
  agent_name?: string | null;
  // §36.2.1 (#3050 PR 3): `load_chat_messages`'s SELECT now returns both
  // (Reed's scope audit §3.6 — the columns existed since migration v93 but
  // nothing selected them, so neither flag could ever reach the UI).
  // `continued=1` renders as the "Continuing — N tool calls so far."
  // progress divider instead of a normal assistant bubble; `synthetic=1`
  // (the "Continue." row a continuation injects) never renders as a
  // visible chat bubble at all. Optional/boolean, not `0 | 1` —
  // `load_chat_messages`'s row-mapper already converts the SQLite integer
  // to a real JSON boolean.
  synthetic?: boolean;
  continued?: boolean;
  // §36.3.6/§36.3.7 (#3050 PR 4): set on the Pane-written §36.3.6
  // confirmation chip ("Scheduled: Monday reminder · next Mon 9:00 AM"),
  // written by `insert_schedule_confirmation_message` on a successful
  // `schedule_job` call. Same optional shape as `synthetic`/`continued`
  // above — `load_chat_messages`'s row-mapper converts the SQLite
  // `Option<String>` directly, `null`/absent for every ordinary message.
  job_id?: string | null;
}

export interface ChatMemory {
  id: string;
  project_id?: string;
  content: string;
  updated_at: string;
  is_system?: boolean;
}

export interface NoteContextResult {
  title: string;
  preview: string;
  content: string;
}

export interface GratitudeEntry {
  id: string;
  entry_date: string;
  entry_index: number;
  text: string;
  created_at: string;
}

// === Sheets (#2339 PR 9, §33.3) ===
// Mirrors src-tauri/src/sheets.rs structs verbatim (field names/casing match
// serde's default snake_case output — no #[serde(rename)] attrs on these
// structs, so no camelCase remapping is needed here).

/** `sheet_columns` row. `col_type` is one of text/number/currency/date/checkbox/select (§33.3.1).
 *  `width` is 0 for freshly-seeded default columns (backend default) — the frontend must apply
 *  a fallback render width (e.g. 148px) when `width === 0`; 0 is never a valid render dimension. */
export interface SheetColumn {
  id: string;
  note_id: string;
  name: string;
  col_type: 'text' | 'number' | 'currency' | 'date' | 'checkbox' | 'select' | string;
  options_json: string;
  width: number;
  sort_order: number;
  created_at: string;
  updated_at: string;
}

/** `sheet_rows` row. `height` is 0 for freshly-created rows (backend default) — the frontend
 *  must apply a fallback render height (e.g. 34px) when `height === 0`. */
export interface SheetRow {
  id: string;
  note_id: string;
  sort_order: number;
  height: number;
  created_at: string;
  updated_at: string;
}

/** `sheet_cells` row — sparse (only populated coordinates have a row). `computed_value` is
 *  present only for formula cells that resolved during the read's recompute pass; falls back
 *  to `raw_value` when absent/undefined. Never mutated by PR 9 (read-only). */
export interface SheetCell {
  row_id: string;
  column_id: string;
  note_id: string;
  raw_value: string;
  computed_value?: string;
  style_json: string;
  updated_at: string;
}

/** Full grid structure returned by `create_sheet` and `get_sheet_data` — columns/rows/cells,
 *  all already ordered by `sort_order` ASC on the backend. `formula_count`/`formulas_paused`
 *  (#2339 PR 14) expose `recompute::FORMULA_CELL_SOFT_LIMIT`'s guard-rail state — drives
 *  `SheetStatusBar`'s `formulas` prop and the `ShPaused` banner between the toolbar and the
 *  formula bar. `raw_value` on each cell in `cells` is already re-rendered in current A1 display
 *  form (ReferenceMapper read path, backend `sheets.rs`'s `translate_cells_for_display`) — never
 *  the stable-ID `{column_id:row_id}` storage form. */
export interface SheetData {
  note_meta: NoteMeta;
  columns: SheetColumn[];
  rows: SheetRow[];
  cells: SheetCell[];
  formula_count: number;
  formulas_paused: boolean;
}

/** Read-only card-face summary returned by `get_sheet_card_summary`/
 *  `get_sheet_card_summaries` (#2339 PR 15a) — counts only, no cell values,
 *  no recompute. Drives the board/list card's micro-grid + `cols × rows`
 *  footer metric without paying for a full `get_sheet_data` grid fetch. */
export interface SheetCardSummary {
  note_id: string;
  col_count: number;
  row_count: number;
  formula_count: number;
  col_types: string[];
}
