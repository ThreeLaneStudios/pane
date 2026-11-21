# Pane threat model

## What this is

What Pane trusts, what it does not, and which code enforces each line. For someone deciding whether to put their notes in Pane, a contributor working near a boundary, and the reviewer checking that the guards still hold.

It describes the app as shipped. Where a control is a policy rather than code, it says so. Known gaps are listed with their issue numbers.

## What Pane protects

Everything of yours lives in `~/Pane`. In order of how much it would hurt to lose or leak:

- **Your notes, sheets, tasks, and chat history.** A SQLite database plus markdown files in `~/Pane`. This is the product.
- **Your agents' memory.** `MEMORY.md`, per-project `Memory.md`, and daily logs. What the agents know about you and your work, in plain text.
- **Your provider keys.** Anthropic, OpenRouter, Venice, and OpenClaw credentials. Stored in the system keychain, never in the database or a config file.
- **The OpenClaw pairing token.** Issued by the gateway at pairing, kept in Pane's keychain, never logged. Holding it means being able to talk to your agent.
- **Your prompts in flight.** The prompt and the context sent with it are the one thing that must leave the machine. Which provider sees them, and on what terms, is your choice.

Pane has no server and no account by default. Nothing of yours is on a machine we run.

## Who Pane trusts

```
                        ┌──────────────────────────────┐
  Model provider        │         Your machine         │        OpenClaw machine
  Sees the conversation │  ┌────────────────────────┐  │        pane-gateway, their side
  Your key, your vendor │  │ Pane                   │  │        Tailnet addresses only
  Ollama: never leaves  │  │ Agents, chat, jobs,    │  │        8-digit code, pinned TLS
            ◄── prompts │  │ memory pipeline        │  │ paired ◄──►
                        │  └────────────────────────┘  │
  The web               │  ┌────────────────────────┐  │        Pane's servers
  Pages the agent       │  │ ~/Pane                 │  │        None hold your data
  fetches               │  │ SQLite plus markdown   │  │        updates.paneapp.ai
  Untrusted for the turn│  │ files, all yours       │  │        download only
  Can read, cannot edit │  └────────────────────────┘  │ version ──►
            content ──► │  ┌────────────────────────┐  │
                        │  │ System keychain        │  │
                        │  │ Provider keys,         │  │
                        │  │ gateway token          │  │
                        │  └────────────────────────┘  │
                        └──────────────────────────────┘

  Four things cross the line, all on your terms. Everything inside the box is yours
  and stays there. Each arrow is a choice you made. Pane's servers never hold your data.
```

Four parties sit outside your machine. Each crossing is one you chose; each has a guard.

- **You, and your machine.** Fully trusted. Pane does not defend your data from you, from root, or from software you installed. If the machine is compromised, so is `~/Pane`. The reverse holds too: the agent is walled into `~/Pane`. Every path it reads or writes is checked against a fixed set of roots in that folder and refused if it escapes.
- **The model provider you picked.** Trusted with everything that enters the conversation, and nothing else. That is more than your typed message: the notes and sheets the agent reads during a turn, its memory files, project memory, and every tool result go to the model as context. What never goes: your keys, the whole database, and anything the agent did not read that turn. Some of this runs in the background: memory extraction after a chat, the end-of-day and consolidation passes, search reranking, and semantic-search embeddings each send the text they work on to the provider you set for Smart Features or embeddings. Point those at Ollama and that traffic stays local. Or use a private model instead.
- **Web content the agent fetches.** Not trusted. A page is data, never instructions. Fetching one taints the turn: the agent can still read and summarize, but tools that change existing notes, sheets, tasks, or jobs are refused until the next turn.
- **Your OpenClaw machine, through `pane-gateway`.** Trusted after pairing, and only from your tailnet. Pane assumes you control that machine too; it cannot check. It is a second computer either way, so the link is a boundary: pairing code, pinned certificate, bearer token.
- **Pane's own infrastructure.** Almost none. `updates.paneapp.ai` serves a version manifest and a download; it learns nothing about you. The account and sync engine in the repo is off by default and unreachable in a normal build (ADR-0070).

## Threats and the guards that hold them

Each row: a way a boundary could be crossed without your say-so, the control that stops it, where it lives, and how we know it works.

