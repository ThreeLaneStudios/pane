# Pane Onboarding Redesign — Dev Dispatch

**For:** Brit (implementation coordination)
**Session date:** 2026-05-06
**Source:** Split from `Pane_Onboarding_Redesign_Decisions__2026-05-06.md` for handoff.
**Companion doc:** `Pane_Onboarding_Design_Brief__2026-05-06.md` (for Claude Design).

This dispatch is the source of truth for implementation work. Foundational decisions (Q1–Q5, E, F) and cross-path concerns (X1–X8) are duplicated from the design brief — both consumers need them. State diagram, sub-task specifications, ADR-track decisions, source-reads, and sub-task running list are exclusive to this doc.
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

## Next steps for Brit

- **Draft three ADRs** (T1/T2/T3 in deliverable 4) before state-machine implementation kicks off. Recommendations included; push back if wrong.
- **Source-read seven items** in pending source-reads section. Some unblock specific sub-tasks (#8 blocks #879).
- **File 42 new sub-tasks + 3 ADR-track issues** as GitHub issues, labeled launch-blocking or post-launch per running list.
- **Look up #596** — issue details not in current session context. Fold into existing sub-task or stand alone.
- **Audit + close obviated #536 bugs** after design lands: #907, #908, #909, #910, #911, #937, #892. Confirm subsumed; close.
- **Reference design brief** for screen-by-screen flows when scoping individual implementation cuts.
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
