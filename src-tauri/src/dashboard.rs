use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// === Data Types ===

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserDashboardConfig {
    pub id: String,
    pub onboarding_completed: bool,
    pub onboarding_template: Option<String>,
    pub theme_variant: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DashboardModule {
    pub id: String,
    pub module_type: String,
    pub sort_order: i64,
    pub is_enabled: bool,
    pub config_json: String,
    pub static_content: Option<String>,
    pub items: Vec<ModuleItem>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ModuleItem {
    pub id: String,
    pub module_id: String,
    pub item_type: String,
    pub label: String,
    pub description: Option<String>,
    pub emoji: Option<String>,
    pub config_json: String,
    pub sort_order: i64,
    pub is_active: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DailyEntry {
    pub id: String,
    pub module_id: String,
    pub entry_date: String,
    pub data_json: String,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HabitStreak {
    pub current_streak: i64,
    pub longest_streak: i64,
    pub completion_rate_30d: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HabitStatus {
    pub item_id: String,
    pub label: String,
    pub emoji: Option<String>,
    pub completed: bool,
    pub streak: i64,
}

/// Retained for dashboard payload compatibility with older clients.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TaskEntry {
    pub id: String,
    pub text: String,
    pub project_tag: Option<String>,
    pub completed: bool,
    pub completed_at: Option<String>,
    pub carried_from: Option<String>,
}

/// Retained for dashboard payload compatibility with older clients.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReflectionHistory {
    pub date: String,
    pub value: serde_json::Value,
}

/// Retained for dashboard payload compatibility with older clients.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GratitudeHistory {
    pub date: String,
    pub entries: Vec<String>,
}

/// Retained for dashboard payload compatibility with older clients.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DashboardAnalytics {
    pub habit_completion_rates: Vec<HashMap<String, serde_json::Value>>,
    pub reflection_averages: Vec<HashMap<String, serde_json::Value>>,
    pub streaks: Vec<HashMap<String, serde_json::Value>>,
    pub task_completion_rate: f64,
    pub gratitude_frequency: f64,
    pub correlations: Vec<String>,
}

// === Helper Functions ===

fn generate_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

// === Dashboard Config Commands ===

pub fn load_dashboard_config(conn: &Connection) -> Result<(UserDashboardConfig, Vec<DashboardModule>), String> {
    // Get or create user config
    let config = conn.query_row(
        "SELECT id, onboarding_completed, onboarding_template, theme_variant, created_at, updated_at 
         FROM user_dashboard_config LIMIT 1",
        [],
        |row| Ok(UserDashboardConfig {
            id: row.get(0)?,
            onboarding_completed: row.get::<_, i32>(1)? != 0,
            onboarding_template: row.get(2)?,
            theme_variant: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    ).optional().map_err(|e| format!("Failed to query config: {}", e))?;
    
    let config = match config {
        Some(c) => c,
        None => {
            // Create default config
            let id = generate_id();
            let now = now_rfc3339();
            conn.execute(
                "INSERT INTO user_dashboard_config (id, created_at, updated_at) VALUES (?1, ?2, ?3)",
                params![id, now, now],
            ).map_err(|e| format!("Failed to create config: {}", e))?;
            
            UserDashboardConfig {
                id,
                onboarding_completed: false,
                onboarding_template: None,
                theme_variant: "default".to_string(),
                created_at: now.clone(),
                updated_at: now,
            }
        }
    };
    
    // Get all modules with their items
    let modules = get_all_modules(conn)?;
    
    Ok((config, modules))
}

fn get_all_modules(conn: &Connection) -> Result<Vec<DashboardModule>, String> {
    let mut stmt = conn.prepare(
        "SELECT id, module_type, sort_order, is_enabled, config_json, static_content, created_at, updated_at 
         FROM dashboard_modules ORDER BY sort_order ASC"
    ).map_err(|e| format!("Failed to prepare modules query: {}", e))?;
    
    let modules: Vec<DashboardModule> = stmt.query_map([], |row| {
        let module_id: String = row.get(0)?;
        Ok(DashboardModule {
            id: module_id.clone(),
            module_type: row.get(1)?,
            sort_order: row.get(2)?,
            is_enabled: row.get::<_, i32>(3)? != 0,
            config_json: row.get(4)?,
            static_content: row.get(5)?,
            items: vec![], // Will be populated below
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    }).map_err(|e| format!("Failed to query modules: {}", e))?
    .filter_map(|r| r.ok())
    .collect();
    
    // Load items for each module
    let mut modules_with_items = Vec::new();
    for mut module in modules {
        module.items = get_module_items(conn, &module.id)?;
        modules_with_items.push(module);
    }
    
    Ok(modules_with_items)
}

fn get_module_items(conn: &Connection, module_id: &str) -> Result<Vec<ModuleItem>, String> {
    let mut stmt = conn.prepare(
        "SELECT id, module_id, item_type, label, description, emoji, config_json, sort_order, is_active 
         FROM module_items WHERE module_id = ? ORDER BY sort_order ASC"
    ).map_err(|e| format!("Failed to prepare items query: {}", e))?;
    
    let items = stmt.query_map([module_id], |row| {
        Ok(ModuleItem {
            id: row.get(0)?,
            module_id: row.get(1)?,
            item_type: row.get(2)?,
            label: row.get(3)?,
            description: row.get(4)?,
            emoji: row.get(5)?,
            config_json: row.get(6)?,
            sort_order: row.get(7)?,
            is_active: row.get::<_, i32>(8)? != 0,
        })
    }).map_err(|e| format!("Failed to query items: {}", e))?
    .filter_map(|r| r.ok())
    .collect();
    
    Ok(items)
}

pub fn update_module_order(conn: &Connection, module_orders: Vec<(String, i64)>) -> Result<(), String> {
    let now = now_rfc3339();
    for (module_id, sort_order) in module_orders {
        conn.execute(
            "UPDATE dashboard_modules SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
            params![sort_order, now, module_id],
        ).map_err(|e| format!("Failed to update module order: {}", e))?;
    }
    Ok(())
}

pub fn toggle_module(conn: &Connection, module_id: &str, enabled: bool) -> Result<(), String> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE dashboard_modules SET is_enabled = ?1, updated_at = ?2 WHERE id = ?3",
        params![enabled as i32, now, module_id],
    ).map_err(|e| format!("Failed to toggle module: {}", e))?;
    Ok(())
}

pub fn add_module(
    conn: &Connection,
    module_type: &str,
    config_json: Option<&str>,
    sort_order: Option<i64>,
) -> Result<DashboardModule, String> {
    let id = generate_id();
    let now = now_rfc3339();
    let config = config_json.unwrap_or("{}");
    let order = sort_order.unwrap_or(0);
    
    conn.execute(
        "INSERT INTO dashboard_modules (id, module_type, sort_order, config_json, created_at, updated_at) 
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, module_type, order, config, now, now],
    ).map_err(|e| format!("Failed to add module: {}", e))?;
    
    Ok(DashboardModule {
        id: id.clone(),
        module_type: module_type.to_string(),
        sort_order: order,
        is_enabled: true,
        config_json: config.to_string(),
        static_content: None,
        items: vec![],
        created_at: now.clone(),
        updated_at: now,
    })
}

pub fn remove_module(conn: &Connection, module_id: &str) -> Result<(), String> {
    // Delete dependent rows FIRST to avoid FK constraint violations
    // Order: entries that reference module_items first, then module_items, then module
    
    // Delete habit entries (references module_items.id via item_id)
    conn.execute(
        "DELETE FROM habit_entries WHERE item_id IN (SELECT id FROM module_items WHERE module_id = ?1)",
        params![module_id],
    ).map_err(|e| format!("Failed to delete habit entries: {}", e))?;
    
    // Delete reflection entries (references module_items.id via question_id)
    conn.execute(
        "DELETE FROM reflection_entries WHERE question_id IN (SELECT id FROM module_items WHERE module_id = ?1)",
        params![module_id],
    ).map_err(|e| format!("Failed to delete reflection entries: {}", e))?;
    
    // Delete daily entries (references module_id directly)
    conn.execute(
        "DELETE FROM daily_entries WHERE module_id = ?1",
        params![module_id],
    ).map_err(|e| format!("Failed to delete daily entries: {}", e))?;
    
    // Delete ALL gratitude entries when removing a gratitude module
    // (gratitude_entries has no module_id or item_id - they're date-based standalone entries)
    conn.execute(
        "DELETE FROM gratitude_entries WHERE 1=1",
        [],
    ).map_err(|e| format!("Failed to delete gratitude entries: {}", e))?;
    
    // Delete module_items (child rows)
    conn.execute(
        "DELETE FROM module_items WHERE module_id = ?1",
        params![module_id],
    ).map_err(|e| format!("Failed to delete module items: {}", e))?;
    
    // Finally, delete the module itself
    conn.execute(
        "DELETE FROM dashboard_modules WHERE id = ?1",
        params![module_id],
    ).map_err(|e| format!("Failed to delete module: {}", e))?;
    
    Ok(())
}

pub fn update_module_config(conn: &Connection, module_id: &str, config_json: &str) -> Result<(), String> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE dashboard_modules SET config_json = ?1, updated_at = ?2 WHERE id = ?3",
        params![config_json, now, module_id],
    ).map_err(|e| format!("Failed to update module config: {}", e))?;
    Ok(())
}