| Threat | Control | Where | How it is tested |
| --- | --- | --- | --- |
| A fetched web page tells the agent to edit your notes, delete tasks, or schedule a job (prompt injection) | Content taint. Once a turn has fetched external content, the 23 tools that mutate existing data are refused for the rest of that turn. `create_note` is deliberately allowed so the agent can still write a summary. | `src-tauri/src/ai/chat.rs`, `check_external_content_taint`, `MUTATE_EXISTING_TOOLS` (ADR-0064 D4) | 21 unit tests on the gate across chat.rs and tools.rs; the five job tools added in the #3106 sprint were written red-first |
| The agent is talked into fetching an internal address: your router, a cloud metadata endpoint, a tailnet node | Network guard. Loopback, RFC 1918, link-local, CGNAT/Tailscale (`100.64.0.0/10`), and IPv6 unique-local and link-local ranges are refused before any request; redirects are re-checked on every hop. | `src-tauri/src/ai/net_guard.rs`, `is_blocked_ip` (ADR-0064 D7) | Unit tests per range |
| A provider key or gateway token ends up in a log file or a crash report | Keys live only in the system keychain and are read at call time. In the gateway, every log site that handles a pairing code, bearer token, or nonce prints its shape, never its value. The test that enforces this derives its list of sites from the source, so a new log site that prints a value fails it rather than slipping past a hand-kept list. | `src-tauri/src/keychain.rs`; `src-gateway/tests/no_credential_logging.rs` | Nine sites fixed in #3200. A tenth, the connect handshake in oc_client.rs logging the full bearer token, was found while making the list source-derived and fixed in #3219 (gateway 1.0.4), with the failing test captured before the fix |
| Someone on your network reaches the gateway | TCP accept allowlist: loopback and the Tailscale ranges by default, operator-editable in `allowlist.json`. Rejected connections log only the remote address. | `src-gateway/src/allowlist.rs` (spec `sync.md` §22C) | Unit tests; red-first in #3200 |
| Someone on the tailnet guesses the pairing code | 8-digit code, five-minute life, locked after ten wrong attempts. A fresh code is minted only on the gateway machine: in-process at first boot, and on request over a local Unix socket after that. | `src-gateway/src/auth.rs`, `MAX_PAIRING_ATTEMPTS` | A test submits ten wrong codes and asserts the eleventh is refused; a literal-count test guards the constant |
| A machine in the middle impersonates the gateway | Pane pins the gateway's certificate on first pairing (SHA-256 of the SPKI) and refuses any other. | `src-tauri/src/gateway_client.rs`, `PinnedCertVerifier` | Wrong-cert tests in `src-tauri/src/gateway_client.rs` |
| A gateway client asks for an agent it was not paired for | The gateway forwards to whichever OpenClaw agent the authenticated client names. Session-to-agent binding is enforced in Pane, not the gateway; the pairing token and the allowlist are the controls. See the next section. | ADR-0067; ruling on #3106 | Documented decision, not a code gate |
| The agent reads or writes outside `~/Pane` | Every path the agent touches is resolved against a fixed set of roots under `~/Pane` and refused if it escapes them; traversal, absolute paths, and symlinks out are rejected at one choke point. The renderer's own file writes go through the same check, and renderer HTML and network egress were locked down in the same series. | `src-tauri/src/filesystem_policy.rs`, `PaneRoot` and `PaneFs` | Unit tests; #2184, #2185, #2186 (July 2026) confined renderer paths, sanitized privileged HTML, and restricted egress |
| The dormant sync engine makes a network call on its own. Multi-device sync and team sharing do not exist as features yet; the code for them is in the repo, switched off | Every sync command checks `account_enabled` first and refuses when it is off, which it always is today; there is no way to turn it on in the app. The notification poller and the Settings tab are gated the same way. When sync ships, this flag is what turns it on, and this row gets rewritten. | `src-tauri/src/sync/commands.rs`, `require_account_enabled` (ADR-0070) | Red-first tests in #3201, #3202, #3203, #3206 |
| A dependency with a known vulnerability ships | `cargo audit` on both Rust trees and `pnpm audit` on the frontend run on every PR and weekly. Any new finding blocks. Accepted findings live in a dated exception list that fails the check when an entry expires. | `.github/workflows/dependency-audit.yml`; `docs/security/dependency-audit-exceptions.yml` | Proven live on an empty scope in #3212. The gate floors at high severity and subtracts a dated exception list, so it can pass while Dependabot reports open alerts; triage of those is #3217 |
| A pull request from a fork runs code on a self-hosted build machine | Self-hosted jobs never run on `pull_request` from outside the repo; the DMG test checks `head.repo == github.repository`, e2e is manual-only. | `.github/workflows/e2e.yml`, `dmg-swap-test.yml` (#3199) | Workflow review in #3106 C1 |
| A build or signing secret prints in a public CI log | Signing and notarization read from environment secrets; build scripts no longer echo them. | `scripts/pre-build.sh`, `scripts/build.sh` | Four echo sites across two scripts removed in #3219. No automated check covers shell scripts yet; the source-derived test covers Rust only |
| A gateway code change ships under the old version number, so nobody can tell which gateway build they are running | Any pull request that touches `src-gateway/` must raise the gateway's version in Cargo.toml and Cargo.lock, or CI fails. | `.github/workflows/gateway-version-gate.yml`, `scripts/check-gateway-version-bump.sh` | Blocked #3219 until the bump to 1.0.4 landed |

## Things we chose not to guard

Decisions, not oversights. A pull request that adds a guard here needs a new ruling first.

- **The gateway does not check which agent a paired client asks for.** A client holding the pairing token can address any OpenClaw agent on that machine. Session-to-agent binding lives in Pane (ADR-0067); the token and the allowlist are the controls. A second check in the gateway would duplicate Pane's logic and drift. Ruled 2026-11-11 on #3106.
- **Pane and the gateway on the same machine is not a supported layout.** The split exists so the agent's machine and your data's machine are different boxes. Pane does not support running both on one machine, and nothing is promised for it.
- **The Supabase anon key is in the client.** It is public by design. Row-level security is on for every table, every policy gates on the signed-in user, and public sign-ups are off, so the key reaches no rows. Verified in the dashboard 2026-11-11.
- **Provider privacy is the provider's promise.** Pane trusts the terms of the vendor you chose. Ollama removes the question. Venice's TEE tier adds a hardware guarantee once the prompt arrives; the hop there is still Venice's no-logging policy. Pane adds no encryption of its own on top of TLS.
- **Your own machine is trusted.** Pane does not encrypt `~/Pane` at rest beyond what your disk does. Root, a malicious app, or a stolen unlocked laptop can read it. Full-disk encryption is the control for that, not Pane.

## Known gaps

Open as of 2026-11-12. None blocks the public release; each is tracked.

- **The gateway's TLS handshake has no test.** The TLS tests in `src-gateway` are stubs and the WebSocket tests use plain `ws://`. The pinned-certificate check is tested from the Pane side only. [#3213](https://github.com/ThreeLaneStudios/pane/issues/3213)
- **Nothing stops a second lockfile or a top-level `overrides` from returning.** The repo is pnpm-only after #3211; the rule is convention, not a gate. [#3214](https://github.com/ThreeLaneStudios/pane/issues/3214)
- **One CI guard installs its tool at run time.** The hardcoded-constants gate installs `ripgrep` with `apt-get` and fails on runner DNS flakes; a gate people re-run is a gate that eventually hides a real failure. [#3215](https://github.com/ThreeLaneStudios/pane/issues/3215)
- **Gate 4d (visual smoke) is advisory.** Skips are marked and reviewed rather than blocked. Whether blocking changes any outcome is an open question. [#3216](https://github.com/ThreeLaneStudios/pane/issues/3216)
- **The spec-first gate does not see `src-gateway/`.** A gateway change can merge without a spec update because `check-spec-first.sh` only watches the app's core paths. [#3198](https://github.com/ThreeLaneStudios/pane/issues/3198)
- **The signing identity is a literal in three build files.** It is public (every notarized binary carries it), so this is hygiene, not exposure; they should read it from the environment like the rest of the pipeline does.
- **Linux gets less use than macOS.** Both shipped together; keys go to the Secret Service keyring, but the Linux packages are not signed and far fewer hours have been spent in Pane on Linux. [#2058](https://github.com/ThreeLaneStudios/pane/issues/2058)

## Reporting a vulnerability

Email [admin@paneapp.ai](mailto:admin@paneapp.ai) or use GitHub's private vulnerability reporting. Say what you found, how to reproduce it, and which boundary it crosses. You get a reply from a person, a tracked fix, and credit in the changelog if you want it. Details in `SECURITY.md`.

If this document is wrong about what the code does, that is also a report. The guards table is checked against the source on every security review; a row that no longer matches is a bug in one or the other.
