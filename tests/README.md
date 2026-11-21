# Pane Test Suite

This document outlines the comprehensive testing strategy for Pane (Tauri 2.0 + SvelteKit app).

## Test Architecture

### 3-Layer Testing Strategy

1. **Rust Unit Tests** - Backend business logic
2. **Frontend Unit Tests** - Service layer and store behavior (Vitest)  
3. **End-to-End Tests** - Full user workflows (Playwright)

## Test Layers

### 1. Rust Backend Tests (`src-tauri/src/tests/`)

**14 Unit Tests** covering:
- Database operations (CRUD for notes, tasks, projects)
- CRSQLite integration and conflict resolution
- Sync functionality between instances
- File system operations and integrity
- Task→Note lifecycle management

**Run:**
```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

**Coverage:** Core backend functionality, database consistency, sync protocol

### 2. Frontend Unit Tests (`src/lib/**/*.test.ts`) 

**New Vitest Test Suite** covering the 5 most bug-prone flows:

#### noteService Tests (`src/lib/services/noteService.test.ts`)
- `createNote()` - Returns proper NoteMeta, updates store immediately
- `selectNote()` - Implements null-then-set pattern, waits for editor save
- `loadNotes()` - Merges without overwriting just-created notes

#### taskService Tests (`src/lib/services/taskService.test.ts`)
- `createTask()` / `updateTask()` cycle - Maintains store consistency
- Task creation and status updates

#### Store Tests (`src/lib/stores/app.test.ts`)
- `displayedNotes` filtering by notebook - Prevents cross-folder bleed
- `activeNote` mutations - Null state handling

**Run:**
```bash
npm run test
```

**Mocking:** Tauri invoke layer is mocked using adapted `tests/e2e/tauri-mock.ts` pattern

### 3. E2E Tests (`tests/e2e/*.spec.ts`)

**4 Playwright Tests** covering critical user journeys:
- `note-persistence.spec.ts` - Content persistence across note switches
- `note-rapid-switch.spec.ts` - Fast switching without race conditions  
- `note-concurrent-save.spec.ts` - Concurrent editing scenarios
- `note-create-delete.spec.ts` - Note lifecycle operations

**Run:**
```bash
npm run test:e2e
```

## Test Configuration

### Vitest Setup (`vitest.config.ts`)
- Environment: jsdom
- Tauri API mocked via `vitest-setup.ts`
- Includes SvelteKit integration

### Playwright Setup (`playwright.config.ts`)  
- Tests against live Tauri development server
- Uses mock Tauri backend for isolated testing

## Running Tests

### Individual Test Layers
```bash
# Backend only
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend unit tests only  
npm run test

# E2E tests only
npm run test:e2e

# Type checking
npx svelte-check --tsconfig ./tsconfig.json
```

### All Tests
```bash
npm run test:all
```

### Pre-build Quality Gate
```bash
bash scripts/pre-build.sh
```
Checks-only (#3243, PR D2): architecture invariants, version parity, spec check, and `svelte-check` (type check). `cargo test`/`npm test` are no longer run here; CI and `pnpm test` own them.

## CI/CD Integration

### GitHub Actions (`.github/workflows/test.yml`)
Triggers on push/PR to main:

1. **Setup:** Install Rust + Node 22, cache dependencies
2. **Rust Tests:** `cargo test`
3. **Type Check:** `npx svelte-check`  
4. **Frontend Tests:** `npm run test`
5. **E2E Tests:** `npm run test:e2e` (optional, continues on failure)

Any step failure blocks the merge.

### Local Pre-build Gate
The `scripts/pre-build.sh` script runs a subset of checks locally (architecture invariants, version parity, spec check, type check):
- Added as `prebuild` script in package.json
- Must pass before builds can proceed
- Rust/frontend tests run in CI (`rust-tests.yml`, `ci.yml`) and via `pnpm test`, not here

## Test Data & Isolation

### Frontend Unit Tests
- Use mocked Tauri API layer
- Fresh mock data for each test via `beforeEach()` reset
- No real file system operations

### E2E Tests  
- Use isolated mock Tauri backend
- Fresh state for each test file
- Temporary data that doesn't persist

### Rust Tests
- Use temporary databases and directories
- Clean up after each test
- No shared state between tests

## Critical Bug Coverage

These tests specifically target known problematic flows:

1. **Note Selection Race Conditions**
   - Null-then-set pattern in `selectNote()`
   - Editor save completion before note switch

2. **Store Merge Conflicts**  
   - `loadNotes()` preserving just-created notes
   - Preventing cross-notebook contamination

3. **Task Lifecycle Consistency**
   - Create→update cycles maintaining store integrity
   - Status changes propagating correctly

4. **Content Mixing Prevention**
   - E2E tests covering rapid note switching scenarios
   - Concurrent save operations

## Metrics & Goals

### Current Test Counts
- **Rust Backend:** 14 tests
- **Frontend Unit:** 15+ tests across 3 files  
- **E2E:** 4 comprehensive workflow tests
- **Total Coverage:** ~33 automated tests

### Quality Gates
- All Rust tests must pass (14/14)
- All frontend unit tests must pass 
- Type checking must pass with no errors
- E2E tests should pass (tracked but non-blocking)

## Troubleshooting

### Frontend Unit Test Issues
- **Mock not working:** Check `vitest-setup.ts` Tauri API mock
- **Store state pollution:** Ensure `beforeEach()` reset in tests
- **Import errors:** Verify relative paths and mock setup

### CI/CD Failures  
- **Rust compile errors:** Check Cargo.lock consistency
- **Node dependency issues:** Clear cache, reinstall dependencies
- **Type check failures:** Run `npx svelte-check` locally first

### Local Development
- **Test debugging:** Use `npm run test -- --reporter=verbose`
- **E2E debugging:** Run `npm run test:e2e -- --headed` 
- **Mock inspection:** Access `window.__TAURI_INTERNALS__._mockData` in browser tests