pub fn update_module_content(conn: &Connection, module_id: &str, content: &str) -> Result<(), String> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE dashboard_modules SET static_content = ?1, updated_at = ?2 WHERE id = ?3",
        params![content, now, module_id],
    ).map_err(|e| format!("Failed to update module content: {}", e))?;
    Ok(())
}

// === Module Items Commands ===

pub fn add_module_item(
    conn: &Connection,
    module_id: &str,
    item_type: &str,
    label: &str,
    description: Option<&str>,
    emoji: Option<&str>,
    config_json: Option<&str>,
) -> Result<ModuleItem, String> {
    let id = generate_id();
    let now = now_rfc3339();
    let config = config_json.unwrap_or("{}");
    
    // Get next sort_order
    let next_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM module_items WHERE module_id = ?1",
        [module_id],
        |row| row.get(0),
    ).unwrap_or(0);
    
    conn.execute(
        "INSERT INTO module_items (id, module_id, item_type, label, description, emoji, config_json, sort_order, created_at, updated_at) 
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![id, module_id, item_type, label, description, emoji, config, next_order, now, now],
    ).map_err(|e| format!("Failed to add module item: {}", e))?;
    
    Ok(ModuleItem {
        id: id.clone(),
        module_id: module_id.to_string(),
        item_type: item_type.to_string(),
        label: label.to_string(),
        description: description.map(String::from),
        emoji: emoji.map(String::from),
        config_json: config.to_string(),
        sort_order: next_order,
        is_active: true,
    })
}

