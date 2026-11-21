#!/bin/bash
# Launch Pane in test mode with WebDriver
export PANE_TEST_MODE=1
export PANE_TEST_DB=~/.pane-test/test.db

# Seed the test database
./scripts/seed-test-db.sh

# Start the app in dev mode with the webdriver server running
cargo tauri dev
