# ADR-0064: Web Browsing Tools (`web_fetch`, `web_search`)

**Status:** Proposed
**Date:** 2026-08-27
**Author:** Matt Carano, Claude (spec session)
**Amended:** 2026-11-09 — D4's withheld set gains the five job tools (`schedule_job`, `spawn_job`, `cancel_job`, `pause_job`, `resume_job`); D4's residual paragraph rewritten. Reed's #3106 security review, finding C2. See Amendment 2026-11-09 at the bottom of the file.
**Relates to:** #2591, #2734 (per-runtime tool capability), #2522 / ADR-0062 / ADR-0063 (runtime-per-agent), ADR-0044 (keychain credentials), ADR-0046 / §4.5.8 (text-fallback tool parsing), #3106 (OSS prep — Reed security review C2)
**Spec:** `docs/spec/ai-integration.md` §4.6
**Research:** `docs/research/2026-08-19-web-browse-tool.md`

## Context

Pane's app tools are local DB and navigation operations. No tool reaches the internet.
Provider-native browsing (Anthropic `web_search`/`web_fetch`, OpenAI `web_search`, Gemini
grounding) is executed on the vendor's servers and only works with that vendor's API. Pane
dispatches to OpenRouter, Anthropic, Ollama, and (later) Venice and Hermes, so browsing
must be executed client-side in Rust and exposed as ordinary function-calling tools.