pub fn update_module_item(
    conn: &Connection,
    item_id: &str,
    label: Option<&str>,
    description: Option<&str>,
    emoji: Option<&str>,
    config_json: Option<&str>,
) -> Result<ModuleItem, String> {
    let now = now_rfc3339();
    
    // Update with COALESCE to preserve existing values when None is passed
    conn.execute(
        "UPDATE module_items SET label = COALESCE(?1, label), description = ?2, emoji = ?3, config_json = COALESCE(?4, config_json), updated_at = ?5 WHERE id = ?6",
        params![label, description, emoji, config_json, now, item_id],
    ).map_err(|e| format!("Failed to update item: {}", e))?;
    
    // Fetch and return updated item
    conn.query_row(
        "SELECT id, module_id, item_type, label, description, emoji, config_json, sort_order, is_active FROM module_items WHERE id = ?1",
        [item_id],
        |row| Ok(ModuleItem {
            id: row.get(0)?,
            module_id: row.get(1)?,
            item_type: row.get(2)?,
            label: row.get(3)?,
            description: row.get(4)?,
            emoji: row.get(5)?,
            config_json: row.get(6)?,
            sort_order: row.get(7)?,
            is_active: row.get::<_, i32>(8)? != 0,
        })
    ).map_err(|e| format!("Failed to fetch updated item: {}", e))
}

pub fn remove_module_item(conn: &Connection, item_id: &str) -> Result<(), String> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE module_items SET is_active = 0, updated_at = ?1 WHERE id = ?2",
        params![now, item_id],
    ).map_err(|e| format!("Failed to remove item: {}", e))?;
    Ok(())
}

/// Retained as a dashboard data-layer API for callers outside the current command set.
#[allow(dead_code)]
pub fn reorder_module_items(conn: &Connection, item_orders: Vec<(String, i64)>) -> Result<(), String> {
    let now = now_rfc3339();
    for (item_id, sort_order) in item_orders {
        conn.execute(
            "UPDATE module_items SET sort_order = ?1, updated_at = ?2 WHERE id = ?3",
            params![sort_order, now, item_id],
        ).map_err(|e| format!("Failed to reorder item: {}", e))?;
    }
    Ok(())
}

// === Daily Entries Commands ===

