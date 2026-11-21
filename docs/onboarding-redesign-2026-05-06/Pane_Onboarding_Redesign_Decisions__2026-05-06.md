# Pane Onboarding Redesign — Decisions Log

**Session:** 2026-05-06 (Matt + Claude design partner, in support of #953 + #950)
**Status:** Design complete. All five paths locked (cross-path X1–X8, OC SameMachine, OC SplitMachine, OpenRouter, Anthropic, Ollama) and all four deliverables landed (old-screens fate, shared screens spec, state diagram, sub-task specifications). Remaining: split into two handoff docs (design brief for Claude Design, dev dispatch for Brit), file sub-tasks + ADR-track issues, audit obviated bugs.
**Purpose:** Durable record of locked decisions. Resumable mid-session if compacted/dropped or session lost. Not a spec — spec diffs come after design completes.

---

## Pre-locked decisions (carried in from session brief — do not reopen)

- **Q1.** Two DMGs (Pane DMG for user machine, Gateway DMG for OC machine).
- **Q2.** SplitMachine prerequisite order: OC installed → Tailscale on both machines → Tailscale connected → Gateway DMG installed on OC machine → Pane DMG installed on user machine → onboard with Gateway URL. OC onboarding fails hard if gateway unreachable. OR/Anthropic/Ollama remain available as escape paths.
- **Q3.** Pairing happens during onboarding. Provider switch later in Settings via cascade-UX (#873).
- **Q4.** SameMachine OC discovery: Pane probes `127.0.0.1:18789`. Found = SameMachine. Not found = customer installs OC themselves; Pane does not auto-install OC.
- **Q5.** Four onboarding paths: OpenRouter, Anthropic, Ollama, OpenClaw. OpenClaw splits into SameMachine vs SplitMachine. No explanation of what OC is.
- **E.** Identity file handling: Pane is source of truth (§22B.3.2). SameMachine = symlink. SplitMachine = initial sync via gateway, then push/pull keeps in sync.
- **F.** Workspace cleanup: SameMachine = archive non-identity files. SplitMachine = sync only whitelisted identity files (no cleanup needed).

---

## Cross-path decisions

### X1 — Provider picker

**Picker is screen two.** Account creation (sign-in) is screen one (already built; see Registration section).

**Layout.** Four cards stacked full-width, same shape as existing screen. Not tabs, not grid.

**Cards on the picker.** Four, not five. OC stays as one card; SameMachine vs SplitMachine is decided after click, not on the picker. Customer doesn't see those terms.

**Order.** OpenRouter, Anthropic, Ollama, OpenClaw. Same as existing.

**Default.** None. Existing screen pre-highlights OR — that's wrong. No auto-default.

**Card content.** Provider names only. No descriptions. Customer who doesn't recognize a provider needs a help link, not a one-line description.

**Help link pattern.** Single bottom-of-screen row: "New to AI providers? [link]". One link, not per-card.

**Skip option.** Yes, on this screen only. (See X2.)

**OC click → Screen 2.5: "Where's OpenClaw running?"** Three choices:
- "On this machine" → probes localhost:18789, routes to SameMachine flow
- "On another machine" → SplitMachine pairing flow
- "I don't have OpenClaw" → returns to picker

Rationale: probe-on-card-mount makes the picker visually jagged and forces Pane to make a topology claim before customer commits. Three-choice screen makes the topology call the customer's, makes "no OC" first-class.

"Probe failed on this machine" becomes a normal error state, not an architectural hole: "We didn't find OpenClaw at localhost:18789. Is it running? [Try again] [Help] [Go back]."

### X2 — Skip and set up later

**Skip exists only on the picker.** Subsequent screens get Back, not Skip. Different intents, different controls.

**Skip behavior.** Customer lands in Home (registered, no provider configured). Persistent banner: "Connect AI to enable chat → Settings". Cascade-UX (#873) handles Settings configuration. One configuration surface.

**State after skip.** Notes work, projects work, chat doesn't, memory extraction inactive. Banner stays until provider configured.

**No re-entry into onboarding screens.** Settings is the surface for post-skip provider config.

**Trade noted.** Keeping Skip is deliberate — customer paid for Pane, shouldn't be walled off if they don't have an OR key handy. If post-launch metrics show high skip-and-bounce, removing Skip is a one-line change.

### X3 — Switching providers mid-onboarding

**Back button on every post-picker screen returns to picker.** Picker is the hub; every path is a spoke. No state cached across path switches. Back from first sub-step of any path = picker. Back within a multi-step path = previous sub-step.

**Picker on return.** Looks identical to first visit. No "you tried OR, here's why it failed" surfacing — failure messaging happens on the path screen, not on return to picker.

**Why not cached drafts:** state model leaks (which draft wins on completion?). Pasting a key is 30 seconds — caching saves nothing.

### X4 — Multiple providers configured (pre-locked)

**Onboarding is single-provider.** Settings cascade-UX supports multiple post-onboard.

The "fallback" language in Q2 means: if customer chose OC and gateway unreachable, they can back out and pick OR/Anthropic/Ollama instead in onboarding. Not parallel config during onboarding — escape hatch from a failed path.

### X5 — Validation, retry, lockout

**Threat model:** registration handles identity; X5 is customer-fatigue, not adversarial.

**Validation timing.** Single surface at Test Connection time. Format + auth combined in one error path. No blur check — interrupts paste→Test Connection flow.

**Retry behavior.** No retry counter. No lockout. Each attempt independent. Provider-side rate limits handle abuse.

**Validation endpoints (source-read confirmed):**
- **OpenRouter:** `GET /auth/key`. Returns key validity + credit balance in one call. No credits consumed. Validates auth AND credits at onboard.
- **Anthropic:** `GET /v1/models`. Validates auth without consuming tokens. Does NOT validate credits — that would require a real completion call (~$0.0001 minimum). **Decision: skip credit validation at onboard for Anthropic.** "No credits" surfaces as `billing_error` on first chat instead.
- **Ollama:** Connection check only. No auth.

**Anthropic error quirk (source-read confirmed).** Anthropic sometimes returns billing failures as 400 `invalid_request_error` with "credit balance is too low" in the message, not as 402 `billing_error`. Error handler needs both type-check AND message-substring check for "credit balance". Not onboarding-specific — bites every Anthropic call. Sub-task filed.

**Error states (shared shape across providers).** Each error names cause + points to fix. No raw HTTP codes surfaced. All surface at Test Connection time.
- Format invalid → "That doesn't look like a valid [Provider] key. They start with [prefix]."
- Auth failed → "Key rejected. [Get a new key →]"
- No credits / suspended (OR only at onboard) → "Key valid but no credits. After adding credits, takes a moment to reflect — try again in 30 seconds. [Add credits →]"
- Network error → "Couldn't reach [Provider]. Check your connection. [Retry]"
- Rate limited (rare) → "Too many attempts. Wait a moment and try again."

### X6 — Network down during onboarding

**Three sub-cases, three answers.**

**Case 1 — Ecom unreachable at sign-in.** Hard block, retry button. Customer cannot proceed. Duplicate accounts are worse than launch-day frustration.

**Case 2 — Provider unreachable during validation.** Error state: "Couldn't reach [Provider]. Check your connection. [Retry]". Add a "Skip for now, set up in Settings" option here — distinct from picker-Skip, justified because network failure is environmental not chosen.

This skip writes the key locally (encrypted), marks provider as **unverified**, lands customer in Home with banner.

**Case 3 — Ollama probe fails.** Different shape, OL1/OL3 territory: "Couldn't find Ollama at localhost:11434. [Try again] [Custom endpoint] [Help with Ollama]". Not a network error in the cloud sense.

**Unverified retry timing.** Silent retry on app-open and chat-screen-open, NOT on first message send. Prevents mid-message validation surprises. Banner clears silently on pass, stays visible with retry on fail.

**Banner copy unification.** Skipped state and unverified state share single banner: "Connect AI to enable chat → Settings". Internal state differs (skipped = empty key field; unverified = saved key + "Last validation failed" inline note in Settings), but customer-facing message identical.

### X7 — Onboarding completion criteria

**Onboarding completes when customer reaches Home for the first time post-sign-in.**

**Three valid completion states:**
- **Verified** — provider validated. No banner.
- **Unverified** — provider configured but validation hit network error and customer chose to proceed. Banner.
- **Skipped** — picker-Skip taken. Banner.

**State stored.** `onboarding_completed_at = now()` plus provider state. Persists across restarts. Customer doesn't see onboarding screens again unless explicitly re-triggered (X8 territory).

**No celebration screen.** Drop into Home directly.

**No forced test message.** Validation = key works; don't make customer send a hello.

**Welcome message from AI lands in Home as first message.** (See X7 details below.)
- **Verified completion only.** Skipped/unverified land with no AI message; chat panel shows empty state.
- **OC paths:** agent-voiced, pulls from agent SOUL.md/IDENTITY.md. Brit's welcome ≠ Stia's welcome.
- **BYOK paths:** templated generic welcome. One message across OR/Anthropic/Ollama for launch.
- **Lands in Home channel.** Real chat message, persists, scroll-back-able. May trigger memory extraction on customer reply.
- **Specific copy/template:** deferred to implementation phase.

**OC paths complete after agent flow, not at pairing.**
- SameMachine: pairing → Your Team → Clean Up Workspace → (Point OC retired for SameMachine) → Home
- SplitMachine: pairing → Your Team → Import Identity → Home (other screens TBD)

**Quit mid-onboarding.** `onboarding_completed_at` stays null until Home reached. Next launch resumes from where they left off. State persists per-screen. No data loss, no forced restart.

### X8 — Re-onboarding

**Case 1 — Customer wipes Pane data.** Full re-onboarding. Treat as first-run.

**Case 2 — Customer migrates to new machine.** New install, same account. After sign-in, before picker:

> "We've detected this is a new install. Do you have a Pane backup to restore?"
> [Restore from file →] | [Start fresh →]

Restore = file picker, restores DB + settings + .pane/system/, skips picker, drops into Home with their data. Start fresh = normal onboarding.

**Sub-task to verify:** backup file format includes encrypted provider keys (already does for SQLite-stored config? confirm).

**Case 3 — Adding OC machine for SplitMachine.** Run OC SplitMachine onboarding from the OC side. Pane stays single-machine. No new branch.

**2-Pane-machines (multi-Pane installs).** NOT supported in v1. Tier 1 hosted handles this post-launch. Two failure shapes:
- 2 Pane + BYOK: files split, no sync — "doesn't work" failure
- 2 Pane + OC: write contention on `.pane/system/` → silent data corruption (worse)

**Mechanical prevention via ecom-backed single-active-machine enforcement at sign-in.** Same UI pattern handles legit migration AND accidental multi-install:

> "This account is signed in on [machine name]. [Take over here] [Cancel]"

Take over → old machine signs out on next ecom check-in. Cheap (active_machine_id on ecom + check at sign-in). Standard for non-syncing single-license SaaS.

**Belt + suspenders.** Gateway should also reject second Pane pairing if one is active — separate trust domain from ecom, edge cases in offline grace windows. Sub-task to verify or implement.

**Docs still useful.** One-liner in onboarding/help: "Pane is single-machine for v1; multi-machine sync in Tier 1." Sets expectation so the take-over prompt isn't a surprise.

---

## Registration / sign-in (screen one)

**Sign-in only.** Registration done at ecom site (already built). No in-app account creation.

**Single email field first.** On Continue, hit ecom backend to check existence:
- Email exists → "Welcome back. Enter your password." (sign-in mode)
- Email doesn't exist → "No account found. [Get Pane →]" (link to ecom signup)
- Ecom unreachable → hard block, retry button (per X6 case 1)
- Wrong password → "[Forgot password →]" (link to ecom reset)

**Account ↔ entitlement.** Account exists ⇒ purchased (by construction at ecom). Single source of truth. Eliminates duplicate-account drift and entitlement orphans.

**Subscription lapsed at sign-in.** App locks. Renew screen with [Renew →]. Hard block. (Read-only fallback rejected — keeps engineering simple, MD files remain accessible via any text editor, brand promise preserved at file level.)

**Grace period.** 5-day grace period with countdown banner before hard lock kicks in. Standard SaaS pattern; protects paying customers whose card just expired. Sub-task #15 below.

---

## OC SameMachine path

**Entry:** Picker → "Where's OpenClaw running?" → "On this machine" → probe localhost:18789.

**Probe success → 5-step flow:**

### 1. Auto-pair (silent ideally)

Pane authenticates to local gateway. Pending Brit source-read on whether gateway has same-machine bypass or pairing code dance runs regardless. Pairing's threat model (untrusted network, fingerprint verification) doesn't apply when both processes are on same trusted machine — but that's a code reality question, not a design decision.

If pairing-code-only: visible screen with URL auto-fill (`localhost:18789`) + customer runs `openclaw gateway pairing-code` and pastes. ~30s friction.

If silent auth available: read auth secret from `~/.openclaw/openclaw.json` (or wherever it lives), authenticate, advance.

### 2. Your Team (display-only, redesigned)

**Old screen retires.** Radio-button main agent picker is replaced.

**New shape:**

> Your team.
> Main: Brit Freed
> Other agents: Stia, Reed, Lane (work behind the scenes)
> [Continue]

**Decisions codified:**
- Main agent determined by OpenClaw config. Pane displays which agent is main, customer does not pick. Read-only.
- Subagents work behind the scenes — spawned and managed by main agent. Customer does not interact directly. Standard pattern.
- Display all agents in onboarding. Read-only system docs viewable per agent (SOUL, MEMORY, etc.) in Settings post-onboard.
- Direct subagent interaction is a future feature, not v1.
- Changing main agent = OC-side config change (`openclaw config set agents.main ...`). Pane Settings shows help link: "To change main agent, edit OpenClaw config."

### 3. Symlink creation (silent)

Pane creates `.pane/system/<agent>/ → OC workspace` for each agent. Filesystem op, <1s. No screen if successful. Failure surfaces with retry or manual path entry.

### 4. Clean Up Workspace (refreshed)

Old screen too busy with file lists. New shape:

> **Clean up your AI's workspace**
> We found 247 files (12.4 MB) that aren't part of your AI's identity — code, drafts, working files. Move them to `~/.openclaw/workspace-archive/` to keep the workspace tidy. Identity files (memory, soul, tools) stay where they are.
> [Archive] [Skip — keep as-is]

Skip valid. Re-triggerable from Settings post-onboard (sub-task).

### 5. Home + welcome message

Per X7. Main agent (Brit, in current example) sends welcome in Home channel as first message customer sees.

### Retired for SameMachine

- **"Point OpenClaw to Pane"** screen retires for SameMachine. Symlink direction is Pane→OC; OC needs no reconfig. Open question for SplitMachine deferred.

### Failure modes (named, with surfaces)

| Failure | When | Surface |
|---|---|---|
| Malformed `~/.openclaw/openclaw.json` | Auto-pair | "We found OpenClaw but couldn't read its config at `~/.openclaw/openclaw.json`. The file may be corrupted. [Retry] [Help]" |
| Symlink name collision (`.pane/system/main/` exists) | Symlink creation | "A previous Pane setup exists at `.pane/system/`. [Use existing] [Replace and re-link] [Cancel]" |
| OC restart between probe and auth (token rotates) | Auto-pair | "OpenClaw restarted during setup. [Retry]" — silent retry once before surfacing |
| Permissions error on symlink | Symlink step | "Couldn't create symlink at `~/.pane/system/`. Check permissions. [Retry] [Help]" |
| OC workspace path not set in OC config | Auto-pair | "OpenClaw doesn't have a default workspace configured. [Set up in OpenClaw] [Help]" — hard block, customer fixes OC-side |

"Use existing" preserves prior `.pane/system/` state for reinstall case. "Replace and re-link" wipes. Decision logic is a sub-task.

---

## OC SplitMachine path

**Entry:** Picker → "Where's OpenClaw running?" → "On another machine" → SplitMachine pairing.

**Three trust boundaries:** network reach (Tailscale), TLS identity (fingerprint), app-level pairing (code).

### 1. Prerequisites — inline note, no checklist screen

No pre-flight checklist. Checklists without enforcement create false confidence; actual failures (gateway unreachable, Tailscale not connected) surface diagnostically anyway.

Inline note on URL entry screen: "Make sure OpenClaw + gateway + Tailscale are running on both machines. [Setup help →]"

### 2. Gateway URL entry

Single field. Placeholder: `https://your-machine.ts.net:18789`. Auto-prepend `https://` if missing. Default port `:18789` if customer types just hostname. Probe on Continue.

Tailscale auto-discovery of gateway URL = post-launch (#24 in sub-task list). Tailscale may not be installed/permitted at first run; only shows same-tailnet machines.

### 3. Combined verify + connect screen

One screen, not two. Mental model is "verify and connect," not "verify, then connect."

> **Verify and connect**
> Pane found a gateway at [URL]. Confirm the fingerprint and enter the pairing code.
>
> Fingerprint: `SHA256:abc123...def456`
> On your OC machine, run: `openclaw gateway fingerprint` and confirm it matches.
>
> Pairing code: `[_ _ _ _ _ _]`
> Get a code by running `openclaw gateway pairing-code` on your OC machine. Codes expire in 5 minutes.
>
> [Connect] [Fingerprint doesn't match]

"Fingerprint doesn't match" → hard block: "Don't continue. The mismatch could mean another machine is intercepting your connection. [Help] [Try a different URL]"

**Fingerprint pinning dependency:** #872 (TLS fingerprint pinning umbrella, shipped via PRs #901/#914/#912/#941 May 4-5). Pinning is real, verification UX is real. (Earlier notes referenced #714 — that was stale; corrected to #872.)

### 4. Initial sync (automatic per E)

Brief informational pause, not silent — silent feels like nothing happened, pause makes the team arrive visibly.

> **Importing your team**
> [progress]
> Synced 4 agents — Brit Freed, Stia, Reed, Lane (12 files)
> [Continue]

**No skip.** Sync is mandatory for SplitMachine.

**No per-agent retry/skip.** Retry-all-or-fail-pairing on any sync failure. v1 ships no partial-success state — customer paired thinking they have 4 agents must not land in app missing one with no clear recovery path. Graceful partial-sync handling filed as post-launch refinement.

Per F, only whitelisted identity files sync. Old "Import Identity Files" screen retires — its skip-per-file behavior doesn't fit the automatic-whitelist model.

### 5. Your Team (display-only)

Same as SameMachine. Main agent shown read-only (determined by OC config), subagents grouped beneath as behind-the-scenes.

### 6. "Point OpenClaw to Pane" — RETIRES for SplitMachine

OC-C resolved. Old screen existed because OC's workspace needed to point to Pane's symlink directory. SplitMachine has no shared filesystem — OC reads/writes its own workspace, Pane reads/writes its own `.pane/system/`, gateway syncs between them. OC needs no reconfig.

Footnote on old screen ("make sure `~/.pane/system/<agent>/` is accessible via shared folder, Tailscale, or Syncthing") was the giveaway that the old design assumed shared filesystem. Gateway architecture replaces it.

Brit source-read sub-task (§22B.3 / §22C) confirms code matches architectural intent.

### 7. Home + welcome message

Per X7. Main agent sends welcome in Home channel.

### Retired for SplitMachine

- **"Connect to Pane Bridge"** (entire screen — bridge replaced by gateway)
- **"Import Identity Files"** (file-list with skip-per-file — replaced by automatic sync)
- **"Point OpenClaw to Pane"** (OC-C resolved, no OC-side reconfig)
- **"Clean Up Workspace"** (per F, only whitelisted files synced — no cleanup needed)

### Second-machine experience: empty by design

Per Q2, all OC-machine work is prerequisite (DMG install, Tailscale, gateway up, fingerprint/pairing-code commands). No further OC-machine actions during the flow itself.

**Headless OC consideration:** customer needs terminal access for `openclaw gateway fingerprint` and `openclaw gateway pairing-code`. For headless OC (closet machine, no display), help link covers ssh-via-Tailscale. #932 documentation must include this path.

### Failure modes (named, with surfaces)

| Failure | When | Surface |
|---|---|---|
| Gateway unreachable (URL wrong, gateway down, Tailscale not connected) | URL probe | "Couldn't reach gateway at [URL]. Check that gateway is running and Tailscale is connected on both machines. [Retry] [Help]" |
| URL reachable, not a gateway | URL probe | "We reached [URL] but it doesn't look like a Pane Gateway. [Try a different URL] [Help]" |
| Fingerprint mismatch (customer says doesn't match) | Verify+connect | Hard block warning, no Continue path |
| Pairing code wrong | Verify+connect | "Code didn't match. Try again or generate a new one." |
| Pairing code expired | Verify+connect | "Code expired. Generate a new code on your OC machine." |
| Connection drops during pair | Verify+connect | "Connection lost. [Retry]" |
| Initial sync fails (any agent) | Sync screen | "Sync failed. [Retry]" — retry-all, no skip path in v1 |
| Gateway sync rejects (config issue) | Sync screen | "Couldn't sync from gateway. Check OC configuration. [Help]" |
| Tailscale auth drops mid-flow | Any post-URL screen | "Tailscale connection lost. Reconnect on both machines. [Retry]" |

---

## OpenRouter path

**Entry:** Picker → "OpenRouter" → API key entry. Simplest path of the four. No sub-screens.

### 1. API key entry

> **Connect OpenRouter**
> Paste your OpenRouter API key.
>
> [`sk-or-v1-...`                        ]
> [Don't have one? Get a key →] (links openrouter.ai/keys)
>
> [Test Connection]                  [Back]

**No format check on blur.** Single validation surface at Test Connection time. Format + auth combined in one error path. Interrupting paste→Test-Connection flow with blur errors creates friction with no benefit.

### 2. Test Connection — `GET /auth/key`

Returns auth + credit balance free, no credits consumed. Validates both at onboard (OR-only capability — Anthropic can't).

**Outcomes:**
- **Success (200, credits available)** → advance to model picker. No celebration screen.
- **Format invalid** → "That doesn't look like a valid OpenRouter key. They start with sk-or-v1-."
- **Auth failed (401)** → "Key rejected. [Get a new key →]". Stays on key entry; customer pastes new key, retries.
- **No credits (402)** → "Key valid but no credits. After adding credits, takes a moment to reflect — try again in 30 seconds. [Add credits →]". Propagation delay acknowledgment is critical — OR billing has eventual consistency, immediate retry after adding credits gets 402 again, customer blames Pane.
- **Network error** → per X6, "Couldn't reach OpenRouter. [Retry] [Skip — set up in Settings]" (unverified-skip path).
- **Rate limited (429, rare)** → "Too many attempts. Wait a moment and try again."

### 3. Model picker

Fixes #910. Old flow had Save & Test below Model dropdown — Model couldn't populate until tested. New flow: validation succeeds → models fetched from OR `/models` endpoint → picker.

**Per OR3, never hardcoded — must fetch from source.**

> **Choose a default model**
> [Search models...                                              ]
>
> [scrollable list of models, fetched live]
>
> [Continue]                                                  [Back]

**No recommended-models list for v1.** Curation ages badly with weekly model drops; "why isn't [my favorite] recommended" is a complaint vector. Search + full list. Curation is a separate post-launch design problem if it's worth doing at all.

**No default selection.** Continue is disabled until customer picks one. Don't auto-select first model — accidental commits.

**Search filter logic (locked):**
- Substring match on model ID and display name
- Provider-prefix shortcut: typing `anthropic/` filters to that provider
- Case-insensitive
- Recently-updated first within match results

**Adaptive search treatment.** Same picker component across OR/Anthropic/Ollama. Visual weight scales with list size: above ~20-25 items (OR's case), search prominent. At or below threshold (Anthropic 5-10 models, Ollama typically 1-5), full list visible immediately with search collapsed-but-available. Customer sees the catalog at a glance on small lists, search is there if needed.

### 4. Welcome message + Home

Per X7. Templated BYOK welcome. Lands in Home channel.

### Failure modes — OpenRouter

| Failure | When | Surface |
|---|---|---|
| Format invalid | Test Connection | "That doesn't look like a valid OpenRouter key. They start with sk-or-v1-." |
| Auth failed (401) | Test Connection | "Key rejected. [Get a new key →]" |
| No credits (402) | Test Connection | "Key valid but no credits. After adding credits, takes a moment to reflect — try again in 30 seconds. [Add credits →]" |
| Network error | Test Connection | Per X6 — "Couldn't reach OpenRouter. [Retry] [Skip — set up in Settings]" |
| Rate limited (429) | Test Connection | "Too many attempts. Wait a moment and try again." |
| Models list fetch fails after auth succeeded | Model picker load | "Connected, but couldn't load models. [Retry]" — auth state preserved |
| No models match search | Model picker | Empty state: "No models match. Try a different search." |

---

## Anthropic path

**Entry:** Picker → "Anthropic" → API key entry. Same shape as OR; differences below.

### 1. API key entry

> **Connect Anthropic**
> Paste your Anthropic API key.
>
> [`sk-ant-...`                                ]
> [Don't have one? Get a key →] (links console.anthropic.com/settings/keys)
>
> [Test Connection]                  [Back]

Same single-validation-surface pattern as OR. Format + auth combined at Test Connection time. No blur check.

### 2. Test Connection — `GET /v1/models`

**Difference from OR: no credit validation at onboard.** Anthropic has no `/auth/key` equivalent. Validating credits would require a real completion call (~$0.0001 minimum). Per X5: skip credit check at onboard. "No credits" surfaces as billing error on first chat.

**Outcomes:**
- **Success (200)** → advance to model picker. Customer may still hit `billing_error` on first chat — handled by global Anthropic billing-error misclassification handler (sub-task #1) and chat-screen error path. Not an onboarding-screen concern.
- **Format invalid** → "That doesn't look like a valid Anthropic key. They start with sk-ant-."
- **Auth failed (401 `authentication_error`)** → "Key rejected. [Get a new key →]"
- **Network error** → per X6, "Couldn't reach Anthropic. [Retry] [Skip — set up in Settings]"
- **Rate limited (429)** → "Too many attempts. Wait a moment and try again."

No 402 path at onboard — credits not validated here.

### 3. Model picker

Models fetched from `/v1/models`. Per A2, never hardcoded. Same shared picker component as OR. Adaptive search treatment per OR section: at Anthropic's 5-10 model count, full list visible immediately with search collapsed-but-available.

### 4. Welcome message + Home

Per X7. Same templated BYOK welcome.

### A3 dropped from scope

Original A3 raised "Workbench access, rate limits, Claude Code overlap" as Anthropic-specific concerns. None are onboarding-screen concerns:
- **Workbench access** — entirely Anthropic console-side, no Pane integration
- **Rate limits** — handled at chat-time (429 retry/backoff), not onboarding
- **Claude Code overlap** — same key works for both; doesn't change onboarding shape. Note in #932 docs but not a screen.

### Failure modes — Anthropic

| Failure | When | Surface |
|---|---|---|
| Format invalid | Test Connection | "That doesn't look like a valid Anthropic key. They start with sk-ant-." |
| Auth failed (401) | Test Connection | "Key rejected. [Get a new key →]" |
| Network error | Test Connection | Per X6 — "Couldn't reach Anthropic. [Retry] [Skip — set up in Settings]" |
| Rate limited (429) | Test Connection | "Too many attempts. Wait a moment and try again." |
| Models list fetch fails after auth succeeded | Model picker load | "Connected, but couldn't load models. [Retry]" |
| **Billing error surfaces on first chat (not onboarding)** | First chat send post-onboard | Handled by sub-task #1 misclassification handler. Out of onboarding scope. |

---

## Ollama path

**Entry:** Picker → "Ollama" → probe `localhost:11434`. Different shape from cloud BYOK — no API key, no credits, no auth, detection-based.

### 1. Probe `localhost:11434`

Same probe-on-click pattern as OC SameMachine.

**Probe success — auto-configure path.** Fetch installed models from `/api/tags`. Three sub-cases:
- Models installed → model picker
- Ollama running, zero models installed → empty state with install guidance (see below)
- Endpoint reachable but unexpected response → "Reached `localhost:11434` but it doesn't look like Ollama. [Try a different endpoint] [Help]"

**Probe failure** → "Couldn't find Ollama at `localhost:11434`. [Custom endpoint] [Help installing →]" (links ollama.com/download).

### 2. Custom endpoint screen (when probe fails or customer chooses non-default)

Single field: `http://hostname:port`. Defaults `http://`, port `:11434`. Probe on Continue. Same outcome shape as default probe.

**v1 ships full custom endpoint** — any URL. Mac Studio in the closet via Tailscale is a real use case. Cost is near-zero (just a URL field, no fingerprint/pairing/TLS like the gateway). Security concern (exposed Ollama with no auth) is the customer's network choice, not Pane's responsibility.

### 3. Model picker

Same shared picker component as OR/Anthropic. Adaptive search treatment: typical Ollama install is 1-5 models, list visible immediately, search collapsed.

**Difference from cloud paths:** customer can't pick a model they don't have. The list IS the entire universe — what's on disk. No "browse all" expansion.

**Performance expectations (OL4):** single neutral line on picker — "Models run on this machine." Factual, no editorializing. Customer who chose Ollama knows the trade.

**Model display naming.**
- Strip `:latest` tag from display (latest is implicit, noisy)
- Show explicit non-latest tags: `:3b`, `:7b`, `:70b` shown verbatim
- Flat alphabetical list, no grouping by base model (overkill at small scale)
- Internal model ID preserved for API calls

Example: `llama3.2:latest` displays as `llama3.2`. `llama3.2:3b` displays as `llama3.2:3b`. Customer with both installed sees two distinct entries.

### 4. "No models installed" empty state

Pane-hosted help page (or inline guidance) covering both customer types:

> Ollama is running but no models are installed yet.
> Pull one to get started — for example:
> `ollama pull llama3.2`
> Or browse the catalog at [ollama.com/library →]
> [Retry]

Covers "I know what I want" (paste-and-run command) and "I need to discover" (browse catalog link). Plain ollama.com/library link alone is wrong target — that lists models but not the install command.

### 5. Welcome message + Home

Per X7. Same templated BYOK welcome.

### Failure modes — Ollama

| Failure | When | Surface |
|---|---|---|
| Default probe fails | Auto-configure | "Couldn't find Ollama at `localhost:11434`. [Custom endpoint] [Help installing →]" |
| Custom endpoint unreachable | Custom endpoint probe | "Couldn't reach Ollama at [URL]. Check the address and that Ollama is running. [Retry]" |
| Endpoint reachable, not Ollama | Either probe | "Reached [URL] but it doesn't look like Ollama. [Try a different endpoint] [Help]" |
| Ollama running, no models installed | After probe success | Pane-hosted empty state with `ollama pull` guidance + library link |
| Models list fetch fails after probe succeeded | Model picker load | "Connected, but couldn't load models. [Retry]" — unusual; possible Ollama version mismatch |
| Network error on remote Ollama (custom endpoint, off-machine) | Custom endpoint probe | Per X6 — "Couldn't reach [URL]. [Retry] [Skip — set up in Settings]" |

### Version compatibility (locked)

Accept-any version for v1. Probe responding = good enough. Ollama version surfaced in Settings for support diagnosis (POST-LAUNCH per #40). Asserting minimum version creates a fragile dependency on Ollama's release cadence and would block customers running pinned older versions.

---

## Reference: Old screens fate (deliverable 1 of 4)

Reference for Claude Design. Each of the 9 reference screens from the previous onboarding flow gets a verdict.

**Summary:** 4 redesign, 5 retire, 0 survive untouched.

### Old screens

**1. "Connect Your AI" (provider picker)**
- **Fate:** Redesigns
- **Paths:** All
- **Changes:** Drop descriptions on cards (provider names only). Drop pre-highlighted default. Add bottom help link ("New to AI providers? [link]"). Becomes screen 2 (after sign-in). Skip option lives only on this screen.
- **Decision ref:** X1, X2

**2. "Connect Your AI" with Gateway URL + Auth Token (OpenClaw selected)**
- **Fate:** Retires
- **Paths:** OC SplitMachine (was), OC SameMachine (was)
- **Replaced by:** SplitMachine "Verify and connect" combined screen (new screen F below). SameMachine: silent auto-pair (or pairing-code dance pending Brit source-read on #872 path).
- **Why retired:** Bridge auth-token model is dead. New gateway uses pairing code + fingerprint TLS pinning, not pre-shared token.
- **Decision ref:** OC-A, OC-B

**3. "Connect Your AI" — OpenClaw detected state**
- **Fate:** Retires
- **Paths:** OC SameMachine (was)
- **Replaced by:** "Where's OpenClaw running?" (new screen E) → localhost probe → silent auto-pair on success.
- **Why retired:** Conflated detection + connection on single screen. New flow separates topology decision (where) from authentication (how).
- **Decision ref:** X1 hole fix, OC-B

**4. "Connect to Pane Bridge"**
- **Fate:** Retires
- **Paths:** OC SplitMachine (was)
- **Replaced by:** "Verify and connect" combined screen (new screen F).
- **Why retired:** Bridge architecture replaced by Pane Gateway. URL + Auth Code paradigm replaced by URL + Fingerprint + Pairing Code.
- **Decision ref:** OC-A

**5. "Your Team"**
- **Fate:** Redesigns
- **Paths:** OC SameMachine, OC SplitMachine
- **Changes:** Radio-button main agent picker → display-only. Main agent shown read-only (determined by OC config, not customer choice). Subagents grouped beneath as behind-the-scenes. Shape: "Main: Brit Freed" + "Other agents: Stia, Reed, Lane (work behind the scenes)" + [Continue].
- **Why changed:** OC-D codification — main agent is OC-config-determined; subagents work via main agent, not directly. Direct subagent interaction is post-launch.
- **Decision ref:** OC-D

**6. "Import Identity Files"**
- **Fate:** Retires
- **Paths:** OC SplitMachine (was)
- **Replaced by:** "Importing your team" sync pause (new screen G).
- **Why retired:** File-list-with-skip-per-file pattern doesn't fit automatic whitelist sync model (per E). Customer doesn't choose what syncs — gateway whitelist determines.
- **Decision ref:** E (locked), OC-A

**7. "Clean Up Workspace" (initial state)**
- **Fate:** Redesigns
- **Paths:** OC SameMachine only (per F, doesn't apply to SplitMachine)
- **Changes:** No detailed file list on initial screen. Summary only: "We found N files (X MB) that aren't part of your AI's identity — code, drafts, working files." [Archive] [Skip — keep as-is]. Re-triggerable from Settings post-onboard (sub-task #11).
- **Why changed:** Old screen too busy with file list.
- **Decision ref:** F (locked), OC SameMachine flow step 4

**8. "Point OpenClaw to Pane"**
- **Fate:** Retires
- **Paths:** OC SameMachine (was), OC SplitMachine (was)
- **Replaced by:** Nothing — no equivalent screen needed.
- **Why retired:** SameMachine: symlink direction is Pane→OC, OC needs no reconfig. SplitMachine: gateway syncs bidirectionally, OC needs no workspace reconfig (pending Brit source-read confirmation per pending #7).
- **Decision ref:** OC-C

**9. "Clean Up Workspace" — Archive success**
- **Fate:** Redesigns
- **Paths:** OC SameMachine only
- **Changes:** Simpler — count of archived files + Continue. Drop full file list (was redundant with the directory it just wrote to).
- **Why changed:** Same rationale as #7.
- **Decision ref:** F (locked)

### New screens to draft

Screens that don't exist in the old reference set. All to be drafted by Claude Design.

**A. Sign-in entry (email + password)**
- **Paths:** All (entry point, before picker)
- **Purpose:** Single email field. On Continue, ecom backend checks existence. Routes to password entry (existing account) or "no account, [Get Pane →]" (new prospect, link to ecom). Hard block on ecom unreachable with retry.
- **Status:** Already built per Matt; may need refinement to match unified email-existence-check pattern.
- **Decision ref:** Registration / sign-in section

**B. Subscription lapsed (hard block)**
- **Paths:** All
- **Purpose:** "Subscription needs renewal. [Renew →]". Shown when ecom returns inactive subscription status. Hard block. Future: 5-day grace period with countdown banner before this kicks in (#15).
- **Decision ref:** Registration / sign-in section, X5

**C. Take-over-machine prompt**
- **Paths:** All
- **Purpose:** "This account is signed in on [machine name]. [Take over here] [Cancel]". Single-active-machine enforcement. Same UI handles legit migration AND accidental multi-install.
- **Decision ref:** X8 (multi-Pane prevention), sub-task #6

**D. Backup restore prompt (X8 case 2)**
- **Paths:** All (after sign-in, before picker)
- **Purpose:** "We've detected this is a new install. Do you have a Pane backup to restore?" [Restore from file →] [Start fresh →]. If restored, drops into Home with restored data, skips picker entirely.
- **Decision ref:** X8

**E. "Where's OpenClaw running?" (screen 2.5)**
- **Paths:** OC paths
- **Purpose:** Three-choice routing screen after OC card click. "On this machine" → SameMachine probe. "On another machine" → SplitMachine pairing. "I don't have OpenClaw" → returns to picker.
- **Decision ref:** X1 hole fix (option B)

**F. "Verify and connect" (SplitMachine, combined screen)**
- **Paths:** OC SplitMachine
- **Purpose:** Two-section screen. Top: fingerprint display + "On your OC machine, run: `openclaw gateway fingerprint`" + match confirmation. Bottom: 6-digit pairing code field + "Get a code by running `openclaw gateway pairing-code`" + 5-min expiration timer. [Connect] and [Fingerprint doesn't match] (hard-block warning path). Replaces old screens #2, #3, #4 collectively.
- **Decision ref:** OC SplitMachine flow step 3

**G. "Importing your team" (sync pause)**
- **Paths:** OC SplitMachine
- **Purpose:** Brief informational pause during initial sync. Progress indicator + agent count + names ("Synced 4 agents — Brit Freed, Stia, Reed, Lane (12 files)"). Auto-advances on success. Retry-all on any sync failure (no skip-per-agent in v1).
- **Decision ref:** OC SplitMachine flow step 4, E (locked)

**H. Ollama custom endpoint**
- **Paths:** Ollama
- **Purpose:** Single URL field with `http://` and `:11434` defaults. Probe on Continue. Full custom endpoint scope (any URL — local-different-port, remote-via-Tailscale, etc.).
- **Decision ref:** Ollama flow step 2, OL3

**I. Ollama "No models installed" empty state**
- **Paths:** Ollama
- **Purpose:** Pane-hosted help. Shows `ollama pull <name>` example command + link to ollama.com/library. Covers "I know what I want" (paste-and-run) and "I need to discover" (browse). [Retry] after install.
- **Decision ref:** Ollama flow step 4, OL2

**J. Welcome message in Home (chat message, not a screen)**
- **Paths:** All verified paths (not skipped/unverified)
- **Purpose:** First AI message in Home channel. Templated for BYOK paths (OR/Anthropic/Ollama). Agent-voiced for OC paths (pulls from agent SOUL.md/IDENTITY.md). Specific copy deferred to implementation.
- **Decision ref:** X7

**K. Persistent banner — "Connect AI to enable chat → Settings"**
- **Paths:** Skipped or unverified completion states
- **Purpose:** Visible in Home until provider configured/verified. Single banner copy shared across skipped + unverified states (internal state differs, customer-facing copy identical).
- **Decision ref:** X2, X6

### Picker component note (cross-cutting)

The shared BYOK model picker (sub-task #38) is a single component used by OR, Anthropic, and Ollama with adaptive search treatment based on list size. Claude Design should produce one component spec, not three picker designs.

- ≤20-25 items (Anthropic, Ollama): full list visible immediately, search collapsed-but-available (icon or compact form)
- &gt;25 items (OR): search prominent at top, scrollable list below

Filter behavior is identical across paths: substring match on model ID + display name, provider-prefix shortcut (`anthropic/` filters), case-insensitive, recently-updated first within match results. Ollama-specific: strip `:latest` tag from display.

---

## Shared Screens Spec (deliverable 2 of 4)

Visual design brief for Claude Design. Where the same component or pattern appears across multiple paths, this section is the source of truth. Path-specific screens live in their path sections above and in deliverable 1.

**Visual language inheritance (from existing screens):** dark theme background (~`#0a0a0a`), cyan-on-dark accent (~`#00d4d4`), centered modal-style layout with ~600px max-width content area, generous vertical rhythm, medium body weight, bold headers. Carry these unless explicitly overridden per component.

**Treat copy as suggested.** Final wording is Claude Design's call within the meaning constraint.

### S1. Sign-in entry

**Purpose:** Entry point. Single email field. Routes to password entry or "no account" message based on ecom existence check.

**When shown:** First screen on cold launch unless `onboarding_completed_at` is set. Already built per Matt; this spec captures intended behavior for refinement.

**Layout:** Centered, vertically middle. App logo above title. Single email field, full content-area width. [Continue] primary button below. Help footer.

**Content:**
> **Sign in to Pane**
> Enter your account email.
> [email field, placeholder: `you@example.com`]
> [Continue]
>
> Don't have Pane yet? [Get Pane →]

**States:**
- Default — empty field, Continue disabled
- Valid email format entered — Continue enabled
- Invalid format on Continue press — inline error: "Enter a valid email"
- Loading — spinner replaces Continue text
- Email exists in ecom → S1b password screen
- Email doesn't exist → S1c soft state below field
- Ecom unreachable → "Couldn't reach Pane. Check your connection. [Retry]" hard block

**Interactions:** Continue or Enter triggers ecom existence check. Get Pane link opens browser to ecom signup.

### S1b. Password entry

Sub-screen following S1 if email exists.

**Layout:** Same structure as S1. Title personalizes if ecom returns first name.

**Content:**
> **Welcome back, [first name]** *(falls back to "Welcome back" if no name)*
> Enter your password.
> [password field]
> [Sign in]
>
> [Forgot password →]

**States:**
- Wrong password — inline error: "Incorrect password. [Forgot password →]"
- Subscription lapsed (per ecom) → S8
- Subscription active + take-over needed → S9
- Subscription active + first machine → S10 (backup restore prompt) → picker

### S1c. No account found

Soft state on S1, not a separate screen. Appears below email field after existence check returns no account.

**Content:**
> No account found for [email]. [Get Pane →]

### S2. Provider picker

**Purpose:** Customer chooses how Pane connects to AI. Screen 2 of onboarding.

**When shown:** After sign-in (and after backup restore prompt if not restored). Skip option lives only on this screen.

**Layout:** Centered, vertically middle. Title only (no subtitle — old "Choose a provider and enter your API key" is wrong because Ollama and OC don't use API keys). Four cards stacked full-width. Single help link in bottom row. Skip text-link below help.

**Content:**
> **Connect Your AI**
>
> [OpenRouter card]
> [Anthropic card]
> [Ollama card]
> [OpenClaw card]
>
> New to AI providers? [Help →]
>
> Skip — set this up later

**Cards:** Plain. Provider name only, no description, no icon. Hover state. Click action varies per card.

**States:**
- Default — no card highlighted (NO pre-selected default per X1)
- Card hover — visual emphasis
- Card click — routes per below

**Interactions:**
- OpenRouter → S3 OR API key entry
- Anthropic → S3 Anthropic API key entry (same component, different config)
- Ollama → silent localhost:11434 probe (no transition screen unless probe finishes; then S5 picker, "no models" state, or custom endpoint screen)
- OpenClaw → "Where's OpenClaw running?" (new screen E from deliverable 1)
- Skip → Home with banner S6 (per X2)

### S3. BYOK API key entry pattern (shared: OR, Anthropic)

**Purpose:** Single-screen API key validation for BYOK paths. Same component, different config per provider.

**When shown:** After picker → OR or Anthropic.

**Layout:** Centered modal. Title with provider name. Helper text. Single key field (full width, monospace recommended). Help link below field. Two-button row.

**Content (template):**
> **Connect [Provider]**
> Paste your [Provider] API key.
> [text field — placeholder: `sk-or-v1-...` or `sk-ant-...`]
> Don't have one? [Get a key →]
>
> [Test Connection]   [Back]

**Per-provider config:**

| Field | OpenRouter | Anthropic |
|---|---|---|
| Title | "Connect OpenRouter" | "Connect Anthropic" |
| Placeholder | `sk-or-v1-...` | `sk-ant-...` |
| Get-a-key link | openrouter.ai/keys | console.anthropic.com/settings/keys |
| Validates against (internal) | `GET /auth/key` | `GET /v1/models` |
| Validates credits at onboard | Yes (free with `/auth/key`) | No (deferred to first chat) |

**States (single validation surface at Test Connection — no blur check, per X5):**
- Default — Continue enabled (test fires regardless of field content; empty → format error)
- Loading — spinner replaces "Test Connection" text
- Format invalid — inline below field: "That doesn't look like a valid [Provider] key. They start with [prefix]."
- Auth failed — inline: "Key rejected. [Get a new key →]"
- No credits (OR only) — inline: "Key valid but no credits. After adding credits, takes a moment to reflect — try again in 30 seconds. [Add credits →]"
- Network error — inline + skip option: "Couldn't reach [Provider]. Check your connection. [Retry] [Skip — set up in Settings]"
- Rate limited — inline: "Too many attempts. Wait a moment and try again."
- Success → advance to S5 model picker (no celebration screen)

**Interactions:**
- Back → picker (per X3)
- Test Connection → validates against provider endpoint
- Skip (network error path only) → writes key locally encrypted, marks unverified, → Home + banner S6

### S4. Shared BYOK model picker component

**Purpose:** Customer picks a default model. Single component used by OR, Anthropic, and Ollama.

**When shown:** After API key validated (OR/Anthropic) or after Ollama probe success with models present.

**Layout:** Centered modal. Title. Optional subtitle (Ollama only). Search field (visual treatment varies by list size). Scrollable model list. Continue + Back.

**Content:**
> **Choose a default model**
> *(Ollama only)* Models run on this machine.
> [search field — adaptive treatment, see below]
> [scrollable model list]
>
> [Continue]   [Back]

**Adaptive search treatment:**
- ≤25 items (Anthropic 5-10, Ollama typically 1-5) — search collapsed-but-available (small icon/compact form, expands inline). Full list visible immediately.
- &gt;25 items (OR 300+) — search prominent at top of list, full-width input.

**Per-provider config:**

| Field | OpenRouter | Anthropic | Ollama |
|---|---|---|---|
| List source | `GET /models` (live) | `GET /v1/models` (live) | `GET /api/tags` (local) |
| Display naming | as returned | as returned | strip `:latest` from display, show explicit non-latest tags verbatim |
| Sort within match | recently-updated first | recently-updated first | alphabetical (no recency signal in Ollama API) |
| Hardcoded models | Never | Never | Never |
| Subtitle | (none) | (none) | "Models run on this machine." |
| Browse-all expansion | Future post-launch (see #27 — no recommended list in v1) | (small list, no need) | N/A — list is universe |

**Filter behavior:**
- Substring match on model ID + display name
- Provider-prefix shortcut (`anthropic/`) filters to that provider — only meaningful for OR
- Case-insensitive
- Sort per per-provider config above

**States:**
- Default — list visible, no selection, Continue disabled (per X5 push, no auto-select)
- Item hover — row highlight
- Item selected — row highlighted with cyan accent indicator (radio-style, designer call on visual)
- Search active — list filters in real-time
- No matches — empty state: "No models match. Try a different search."
- List fetch failed (after auth succeeded) — "Connected, but couldn't load models. [Retry]" with auth state preserved
- (Ollama-only) Zero models installed at probe time — don't show this screen, show new screen I instead

**Interactions:**
- Click item → selects, enables Continue
- Continue → writes provider+model state, advances to Home + welcome message
- Back → picker (per X3)

### S5. Persistent banner — "Connect AI to enable chat"

**Purpose:** Visible reminder when no provider configured or unverified. Single banner copy across both states.

**When shown:** In Home, top of content area or top of chat panel. Visible until provider verified.

**Layout:** Top-of-content banner, full width. Subdued accent color (informational, NOT error red). Not dismissable — stays until resolved.

**Content:**
> Connect AI to enable chat → [Settings]

The "→ Settings" portion is a clickable link.

**States:**
- Hidden — provider verified
- Visible (skipped state) — Settings link opens to empty key field
- Visible (unverified state) — Settings link opens to key+test panel showing "Last validation failed" inline note (per X6)
- Resolves — provider verified successfully (silent retry on app-open/chat-screen-open or user action) → banner clears

**Interactions:**
- Click → Settings opens to provider section
- No dismiss (intentional)

### S6. Welcome message in Home

**Purpose:** First AI message in Home channel after verified completion.

**When shown:** Verified completion only. NOT skipped/unverified.

**Layout:** Standard chat message in Home, AI side. Persists as real chat message — scroll-back-able, may trigger memory extraction on customer reply.

**Content (per path family):**
- BYOK paths (OR, Anthropic, Ollama) — templated welcome shared across all three. Specific copy deferred to implementation.
- OC paths (SameMachine, SplitMachine) — agent-voiced welcome. Pulls from main agent's SOUL.md + IDENTITY.md. Distinct per agent.

**States:**
- Sent on first home-load post-onboarding
- Persistent — not re-sent on subsequent app launches
- Customer reply triggers normal chat flow

### S7. Subscription lapsed (hard block)

**Purpose:** Block app access when ecom returns inactive subscription.

**When shown:** Sign-in succeeded but ecom flags subscription inactive. Also shown if subscription lapses mid-session and customer reopens.

**Layout:** Centered modal-style, full-screen overlay. Blocks all app interaction. No close button.

**Content:**
> **Subscription needs renewal**
>
> Your Pane subscription is no longer active. Renew to continue using the app.
>
> [Renew →]
>
> Sign in as different user

**States:**
- Default
- After [Renew →] click — opens browser to ecom renewal page; on app refocus, ecom check re-runs
- Renewal succeeded → drops into normal flow (Home if onboarding done, picker if not)

**Future (post-launch sub-task #15):** 5-day grace period with countdown banner before this screen blocks access. Not in v1.

### S8. Take-over-machine prompt

**Purpose:** Single-active-machine enforcement at sign-in.

**When shown:** Sign-in successful but ecom shows account active on a different machine.

**Layout:** Centered modal. Title + body + two buttons.

**Content:**
> **Pane is already signed in elsewhere**
>
> This account is signed in on [machine name]. Continuing here will sign out the other machine.
>
> [Take over here]   [Cancel]

**States:**
- Default
- Take over → ecom updates `active_machine_id`, old machine signs out on next ecom check, customer continues onboarding
- Cancel → returns to S1 sign-in

### S9. Backup restore prompt

**Purpose:** Detect new install, offer restore for migrating customers.

**When shown:** After sign-in, before picker, only if Pane local data is fresh-install (no existing DB).

**Layout:** Centered modal. Title + body + two buttons.

**Content:**
> **Restore from backup?**
>
> We've detected this is a new install. Do you have a Pane backup to restore?
>
> [Restore from file →]   [Start fresh →]

**States:**
- Default
- Restore → file picker opens (`.pane` backup format) → restore in progress (loader) → success drops customer into Home with restored data, skipping picker
- Restore fails (corrupted, version mismatch) → error message, fallback to Start fresh
- Start fresh → advances to picker (S2)

### S10. Error patterns (cross-cutting)

Shared shape for errors across paths. Inline patterns, not full screens.

**Network unreachable (provider validation):**
- Pattern: "Couldn't reach [Provider]. Check your connection. [Retry] [Skip — set up in Settings]"
- Skip only on path screens (per X3), never on picker
- Skip writes key locally encrypted, marks unverified, → Home + S5 banner

**Provider auth failure:**
- Pattern: "Key rejected. [Get a new key →]"
- Inline below field. Key remains in field for editing.

**Provider rate limited:**
- Pattern: "Too many attempts. Wait a moment and try again."
- No retry counter, no lockout. Provider-side rate limits handle abuse.

**No credits / billing:**
- OR (at onboard): "Key valid but no credits. After adding credits, takes a moment to reflect — try again in 30 seconds. [Add credits →]"
- Anthropic (at first chat — out of onboarding scope): handled by sub-task #1 misclassification handler

**Hard block (fingerprint mismatch, ecom unreachable, subscription lapsed):**
- Customer cannot proceed without resolution
- No skip option

**Service-unreachable (gateway, Ollama):**
- Pattern: "Couldn't reach [service]. [Retry] [Help]"
- Help link points to relevant setup docs (#932 for gateway, ollama.com/download for Ollama)

---

## State Diagram (deliverable 3 of 4)

For Brit's dev dispatch. Captures what's stored, what persists, what triggers state transitions across onboarding and post-onboarding lifecycle.

### Data model

#### Account / auth (source of truth: ecom backend)

| Field | Type | Purpose |
|---|---|---|
| `email` | string | Account identifier |
| `subscription_status` | enum: active, lapsed, cancelled | Drives lapse hard-block |
| `subscription_lapsed_at` | timestamp nullable | For future grace-period (#15) |
| `active_machine_id` | string nullable | Which machine has current session. Single-machine enforcement. |
| `account_first_name` | string nullable | For S1b personalization |

#### App-side onboarding state (source of truth: local DB)

| Field | Type | Purpose |
|---|---|---|
| `onboarding_completed_at` | timestamp nullable | Null = onboarding not done. Set when customer reaches Home (per X7). |
| `onboarding_resume_state` | enum: picker, byok_key_entry, byok_model_picker, oc_topology_choice, oc_pairing, oc_team_view, oc_workspace_cleanup, sync_in_progress, complete | Per-screen resumability per X7 |
| `provider_type` | enum: none, openrouter, anthropic, ollama, openclaw | Which provider configured |
| `provider_state` | enum: none, skipped, configured_unverified, configured_verified | Drives banner S5 + retry behavior |
| `last_validation_at` | timestamp nullable | When provider was last verified |
| `last_validation_result` | enum: success, auth_failed, no_credits, network_error, other | Inline note in Settings unverified state |

#### Provider-specific data (source of truth: local encrypted store + DB)

| Provider | Encrypted (keychain) | Plain (DB) |
|---|---|---|
| OpenRouter | `api_key` | `model_id`, `last_models_fetch_at` |
| Anthropic | `api_key` | `model_id`, `last_models_fetch_at` |
| Ollama | (none — no auth) | `endpoint_url` (default `http://localhost:11434`), `model_id`, `last_tags_fetch_at` |
| OpenClaw | `gateway_token`, `tls_cert_pin` | `topology` (samemachine/splitmachine), `gateway_url`, `paired_at`, `main_agent_id` |

#### Backup file format

Per sub-task #5 (verify exists; extend if not). Should contain:
- DB snapshot (SQLite)
- Settings JSON
- `.pane/system/` directory
- Encrypted provider keys (keychain export — needs verification)
- Backup metadata (app version, backup timestamp, account email)

### State machine: app launch

Runs every launch. Decision tree from cold launch to either Home or onboarding-resume.

```
1. Local DB exists?
   No  → fresh_install path: S1 sign-in
   Yes → continue

2. Sign-in still valid (token cached, not expired)?
   No  → S1 sign-in
   Yes → continue

3. Ecom check: account exists, subscription active, active_machine_id matches this machine?
   Network unreachable     → see "verify with Brit" below
   account_invalid         → S1 (account deleted on ecom)
   subscription_lapsed     → S7 hard block
   active_machine_mismatch → S8 take-over prompt
   all OK                  → continue

4. onboarding_completed_at IS NOT NULL?
   No  → resume from onboarding_resume_state
   Yes → drop into Home (provider_state determines banner S5 vs welcome message replay)
```

**Verify with Brit (launch state machine):**
- Does local data decryption require an ecom-issued token? Drives "ecom unreachable" hard-block decision. If yes, hard block until reachable. If no, allow read-only access in offline state (would change the X6 case 1 hard-block decision — flag this if applicable).
- How often does the ecom check run after launch — every launch only, or periodic during session?
- What's the `active_machine_id` matching algorithm — device fingerprint, install UUID, OS-level machine ID? Different choices have different reinstall implications (fresh install on same machine = same ID? or new ID?).

### State machine: onboarding flow

Linear with branches per path. Quit-mid-flow resumes from `onboarding_resume_state`.

```
S1 sign-in
  → S1b password (if email exists)
  → ecom checks: lapse (S7) | takeover (S8) | active+first-machine
  → S9 backup restore? (if fresh-install)
     → restored: drop into Home (skip picker entirely)
     → start fresh: continue
  → S2 picker
  → branch per path:
     OR/Anthropic:    S3 key entry → S4 model picker → Home + welcome
     Ollama:          probe localhost → S4 model picker | "no models" state | custom endpoint screen → Home + welcome
     OC SameMachine:  screen 2.5 topology → probe localhost → auto-pair* → Your Team → Workspace cleanup → Home + welcome
     OC SplitMachine: screen 2.5 topology → URL entry → Verify+connect → Importing your team → Your Team → Home + welcome
```

*Auto-pair shape pending Brit's source-read on gateway SameMachine bypass (sub-task #8).

**State writes during flow:**

| Trigger | Writes |
|---|---|
| Picker click | `provider_type = <choice>` |
| Path-screen advance | `onboarding_resume_state = <new>` |
| Test Connection / pairing success | `provider_state = configured_verified`, `last_validation_at = now()`, `last_validation_result = success`, encrypted key/cert/token stored |
| Skip on key-entry (network error path) | `provider_state = configured_unverified`, key stored encrypted, `last_validation_result = network_error` |
| Picker skip | `provider_state = skipped` |
| Reaching Home | `onboarding_completed_at = now()`, `onboarding_resume_state = complete` |

**No `onboarding_completed_at` until Home is reached.** Quit at any point before that = resume from last `onboarding_resume_state` on next launch.

### State machine: provider config (post-onboarding)

Mutable via Settings cascade-UX (#873). Same state values as during onboarding, managed by Settings instead.

```
configured_verified
  → (key edited / provider switched) → configured_unverified
  → (skip-equivalent — rare, Settings UX may not surface) → skipped

configured_unverified
  → (silent retry succeeds) → configured_verified
  → (silent retry fails) → stays configured_unverified

skipped
  → (provider configured in Settings) → configured_verified or configured_unverified
```

**Silent retry triggers (per X6):**
- App open (Pane process launch after `configured_unverified`)
- Chat screen open (customer navigates into chat panel)
- NOT on first message send (avoids mid-message validation surprises)

**Banner S5 visibility:** any state except `configured_verified`.

### State machine: subscription lapse

Driven by ecom `subscription_status`. Pane checks at:
- Sign-in (S1b password submit)
- App launch (post-S1)
- Periodic during session — TBD with Brit

```
subscription_active → (ecom flips lapsed) → S7 hard block on next ecom check
subscription_lapsed → (renewal succeeds, ecom flips active) → unlocks
                      → drops into Home (or resumes onboarding if onboarding_completed_at IS NULL)
```

**v1 behavior:** lapse → S7 immediately, no grace.
**Future (#15):** lapse → 5-day countdown banner → S7 on day 6+.

**Verify with Brit:** ecom check cadence post-launch. If only at launch, customer can use the app for a full session after lapse before being blocked. Acceptable for v1, but worth naming.

### State machine: take-over (single-active-machine)

Driven by ecom `active_machine_id`. Pane checks at sign-in.

```
S1b password OK → ecom returns active_machine_id
  matches this machine OR null → continue (claim as active if null)
  matches different machine     → S8 take-over prompt

S8 take-over confirmed → ecom updates active_machine_id → continue onboarding/Home
S8 cancel              → return to S1 sign-in
```

**On the displaced machine:** next ecom check (next launch or periodic) sees `active_machine_id ≠ this machine` → silent sign-out → S1 on next launch. Files on disk unaffected (per brand promise).

**Belt-and-suspenders:** gateway also rejects second Pane pairing if one is active (sub-task #7). Even if ecom enforcement is bypassed (offline grace, race condition, edge case), gateway holds the line for OC paths.

### What persists vs what's session-only

| Data | Persistence layer | Survives quit | Cleared by |
|---|---|---|---|
| Account, subscription | Ecom backend | Always | Account deletion at ecom |
| `onboarding_completed_at` | Local DB | Yes | Wipe Pane data (X8 case 1) |
| `onboarding_resume_state` | Local DB | Yes (resumable) | Reaching Home (set to `complete`); wipe |
| Provider type + state | Local DB | Yes | Wipe, or provider switch in Settings |
| API keys, certs, pins | Encrypted store (keychain) | Yes | Wipe, provider switch (key cleared) |
| Welcome message | Real chat message in Home | Yes | Customer deletes chat history |
| Banner S5 visibility | Derived from `provider_state` | Computed each render | State change resolves it |
| Active validation in flight | Session memory | No | App close, network failure |

### Edge cases / triggers worth naming

- **Customer wipes Pane data (X8 case 1):** entire local DB + encrypted store cleared. Account survives at ecom. Next launch → fresh-install path → S1 → S9 (backup restore offered).
- **Customer's account deleted at ecom:** sign-in fails at S1 with "no account found." Local data orphaned but not auto-cleared — customer takes manual action.
- **Network drops mid-pairing (OC SplitMachine):** state is whatever the last successful screen wrote. `onboarding_resume_state` reflects last advance. Next launch resumes.
- **Customer quits during initial sync (SplitMachine):** sync mid-state. Per retry-all-or-fail decision, incomplete sync = pairing not committed. `onboarding_resume_state = oc_pairing` so customer re-pairs and re-syncs from scratch.
- **Subscription lapses while customer is mid-onboarding:** if ecom check doesn't run mid-flow, customer reaches Home and next launch flips to S7. Verify with Brit whether lapse-mid-flow needs explicit detection or is fine to defer.
- **Take-over while customer is mid-onboarding on the original machine:** when does the displaced machine notice? Same answer as lapse — depends on ecom check cadence. Worst case: customer finishes onboarding on original machine, takes over from new one, original machine signs out on next launch. Acceptable.

### Three ADR-track decisions before implementation

State diagram surfaces three implementation decisions that need ADRs before code lands. Full trade-off framing in **deliverable 4 § ADR-track decisions**:

1. Local data decryption ↔ ecom token dependency (offline launch viability)
2. Ecom check cadence post-launch (lapse + take-over responsiveness)
3. `active_machine_id` matching algorithm (reinstall + hardware-change behavior)

Filed as sub-task #41.

---

## Sub-Task Specifications (deliverable 4 of 4)

For Brit's dev dispatch. Three sections: ADR-track decisions surfacing for ADR drafts, original 11 from #536 with full scope, references for new sub-tasks (1-42) and obviated bugs.

### ADR-track decisions before implementation

These are not code-reality source-reads. They're design-intent decisions with downstream implications. Each warrants an ADR per Dev Hygiene Playbook §2.

#### ADR-T1: Local data decryption ↔ ecom token dependency

**Question:** Does decryption of locally-stored Pane data (encrypted DB fields, keychain-stored API keys) require an ecom-issued token, or can decryption complete with only local credentials?

**Why ADR territory:** Decides whether offline launch is ever possible. Affects X6 case 1 hard-block decision, lapse hard-block UX (S7), Tier 1 hosted future architecture, and the "your notes, your machine, your rules" brand promise.

**Options:**

- **A. Decryption is local-only (OS keychain).** Pane can launch offline. Ecom check is for entitlement gating only. Customer with valid local data + lapsed subscription would technically be able to read their data offline (matches brand promise). Lapse hard-block becomes a UI overlay over still-decryptable data.
- **B. Decryption requires ecom-issued token.** Pane cannot launch offline. Ecom unreachable = hard block. Stronger entitlement enforcement; weaker brand promise (your data, but only when our server is reachable).
- **C. Hybrid — recent-token caching.** Decryption uses a local token that ecom refreshes periodically. Offline grace window (e.g. 7 days), then hard block. Compromise position.

**Required before:** X6 case 1 implementation, S7 implementation, Tier 1 hosted architecture decisions.

**Recommendation (push back if wrong):** Option A. Matches brand promise. Entitlement gating via ecom check at launch is sufficient — UI hard-block prevents app use when subscription lapsed, but underlying data remains the customer's. Option C is over-engineered for v1.

#### ADR-T2: Ecom check cadence post-launch

**Question:** How often does Pane verify subscription status and `active_machine_id` against the ecom backend during a session?

**Why ADR territory:** Determines lapse + take-over propagation responsiveness. Performance + privacy + correctness trade-offs.

**Options:**

- **A. Every launch only.** Simplest. Customer can use the app for a full session post-lapse before being blocked. Take-over from another machine doesn't affect this machine until it relaunches.
- **B. Every launch + on-focus.** Pane re-checks when window gains focus after being backgrounded. Catches lapse + take-over within minutes for active users.
- **C. Periodic poll (e.g. every 15 min during session).** Most aggressive enforcement, but adds network chatter, privacy noise (every active session reports to ecom regularly), battery cost.

**Required before:** lapse state machine + take-over state machine implementation.

**Recommendation:** Option B. Simple, responsive, no polling cost. On-focus matches typical web SaaS auth pattern. Option C is overbuilt for v1.

#### ADR-T3: `active_machine_id` matching algorithm

**Question:** What identifier does Pane use to match the current machine against ecom's `active_machine_id`?

**Why ADR territory:** Different choices have different threat models and customer-friendliness implications. Reinstall, hardware change, OS reinstall, Time Machine restore each behave differently per choice.

**Options:**

- **A. Device fingerprint** (composite: hardware identifiers, OS version, etc.). Survives reinstall on same machine. Breaks on RAM upgrade or motherboard swap. Customer doing legit hardware upgrade hits S8 take-over prompt unexpectedly.
- **B. Install UUID.** Generated at install time, stored in Pane local config. Doesn't survive reinstall — customer reinstalling Pane appears as a "new machine" to ecom. Catches sneak-around attempts. Most customer-friendly for legit hardware changes.
- **C. OS machine ID** (macOS hardware UUID, Linux machine-id). Survives reinstall + OS reinstall. Most aggressive. Closest to actual hardware identity.

**Customer scenarios to weigh:**

| Scenario | Option A | Option B | Option C |
|---|---|---|---|
| Reinstall-to-clear-state | Same machine | New machine (S8 prompt) | Same machine |
| Time Machine restore | Same | Same (UUID in backup) | Same |
| Hardware upgrade (RAM/SSD/board) | May break | Preserved | Preserved on macOS |
| OS reinstall (clean macOS) | May break | New machine | Preserved |

**Required before:** take-over state machine implementation.

**Recommendation:** Option B (install UUID). Customer-friendliest for legit hardware changes. Reinstall-to-clear-state edge case (customer expects fresh start to skip take-over) is acceptable friction — if they own both machines, take-over is a one-tap UX. Sneak-around concern is mitigated by gateway-side belt-and-suspenders (sub-task #7).

### Original 11 from #536 — full scope

All launch-blocking unless flagged. References point to design sections above.

#### #879 — SameMachine: auto-discover OC token from `~/.openclaw/openclaw.json`

- **Scope:** On localhost probe success, read OC config from disk, extract auth secret, authenticate to gateway. If pairing-code dance is required (per #8 source-read outcome), surface paste-from-terminal screen instead.
- **Out of scope:** Gateway-side bypass implementation (separate gateway concern). Fallback if openclaw.json missing or malformed (covered by SameMachine failure-mode #1).
- **Dependencies:** Sub-task #8 (Brit source-read on gateway SameMachine bypass).
- **Design ref:** OC SameMachine flow step 1.
- **Status:** Launch-blocking. Blocked on #8.

#### #880 — Onboarding: inline pairing flow for SplitMachine (fingerprint + code + test)

- **Scope:** Build combined "Verify and connect" screen (deliverable 1 screen F, deliverable 2 references). Fingerprint display, "On your OC machine, run: `openclaw gateway fingerprint`" instruction, 6-digit pairing code field with 5-min expiration timer, [Connect] primary, [Fingerprint doesn't match] hard-block path. Stacked-section layout pattern.
- **Out of scope:** Auto-discovery of Tailscale machines (#24, post-launch). URL probe pre-validation (#17, separate).
- **Dependencies:** Fingerprint pinning shipped via #872 (PRs #901/#914/#912/#941).
- **Design ref:** OC SplitMachine flow step 3, screen F.
- **Status:** Launch-blocking.

#### #881 — Onboarding: write topology on completion (SameMachine/SplitMachine)

- **Scope:** Persist `provider_type = openclaw` + `topology` enum (samemachine|splitmachine) at pairing success. Read by cascade-UX (#873) for Settings provider-state surfacing and topology switching.
- **Out of scope:** Topology-switching UX in Settings (cascade-UX already handles).
- **Dependencies:** State machine fields per deliverable 3.
- **Design ref:** State diagram → onboarding flow → state writes table.
- **Status:** Launch-blocking.

#### #882 — OC model picker: read-only display (no user selection)

- **Scope:** Settings shows the model OC is using (read-only display), pulled from OC's config. Customer cannot change model from Pane (must edit OC config OC-side). Help link: "Models are managed by OpenClaw. To change, edit OpenClaw config."
- **Out of scope:** Onboarding for OC paths does not include a model picker (OC determines).
- **Dependencies:** Sub-task #275 (hide picker UX for OC users).
- **Design ref:** OC-D codification.
- **Status:** Launch-blocking.

#### #883 — Drop agent picker from Settings + Onboarding (read-only main agent display)

- **Scope:** Replace radio-button main agent picker in onboarding "Your Team" screen with read-only display ("Main: [Agent]" + "Other agents: [list] (work behind the scenes)"). Same change in Settings agent view. Help link: "To change main agent, edit OpenClaw config."
- **Out of scope:** Per-agent system docs view (separate task — likely #813/#814).
- **Dependencies:** OC-D codified.
- **Design ref:** OC-D codification, deliverable 1 screen #5 redesign.
- **Status:** Launch-blocking.

#### #884 — Onboarding + Settings: concrete instructions for token sources (absorbs #374)

- **Scope:** Help text + commands documented for: gateway URL discovery, fingerprint command (`openclaw gateway fingerprint`), pairing-code command (`openclaw gateway pairing-code`). Surfaced inline on SplitMachine URL entry screen ("Make sure OpenClaw + gateway + Tailscale are running on both machines. [Setup help →]"), "Verify and connect" screen (F), and Settings provider sections. Per-step instructions per screen.
- **Out of scope:** External setup documentation (#932). Token field is gone (replaced by pairing code) — old "auth token" copy retires entirely.
- **Dependencies:** #932 documentation must include ssh-via-Tailscale path for headless OC (sub-task #23).
- **Design ref:** OC SplitMachine flow steps 2 and 3.
- **Status:** Launch-blocking. Absorbs #374 per session decision 2026-05-06.

#### #887 — UX: SameMachine 'Edit manually' disclosure — verify during exercise

- **Scope:** In Settings (not onboarding), expandable "Edit manually" disclosure for SameMachine power users. Reveals direct gateway URL + token edit fields for cases where auto-discovery from openclaw.json fails or customer wants custom config. Hidden by default. Verify behavior matches design intent during dogfood.
- **Out of scope:** Onboarding doesn't expose this — auto-pair is the only SameMachine onboarding path. Failure modes route through error UI per failure-mode table.
- **Dependencies:** Settings cascade-UX (#873).
- **Design ref:** OC SameMachine flow step 1, failure-mode "malformed openclaw.json".
- **Status:** Launch-blocking.

#### #374 — Merged into #884

- **Status:** Merged 2026-05-06 per session decision. Scope (inline helper copy on SplitMachine URL entry + Verify+connect screens) absorbed into #884. Close after #884 lands.

#### #275 — Settings: hide agent/model picker for OpenClaw users, show for OpenRouter (and Anthropic, Ollama) only

- **Scope:** Settings UX differentiates by `provider_type`. For OC: no agent picker (per #883), no model picker (per #882), read-only displays only. For OR/Anthropic/Ollama: model picker shown, customer can change default in Settings.
- **Out of scope:** Settings provider switching (cascade-UX #873).
- **Dependencies:** #882, #883.
- **Design ref:** OC-D codification, Settings UX.
- **Status:** Launch-blocking.

#### #596 — [details unknown — flag for Brit lookup]

- **Scope:** Issue details not in current session context. Brit to confirm scope and folder under appropriate design section.
- **Status:** Flag for triage. May fold into existing sub-task or remain standalone.

#### #953 — Onboarding redesign for gateway architecture (umbrella)

- **Scope:** This entire decisions doc + deliverables 1-4 IS the design output for #953. Implementation = sum of sub-tasks 1-42 + the original 11 above.
- **Status:** Design lands; implementation follows.

### New sub-tasks 1-42 — see running list

The 42 sub-tasks identified during this session are itemized in the **Sub-task list (running)** section below. Each has a one-line scope. Brit may need to expand individual ones into proper specs but the design constraints, decisions, and dependencies are captured.

Notable cross-references:

- Sub-tasks #1, #32 (Anthropic billing-error misclassification): bites every Anthropic call, not just onboarding
- Sub-tasks #6, #7 (single-active-machine + gateway pairing rejection): defense in depth, both required
- Sub-tasks #38 (shared BYOK picker component): one component used by OR + Anthropic + Ollama
- Sub-tasks #41 (ADR-track decisions): blocks state machine implementation across launch/lapse/take-over

### Sub-tasks obviated — close after design lands

These bugs surfaced by old onboarding flow are subsumed by the redesign. Per Matt: post-design, audit bug list and close anything subsumed.

- **#910** Save & Test position on OpenRouter — obviated by OR flow redesign
- **#911** Gateway URL no provenance indicator — obviated by SplitMachine pairing redesign
- **#907** Pairing spinner persists across provider switches — obviated by gateway pairing redesign
- **#908** Step 3 pairing spinner has no timeout — obviated by gateway pairing redesign
- **#909** Pairing error invisible on fast 1→0 fail — obviated by gateway pairing redesign
- **#937** Settings page fires models load before token in memory — obviated by Settings + onboarding state-machine redesign
- **#892** 3-model validation blocks OC-only Gateway save — obviated by per-provider Settings differentiation (#275)

### Implementation sequencing notes for Brit

- ADR-T1, T2, T3 should land before any state-machine implementation. They define the data model.
- Sub-task #8 (gateway SameMachine bypass source-read) blocks #879 — do that first.
- #874 chain (cascade-UX) is already shipped; verify integration points listed in source-read questions.
- Shared picker component (#38) should land before #26, #31, #35 — three consumers depend on it.
- #884 + #374 should be merged or sequenced together (same screens).
- #6 + #7 (single-active-machine enforcement) span ecom and gateway — sequence with backend coordination.

---

## Sub-task list (running)

### New sub-tasks identified this session

Launch-blocking unless tagged POST-LAUNCH inline.

1. **Anthropic billing-error misclassification handler** — handle 400 `invalid_request_error` with "credit balance is too low" in message as billing-related, not generic invalid-request. Type-check AND message-substring check. Bites every Anthropic call.
2. **Verify cascade-UX handles unverified-provider state** — silent retry on app-open and chat-screen-open. Single banner copy shared with skipped state. May be no-op if #873 already covers.
3. **Onboarding completion: send welcome message on verified provider config** — templated for BYOK, agent-voiced for OC (pulls SOUL.md/IDENTITY.md). Lands in Home channel. Specific content deferred to implementation.
4. **Backup restore in onboarding flow (X8 case 2)** — "Restore from backup?" screen after sign-in, before picker. File picker, restores DB + settings + .pane/system/. Skips picker if restored.
5. **Verify backup file format includes encrypted provider keys** — extend if not.
6. **Single-active-machine enforcement at sign-in** — ecom-side `active_machine_id` + app-side check. "Take over here" UI. Same flow handles migration AND multi-install.
7. **Gateway: verify or implement single-Pane pairing rejection** — belt + suspenders for X8 multi-Pane case. Separate trust domain from ecom.
8. **Brit source-read: does gateway have SameMachine pairing bypass?** — code-reality question. Determines whether SameMachine auto-pair is silent or pairing-code dance.
9. **Redesign "Your Team" screen** — display-only, main agent shown not picked, subagents grouped beneath read-only.
10. **Settings: agent system-docs read-only view per agent** — likely already #813/#814; verify scope covers per-agent SOUL/MEMORY/etc. read-only display.
11. **Settings: workspace archive re-trigger** — for customers who Skip workspace cleanup at onboard and later want it.
12. **Settings copy: "to change main agent, edit OpenClaw config"** — help link on agent view.
13. **Symlink creation pipeline: handle 5 named failure modes** — malformed openclaw.json, collision, OC restart, permissions, missing workspace path.
14. **Symlink collision: "Use existing" vs "Replace" decision logic** — preserves reinstall case.
15. **5-day grace period before subscription lapse hard-lock** — countdown banner during grace, hard lock on day 6+.
16. **Onboarding/help copy: single-machine-for-v1 expectation** — doc-only; sets expectation for take-over prompt.
17. **Gateway URL probe — distinguish "unreachable" from "reachable but not gateway"** — protocol check after TCP connect succeeds.
18. **Auto-prepend `https://` and default `:18789` on URL field** — input handling.
19. **Fingerprint screen UX + "doesn't match" hard-block path** — depends on #872 (shipped). UX implementation only.
20. **Pairing code entry: 6-digit field, expiration timer, retry on wrong code** — UI.
21. **Initial sync: retry-all-or-fail-pairing on any failure** — no partial-success state in v1. Graceful partial-sync handling filed as post-launch.
22. **Brit source-read: SplitMachine needs no OC-side workspace reconfig** — confirms OC-C answer holds in code (§22B.3 / §22C).
23. **#932 setup documentation scope check** — must cover prerequisite chain end-to-end + ssh-via-Tailscale for headless OC machines.
24. **Tailscale auto-discovery of gateway URL** — POST-LAUNCH only. Slick but complexity not justified for v1.
25. **Graceful partial-sync handling** — POST-LAUNCH. Per-agent retry/skip on initial sync, with clear recovery state for missing agents. Split out from #21 because v1 ships retry-all-or-fail.
26. **OpenRouter API key entry screen** — single Test Connection surface, format + auth in one error path, no blur check.
27. **OpenRouter model picker post-validation** — search field, model list fetched from `/models` (no hardcoding), Continue disabled until selection, no auto-select.
28. **OpenRouter search filter logic** — substring match on model ID + display name, provider-prefix shortcut, case-insensitive, recently-updated first within match results.
29. **OpenRouter 402 propagation delay copy** — "After adding credits, takes a moment to reflect — try again in 30 seconds." Sets expectation around OR billing eventual consistency.
30. **Anthropic API key entry screen** — same single-validation-surface pattern as OR.
31. **Anthropic model picker** — fetch from `/v1/models`, no hardcoding, uses shared picker component.
32. **Verify Anthropic billing-error surfaces correctly on first chat** — depends on sub-task #1 (misclassification handler). Test path: configured Anthropic key with $0 credits → first chat → expect clean "no credits" surface, not generic "invalid request."
33. **Ollama probe at `localhost:11434`** — auto-configure path on success, three sub-case handling (models installed / zero models / wrong service).
34. **Ollama custom endpoint screen** — full URL scope, `http://` and `:11434` defaults, probe on Continue.
35. **Ollama model picker** — fetch from `/api/tags`, uses shared picker component.
36. **Ollama "No models installed" empty state** — Pane-hosted help showing `ollama pull <name>` command + link to ollama.com/library. Plain library link alone is wrong target.
37. **Ollama "not installed" help screen** — link to ollama.com/download.
38. **Shared BYOK model picker component** (cross-cutting) — adaptive search treatment (full list visible at ≤20-25 items with collapsed search, prominent search above threshold). Filter logic from #28 lives here. Used by OR, Anthropic, Ollama.
39. **Ollama model display naming** — strip `:latest` tag from display, show explicit non-latest tags verbatim, flat alphabetical list, internal model ID preserved for API calls.
40. **Surface Ollama version in Settings** — POST-LAUNCH. For support diagnosis. Doesn't block onboarding.
41. **Three ADR-track decisions before implementation** — (a) local data decryption ↔ ecom token dependency (offline launch viability + brand promise), (b) ecom check cadence post-launch, (c) `active_machine_id` matching algorithm (reinstall + hardware-change behavior). Per Dev Hygiene §2, each warrants an ADR. Full trade-off framing in deliverable 4 § ADR-track decisions. Launch-blocking.
42. **Smart resume on mid-sync quit** — POST-LAUNCH. Allow OC SplitMachine to commit partial state (pairing succeeded, sync incomplete) and resume sync without re-pairing. Adds `pairing_committed_at` field distinct from `onboarding_completed_at`. v1 ships retry-all-or-fail; this is friction-reduction for slow networks or customers with many agents.

### Existing #536 sub-tasks implicated/refined (full specs in deliverable 4)

Original 11 sub-tasks: #879, #880, #881, #882, #883, #884 (absorbs #374), #887, #275, #596 (Brit lookup), plus #953 umbrella.

Full scope, dependencies, and launch-blocking status for each lives in **deliverable 4 § Original 11 from #536**.

### #536 bugs obviated by redesign (audit + close after design lands)

- **#910** Save & Test position on OpenRouter
- **#911** Gateway URL no provenance indicator
- **#907** Pairing spinner persists across provider switches
- **#908** Step 3 pairing spinner has no timeout
- **#909** Pairing error invisible on fast 1→0 fail
- **#937** Settings page fires models load before token in memory
- **#892** 3-model validation blocks OC-only Gateway save

Per Matt: post-design, audit bug list and close anything subsumed.

---

## Pending Brit source-reads

1. **Gateway pairing path on SameMachine.** Does gateway have a same-machine bypass, or does pairing-code dance run regardless? §22C and `src-gateway/` source.
2. **Backup file format coverage.** Does backup include encrypted provider keys, or only DB + settings without secrets?
3. **Cascade-UX (#873) coverage of unverified-provider state.** Already handles silent retry on app-open + chat-screen-open, or new logic needed?
4. **Settings agent system-docs scope (#813/#814).** Do these cover per-agent read-only SOUL/MEMORY/etc. display, or is that additional?
5. **Settings workspace archive re-trigger.** Exists or needs build?
6. **Gateway single-Pane pairing rejection.** Implemented or needs build?
7. **SplitMachine: confirm no OC-side workspace reconfig needed.** §22B.3 / §22C source-read confirms code matches the architectural intent that gateway sync handles bidirectional file movement and OC needs no workspace reconfig in SplitMachine. Discipline check on OC-C resolution.

---

## Outstanding (not yet designed in this session)

All five paths and all four deliverables locked. Remaining work is handoff:

- **Split into two handoff docs** at session end per Matt's preference: design brief (paths + deliverables 1-2) for Claude Design, dev dispatch (deliverables 3-4 + sub-task list) for Brit. Working notes, source-reads, and pre-locked decisions go where most useful.
- **File new sub-tasks** — 42 new + 3 ADRs (T1/T2/T3) need GitHub issues created, labeled launch-blocking or post-launch.
- **Audit + close obviated #536 bugs** — #907, #908, #909, #910, #911, #937, #892 confirmed subsumed.
- **Triage #596** — issue details not in current session context, Brit to look up.
- **Merge candidate #884 + #374** — both inline help/copy on same screens, recommend single sub-task.

---

## Working notes

- Walked X1–X8 in order; all locked.
- OC SameMachine fully designed except auto-pair (pending Brit source-read).
- OC SplitMachine fully designed. Three retired screens, one combined verify+connect screen, retry-all-or-fail-pairing on initial sync (no partial-success in v1).
- OpenRouter, Anthropic, Ollama all use shared BYOK picker component (#38) with adaptive search treatment scaling visual weight to list size.
- Ollama is the only path with no auth surface — local probe + custom endpoint as escape valve.
- Welcome message specifics deferred to implementation phase (Matt's call).
- Decision: brand promise preserved at file level on lapse; app locks but MD files remain accessible. Standard SaaS contract (Notion, Obsidian Sync, Bear pattern).
- Picker-Skip kept deliberately. Trade flagged: post-launch metrics may justify removal if skip-and-bounce is high.
- **Source-read discipline note:** cited #714 for fingerprint pinning from May 2 doc without verifying against current #536 — was stale; #872 is the umbrella, shipped May 4-5 via PRs #901/#914/#912/#941. Pattern: verify launch-blocking dependencies against current #536, not against earlier session docs.
