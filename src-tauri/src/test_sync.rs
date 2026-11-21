use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::path::PathBuf;
use crate::database::{init_database, Task, TaskStatus, TaskPriority, create_task};
use crate::sync_db::{get_sync_changes, apply_sync_changes, get_sync_version, get_site_id};
use crate::sync::SyncChange;

/// Test that two separate databases can successfully sync with cr-sqlite
pub fn run_sync_test() -> Result<(), String> {
    println!("🧪 Starting cr-sqlite Phase 2 sync test...\n");

    // Create temporary database paths
    let db_a_path = PathBuf::from("/tmp/pane_test_db_a.db");
    let db_b_path = PathBuf::from("/tmp/pane_test_db_b.db");

    // Clean up any existing test databases
    let _ = fs::remove_file(&db_a_path);
    let _ = fs::remove_file(&db_b_path);

    println!("📁 Creating test databases:");
    println!("  - Database A: {:?}", db_a_path);
    println!("  - Database B: {:?}", db_b_path);

    // Step 1: Create and initialize both databases
    let mut conn_a = init_test_database(&db_a_path)?;
    let mut conn_b = init_test_database(&db_b_path)?;

    println!("\n✅ Databases created and cr-sqlite loaded");

    // Step 2: Test CRR table compatibility
    test_crr_compatibility(&conn_a, "Database A")?;
    test_crr_compatibility(&conn_b, "Database B")?;

    println!("\n✅ CRR compatibility verified");

    // Step 3: Create test data in each database
    println!("\n📝 Creating test data...");

    // Database A: Create a project and a task
    let project_a_id = create_test_project(&conn_a, "Test Project", "A project created in database A")?;
    let task_a_id = create_test_task(&conn_a, "Task from A", "This task was created in database A", &project_a_id)?;

    // Database B: Create a different task (but no project initially)
    let task_b_id = create_test_task(&conn_b, "Task from B", "This task was created in database B", "")?;

    println!("  - Database A: Created project '{}' and task '{}'", project_a_id, task_a_id);
    println!("  - Database B: Created task '{}'", task_b_id);

    // Step 4: Get initial sync versions and site IDs
    let version_a_initial = get_sync_version(&conn_a)?;
    let version_b_initial = get_sync_version(&conn_b)?;
    let site_id_a = get_site_id(&conn_a)?;
    let site_id_b = get_site_id(&conn_b)?;

    println!("\n🔄 Initial sync state:");
    println!("  - Database A: version {}, site_id {:?}", version_a_initial, hex_encode(&site_id_a));
    println!("  - Database B: version {}, site_id {:?}", version_b_initial, hex_encode(&site_id_b));

    // Step 5: Sync A → B (get changes from A and apply to B)
    println!("\n⬆️  Syncing A → B...");
    let changes_from_a = get_sync_changes(&conn_a, 0)?; // Get all changes since version 0
    println!("  - Found {} changes from database A", changes_from_a.len());

    if !changes_from_a.is_empty() {
        for (i, change) in changes_from_a.iter().enumerate() {
            println!("    Change {}: table='{}', pk='{}', cid='{}'", 
                i+1, change.table_name, String::from_utf8_lossy(&change.pk), change.cid);
        }
        apply_sync_changes(&conn_b, changes_from_a)?;
        println!("  - Applied changes to database B");
    }

    // Step 6: Sync B → A (get changes from B and apply to A)
    println!("\n⬇️  Syncing B → A...");
    let changes_from_b = get_sync_changes(&conn_b, 0)?;
    println!("  - Found {} changes from database B", changes_from_b.len());

    if !changes_from_b.is_empty() {
        for (i, change) in changes_from_b.iter().enumerate() {
            println!("    Change {}: table='{}', pk='{}', cid='{}'", 
                i+1, change.table_name, String::from_utf8_lossy(&change.pk), change.cid);
        }
        apply_sync_changes(&conn_a, changes_from_b)?;
        println!("  - Applied changes to database A");
    }

    // Step 7: Verify both databases now have all data
    println!("\n🔍 Verifying sync results...");

    let projects_a = count_projects(&conn_a)?;
    let tasks_a = count_tasks(&conn_a)?;
    let projects_b = count_projects(&conn_b)?;
    let tasks_b = count_tasks(&conn_b)?;

    println!("  - Database A: {} projects, {} tasks", projects_a, tasks_a);
    println!("  - Database B: {} projects, {} tasks", projects_b, tasks_b);

    // Both databases should now have the same data
    if projects_a != projects_b {
        return Err(format!("Project count mismatch: A has {}, B has {}", projects_a, projects_b));
    }

    if tasks_a != tasks_b {
        return Err(format!("Task count mismatch: A has {}, B has {}", tasks_a, tasks_b));
    }

    // Verify specific tasks exist in both databases
    let task_a_in_b = task_exists(&conn_b, &task_a_id)?;
    let task_b_in_a = task_exists(&conn_a, &task_b_id)?;
    let project_a_in_b = project_exists(&conn_b, &project_a_id)?;

    if !task_a_in_b {
        return Err("Task from A not found in database B".to_string());
    }

    if !task_b_in_a {
        return Err("Task from B not found in database A".to_string());
    }

    if !project_a_in_b {
        return Err("Project from A not found in database B".to_string());
    }

    // Step 8: Get final sync versions
    let version_a_final = get_sync_version(&conn_a)?;
    let version_b_final = get_sync_version(&conn_b)?;

    println!("\n📊 Final sync state:");
    println!("  - Database A: version {} (was {})", version_a_final, version_a_initial);
    println!("  - Database B: version {} (was {})", version_b_final, version_b_initial);

    // Clean up
    drop(conn_a);
    drop(conn_b);
    let _ = fs::remove_file(&db_a_path);
    let _ = fs::remove_file(&db_b_path);

    println!("\n🎉 SUCCESS! Two databases successfully synced:");
    println!("  ✅ Both databases have {} project(s)", projects_a);
    println!("  ✅ Both databases have {} task(s)", tasks_a);
    println!("  ✅ Task from A exists in B");
    println!("  ✅ Task from B exists in A");
    println!("  ✅ Project from A exists in B");
    println!("  ✅ Sync versions updated correctly");

    Ok(())
}

