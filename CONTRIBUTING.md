# Contributing to Pane

Thanks for being here. Pane is built by one person and a crew of agents, so the process below is tuned for small, verifiable changes that one reviewer can hold in their head. It is not bureaucracy for its own sake; every gate exists because something slipped through once without it.

The short version: find or open an issue, build only what the issue says, update the spec and the changelog with the code, and open a pull request that references the issue. CI checks the rest.

## How the project works

GitHub is the source of truth. Decisions live on issues, not in chat. If a change is not tied to an issue, it does not exist yet.

We build one thing at a time. A feature goes through scope, build, audit, and merge in that order, and a new build does not start until the previous one lands. This keeps the main branch always releasable and keeps review honest. If your pull request sits for a bit while something else finishes, that is the queue, not a verdict.

## Before you start

Look for an existing issue first. If there is one, comment that you are taking it so nobody else does. If there is not, open one before writing code, and keep it to one thing: what is wrong or missing, how to reproduce it or where it would live, and a `Spec Section:` line. Bugs with reproduction steps and a version number get fixed fastest.

**Every issue names its spec section.** Pane's behavior is defined in `docs/spec/`, and an issue has to say which part it touches: `Spec Section: §X.Y` for existing behavior, or `Spec Section: NEW` for a feature the spec does not cover yet. A check runs on every new issue and labels it `needs-spec-section` until that line is there; the pull request gate downstream reads the same spec reference, so an issue without one cannot clear CI.

**New features get specified before they get built.** If your issue says `NEW`, the first deliverable is the spec section, not the code: a short addition to the right file in `docs/spec/` describing what the feature does, what it stores, and how it fails, agreed on the issue with a maintainer. Once that section exists, the issue updates to point at it and the build can start. This is how Pane stays one coherent app rather than a pile of features; the spec is the contract the tests and the agents work from.

For anything bigger than a bug fix, wait for a maintainer to agree on the scope before building. Pane has a direction (see [ROADMAP.md](ROADMAP.md)) and a spec, and a change that cuts against either is hard to merge no matter how good the code is. A quick comment up front saves a long pull request later.

## Setting up

Pane is a Tauri app: Rust in `src-tauri/`, Svelte in `src/`, and the OpenClaw bridge in `src-gateway/`. The README's [Build from source](README.md#build-from-source) section has the full prerequisites (Rust 1.77.2 or newer, Node, pnpm 10 or newer, and the platform packages for Tauri). Once those are in place:

```
pnpm install
cargo tauri dev
```

The repo is pnpm-only. Do not add a `package-lock.json` or a `yarn.lock`, and do not add a top-level `overrides` block to `package.json`; both have caused drift before and the first pull request to reintroduce one will be asked to take it out.

Before you push, run what CI will run:

```
pnpm check                                       # svelte-check and TypeScript
pnpm test                                        # frontend unit tests (vitest)
cargo test --manifest-path src-tauri/Cargo.toml  # Rust tests
cargo test --manifest-path src-gateway/Cargo.toml  # gateway tests, if you touched it
```

End-to-end tests (`pnpm test:e2e`) need a built app and are run by a maintainer, not on every pull request.

## Making a change

Branch from `main`. Keep the change to what the issue asked for; if you find something else worth fixing on the way, open a second issue rather than widening the first. A pull request that does one thing gets reviewed in one sitting.

**Spec first.** `docs/spec/` describes how Pane behaves, and `docs/adr/` records why. If your change alters behavior, update the matching spec section in the same pull request, or point to the merged pull request that already did. Core files (the AI pipeline, sync, migrations, agents, notes, projects, the gateway) cannot skip this. For a change that only touches tests or tooling, write `spec-skip:` in the pull request body with a reason.

**Changelog with the code.** Any change under `src/` or `src-tauri/src/` needs a line in `CHANGELOG.md` under the unreleased heading, written for the person using the app, not for the person who wrote the patch. Test-only or refactor-only changes can carry `changelog-skip:` in the pull request body instead. A change to the gateway's version gets its own line too; the gateway ships as its own install.

**No hardcoded constants in behavior.** Timeouts, limits, model names, and paths belong in the config and constants modules, not inline. A CI check searches for the common offenders and fails on them.

**Tests for what you touched.** Rust changes come with Rust tests, frontend changes with vitest. If you are fixing a bug, the first commit should be a test that fails on `main` and passes on your branch; reviewers look for that.

## Pull requests

Open the pull request against `main`. The body must reference the issue it closes or relates to (`Closes #123`, `Relates to #123`); a check fails without one. Say what changed, how you verified it, and anything the reviewer should look at first.

CI runs on every pull request. The checks that matter most:

- **Frontend** (`pnpm check`, `pnpm test`) and **Rust tests** on both trees.
- **Spec-first** and the **changelog** rule described above.
- **Hardcoded constants** search.
- **Dependency audit** (`cargo audit`, `pnpm audit`): a new advisory blocks until it is fixed or added to `docs/security/dependency-audit-exceptions.yml` with an expiry date and a reason.
- **Gateway version gate**: any change under `src-gateway/` must raise the gateway's version in `Cargo.toml` and `Cargo.lock`.
- **Audit gate**, which applies when the change touches core paths: `src-tauri/src/ai/`, `src-tauri/src/sync/`, migrations, agents, notes, projects, system files, import, backup, the watcher, `.github/workflows/`, or `src-gateway/`. Those changes get an independent audit of the diff against the issue before merge. The check shows up in CI as Reed Gate, and the `reed-reviewed` label that satisfies it is applied by the maintainer once the audit passes; applying it yourself removes it and leaves a comment.

A red check is yours to fix before review starts. If a check is red for a reason unrelated to your change (a runner outage, a flaky install), say so in a comment rather than re-running it quietly; we track those.

Merges are done by the maintainer, squash by default, once CI is green and the audit (where required) has passed. Pull requests from forks never run the self-hosted jobs, so build and release checks will show as skipped for you; that is expected.

## Guards and security

[docs/security/threat-model.md](docs/security/threat-model.md) lists the guards that keep data on the user's machine and the code that enforces each one. Two rules follow from it:

- A pull request that adds, removes, or weakens a guard needs a ruling on its issue first. Open the issue, make the case, and wait for a maintainer to agree before writing code. This includes the items under "Things we chose not to guard"; those are decisions, and reversing one is a decision too.
- If your change makes the threat model wrong about what the code does, update the threat model in the same pull request. The guards table is checked against the source on every security review.

If you find a vulnerability, do not open a public issue. [SECURITY.md](SECURITY.md) has the private channels.

## Style

Match the code around you. Rust is formatted with `cargo fmt` and linted with `cargo clippy`; the frontend goes through `svelte-check`. Commit messages follow the pattern already in the log: a type and the issue number, then a short summary (`fix(#123): refuse symlinks that escape ~/Pane`).

For documentation and user-facing text: Oxford commas, no em dashes, and say the plain thing. If a sentence would need a footnote to be true, rewrite it until it does not.

## License

Pane is licensed under Apache 2.0. By contributing you agree that your contribution is licensed under the same terms; see [LICENSE](LICENSE). There is no contributor agreement to sign.

## Questions

Questions and ideas go to the [Pane Discord](https://discord.com/invite/PhRd2yGwR8). Bugs and requests go on issues. Vulnerabilities go to neither; see [SECURITY.md](SECURITY.md).