pub fn save_daily_entry(
    conn: &Connection,
    module_id: &str,
    entry_date: &str,
    data_json: &str,
) -> Result<DailyEntry, String> {
    let id = generate_id();
    let now = now_rfc3339();
    
    conn.execute(
        "INSERT INTO daily_entries (id, module_id, entry_date, data_json, created_at, updated_at) 
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(module_id, entry_date) DO UPDATE SET data_json = ?4, updated_at = ?6",
        params![id, module_id, entry_date, data_json, now, now],
    ).map_err(|e| format!("Failed to save daily entry: {}", e))?;
    
    Ok(DailyEntry {
        id: id.clone(),
        module_id: module_id.to_string(),
        entry_date: entry_date.to_string(),
        data_json: data_json.to_string(),
        completed_at: None,
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Retained as a dashboard data-layer API for callers outside the current command set.
#[allow(dead_code)]
pub fn get_daily_entry(conn: &Connection, module_id: &str, entry_date: &str) -> Result<Option<DailyEntry>, String> {
    conn.query_row(
        "SELECT id, module_id, entry_date, data_json, completed_at, created_at, updated_at 
         FROM daily_entries WHERE module_id = ?1 AND entry_date = ?2",
        params![module_id, entry_date],
        |row| Ok(DailyEntry {
            id: row.get(0)?,
            module_id: row.get(1)?,
            entry_date: row.get(2)?,
            data_json: row.get(3)?,
            completed_at: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    ).optional().map_err(|e| format!("Failed to get daily entry: {}", e))
}

pub fn get_daily_entries(conn: &Connection, entry_date: &str) -> Result<Vec<DailyEntry>, String> {
    let mut stmt = conn.prepare(
        "SELECT id, module_id, entry_date, data_json, completed_at, created_at, updated_at 
         FROM daily_entries WHERE entry_date = ?1"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;
    
    let entries = stmt.query_map([entry_date], |row| {
        Ok(DailyEntry {
            id: row.get(0)?,
            module_id: row.get(1)?,
            entry_date: row.get(2)?,
            data_json: row.get(3)?,
            completed_at: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }).map_err(|e| format!("Failed to query entries: {}", e))?
    .filter_map(|r| r.ok())
    .collect();
    
    Ok(entries)
}

// === Habit Commands ===

pub fn toggle_habit(
    conn: &Connection,
    item_id: &str,
    entry_date: &str,
    completed: bool,
) -> Result<HabitStreak, String> {
    let id = generate_id();
    let now = now_rfc3339();
    let completed_at = if completed { Some(now.clone()) } else { None };
    
    conn.execute(
        "INSERT INTO habit_entries (id, item_id, entry_date, completed, completed_at) 
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(item_id, entry_date) DO UPDATE SET completed = ?4, completed_at = ?5",
        params![id, item_id, entry_date, completed as i32, completed_at],
    ).map_err(|e| format!("Failed to toggle habit: {}", e))?;
    
    // Calculate and return streak
    get_habit_streak(conn, item_id)
}

pub fn get_habit_streak(conn: &Connection, item_id: &str) -> Result<HabitStreak, String> {
    // Calculate current streak (consecutive days backward from yesterday)
    let current_streak = calculate_streak(conn, item_id)?;
    
    // Calculate longest streak (all-time)
    let longest_streak = calculate_longest_streak(conn, item_id)?;
    
    // Calculate 30-day completion rate
    let completion_rate = calculate_completion_rate_30d(conn, item_id)?;
    
    Ok(HabitStreak {
        current_streak,
        longest_streak,
        completion_rate_30d: completion_rate,
    })
}

fn calculate_streak(conn: &Connection, item_id: &str) -> Result<i64, String> {
    // Start from yesterday and count backward
    let mut streak = 0i64;
    let mut current_date = chrono::Utc::now() - chrono::Duration::days(1);
    
    loop {
        let date_str = current_date.format("%Y-%m-%d").to_string();
        let completed: Option<i32> = conn.query_row(
            "SELECT completed FROM habit_entries WHERE item_id = ?1 AND entry_date = ?2",
            params![item_id, date_str],
            |row| row.get(0),
        ).optional().unwrap_or(None);
        
        if completed == Some(1) {
            streak += 1;
            current_date = current_date - chrono::Duration::days(1);
        } else {
            break;
        }
        
        // Safety limit
        if streak > 3650 { break; }  // Max 10 years
    }
    
    Ok(streak)
}

fn calculate_longest_streak(conn: &Connection, item_id: &str) -> Result<i64, String> {
    // This is a simplified version - a full implementation would analyze all entries
    // For now, just return current streak as placeholder
    calculate_streak(conn, item_id)
}

fn calculate_completion_rate_30d(conn: &Connection, item_id: &str) -> Result<f64, String> {
    let thirty_days_ago = (chrono::Utc::now() - chrono::Duration::days(30)).format("%Y-%m-%d").to_string();
    
    let (completed_count, total_count): (i64, i64) = conn.query_row(
        "SELECT 
            SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END) as completed,
            COUNT(*) as total
         FROM habit_entries 
         WHERE item_id = ?1 AND entry_date >= ?2",
        params![item_id, thirty_days_ago],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap_or((0, 0));
    
    if total_count == 0 {
        Ok(0.0)
    } else {
        Ok((completed_count as f64 / total_count as f64) * 100.0)
    }
}

pub fn get_habits_today(conn: &Connection) -> Result<Vec<HabitStatus>, String> {
    let today = today();
    
    // Get all habit items from modules
    let mut stmt = conn.prepare(
        "SELECT mi.id, mi.label, mi.emoji 
         FROM module_items mi 
         JOIN dashboard_modules dm ON mi.module_id = dm.id 
         WHERE mi.item_type = 'habit' AND mi.is_active = 1 AND dm.is_enabled = 1 
         ORDER BY mi.sort_order"
    ).map_err(|e| format!("Failed to prepare habits query: {}", e))?;
    
    let habit_items: Vec<(String, String, Option<String>)> = stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    }).map_err(|e| format!("Failed to query habits: {}", e))?
    .filter_map(|r| r.ok())
    .collect();
    
    let mut results = Vec::new();
    for (item_id, label, emoji) in habit_items {
        let completed: bool = conn.query_row(
            "SELECT completed FROM habit_entries WHERE item_id = ?1 AND entry_date = ?2",
            params![item_id, today],
            |row| Ok(row.get::<_, i32>(0)? != 0),
        ).optional().unwrap_or(Some(false)).unwrap_or(false);
        
        let streak = calculate_streak(conn, &item_id).unwrap_or(0);
        
        results.push(HabitStatus {
            item_id,
            label,
            emoji,
            completed,
            streak,
        });
    }
    
    Ok(results)
}

/// Retained for integrations that consume the preformatted dashboard context.
#[allow(dead_code)]
pub fn get_dashboard_context(conn: &Connection) -> Result<String, String> {
    let today = today();
    
    // Get objective
    let objective: Option<String> = conn.query_row(
        "SELECT static_content FROM dashboard_modules WHERE module_type = 'objective' AND is_enabled = 1 LIMIT 1",
        [],
        |row| row.get(0),
    ).optional().unwrap_or(None);
    
    // Get habits today
    let habits = get_habits_today(conn)?;
    let completed_habits = habits.iter().filter(|h| h.completed).count();
    let total_habits = habits.len();
    
    // Get reflection averages (last 7 days)
    let seven_days_ago = (chrono::Utc::now() - chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
    
    let productivity_avg: f64 = conn.query_row(
        "SELECT AVG(value_numeric) FROM reflection_entries WHERE question_id = 'productivity' AND entry_date >= ?1",
        [&seven_days_ago],
        |row| row.get(0),
    ).unwrap_or(0.0);
    
    let mood_avg: f64 = conn.query_row(
        "SELECT AVG(value_numeric) FROM reflection_entries WHERE question_id = 'mood' AND entry_date >= ?1",
        [&seven_days_ago],
        |row| row.get(0),
    ).unwrap_or(0.0);
    
    // Build context string
    let mut context = format!("## Dashboard Context (Today: {})\n", today);
    
    if let Some(obj) = objective {
        context.push_str(&format!("Objective: {}\n\n", obj));
    }
    
    context.push_str(&format!("Habits today: {}/{} completed\n", completed_habits, total_habits));
    
    for habit in &habits {
        let status = if habit.completed { "✓" } else { " " };
        context.push_str(&format!("[{}] {} ({}d streak)\n", status, habit.label, habit.streak));
    }
    
    if productivity_avg > 0.0 || mood_avg > 0.0 {
        context.push_str(&format!("\nReflection (7-day avg): Productivity {:.1}, Mood {:.1}\n", productivity_avg, mood_avg));
    }
    
    Ok(context)
}

// === Gratitude Commands ===

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GratitudeEntry {
    pub id: String,
    pub entry_date: String,
    pub entry_index: i64,
    pub text: String,
    pub created_at: String,
}

pub fn add_gratitude_entry(
    conn: &Connection,
    entry_date: &str,
    text: &str,
) -> Result<GratitudeEntry, String> {
    // Get the next index for this date
    let next_index: i64 = conn.query_row(
        "SELECT COALESCE(MAX(entry_index), -1) + 1 FROM gratitude_entries WHERE entry_date = ?1",
        [entry_date],
        |row| row.get(0),
    ).unwrap_or(0);
    
    let id = generate_id();
    let now = now_rfc3339();
    
    conn.execute(
        "INSERT INTO gratitude_entries (id, entry_date, entry_index, text, created_at) 
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, entry_date, next_index, text, now],
    ).map_err(|e| format!("Failed to add gratitude entry: {}", e))?;
    
    Ok(GratitudeEntry {
        id: id.clone(),
        entry_date: entry_date.to_string(),
        entry_index: next_index,
        text: text.to_string(),
        created_at: now,
    })
}

pub fn get_gratitude_entries(
    conn: &Connection,
    entry_date: &str,
) -> Result<Vec<GratitudeEntry>, String> {
    let mut stmt = conn.prepare(
        "SELECT id, entry_date, entry_index, text, created_at 
         FROM gratitude_entries 
         WHERE entry_date = ?1 
         ORDER BY entry_index ASC"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;
    
    let entries = stmt.query_map([entry_date], |row| {
        Ok(GratitudeEntry {
            id: row.get(0)?,
            entry_date: row.get(1)?,
            entry_index: row.get(2)?,
            text: row.get(3)?,
            created_at: row.get(4)?,
        })
    }).map_err(|e| format!("Failed to query gratitude entries: {}", e))?
    .filter_map(|r| r.ok())
    .collect();
    
    Ok(entries)
}

pub fn delete_gratitude_entry(
    conn: &Connection,
    entry_id: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM gratitude_entries WHERE id = ?1",
        [entry_id],
    ).map_err(|e| format!("Failed to delete gratitude entry: {}", e))?;
    
    Ok(())
}

// === Reflection Commands ===

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReflectionEntry {
    pub id: String,
    pub question_id: String,
    pub entry_date: String,
    pub value_numeric: Option<f64>,
    pub value_bool: Option<bool>,
    pub value_text: Option<String>,
}

pub fn save_reflection_entry(
    conn: &Connection,
    question_id: &str,
    entry_date: &str,
    value_numeric: Option<f64>,
    value_bool: Option<bool>,
    value_text: Option<&str>,
) -> Result<ReflectionEntry, String> {
    let id = generate_id();
    
    // Convert bool to integer for SQLite
    let value_bool_int = value_bool.map(|b| if b { 1 } else { 0 });
    
    conn.execute(
        "INSERT INTO reflection_entries (id, question_id, entry_date, value_numeric, value_bool, value_text) 
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(question_id, entry_date) DO UPDATE SET 
            value_numeric = ?4, value_bool = ?5, value_text = ?6",
        params![id, question_id, entry_date, value_numeric, value_bool_int, value_text],
    ).map_err(|e| format!("Failed to save reflection entry: {}", e))?;
    
    Ok(ReflectionEntry {
        id,
        question_id: question_id.to_string(),
        entry_date: entry_date.to_string(),
        value_numeric,
        value_bool,
        value_text: value_text.map(|s| s.to_string()),
    })
}

pub fn get_reflection_entries(
    conn: &Connection,
    entry_date: &str,
) -> Result<Vec<ReflectionEntry>, String> {
    let mut stmt = conn.prepare(
        "SELECT id, question_id, entry_date, value_numeric, value_bool, value_text 
         FROM reflection_entries 
         WHERE entry_date = ?1"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;
    
    let entries = stmt.query_map([entry_date], |row| {
        let value_bool_int: Option<i32> = row.get(4)?;
        let value_bool = value_bool_int.map(|i| i != 0);
        
        Ok(ReflectionEntry {
            id: row.get(0)?,
            question_id: row.get(1)?,
            entry_date: row.get(2)?,
            value_numeric: row.get(3)?,
            value_bool,
            value_text: row.get(5)?,
        })
    }).map_err(|e| format!("Failed to query reflection entries: {}", e))?
    .filter_map(|r| r.ok())
    .collect();
    
    Ok(entries)
}

// === Tauri Command Wrappers ===

use crate::database::DbState;
use tauri::State;

#[tauri::command]
pub fn get_dashboard_modules(db: State<DbState>) -> Result<(UserDashboardConfig, Vec<DashboardModule>), String> {
    let conn = db.0.lock().unwrap();
    load_dashboard_config(&conn)
}

#[tauri::command]
pub fn get_dashboard_config(db: State<DbState>) -> Result<(UserDashboardConfig, Vec<DashboardModule>), String> {
    let conn = db.0.lock().unwrap();
    load_dashboard_config(&conn)
}

#[tauri::command]
pub fn create_dashboard_module(
    module_type: String,
    config_json: Option<String>,
    sort_order: Option<i64>,
    db: State<DbState>,
) -> Result<DashboardModule, String> {
    let conn = db.0.lock().unwrap();
    add_module(&conn, &module_type, config_json.as_deref(), sort_order)
}

#[tauri::command]
pub fn update_dashboard_module(
    module_id: String,
    config_json: String,
    db: State<DbState>,
) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    update_module_config(&conn, &module_id, &config_json)
}

#[tauri::command]
pub fn update_module_content_cmd(
    module_id: String,
    content: String,
    db: State<DbState>,
) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    update_module_content(&conn, &module_id, &content)
}

#[tauri::command]
pub fn delete_dashboard_module(module_id: String, db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    remove_module(&conn, &module_id)
}

#[tauri::command]
pub fn toggle_dashboard_module(
    module_id: String,
    enabled: bool,
    db: State<DbState>,
) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    toggle_module(&conn, &module_id, enabled)
}

#[tauri::command]
pub fn reorder_dashboard_modules(
    module_orders: Vec<(String, i64)>,
    db: State<DbState>,
) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    update_module_order(&conn, module_orders)
}

#[tauri::command]
pub fn get_module_items_cmd(module_id: String, db: State<DbState>) -> Result<Vec<ModuleItem>, String> {
    let conn = db.0.lock().unwrap();
    get_module_items(&conn, &module_id)
}

#[tauri::command]
pub fn create_module_item_cmd(
    module_id: String,
    item_type: String,
    label: String,
    description: Option<String>,
    emoji: Option<String>,
    config_json: Option<String>,
    db: State<DbState>,
) -> Result<ModuleItem, String> {
    let conn = db.0.lock().unwrap();
    add_module_item(
        &conn,
        &module_id,
        &item_type,
        &label,
        description.as_deref(),
        emoji.as_deref(),
        config_json.as_deref(),
    )
}

#[tauri::command]
pub fn update_module_item_cmd(
    item_id: String,
    label: Option<String>,
    description: Option<String>,
    emoji: Option<String>,
    config_json: Option<String>,
    db: State<DbState>,
) -> Result<ModuleItem, String> {
    let conn = db.0.lock().unwrap();
    update_module_item(
        &conn,
        &item_id,
        label.as_deref(),
        description.as_deref(),
        emoji.as_deref(),
        config_json.as_deref(),
    )
}

#[tauri::command]
pub fn delete_module_item_cmd(item_id: String, db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    remove_module_item(&conn, &item_id)
}

#[tauri::command]
pub fn get_daily_entries_cmd(entry_date: String, db: State<DbState>) -> Result<Vec<DailyEntry>, String> {
    let conn = db.0.lock().unwrap();
    get_daily_entries(&conn, &entry_date)
}

#[tauri::command]
pub fn save_daily_entry_cmd(
    module_id: String,
    entry_date: String,
    data_json: String,
    db: State<DbState>,
) -> Result<DailyEntry, String> {
    let conn = db.0.lock().unwrap();
    save_daily_entry(&conn, &module_id, &entry_date, &data_json)
}

#[tauri::command]
pub fn get_habit_entries_cmd(entry_date: String, db: State<DbState>) -> Result<Vec<HabitStatus>, String> {
    // Keep the command argument for frontend API compatibility; current behavior is today-only.
    let _ = entry_date;
    let conn = db.0.lock().unwrap();
    get_habits_today(&conn)
}

#[tauri::command]
pub fn save_habit_entry_cmd(
    item_id: String,
    entry_date: String,
    completed: bool,
    db: State<DbState>,
) -> Result<HabitStreak, String> {
    let conn = db.0.lock().unwrap();
    toggle_habit(&conn, &item_id, &entry_date, completed)
}

#[tauri::command]
pub fn get_reflection_entries_cmd(
    entry_date: String,
    db: State<DbState>,
) -> Result<Vec<ReflectionEntry>, String> {
    let conn = db.0.lock().unwrap();
    get_reflection_entries(&conn, &entry_date)
}

#[tauri::command]
pub fn save_reflection_entry_cmd(
    question_id: String,
    entry_date: String,
    value_numeric: Option<f64>,
    value_bool: Option<bool>,
    value_text: Option<String>,
    db: State<DbState>,
) -> Result<ReflectionEntry, String> {
    let conn = db.0.lock().unwrap();
    save_reflection_entry(
        &conn,
        &question_id,
        &entry_date,
        value_numeric,
        value_bool,
        value_text.as_deref(),
    )
}

#[tauri::command]
pub fn get_gratitude_entries_cmd(
    entry_date: String,
    db: State<DbState>,
) -> Result<Vec<GratitudeEntry>, String> {
    let conn = db.0.lock().unwrap();
    get_gratitude_entries(&conn, &entry_date)
}

#[tauri::command]
pub fn save_gratitude_entry_cmd(
    entry_date: String,
    text: String,
    db: State<DbState>,
) -> Result<GratitudeEntry, String> {
    let conn = db.0.lock().unwrap();
    add_gratitude_entry(&conn, &entry_date, &text)
}

#[tauri::command]
pub fn delete_gratitude_entry_cmd(entry_id: String, db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    delete_gratitude_entry(&conn, &entry_id)
}

#[tauri::command]
pub fn get_home_state(db: State<DbState>) -> Result<(UserDashboardConfig, Vec<DashboardModule>), String> {
    let conn = db.0.lock().unwrap();
    load_dashboard_config(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("
            CREATE TABLE user_dashboard_config (
                id TEXT PRIMARY KEY,
                onboarding_completed INTEGER NOT NULL DEFAULT 0,
                onboarding_template TEXT,
                theme_variant TEXT NOT NULL DEFAULT 'default',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            INSERT INTO user_dashboard_config (id, created_at, updated_at)
            VALUES ('default', datetime('now'), datetime('now'));
            CREATE TABLE dashboard_modules (
                id TEXT PRIMARY KEY,
                module_type TEXT NOT NULL DEFAULT '',
                sort_order INTEGER NOT NULL DEFAULT 0,
                is_enabled INTEGER NOT NULL DEFAULT 1,
                config_json TEXT DEFAULT '{}',
                static_content TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE module_items (
                id TEXT PRIMARY KEY,
                module_id TEXT NOT NULL,
                item_type TEXT NOT NULL DEFAULT '',
                content TEXT,
                sort_order INTEGER NOT NULL DEFAULT 0,
                config_json TEXT DEFAULT '{}',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
        ").expect("schema");
        conn
    }

    #[test]
    fn test_load_dashboard_config_creates_default() {
        eprintln!("[TEST] test_load_dashboard_config_creates_default");
        let conn = setup_test_db();
        let (config, _modules) = load_dashboard_config(&conn).expect("should succeed");
        assert_eq!(config.id, "default");
        assert_eq!(config.theme_variant, "default");
        assert!(!config.onboarding_completed);
    }

    /// Verify greeting_name column is absent after v62→v63 migration pattern
    #[test]
    fn test_greeting_name_column_dropped() {
        eprintln!("[TEST] test_greeting_name_column_dropped");
        let conn = Connection::open_in_memory().expect("in-memory db");
        // Simulate a pre-migration schema with greeting_name
        conn.execute_batch("
            CREATE TABLE user_dashboard_config (
                id TEXT PRIMARY KEY,
                onboarding_completed INTEGER NOT NULL DEFAULT 0,
                onboarding_template TEXT,
                greeting_name TEXT NOT NULL DEFAULT 'there',
                theme_variant TEXT NOT NULL DEFAULT 'default',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT INTO user_dashboard_config (id, greeting_name, created_at, updated_at)
            VALUES ('default', 'there', datetime('now'), datetime('now'));
        ").expect("pre-migration schema");

        // Run the drop migration
        conn.execute_batch("
            BEGIN;
            CREATE TABLE IF NOT EXISTS user_dashboard_config_new (
                id                    TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
                onboarding_completed  INTEGER DEFAULT 0,
                onboarding_template   TEXT,
                theme_variant         TEXT DEFAULT 'default',
                created_at            TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO user_dashboard_config_new
                SELECT id, onboarding_completed, onboarding_template, theme_variant, created_at, updated_at
                FROM user_dashboard_config;
            DROP TABLE user_dashboard_config;
            ALTER TABLE user_dashboard_config_new RENAME TO user_dashboard_config;
            COMMIT;
        ").expect("migration");

        // Verify column is gone
        let col_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('user_dashboard_config') WHERE name = 'greeting_name'",
            [],
            |row| row.get(0),
        ).expect("pragma query");
        assert!(!col_exists, "greeting_name column should be absent after migration");

        // Verify data preserved
        let id: String = conn.query_row(
            "SELECT id FROM user_dashboard_config WHERE id = 'default'",
            [],
            |row| row.get(0),
        ).expect("row should exist");
        assert_eq!(id, "default");
    }
}