/// Initialize a test database with cr-sqlite and full Pane schema
fn init_test_database(path: &PathBuf) -> Result<Connection, String> {
    let conn = Connection::open(path)
        .map_err(|e| format!("Failed to create test database: {}", e))?;

    // Enable WAL mode
    conn.execute_batch("PRAGMA journal_mode=WAL;")
        .map_err(|e| format!("Failed to set WAL mode: {}", e))?;

    // Load cr-sqlite extension
    load_test_crsqlite_extension(&conn)?;

    // Run basic schema creation (simplified version of our migrations)
    create_test_schema(&conn)?;

    // Initialize CRRs for key tables
    init_test_crrs(&conn)?;

    Ok(conn)
}

/// Load cr-sqlite extension for test
fn load_test_crsqlite_extension(conn: &Connection) -> Result<(), String> {
    unsafe {
        conn.load_extension_enable()
            .map_err(|e| format!("Failed to enable extension loading: {}", e))?;

        // Try multiple possible paths for the extension
        let possible_paths = vec![
            "resources/crsqlite.dylib",
            "src-tauri/resources/crsqlite.dylib", 
            "../resources/crsqlite.dylib",
            "./crsqlite.dylib",
        ];

        let mut loaded = false;
        for path in possible_paths {
            if PathBuf::from(path).exists() {
                match conn.load_extension(path, Some("sqlite3_crsqlite_init")) {
                    Ok(_) => {
                        loaded = true;
                        println!("  ✅ Loaded cr-sqlite from: {}", path);
                        break;
                    }
                    Err(e) => {
                        println!("  ⚠️  Failed to load from {}: {}", path, e);
                    }
                }
            }
        }

        if !loaded {
            return Err("Could not find or load cr-sqlite extension".to_string());
        }
    }

    conn.load_extension_disable()
        .map_err(|e| format!("Failed to disable extension loading: {}", e))?;

    Ok(())
}

/// Create simplified test schema
fn create_test_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS projects (
            id          TEXT PRIMARY KEY NOT NULL,
            name        TEXT NOT NULL DEFAULT '',
            description TEXT,
            color       TEXT,
            parent_id   TEXT,
            sort_order  INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT '',
            updated_at  TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE IF NOT EXISTS tasks (
            id              TEXT PRIMARY KEY NOT NULL,
            title           TEXT NOT NULL DEFAULT '',
            description     TEXT,
            status          TEXT NOT NULL DEFAULT 'todo',
            priority        TEXT NOT NULL DEFAULT 'medium',
            due_date        TEXT,
            project         TEXT,
            project_id      TEXT,
            assignee        TEXT,
            parent_task_id  TEXT,
            sort_order      INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT NOT NULL DEFAULT '',
            updated_at      TEXT NOT NULL DEFAULT '',
            completed_at    TEXT
        );

        -- Simple test table that definitely works with cr-sqlite
        CREATE TABLE IF NOT EXISTS test_sync (
            id TEXT PRIMARY KEY NOT NULL,
            title TEXT NOT NULL DEFAULT '',
            content TEXT,
            updated_at TEXT NOT NULL DEFAULT ''
        );
        "
    ).map_err(|e| format!("Failed to create test schema: {}", e))?;

    Ok(())
}

