# Categories Spec
**Added:** v0.68.43 (#2141)  
**Status:** BUILT, v0.68.43

---

## Overview

Categories are named groups with an associated color, used to visually organize favorited projects in the sidebar. Each project can belong to at most one category. Category definitions sync across devices via CRR.

---

## 1. Data Model

### 1.1 `projects.category` — TEXT, nullable

```sql
category TEXT,  -- assigned category name; NULL = uncategorized
```

- Stored on the project row. CRR-synced (projects table is a CRR table).
- Multiple projects may share the same category value.
- Clearing a category sets this column to NULL.

### 1.2 `category_colors` — CRR table (schema v75)

```sql
CREATE TABLE IF NOT EXISTS category_colors (
    category TEXT PRIMARY KEY,
    color TEXT NOT NULL
);
```

- Stores the display color for each defined category.
- Schema v75: `category_colors` joins `CRR_TABLES` (fresh-install path in `migrations.rs`) and `upgrade_tables_to_crr_inner` (upgrade path in `database.rs`), so category definitions sync across devices on both paths.
- Prior to v75: `category_colors` was local-only.

### 1.3 `projects.color` — reserved, unused

```sql
color TEXT,  -- RESERVED: not used for any current UI display.
             -- Category color comes from category_colors.color, not this column.
             -- Do not read this column for visual rendering.
```

---

## 2. Category CRUD (Settings → Categories)

### 2.1 Create

- User enters a name and picks a color in Settings → Categories.
- Calls `set_category_color(category, color)` → `INSERT OR REPLACE INTO category_colors`.

### 2.2 Update color only

- Same surface. Calls `set_category_color(same_name, new_color)`.

### 2.3 Rename

- User edits an existing category name in Settings → Categories.
- Calls `rename_category(old_name, new_name, color)`.
- Atomic transaction:
  1. `UPDATE category_colors SET category = new_name, color = new_color WHERE category = old_name`
  2. `UPDATE projects SET category = new_name WHERE category = old_name`
  3. COMMIT
- If either step fails, ROLLBACK is issued.

### 2.4 Delete

- Calls `delete_category(name)`.
- `UPDATE projects SET category = NULL WHERE category = name`
- `DELETE FROM category_colors WHERE category = name`

---

## 3. Category Assignment to Projects

### 3.1 ProjectSettings dropdown

- Calls `update_project_settings` with `category` field.
- `Some("")` = clear (set to NULL); `Some("Work")` = assign; `None` = don't touch.
- Frontend sends the raw string value — no `|| null` coercion (#2141 fix).

### 3.2 Context menu

- Calls `set_project_category(project_id, Option<String>)`.
- `None` = clear to NULL, `Some(name)` = assign.

---

## 4. Sidebar Grouping (C1)

Grouping activates when at least one favorited project has a category assigned.

### 4.1 Group structure

- Favorited root projects are grouped by `projects.category`.
- Named groups (non-null category): sorted alphabetically by category name.
- UNCATEGORIZED group (null/empty category): always last, rendered when at least one uncategorized favorite exists.
- Projects within each group: sorted alphabetically by name.
- Empty categories (no favorited projects with that category): no header rendered.

### 4.2 Category header anatomy

- Whole header row = collapse/expand click target.
- Dot (8×8px, border-radius 50%): category color.
- Label: 12px, font-weight 600, letter-spacing 0.06em, uppercase, `color: var(--fg-3)`.
- Hover: label transitions to `var(--fg-2)`.
- Uncategorized: label `color: var(--fg-4)`, `margin-top: 12px`.
- Named group: `padding: 6px 8px 5px`, `margin-top: 6px`, `gap: 9px`.

### 4.3 Collapse persistence

- Per-category collapse state stored in localStorage: key = `sidebar-cat-collapsed:{category}`.
- Uncategorized group key: `sidebar-cat-collapsed:` (empty string).
- Collapsed groups show header only; projects hidden.

### 4.4 Project rows in grouped mode

- `margin-left: 6px` indent relative to flat-list rows (`ps-cat__projects`).
- Category dot always visible in grouped context (opacity 1).
- Dot color = category color; `var(--fg-4)` for uncategorized rows.

### 4.5 Flat fallback

- When no favorited project has any category assigned: flat alphabetical list, no headers.

---

## 5. Collapsed Rail (C2)

When `$sidebarCollapsed` is true and grouping is active:

- One dot per category group rendered in the `.ps-rail` container.
- Dot order: named groups (alpha), then uncategorized.
- Dot: 9×9px circle; color = category color (uncategorized → grey/`var(--fg-4)`).
- Container button: 38×30px, border-radius 8px.
- Clicking any dot expands the sidebar (`$sidebarCollapsed = false`).
- No per-project targets in rail.

---

## 6. Backend Commands

| Command | Signature | Notes |
|---|---|---|
| `list_categories` | `() → Vec<CategoryInfo>` | UNION of projects.category + category_colors, alphabetical |
| `set_category_color` | `(category, color) → ()` | Create or update color |
| `rename_category` | `(old_category, new_category, color) → ()` | Atomic rename, propagates to projects |
| `delete_category` | `(category) → ()` | Clears projects.category, removes color |
| `set_project_category` | `(project_id, Option<String>) → ()` | Set or clear category on a single project |
| `update_project_settings` | `(..., category: Option<String>, ...) → ()` | `Some("")` = clear, `Some(v)` = set, `None` = skip |

---

## 7. Sync Behavior

- `projects` table: CRR. Category assignments (`projects.category`) sync across devices.
- `category_colors` table: CRR (since v75). Category definitions sync across devices.
- Prior to v75: category definitions were local-only. Projects on other devices showed `#00E4E4` fallback dots.
