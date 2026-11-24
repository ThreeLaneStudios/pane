# Release Runbook

**Relates to:** #2340 (Production branch + protected release pipeline)
**Status:** `prod` prepared for v1.0.1

This is the operational runbook for cutting Pane builds — test builds for
testers, and public GitHub Releases. It exists because we now have a
protected `prod` branch that only ever receives deliberate promotions
from `main` by the Release Owner, and a CI guard (`.github/workflows/release-publish.yml`) that
mechanically enforces which branch each kind of tag may be built from.

## The two tag shapes

| Tag shape | Example | Built from | GitHub Release type | Delivery |
|---|---|---|---|---|
| `vX.Y.Z` | `v0.69.1` | `prod` **only** | Public Release, created as a draft (Matt publishes manually) | GitHub Releases (public consumers, once auto-update ships) |
| `vX.Y.Z-dev` | `v1.0.86-dev` | `main` **only** | Prerelease (`--prerelease`) | Discord (testers channel) |

The build-source guard job (`guard-tag-branch` in `release-publish.yml`)
checks tag shape against commit ancestry **before any build step runs**:

- A bare `vX.Y.Z` tag on a commit not yet reachable from `prod` → **fails**.
- A `vX.Y.Z-dev` tag on a commit not reachable from `main` → **fails**.

There is no skip mechanism. This is deliberate — it's the enforcement that
makes `prod`'s branch protection matter instead of being governance
theater.

## Version bump (before any tag)

Whenever a release raises the app's version, the bump is its own PR and
updates all four files together — never the three manifests alone:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.lock` (the `[[package]] name = "pane"` version line —
  `cargo check --manifest-path src-tauri/Cargo.toml` regenerates it)

`scripts/check-version-parity.sh` (wired into CI as the `version-parity`
job, every PR to `main`, no path filter) fails the build if any of the four
disagree. Gateway-only bumps are the same pattern one level down:
`src-gateway/Cargo.toml` + `src-gateway/Cargo.lock`, checked by
`scripts/check-gateway-version-bump.sh`.

The standing no-lock-changes-in-PRs rule (`docs/REVIEW-CHECKLIST.md` §8)
does not apply to this one line in a bump PR — see §8a's narrow, documented
carve-out. Added by #3056 after #3048's bump PR had its correct
`Cargo.lock` line reverted under §8 with no exception on record, leaving
`main`'s lock a version behind until this check existed.

## Test builds (tester DMGs)

1. Land your fix/feature on `main` through the normal PR gates (spec-first,
   Reed Gate where applicable, CHANGELOG, tests).
2. Before tagging, comment build/test status on every issue referenced
   (`#NNN`) in the commits since the last tag.
3. Tag the `main` commit you want testers running:
   ```
   git tag vX.Y.Z-dev
   git push origin vX.Y.Z-dev
   ```
   There is exactly one `-dev` shape per base version; it does **not** need
   to match the app's internal `package.json`/`tauri.conf.json` version
   exactly, since dev builds can iterate faster than version bumps land.
4. `release-publish.yml` builds and publishes a GitHub **prerelease**
   (`isPrerelease: true` — renders as "Pre-release", never "Latest").
5. **Delivery is still Discord, not the GitHub Release link.** The repo is
   private — external testers can't open GitHub Release URLs. Post the DMG
   directly in the testers channel with the provenance line: version,
   commit SHA, one-line what-to-test. The GitHub dev prerelease is the
   canonical archive record (provenance, checksums, build history), not the
   distribution channel.

## Public releases

1. Public releases are cut **only** from `prod`, tagged with a bare
   `vX.Y.Z` — no `-dev` suffix.
2. Existing releases through `v1.0.0` are left as-is. The next public
   release is `v1.0.1`.
3. To cut a public release, the Release Owner (currently Xardas) promotes
   the tested `main` commit to `prod` through a PR, merges it, and then tags
   the resulting `prod` commit:
   ```
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
4. `release-publish.yml`'s guard verifies the tagged commit is reachable
   from `prod` before building. It builds signed and notarized Pane and Pane
   Gateway DMGs on the self-hosted Mac. Ubuntu builds Pane as DEB, RPM, and
   AppImage, and Pane Gateway as DEB, RPM, and a portable tarball. The workflow
   creates a draft GitHub Release, uploads and verifies all nine assets, and
   leaves it a draft. Matt publishes the release manually (GitHub UI, Publish
   release, which sets `--latest`). A failed upload or a wrong asset count
   also leaves a draft; the two states are not distinguishable from the
   release alone, check the run's log.
   Agents never click Publish on a draft release — that action belongs to Matt alone.
   `SHA256SUMS.txt` covers every Linux asset. Like Zed releases, filenames omit
   the version because the release tag is authoritative: `Pane-universal.dmg` (Intel + Apple Silicon),
   `pane-linux-x86_64.{deb,rpm,AppImage}`, `PaneGateway-aarch64.dmg`, and
   `pane-gateway-linux-x86_64.{deb,rpm,tar.gz}`. Package metadata still uses
   each ecosystem's native architecture label (`amd64` for DEB, `x86_64` for
   RPM).

## Hotfix path

Fixes always land on `main` first — there is no way to commit directly to
`prod`:

1. Fix lands on `main` through the normal PR gates (same as any other
   change — no shortcut for hotfixes).
2. The Release Owner opens a PR from `main` (or a specific commit range)
   into `prod`, approves the candidate after testing, and merges it.
3. Once the fix is on `prod`, cut a new public Release from that `prod`
   commit per the "Public releases" steps above.

## Required GitHub secrets

The release build reads updater signing credentials from repository or
environment secrets. Configure both values without adding them to the
repository or the self-hosted runner filesystem:

- `TAURI_SIGNING_PRIVATE_KEY` — the full updater private key generated by Tauri.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — the password for that private key.
- `APPLE_ID` — Apple Developer account used for notarization.
- `APPLE_PASSWORD` — app-specific password for `APPLE_ID`.
- `APPLE_TEAM_ID` — Apple Developer team identifier.

These secrets live in GitHub only and are never read from local disk.
Signed, notarized builds come exclusively from `release-publish.yml`.
`scripts/build.sh` is a local-dev-only script: it produces an unsigned,
non-notarized artifact and is not a path to a distributable build (#3239).

Gateway macOS releases fail closed if notarization credentials are missing or
Apple rejects notarization. The Developer ID signing certificate must also be
installed in the self-hosted macOS runner's keychain.

The updater public key remains in `src-tauri/tauri.conf.json`. It is embedded
in Pane so installed applications can verify update signatures; it is not a
secret.

## Auto-updater

Public releases carry the updater source-of-truth assets alongside installers:

- `pane-updater-macos-aarch64.tar.gz` and its Tauri `.sig`.
- Tauri `.sig` files for the exact published Linux DEB, RPM, and AppImage.
- `latest.json`, uploaded last after every payload/signature digest is verified.

The manifest uses package-aware Tauri targets (`linux-x86_64-deb`,
`linux-x86_64-rpm`, and AppImage fallback `linux-x86_64`) and points to
versioned URLs under `https://updates.paneapp.ai/<tag>/...`.

GitHub Release attachment does not itself activate updates because the repo is
private. The backend must serve the selected release's `latest.json` at the
configured root endpoint and proxy immutable `/<tag>/<asset>` bytes from the
private GitHub release. Until those routes are deployed and byte-verified,
`https://updates.paneapp.ai/latest.json` remains the legacy endpoint and must
not be overwritten by the release workflow.
