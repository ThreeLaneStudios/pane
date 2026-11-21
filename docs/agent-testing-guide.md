# Agent Testing Guide — How to Test Pane
*For any AI agent that needs to interact with the Pane desktop app.*

## Automated E2E Tests (PRIMARY METHOD)

### Quick Reference
```bash
# Tier 1 — every build (~30 seconds)
cd ~/Projects/example-app && npx playwright test --config=e2e/playwright.config.ts --project=tier1

# Tier 1 + 2 — before shipping DMG to Matt (~7 minutes)
cd ~/Projects/example-app && npx playwright test --config=e2e/playwright.config.ts --project=tier1 --project=tier2

# Full suite — weekly or pre-major-release (~10 minutes)
cd ~/Projects/example-app && npx playwright test --config=e2e/playwright.config.ts
```

### Prerequisites
1. Pane running in dev mode: `PANE_TEST_MODE=1 cargo tauri dev`
2. Socket alive: `ls /tmp/tauri-playwright.sock`
3. Ollama running (for Tier 2): `ollama list` should show `llama3.2:3b`

### When to Run What

| Situation | Tests | Time | Who |
|---|---|---|---|
| Every build (Stia finishes code) | Tier 1 only | ~30s | Stia |
| Before shipping DMG to Matt | Tier 1 + Tier 2 | ~7 min | Quinn |
| Sunday automated sweep | All 19 | ~10 min | Quinn/cron |
| Major version release | All 19 + manual review | ~10 min | Quinn |

### Test Tiers

**Tier 1: Must-Pass (6 tests)** — UI basics
- Auth screen elements
- Project creation
- Note CRUD (create, edit, delete, restore)
- Task lifecycle (status, priority, due date)
- Chat basics (send message, get response)
- Sidebar navigation

**Tier 2: AI Systems (6 tests)** — AI pipeline
- Smart routing (simple vs complex classification)
- Memory extraction (fact storage + recall)
- Memory search & rerank
- Context budget (many messages, no crash)
- AI tool use (AI creates a note via tool call)
- Model fallback (graceful degradation)

**Tier 3: Regression (7 tests)** — Edge cases
- Settings persistence
- Onboarding flow (placeholder — needs fresh DB)
- Search (Cmd+K, results, navigation)
- Keyboard shortcuts (Cmd+N, Cmd+K, Escape)
- Editor (title, markdown, toolbar)
- Kanban view (columns, cards)
- Agent management (read-only main agent)

### Interpreting Results
```
✓  = passed
✘  = failed (read error message)
⊘  = skipped (missing prerequisite)
```

If Tier 1 fails → **do not ship.** Fix and re-run.
If Tier 2 fails → investigate. May be Ollama issue, not app issue.
If Tier 3 fails → log as issue, ship if Tier 1+2 pass.

### Known Limitations
- Tests use `evaluate()` for text-based queries (`:has-text()` not supported)
- `waitForSelector` takes a plain number for timeout, not `{ timeout: N }`
- Auth bypass only works in dev builds (`import.meta.env.DEV`)
- Onboarding test is a placeholder (requires fresh DB setup)
- Kanban drag-and-drop is skipped (unreliable in WebView)

---

## Manual Testing (FALLBACK — when automated tests can't cover it)

### Starting Test Mode
```bash
cd ~/Projects/example-app && PANE_TEST_MODE=1 cargo tauri dev
```

### Manual Test Flows

#### 1. Settings — AI Connection
**What to test:** Smart routing configuration persists correctly.
1. Navigate to Settings (gear icon)
2. Click "AI" tab
3. Verify provider shows current config
4. Enter a test API key, click "Save & Test Connection"
5. Verify error for invalid key
6. Navigate away and back — verify settings persisted

#### 2. Note Creation via Chat
1. Type: "Create a note called Test Note with status todo"
2. Verify note appears in sidebar
3. Click to open — verify title and status

#### 3. Navigation
1. Click through: Home → Project → Settings → Home
2. Verify each view renders correctly

#### 4. Chat / AI Conversation
1. Send "Hello", verify response
2. Navigate away and back — verify history persists

#### 5. Search
1. Search for existing note — verify results
2. Search for gibberish — verify "no results"

#### 6. Smart Routing
1. Send simple: "What's 2+2?" — check model label
2. Send complex: "Write a detailed analysis" — check model label

#### 7. Compaction
1. Send 15+ messages
2. Verify AI still remembers earlier context

---

## Reporting

Save reports to: `~/Shared/ExampleApp/docs/test-reports/YYYY-MM-DD-agent-name.md`

```markdown
## Test Report — [Date] — [Agent Name]

### Environment
- Pane version: vX.Y.Z
- Test mode: automated / manual
- Tiers run: 1 / 1+2 / all

### Automated Results
- Tier 1: X/6 passed
- Tier 2: X/6 passed
- Tier 3: X/7 passed

### Failures
1. **[Test name]** — Error message, steps to reproduce

### Bugs Filed
- #NNN — Description
```

## Rules
1. **Never test against production DB.** Always use test mode.
2. **Reset test DB between runs** for consistency: `./scripts/seed-test-db.sh`
3. **Screenshot every failure.**
4. **File bugs as GitHub issues** with reproduction steps.
5. **Don't modify app code during testing.**