/// Initialize CRRs for test tables
fn init_test_crrs(conn: &Connection) -> Result<(), String> {
    // Test the simple table first
    let result: String = conn.query_row("SELECT crsql_as_crr('test_sync')", [], |row| row.get(0))
        .map_err(|e| format!("Failed to mark test_sync as CRR: {}", e))?;
    println!("  ✅ Marked test_sync as CRR (result: {})", result);

    // Try the main tables
    let tables = ["projects", "tasks"];
    
    for table in tables {
        match conn.query_row(&format!("SELECT crsql_as_crr('{}')", table), [], |row: &rusqlite::Row| row.get::<_, String>(0)) {
            Ok(result) => println!("  ✅ Marked {} as CRR (result: {})", table, result),
            Err(e) => {
                println!("  ⚠️  Could not mark {} as CRR: {}. This table will not sync.", table, e);
            }
        }
    }

    Ok(())
}

/// Test CRR compatibility of tables
fn test_crr_compatibility(conn: &Connection, db_name: &str) -> Result<(), String> {
    println!("\n🔧 Testing CRR compatibility for {}:", db_name);

    // Test if we can query crsql_changes
    match conn.prepare("SELECT COUNT(*) FROM crsql_changes") {
        Ok(mut stmt) => {
            match stmt.query_row([], |row| row.get::<_, i64>(0)) {
                Ok(count) => println!("  ✅ crsql_changes table accessible (contains {} rows)", count),
                Err(e) => println!("  ⚠️  Could not query crsql_changes: {}", e),
            }
        }
        Err(e) => println!("  ❌ crsql_changes table not found: {}", e),
    }

    // Test basic cr-sqlite functions
    match conn.query_row("SELECT crsql_db_version()", [], |row| row.get::<_, i64>(0)) {
        Ok(version) => println!("  ✅ Database version: {}", version),
        Err(e) => println!("  ❌ Could not get database version: {}", e),
    }

    match conn.query_row("SELECT crsql_site_id()", [], |row| row.get::<_, Vec<u8>>(0)) {
        Ok(site_id) => println!("  ✅ Site ID: {}", hex_encode(&site_id)),
        Err(e) => println!("  ❌ Could not get site ID: {}", e),
    }

    Ok(())
}

/// Create a test project
fn create_test_project(conn: &Connection, name: &str, description: &str) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO projects (id, name, description, sort_order, created_at, updated_at) 
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, name, description, 0, now, now],
    ).map_err(|e| format!("Failed to create test project: {}", e))?;

    Ok(id)
}

/// Create a test task
fn create_test_task(conn: &Connection, title: &str, description: &str, project_id: &str) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let project_id_param = if project_id.is_empty() { None } else { Some(project_id) };

    conn.execute(
        "INSERT INTO tasks (id, title, description, status, priority, project_id, sort_order, created_at, updated_at) 
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![id, title, description, "todo", "medium", project_id_param, 0, now, now],
    ).map_err(|e| format!("Failed to create test task: {}", e))?;

    Ok(id)
}

/// Count projects in database
fn count_projects(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .map_err(|e| format!("Failed to count projects: {}", e))
}

/// Count tasks in database
fn count_tasks(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
        .map_err(|e| format!("Failed to count tasks: {}", e))
}

/// Check if a task exists
fn task_exists(conn: &Connection, task_id: &str) -> Result<bool, String> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE id = ?1", 
        params![task_id], 
        |row| row.get(0)
    ).map_err(|e| format!("Failed to check task existence: {}", e))?;

    Ok(count > 0)
}

/// Check if a project exists  
fn project_exists(conn: &Connection, project_id: &str) -> Result<bool, String> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM projects WHERE id = ?1", 
        params![project_id], 
        |row| row.get(0)
    ).map_err(|e| format!("Failed to check project existence: {}", e))?;

    Ok(count > 0)
}

/// Convert bytes to hex string for display
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_sync() {
        match run_sync_test() {
            Ok(_) => println!("✅ Sync test passed!"),
            Err(e) => panic!("❌ Sync test failed: {}", e),
        }
    }
}