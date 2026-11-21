# Security

Pane is a local-first app. Your notes, your agents' memory, and your keys live on your machine, and the guards that keep them there are listed in [docs/security/threat-model.md](docs/security/threat-model.md). If you find a way past one of them, or a gap that document does not mention, we want to hear about it.

## Reporting a vulnerability

Use one of these, not a public issue:

- Email [admin@paneapp.ai](mailto:admin@paneapp.ai).
- Open a private report on GitHub, under the Security tab of this repo.

Tell us what you found, how to reproduce it, and which boundary it crosses (your machine, the model provider, web content, the OpenClaw gateway, or Pane's own infrastructure). A minimal set of steps or a short script is worth more than a long write-up. Say which version you tested; the version is in Settings and in the Releases page.

## What happens next

A person reads it and replies. We do not run a bug bounty and we do not promise a response time, but every report gets acknowledged, confirmed or ruled out, and, if confirmed, tracked to a fix.

Once it is fixed, the fix ships in a public release and the changelog credits you by the name you give us, or not at all if you prefer. If the report was about something we deliberately chose not to guard (see below), we say so and explain the ruling. Please keep the details private until the fix is out; we will tell you when it is.

## What counts

Anything that moves data across a boundary without the user's say-so, or lets an agent act outside the limits the app sets. For example:

- A fetched web page getting an agent to edit, delete, or schedule something.
- An agent reading or writing outside `~/Pane`.
- A provider key or pairing token showing up in a log, a crash report, or a build output.
- Reaching `pane-gateway` from a machine that is not on the allowlist, or past the pairing code without it.
- A path to a user's data through the update check or any other call to Pane's infrastructure.
- A supply-chain problem: a dependency, a build script, or a workflow that could run untrusted code in a release.

## What does not count

The threat model lists decisions we made on purpose under "Things we chose not to guard." Reports about those are welcome as discussion, but they are not vulnerabilities: a user with root on their own machine reading `~/Pane`, the gateway forwarding to any agent a paired client names, the Supabase anon key being visible in the client, and what a model provider does with a prompt once it arrives.

If the threat model is wrong about what the code does, that is a vulnerability report too, and the same channels apply.

## Supported versions

The latest public release, on macOS and Linux. Public releases are cut from the `prod` branch and listed on the Releases page; the in-app update check points at the newest one. Fixes land in the next release rather than as patches to old ones, so if you are on an older build, update first and check whether the issue is still there.

## Read more

- [docs/security/threat-model.md](docs/security/threat-model.md): what Pane trusts, what it does not, which code enforces each line, and the known gaps with their issue numbers.
- [docs/security/dependency-audit-exceptions.yml](docs/security/dependency-audit-exceptions.yml): the accepted dependency findings, each with an expiry date. `cargo audit` and `pnpm audit` run on every pull request and weekly; a new finding blocks the build.
- [CONTRIBUTING.md](CONTRIBUTING.md): how changes get in, including the rule that a pull request adding or removing a guard needs a ruling first.