Two constraints shape every decision below. First, the week of 2026-08-26 re-established
(four simulated-pass/real-fail incidents, #2725 through #2741) that discipline-only guards
fail; every security claim here names a mechanism or an accepted risk. Second, OpenClaw-bound
sessions already browse through OC's own tools, so "which runtimes see these tools" is a
first-class decision, not an implementation detail.

Source facts this ADR relies on (commands.rs, tools.rs, runtime_resolution.rs,
extraction_worker.rs, credential_migration.rs, database.rs, Cargo.toml at 1.0.31):

- `ai_chat_stream` resolves `dispatch_cfg.runtime_type` before building the tool list
  (`[DISPATCH] session {} -> agent {} (runtime_type: {})`), and builds tools as
  `if is_lightweight || has_provider_override { vec![] } else { get_pane_tools() }`.
- `build_openclaw_tool_text_content()` also calls `get_pane_tools()`.
- `parse_text_tool_calls` runs for any provider when native parsing yields zero calls
  (§4.5.8), and `execute_single_tool` dispatches by name with no notion of which tools were
  offered.
- `build_messages_with_context` strips `role: tool` messages from history.
- The extraction worker reads `chat_messages_local.content` only; tool results live in the
  `tool_results` column.
- Provider keys go through `credential_migration::write_provider_key` (anthropic and
  openrouter only) into per-provider keychain accounts; `ollama_endpoint` is the precedent
  for a non-secret endpoint in `ai_settings`.
- `reqwest 0.12` (rustls-tls, json, stream, blocking) and `url 2.5` are present; no HTML
  parsing or readability crate is.

## Decisions

### D1. Two tools, two PRs, one release

`web_fetch(url, max_chars)` and `web_search(query, count)`, mirroring the Anthropic split.
Part 1 ships `web_fetch` alone; Part 2 ships `web_search` plus Settings. Both land in one
release; research-style use requires Part 2. Locked (research §3; Matt 2026-08-27).

### D2. Fetch is reqwest plus readability extraction in Rust; no browser

No bundled or spawned headless Chromium, no Tauri WebView reuse. Coverage is the static and
server-rendered web (roughly 70 to 85 percent of pages per research §2). JS-heavy pages fail
with a clear message. Locked (research §2, §5; issue Out of Scope).

### D3. Runtime allowlist, enforced at registry and at dispatch

Web tools are offered when the session's bound agent has `runtime_type` in
`{anthropic, openrouter, ollama}`. OC sessions are excluded (locked, Matt 2026-08-27 item 1).

The list is an allowlist rather than an `openclaw` denylist so Venice and Hermes have to opt
in when their runtime types are added; that is the #2734 capability question and this gate
is its first concrete instance. Until #2734 designs a general capability flag, the allowlist
is a constant in `tools.rs` with a forward pointer.

The registry gate (`get_pane_tools_for_runtime`) is not a mechanism on its own because the
§4.5.8 text fallback will parse `{"tool":"web_fetch",...}` from any provider's text and the
match arm would execute it. The dispatch gate (refuse any call whose name is not in this
dispatch's offered set) is the mechanism. Both ship in Part 1.

### D4. Injection guard: delimiters plus per-turn create-only withholding (option a, narrowed)

Once external web content has entered a turn, every tool that mutates existing data is
refused at dispatch for the rest of that turn. (There is no offer-time filter alongside
the dispatch gate here — see the Amendment 2026-11-09 section below.) `create_note` stays
available. Taint is per dispatch and ends naturally because tool
messages are stripped from history on the next turn.

Why create-only rather than full write withholding (Matt, 2026-08-27): blast radius. A
steered `create_note` produces one new note the user can see and delete. A steered edit,
append, move, delete, status change, or project/folder creation damages or reshapes existing
data. Keeping `create_note` open preserves the most common one-turn flow, "read this and
save a summary," without exposing existing notes to page-driven mutation.

Option (b), delimiters plus a system-prompt rule only, was rejected: it is exactly the
discipline-only guard the week's incidents argued against.

Accepted residual (recorded, not fixed; rewritten 2026-11-09, see Amendment below): within
the tainted turn the model can still be steered to issue read tools, further fetches, to
create a new note or sheet containing page-derived text, or to say things to the user. No
mutation of existing data is reachable, and no deferred execution is reachable: a tainted
turn cannot create, alter or spawn a job (the five job tools are in the withheld set), so
no later turn runs an instruction the page wrote. What remains cross-turn is the model's
own paraphrase, the same class D6 records for memory: (i) page-derived text the tainted
turn put into a new note is ordinary note content on every later read, including by a
user-scheduled job — `read_note` is not taint, by design; (ii) a job turn that fetches the
web writes a report in its own words, and the wake turn (§36.3) receives that report as a
synthetic user message. Both are model-authored, not raw page content, and both are
accepted here. The pre-agree toggle (Alternatives) and §36.16's `ToolPolicy.tainted`
field are where a stricter stance would live.

The user-facing "Tools used" box shows every `web_fetch` as `web_fetch(<url>) → <Title>`
(or the error text), using the existing tool-log path, so the user can see every page the
model read in the turn that produced the note.

### D5. Brave Search API with a user-supplied key; optional SearXNG endpoint

No Pane-managed shared key and no proxy (locked; issue). Brave key lives in the keychain
under ADR-0044 as a new account `ai_brave_key`, keychain-only read (no plaintext fallback,
the key postdates the migration). SearXNG endpoint is a non-secret `ai_settings` key
`searxng_endpoint` following the `ollama_endpoint` precedent, and overrides Brave when set.

Settings copy states no prices and links to Brave's key page.

### D6. Memory extraction ignores fetched content

Structural: the worker reads `content` only, and tool results are not in `content`. Pinned
by test. The residual (model paraphrases page content in its own reply) is mitigated with a
prompt line keyed on the assistant row's `tool_calls` column naming a web tool, and recorded
as accepted risk. Brit files the follow-up issue for a mechanical approach (for example,
tagging extracted facts with a `source` that the worker can drop). Locked scope: facts from the
user's own words stay extractable; facts from pages do not (Matt 2026-08-27 item 3).

### D7. SSRF check after DNS, pinned connection, re-check per redirect

Scheme allowlist, hostname pre-check, resolve, check every address against the blocked
classes, then connect only to the checked addresses via `ClientBuilder::resolve_to_addrs`.
Redirects are followed manually with the full check on each hop. `100.64.0.0/10` is blocked
deliberately even though it is the Tailscale range the Mac Studio gateway uses: the model
must not be able to fetch from the user's gateway or LAN. Locked in substance (issue;
research §4); the pinned-resolution and per-hop details are this session's.

### D8. Per-turn cap is a constant, not a setting

`WEB_TOOL_CALLS_PER_TURN = 5`, per dispatch, both tools combined. The issue said
"configurable". Rejected for v1: a setting needs a key, a writer, a UI control, and a
migration disposition (ADR-0061 D7), and no user has asked for a different number.
Promote to a setting when someone hits the cap for a real reason.

### D9. robots.txt is not honored in v1

A single fetch performed because the user asked is user-agent behavior, the same posture
as a person clicking a link. `robots.txt` governs crawlers. Revisit if Pane ever fetches
without an explicit user-initiated turn (scheduled research, background enrichment); that
would be crawling and would need `Disallow` handling. Locked (Matt 2026-08-27 item 5).

### D10. No authenticated or paywalled content

Fresh client per request, no cookie jar, no stored credentials, no login flows. A page that
needs a login returns the "could not extract" message. Locked (issue).

### D11. Part 3 forward pointer

Rendering fallback for JS-heavy pages (hosted renderer such as Jina Reader or Firecrawl,
opt-in user key) is Part 3. Not designed here; §4.6.6 carries one line pointing at it.

## Consequences

**Positive.** Every direct-provider model, including small OpenRouter and local Ollama
models, gets read-the-web and search-the-web with one uniform tool shape. Zero new native
dependencies. OC sessions are unaffected. Security claims each name a mechanism.

**Negative.** No JS-rendered pages in v1. Research-then-edit of an existing note takes
two turns (research-then-create is one). Brave is
metered and requires a card, which is friction for a BYO-key feature; SearXNG is the escape
hatch for users who object. The dispatch gate adds a per-call set lookup to every tool
dispatch (negligible).

**Risks.** The readability crate choice is deferred to build; both candidates are ports of
Mozilla Readability and either can be swapped without spec change. `resolve_to_addrs` must
be used with the exact host string reqwest will match on (no trailing dot, same case);
Reed's SSRF table test covers a mismatch. The extraction paraphrase gap is prompt-mitigated
only until the follow-up issue lands.

## Alternatives considered

**Provider-native browsing where available, Pane tools elsewhere.** Rejected: two code
paths, two behaviors, and the Pane path is still required for OpenRouter and Ollama. One
path for all direct providers is simpler and the coverage difference is Part 3's job.

**Headless browser (bundled Chromium, CDP to installed Chrome, or a hidden Tauri window).**
Rejected: hundreds of MB, platform packaging pain, seconds of startup per fetch, and in the
Tauri-window case loading arbitrary third-party pages into the app's WebView surface.
Hosted rendering is the right fallback and is Part 3.

**Single `browse` tool.** Rejected: separate tools let the model reason "find" versus
"read", keep each schema small for weaker tool-callers, and allow independent caps.

**Pane-managed Brave key.** Rejected: needs a proxy, a bill, and abuse controls. BYO key
matches the rest of Pane's provider posture.

**OpenClaw denylist instead of allowlist.** Rejected: Venice and Hermes would silently
receive web tools the day their runtime types are added, before anyone decided whether
their runtimes already browse. Allowlist forces the decision.

**Injection guard option (b): delimiters plus system-prompt rule only.** Rejected as
discipline-only; see D4.

**Full write withholding after taint (including `create_note`).** Considered and rejected
for v1: it breaks "read this and save a summary" into two turns for no data-safety gain,
since a new note is visible and deletable. Recorded so the narrowing in D4 is not read as an
oversight.

**Taint propagation into the job row (2026-11-09).** A column on `agent_jobs` set when the
creating turn was tainted, so a job born tainted dispatches tainted. Considered on Reed's
C2 and recorded as the more general mechanism, but not built: once the job tools are in the
withheld set no tainted turn can create a job, so there is nothing to propagate; on its own
it would still let an attacker-written brief run (reads, further fetches, `create_note`,
things said to the user) and does nothing for `cancel_job` / `pause_job` / `resume_job`.
If a future ruling permits post-taint scheduling, this is the mechanism, and §36.16's
`ToolPolicy.tainted` field is its home.

**Pre-agree toggle.** A per-turn or per-session user affirmation ("allow edits from web
content this session") that lifts the taint restriction. Not built now; may be added later
as an additive setting once the create-only guard has shipped and been observed. Any such
toggle must be a user action, never a model-callable tool.

**Per-turn cap as an `ai_settings` key.** Rejected; see D8.

**Honor robots.txt.** Rejected for v1; see D9.

**Persist a fetch cache or search history.** Rejected: no rows, no constraints, no
migration, no sync question. Revisit only with a concrete need.

## Amendment 2026-11-09: a tainted turn cannot create, alter or spawn a job

**Source.** Reed's #3106 security review (comment 6749034729, finding C2), read at
`2e5109d7`. Matt's ruling 2026-11-09. Lands before the repo flips public.

**What D4 missed.** D4 draws its withheld set on one axis: does the tool mutate existing
data? The harness sprint (#3050 PRs 4 and 5) added `schedule_job`, `cancel_job`,
`pause_job`, `resume_job` and `spawn_job` as `ToolAccess::Write` and, reasoning on that
axis, left them out of `MUTATE_EXISTING_TOOLS` — scheduling a job is not editing a note. A
test in `ai/chat.rs` pinned the exclusion. Reed's chain: a tainted turn calls
`schedule_job` with a brief the page wrote; `execute_schedule_job` inserts into
`agent_jobs` with no approval step; `scheduler_tick` later dispatches the job as a fresh
`TurnOrigin::Job` turn whose `external_content_tainted` starts `false`; that turn has every
write tool. Taint is per dispatch and does not follow the job. D4's residual sentence — "no
cross-turn effect is reachable" — was false from PR 4 onward.

The axis D4 did not ask about is whether the thing a tainted turn creates is **inert** (a
note the user sees and can delete) or **executable** (a job that will act later with full
tools). `create_note` and `create_sheet` are the first kind and stay exempt. A job is the
second kind.

**Decision.**

1. **The five job tools join `MUTATE_EXISTING_TOOLS`.** `schedule_job`, `spawn_job`,
   `cancel_job`, `pause_job`, `resume_job`. The constant goes from 18 to 23 members, which
   is every `ToolAccess::Write` tool except `create_note` and `create_sheet`. The fix is the
   one existing layer, not two: refused at dispatch by `check_external_content_taint` with
   the existing refusal text. There is no offer-time filter that withholds a tool from the
   list sent to the provider — the dispatch gate is the only mechanism that exists today
   (`get_pane_tools_for_runtime` takes no taint parameter; the tool list is computed once
   per dispatch and never rebuilt per iteration). No new mechanism, no new list, no prompt
   rule.
2. **Why all five.** `schedule_job` and `spawn_job` are the deferred-execution hole.
   `cancel_job`, `pause_job`, `resume_job` UPDATE existing `agent_jobs` rows — mutation of
   existing data by D4's own axis; leaving them out would be a second oversight of the
   same shape. A tainted turn that wants any of the five says so in its reply and the user
   does it, or asks for it, in the next turn.
3. **Inert / executable is now a stated D4 criterion.** A future tool that creates
   something which acts later without a user action — a job, a trigger, a webhook, an
   automation — is in the withheld set by default, whatever its `ToolAccess` mark.
4. **Taint propagation into the job row is recorded, not built** (Alternatives above).
5. **Proactive and triggered turns inherit this by construction.** Every Pane-dispatched
   turn runs `run_agent_loop` with a `TurnOrigin` (§36.11); the taint flag and the
   withheld set are properties of that loop, not of the turn's origin. A job turn that
   fetches the web loses the same 23 tools (§36.5.2). Events (#3142) inherit the rule the
   day they dispatch through the same loop; they need their own line only if they ever
   dispatch some other way, which §36.11 forbids.

**Red-first tests (named for Reed's scope).**

- `test_mutate_existing_tools_and_project_scoped_tools_differ_exactly_by_three`
  (`ai/chat.rs`, existing) — today asserts the five are NOT in `MUTATE_EXISTING_TOOLS` and
  that the symmetric difference is 8. **This is the red run**: the constant change flips it
  without touching the test; then its assertions are inverted and the difference becomes
  3 (`create_project` ∪ `create_note`, `create_sheet`) — the number its name has claimed
  all along. Red output pasted in the PR body.
- `test_check_external_content_taint_refuses_schedule_job_when_tainted` (`ai/chat.rs`, new)
  — `check_external_content_taint(true, "schedule_job")` returns the refusal string; same
  for the other four; `check_external_content_taint(false, "schedule_job")` is `Ok`.
- `test_tainted_dispatch_refuses_schedule_job_and_inserts_no_row` (`ai/chat.rs`, new, same
  shape as the existing tainted-dispatch loop test ~L5040) — after a successful `web_fetch`
  outcome, a `schedule_job` call in the next iteration is refused with the taint message
  and `SELECT COUNT(*) FROM agent_jobs` is unchanged.
- No fourth test for an offer-time filter: there is no offer-time filter to test (see
  decision point 1 above). Recorded as a post-launch hardening item, not a test.
- No scheduler test: under this decision no tainted job can exist, so there is nothing
  for `scheduler_tick` to inherit.

**Consequences.** Positive: the exploit chain has no first link; the residual paragraph
is true again; the fix is five string literals and one inverted test. Negative: "read this
page and schedule a follow-up" becomes two turns, like "read this and edit that note"
already is. Risk: a future `ToolAccess::Write` tool that creates an executable thing could
repeat the omission — Decision 3 is the rule, and the pinned test's new "exactly 3"
assertion is the tripwire (a sixth Write tool outside the set changes the number).

**Spec and invariant text amended in the same PR:** `docs/spec/ai-integration.md` §4.6.3
(member list, count, flag name, refusal string, residual), `docs/spec/runtime.md` §36.5.2
(one sentence), `docs/INVARIANTS.md` "Web Browsing Tools (§4.6)" third bullet (member
list and cross-turn claim), `docs/spec/README.md` decision log.

**Code the build scope touches (not this PR):** `ai/tools.rs::MUTATE_EXISTING_TOOLS`
(~L9868, five entries and the comment above it); `ai/chat.rs` test
`test_mutate_existing_tools_and_project_scoped_tools_differ_exactly_by_three` (~L4657,
assertions inverted, "8" → "3"); the `#3050 PR 4` / `#3050 PR 5` comments inside that
test; `ai/chat.rs` L1880 ABSOLUTE RULE 30 text — its first clause ("only create_note and
create_sheet are allowed as follow-up writes") is already categorical and covers this; its
enumerating clause could gain "schedules, spawns, or alters a job". Cosmetic — the rule is
discipline, the gate is the mechanism — so Reed's scope decides whether to touch it.
