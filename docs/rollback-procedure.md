# Rollback Procedure

*What to do when a DMG breaks Matt's data or app. Speed matters — read this once, execute from memory.*

> **Scope:** This procedure covers pre-launch rollback (Matt is the only user). Post-launch rollback requires a different procedure — user communication, release channel management, forced update paths. File an issue to write the post-launch version within 2 weeks of launch.

---

## Severity Check

Before rolling back, confirm severity with Matt (see [Matt's Checklist](matts-checklist.md#when-something-breaks)):

- **Cosmetic/Functional** → normal bug report, no rollback needed
- **Blocking** → rollback if no quick fix
- **Data** → rollback IMMEDIATELY, investigate after

## Prerequisites

Confirm `~/Pane/.pane/backups/` exists and has recent `.db` files. If not, **stop and fix the backup system before trusting this procedure.** We don't want to discover mid-crisis that auto-backups haven't run in three weeks.

```bash
ls -lt ~/Pane/.pane/backups/*.db | head -5
```

---

## Step 1: Stop the Bleeding

```bash
# Kill Pane if it's running
pkill -f "Pane" || true
```

Do NOT reopen Pane until rollback is complete.

---

## Step 2: Restore the Database

Pane keeps automated backups at `~/Pane/.pane/backups/`.

First, **preserve the broken DB for forensics** — Reed needs this for root cause analysis:

```bash
# Preserve the broken DB before overwriting
cp ~/Pane/.pane/pane.db ~/Pane/.pane/pane.db.broken-$(date +%Y-%m-%d)
```

Then identify the right backup. Check `~/Shared/ExampleApp/builds/` modification dates or Brit's Discord announcement timestamps to identify when the broken version went live. Pick a backup from **before that time with margin.**

```bash
# List available backups (newest first)
ls -lt ~/Pane/.pane/backups/*.db

# Copy the pre-incident backup over the live database
cp ~/Pane/.pane/backups/pane-backup-YYYY-MM-DD.db ~/Pane/.pane/pane.db
```

**If no backups exist:** The flat `.md` files in `~/Pane/.pane/` are the fallback. Pane's integrity check will reconstruct the DB from them on next launch. But AI memory and conversation history won't be recovered — only note/task content.

---

## Step 3: Install the Previous DMG

```bash
# List available builds (newest first)
ls -lt ~/Shared/ExampleApp/builds/*.dmg
```

Install the DMG from BEFORE the broken build. If unsure which version was last good, check the Git log:

```bash
cd ~/Projects/example-app && git log --oneline -10
```

Find the last known-good version tag and match it to a DMG.

---

## Step 4: Verify

1. Open Pane
2. Verify concrete checks with Matt:
   - **Note count** matches expectations (roughly — "I had about 50 notes")
   - **Recent notes** — open a few specific recent notes, confirm content is present and correct
   - **Tasks** — check that tasks are in expected statuses (not all reset or missing)
   - **AI chat history** — recent conversations should appear
3. Matt confirms: "Data is intact" or "Something's still wrong — [what]"

---

## Step 5: Sync Implications

If sync is active (Supabase connected), the rolled-back local DB has older state than what's on Supabase. This will cause CRDT conflicts on next sync.

**Action:** Pause sync (or disconnect from Supabase) until root cause is resolved and we've decided how to reconcile. Options:
- Push local state to overwrite remote (if local is authoritative)
- Pull remote to catch up (if remote has good data local lost)
- Manual merge (worst case)

---

## Step 6: File the Bug

After rollback is stable:

1. Create a GitHub issue with:
   - Version that broke things
   - What happened (data loss? crash? corruption?)
   - What was rolled back to
   - Any error messages or logs
   - Path to preserved broken DB: `~/Pane/.pane/pane.db.broken-YYYY-MM-DD`
2. Label: `bug`, `data-integrity` (if data was affected)
3. Tag the issue as **Blocking** priority

---

## Step 7: Root Cause Before Next Build

**No new builds ship until the root cause is identified and fixed.** This is not optional.

1. Reed reviews the diff between the broken version and the last good version
2. Reed examines the preserved broken DB for clues
3. Identify the exact change that caused the failure
4. Write a test that reproduces the failure
5. Fix passes the test
6. Then — and only then — build the next DMG

---

## Git Rollback (if code needs reverting)

```bash
cd ~/Projects/example-app

# Find the last good commit
git log --oneline -20

# Revert the bad commit(s) — PREFERRED, preserves history
git revert <bad-commit-hash>

# Or hard reset to last good — LAST RESORT
git reset --hard <good-commit-hash>

# DANGEROUS: rewrites history. Never do this without Matt's explicit "go ahead for force push."
# Last resort only. Revert is almost always better.
git push origin main --force
```

**Prefer `git revert` over `git reset`.** Revert preserves history. Reset destroys it.

---

## Roles During Rollback

- **Matt** — confirms severity, authorizes rollback, verifies data integrity (Step 4)
- **Brit** — executes Steps 1-3, files bug in Step 6, spawns Reed in Step 7
- **Reed** — investigates diff, examines broken DB, identifies root cause

---

## Why This Exists

Pre-launch, Matt is the only user. A data-severity bug means Matt's real notes and tasks are at risk. Speed of rollback directly reduces data loss. This procedure exists so nobody has to think during a crisis — just execute the steps.
