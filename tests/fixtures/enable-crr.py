#!/usr/bin/env python3
"""Enable CRR on v61 fixture tables using cr-sqlite extension"""

import sqlite3
import sys

FIXTURE = "tests/fixtures/upgrade-from-v61.db"
CRSQLITE = "src-tauri/target/debug/resources/crsqlite.dylib"

CRR_TABLES = [
    "projects",
    "notes",
    "tags",
    "note_tags",
    "note_links",
    "ai_context_markers",
    "ai_pinned_messages",
    "ai_conversation_summaries",
    "ai_memory",
    "ai_actions",
    "user_profiles",
    "user_dashboard_config",
    "dashboard_modules",
    "module_items",
    "daily_entries",
    "habit_entries",
    "reflection_entries",
    "gratitude_entries",
    "sync_state",
    "note_versions",
]

def main():
    print("[ENABLE-CRR] Opening fixture database...")
    conn = sqlite3.connect(FIXTURE)
    
    # Enable extension loading
    conn.enable_load_extension(True)
    
    print(f"[ENABLE-CRR] Loading cr-sqlite extension from {CRSQLITE}...")
    try:
        conn.load_extension(CRSQLITE)
    except Exception as e:
        print(f"[ENABLE-CRR] ❌ Failed to load extension: {e}")
        sys.exit(1)
    
    print("[ENABLE-CRR] Enabling CRR on 20 tables...")
    for table in CRR_TABLES:
        try:
            conn.execute(f"SELECT crsql_as_crr('{table}');")
            print(f"[ENABLE-CRR] ✓ {table}")
        except Exception as e:
            print(f"[ENABLE-CRR] ❌ Failed on {table}: {e}")
            sys.exit(1)
    
    conn.commit()
    
    # Verify CRR tables exist
    cursor = conn.execute(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE '%__crsql_clock'"
    )
    crr_count = cursor.fetchone()[0]
    
    print(f"[ENABLE-CRR] ✓ CRR enabled successfully")
    print(f"[ENABLE-CRR] ✓ CRR clock tables: {crr_count}")
    
    if crr_count < 20:
        print(f"[ENABLE-CRR] ⚠️  Warning: Expected 20 CRR clock tables, found {crr_count}")
        sys.exit(1)
    
    conn.close()
    print("[ENABLE-CRR] ✓ Done")

if __name__ == "__main__":
    main()
