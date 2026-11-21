#!/bin/bash
set -e
cd "$(dirname "$0")/.."

# Shared hdiutil attach/detach helpers (#3049) — same functions used by the
# swap-path regression test (scripts/tests/dmg-swap/test-swap-path.sh), so
# a mount-safety fix only has to be made once. See scripts/lib/dmg.sh.
# shellcheck source=lib/dmg.sh
source "$(dirname "$0")/lib/dmg.sh"

# Parse flags
ARM64_ONLY=false
ALLOW_BRANCH=false
for arg in "$@"; do
  if [ "$arg" = "--arm64-only" ]; then
    ARM64_ONLY=true
  fi
  if [ "$arg" = "--allow-branch" ]; then
    ALLOW_BRANCH=true
  fi
done

# =============================================================================
# BUILD-SOURCE GUARD
# =============================================================================
# A DMG was once accidentally built from an unmerged branch and shipped as
# v1.0.46-dev. This guard refuses to produce an artifact unless HEAD is on
# origin/main, so that structurally can't happen again.
#
# --allow-branch overrides the guard for legitimate local dev builds; those
# artifacts get an -UNOFFICIAL stamp in the DMG filename so they can never be
# mistaken for an official build.
UNOFFICIAL_SUFFIX=""
git fetch origin main --quiet || true
HEAD_SHA=$(git rev-parse HEAD)
HEAD_BRANCH=$(git rev-parse --abbrev-ref HEAD)
if git merge-base --is-ancestor HEAD origin/main; then
  echo "✅ BUILD-SOURCE GUARD: HEAD ($HEAD_SHA) is on origin/main"
else
  if [ "$ALLOW_BRANCH" = true ]; then
    echo "⚠️ Building from non-main branch — artifact will be stamped UNOFFICIAL"
    UNOFFICIAL_SUFFIX="-UNOFFICIAL"
  else
    echo "❌ BUILD-SOURCE GUARD: HEAD is not on origin/main. Builds must come from merged main."
    echo "   HEAD SHA:    $HEAD_SHA"
    echo "   HEAD branch: $HEAD_BRANCH"
    exit 1
  fi
fi

# Verify universal crsqlite.dylib (unless --arm64-only)
if [ "$ARM64_ONLY" = false ]; then
  if [ ! -f "src-tauri/resources/crsqlite.dylib" ]; then
    echo "❌ ERROR: crsqlite.dylib not found. Run scripts/build-crsqlite-universal.sh first."
    exit 1
  fi
  
  LIPO_INFO=$(lipo -info src-tauri/resources/crsqlite.dylib 2>&1)
  if ! echo "$LIPO_INFO" | grep -q "arm64"; then
    echo "❌ ERROR: crsqlite.dylib is missing arm64 architecture."
    echo "   Run scripts/build-crsqlite-universal.sh first."
    exit 1
  fi
  if ! echo "$LIPO_INFO" | grep -q "x86_64"; then
    echo "❌ ERROR: crsqlite.dylib is arm64-only. Run scripts/build-crsqlite-universal.sh first."
    exit 1
  fi
  echo "✅ crsqlite.dylib is universal (arm64 + x86_64)"
fi

# Signed builds come only from .github/workflows/release-publish.yml (#3239).
# This script no longer reads a local private key or .env file at all; it
# always produces an unsigned, non-notarized dev build. The
# createUpdaterArtifacts override below makes that explicit rather than
# relying on Tauri's version-dependent behavior when no signing key is set.

# Build (Tauri's built-in signing may fail on bundled dylibs — we sign manually after)
if [ "$ARM64_ONLY" = true ]; then
  echo "🔨 Building arm64-only (dev mode)"
  SKIP_SPEC_CHECK=${SKIP_SPEC_CHECK:-0} npx tauri build --no-sign --config '{"bundle":{"createUpdaterArtifacts":false}}' "$@" || true
  APP_PATH="src-tauri/target/release/bundle/macos/Pane.app"
else
  echo "🔨 Building universal (arm64 + x86_64)"
  SKIP_SPEC_CHECK=${SKIP_SPEC_CHECK:-0} npx tauri build --no-sign --target universal-apple-darwin --config '{"bundle":{"createUpdaterArtifacts":false}}' "$@" || true
  APP_PATH="src-tauri/target/universal-apple-darwin/release/bundle/macos/Pane.app"
fi

VERSION=$(node -p "require('./package.json').version")
VERSION="${VERSION}${UNOFFICIAL_SUFFIX}"

if [ ! -d "$APP_PATH" ]; then
  echo "❌ App bundle not found at $APP_PATH"
  exit 1
fi

echo ""
echo "✅ Build complete"

# =============================================================================
# FAIL-CLOSED CLEANUP
# =============================================================================
# Everything from here on (signing, notarizing, stapling, packaging) writes
# only to scratch paths in /tmp and to a hidden staging file inside the
# output directory — never directly to the final DMG name. The final name
# is only ever created by the single atomic `mv` at the very end, after every
# verification check below has passed. That means a mid-pipeline failure
# structurally cannot leave a half-made or unverified DMG at the real path;
# there is nothing partially-written to clean up at the real name. This trap
# still clears the scratch/staging paths on exit so failed runs don't leave
# debris in /tmp or the output directory.
NOTARIZE_ZIP=""
MOUNT_POINT=""
WORK_DMG=""
RW_DMG=""
STAGING_DMG_PATH=""
cleanup_scratch() {
  # #3049 (fast-follow): the exit code this trap's caller is actually
  # exiting with MUST be captured as the very first thing this function
  # does — `$?` only reflects "the thing that ran right before this trap
  # fired" for exactly one statement, and every statement below
  # (hdiutil detach, rm -f, the dmg.sh helpers) overwrites it. The old
  # version of this trap had no such capture and instead let its own LAST
  # statement's exit status leak out as the script's exit status: a
  # trailing `[ -n "$STAGING_DMG_PATH" ] && rm -f ...` line evaluates to 1
  # whenever $STAGING_DMG_PATH is empty — which is exactly the state it's
  # in after a *successful* build (cleared right after the final `mv`
  # below) — so every successful build was reported as a failure. See
  # dmg_rm_scratch in lib/dmg.sh for the full root-cause writeup.
  local exit_code=$?

  # This is a best-effort cleanup running during unwind (possibly after a
  # real failure already happened) so nothing below may itself call
  # `exit` — but "best effort" must not mean "silent". dmg_cleanup_mount
  # surfaces a failed detach as a warning instead of redirecting it to
  # /dev/null, so a failed run's log always explains what state the
  # mountpoint was left in.
  dmg_cleanup_mount "$MOUNT_POINT"
  dmg_rm_scratch "$NOTARIZE_ZIP" "$WORK_DMG" "$RW_DMG" "$STAGING_DMG_PATH"

  # A failed build must still exit non-zero, and a successful build must
  # still exit 0 — re-assert the captured code as this function's actual
  # last statement so nothing above (all of which ran only for its side
  # effects) gets to silently override it.
  exit "$exit_code"
}
trap cleanup_scratch EXIT

# Sign bundled dylibs first (crsqlite.dylib etc.)
if [ -n "$APPLE_SIGNING_IDENTITY" ]; then
  echo "🔏 Signing bundled libraries..."
  # NOTE: `find ... -exec cmd \;` does not propagate cmd's exit status into
  # find's own exit status on macOS/BSD find, so a failing codesign here used
  # to go unnoticed even under `set -e`. Loop explicitly so a failed codesign
  # on any dylib is fatal.
  while IFS= read -r -d '' dylib; do
    codesign --force --timestamp --options runtime \
      -s "$APPLE_SIGNING_IDENTITY" "$dylib"
  done < <(find "$APP_PATH/Contents/Resources" -name "*.dylib" -print0)
  echo "✅ Bundled libraries signed"

  # Sign the main app bundle
  echo "🔏 Signing app bundle..."
  codesign --force --timestamp --deep --options runtime \
    --entitlements src-tauri/Entitlements.plist \
    -s "$APPLE_SIGNING_IDENTITY" "$APP_PATH"
  echo "✅ App signed"

  # Verify. A signature that isn't Developer ID means notarization will
  # fail downstream anyway (Apple only notarizes Developer ID + hardened
  # runtime signatures) — fail now instead of discovering it mid-notarize.
  SIGN_INFO=$(codesign -dvv "$APP_PATH" 2>&1 | grep "Authority" | head -1)
  if echo "$SIGN_INFO" | grep -q "Developer ID"; then
    echo "✅ Signed with Developer ID"
  else
    echo "❌ Signing did not produce a Developer ID signature: $SIGN_INFO"
    exit 1
  fi

  # Notarize + staple if credentials available. If credentials are absent
  # (local dev machine without Apple creds configured), this whole block is
  # skipped — that's not a failure, it's the expected --allow-branch /
  # unofficial dev-build path, and it still produces a DMG below (just an
  # unnotarized one). But if credentials ARE present and notarization or
  # stapling fails, that is fatal: a failed notarization must never
  # masquerade as a good build, official or not.
  if [ -n "$APPLE_ID" ] && [ -n "$APPLE_PASSWORD" ] && [ -n "$APPLE_TEAM_ID" ]; then
    echo "📤 Submitting app for notarization..."
    NOTARIZE_ZIP="/tmp/Pane_${VERSION}_notarize.zip"
    rm -f "$NOTARIZE_ZIP"
    ditto -c -k --keepParent "$APP_PATH" "$NOTARIZE_ZIP"
    if [ ! -s "$NOTARIZE_ZIP" ]; then
      echo "❌ ditto produced no (or empty) notarization zip at $NOTARIZE_ZIP"
      exit 1
    fi
    if ! xcrun notarytool submit "$NOTARIZE_ZIP" \
      --apple-id "$APPLE_ID" \
      --password "$APPLE_PASSWORD" \
      --team-id "$APPLE_TEAM_ID" \
      --wait 2>&1; then
      echo "❌ App notarization failed — aborting build (no DMG will be produced)"
      exit 1
    fi
    rm -f "$NOTARIZE_ZIP"
    NOTARIZE_ZIP=""

    echo "📎 Stapling ticket to app..."
    if ! xcrun stapler staple "$APP_PATH" 2>&1; then
      echo "❌ Stapling the app failed — aborting build (no DMG will be produced)"
      exit 1
    fi
    echo "✅ App stapled"
  else
    echo "⚠️  No Apple notarization credentials — app will not be notarized/stapled (dev build)"
  fi
else
  echo "⚠️  No APPLE_SIGNING_IDENTITY — app is ad-hoc signed"
fi

# =============================================================================
# PACKAGE: Tauri's own DMG layout, built from the (possibly stapled) app
# =============================================================================
# `npx tauri build` above already produced a DMG with the correct installer
# layout (Applications shortcut, background image, icon positions, window
# size/position) — Tauri's macOS bundler assembles that. But it was built
# *before* this script notarized+stapled Pane.app above, so its embedded
# copy of Pane.app is the pre-staple one. Shipping that DMG as-is would be
# exactly the trap this fix exists to close: a DMG that looks right but
# fails Gatekeeper's offline/no-network check.
#
# Tauri v2's DMG bundler is compiled into the @tauri-apps/cli native binary
# (@tauri-apps/cli-darwin-arm64 ships a single .node file) — there is no
# standalone `bundle_dmg.sh` to invoke directly against the stapled app in
# this Tauri version. Re-running `tauri build --bundles dmg` after stapling
# was the other option, but Tauri's macOS bundler can re-copy/re-sign the
# .app bundle as a side effect of producing the dmg bundle, which would
# silently discard the staple we just applied — recreating the exact
# ordering trap this fix exists to close, just one step later.
#
# So instead: mount Tauri's already-correct DMG, swap in the real (signed,
# notarized, stapled) Pane.app in place of the pre-staple copy Tauri wrote,
# and repack. This keeps Tauri's layout byte-for-byte (the swap never
# touches the background image, Applications alias, or window-position
# .DS_Store) while guaranteeing the app inside is the one just verified
# above.
if [ "$ARM64_ONLY" = true ]; then
  TAURI_DMG_DIR="src-tauri/target/release/bundle/dmg"
else
  TAURI_DMG_DIR="src-tauri/target/universal-apple-darwin/release/bundle/dmg"
fi
TAURI_DMG_PATH=$(find "$TAURI_DMG_DIR" -maxdepth 1 -name "*.dmg" 2>/dev/null | head -1)
if [ -z "$TAURI_DMG_PATH" ] || [ ! -f "$TAURI_DMG_PATH" ]; then
  echo "❌ Tauri did not produce a DMG at $TAURI_DMG_DIR — cannot package"
  exit 1
fi
echo "📦 Found Tauri-built DMG layout: $TAURI_DMG_PATH"

OUTPUT_DIR="$HOME/Shared/Pane/builds"
mkdir -p "$OUTPUT_DIR"
FINAL_DMG_PATH="$OUTPUT_DIR/Pane_${VERSION}.dmg"
# #3049 (fast-follow): this name MUST end in .dmg. hdiutil convert -o
# silently appends .dmg to whatever path it's given if that path doesn't
# already end in .dmg — the staging path used to be
# ".Pane_${VERSION}.dmg.building" (no trailing .dmg), so convert actually
# wrote ".Pane_${VERSION}.dmg.building.dmg" and every check below that
# looked for the staging file at the name this variable holds found
# nothing there. dmg_convert_or_die (lib/dmg.sh) now enforces this
# invariant for every caller, so this naming mistake can't come back —
# but the name here still has to actually end in .dmg to pass that check.
STAGING_DMG_PATH="$OUTPUT_DIR/.Pane_${VERSION}.building.dmg"
rm -f "$STAGING_DMG_PATH"

WORK_DMG="/tmp/Pane_${VERSION}_work.dmg"
rm -f "$WORK_DMG"
cp "$TAURI_DMG_PATH" "$WORK_DMG"
if [ ! -s "$WORK_DMG" ]; then
  echo "❌ cp of Tauri's DMG produced no (or empty) copy at $WORK_DMG"
  exit 1
fi

# #3049: Tauri's finished DMG is UDZO (read-only compressed) — that format
# can never mount -readwrite, on any machine. Convert to UDRW (raw,
# writable) *before* the swap mount below; the WORK_DMG variable is
# repointed at the UDRW copy so the EXIT trap still cleans up whichever
# scratch file is current.
echo "🔄 Converting Tauri's read-only DMG to a writable (UDRW) image for the swap..."
RW_DMG="/tmp/Pane_${VERSION}_work_rw.dmg"
rm -f "$RW_DMG"
dmg_convert_or_die "$WORK_DMG" UDRW "$RW_DMG"
rm -f "$WORK_DMG"
WORK_DMG="$RW_DMG"
RW_DMG=""
echo "✅ Converted to UDRW: $WORK_DMG"

echo "📦 Swapping in the stapled app and repacking..."
MOUNT_POINT=$(mktemp -d "/tmp/pane-dmg-mount.XXXXXX")
# #3049: dmg_attach_or_die fails loudly (real hdiutil error surfaced, not
# swallowed by -quiet) on EITHER a non-zero exit OR an exit-0 attach that
# didn't actually produce a live mount — the shape that let a never-mounted
# empty directory sail past the next check undetected.
dmg_attach_or_die "$WORK_DMG" "$MOUNT_POINT" -readwrite

if [ ! -d "$MOUNT_POINT/Pane.app" ]; then
  echo "❌ Pane.app missing inside Tauri's DMG — cannot swap in the stapled copy"
  exit 1
fi

rm -rf "$MOUNT_POINT/Pane.app"
if [ -e "$MOUNT_POINT/Pane.app" ]; then
  echo "❌ rm -rf failed to remove the pre-staple Pane.app from the mount (read-only mount?)"
  exit 1
fi
ditto "$APP_PATH" "$MOUNT_POINT/Pane.app"
if [ ! -x "$MOUNT_POINT/Pane.app/Contents/MacOS/Pane" ]; then
  echo "❌ ditto did not land a usable Pane.app inside the mount — the swap did not take"
  exit 1
fi
echo "✅ Swapped in the signed/notarized/stapled app"

# #3247: apply the prebuilt .DS_Store (window layout, icon positions) onto
# this same already-open, already-readwrite mount. Tauri's own bundle_dmg
# deletes any .DS_Store from its own staging folder before it ever creates
# the DMG (see scripts/lib/dmg.sh's dmg_apply_ds_store), so the only place
# a custom layout survives is here, after Tauri's DMG already exists, in
# the exact mount window the app-swap above already opened. One mount,
# both operations, no second attach/detach cycle.
dmg_apply_ds_store "$MOUNT_POINT" "$(dirname "$0")/assets/dmg-layout/pane-1.0.85.dsstore"
echo "✅ Applied .DS_Store window layout"

dmg_detach_or_die "$MOUNT_POINT"
rmdir "$MOUNT_POINT" 2>/dev/null || true
MOUNT_POINT=""

dmg_convert_or_die "$WORK_DMG" UDZO "$STAGING_DMG_PATH"
rm -f "$WORK_DMG"
WORK_DMG=""
echo "✅ DMG packaged with Tauri's installer layout (staging): $STAGING_DMG_PATH"

# =============================================================================
# NOTARIZE + STAPLE THE DMG ITSELF
# =============================================================================
# Same fail-closed rule as the app above: skipped (not fatal) when creds are
# absent — that's the dev-build path. Fatal when creds are present and the
# call fails — a failed notarization never ships.
if [ -n "$APPLE_ID" ] && [ -n "$APPLE_PASSWORD" ] && [ -n "$APPLE_TEAM_ID" ]; then
  echo "📤 Submitting DMG for notarization..."
  if ! xcrun notarytool submit "$STAGING_DMG_PATH" \
    --apple-id "$APPLE_ID" \
    --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" \
    --wait 2>&1; then
    echo "❌ DMG notarization failed — aborting build (no DMG will be shipped)"
    exit 1
  fi

  echo "📎 Stapling ticket to DMG..."
  if ! xcrun stapler staple "$STAGING_DMG_PATH" 2>&1; then
    echo "❌ Stapling the DMG failed — aborting build (no DMG will be shipped)"
    exit 1
  fi
  echo "✅ DMG stapled"
else
  echo "⚠️  No Apple notarization credentials — DMG will not be notarized/stapled (dev build)"
fi

chmod a+r "$STAGING_DMG_PATH"

# =============================================================================
# SELF-VERIFY THE OUTPUT — a failure here is a build failure
# =============================================================================
echo "🔎 Verifying DMG..."

if [ -n "$APPLE_ID" ] && [ -n "$APPLE_PASSWORD" ] && [ -n "$APPLE_TEAM_ID" ]; then
  if ! xcrun stapler validate "$STAGING_DMG_PATH" 2>&1; then
    echo "❌ stapler validate failed on $STAGING_DMG_PATH"
    exit 1
  fi
  echo "✅ stapler validate passed"
else
  echo "⚠️  Skipping stapler validate — no notarization credentials (dev build)"
fi

DMG_MODE=$(stat -f "%Lp" "$STAGING_DMG_PATH")
OTHER_BIT=${DMG_MODE: -1}
if [ $(( OTHER_BIT & 4 )) -eq 0 ]; then
  echo "❌ chmod a+r did not take — DMG is not readable by other users (mode $DMG_MODE)"
  exit 1
fi
echo "✅ DMG is readable by other users (mode $DMG_MODE)"

MOUNT_POINT=$(mktemp -d "/tmp/pane-dmg-verify.XXXXXX")
# #3049: same fail-closed helper as the swap mount above — a verify mount
# that silently didn't happen must not be mistaken for an empty DMG.
dmg_attach_or_die "$STAGING_DMG_PATH" "$MOUNT_POINT" -readonly
APPLICATIONS_OK=false
if [ -d "$MOUNT_POINT/Applications" ] || [ -L "$MOUNT_POINT/Applications" ]; then
  APPLICATIONS_OK=true
fi
APP_OK=false
if [ -d "$MOUNT_POINT/Pane.app" ]; then
  APP_OK=true
fi
dmg_detach_or_die "$MOUNT_POINT"
rmdir "$MOUNT_POINT" 2>/dev/null || true
MOUNT_POINT=""

if [ "$APPLICATIONS_OK" != true ]; then
  echo "❌ Applications shortcut missing from final DMG"
  exit 1
fi
echo "✅ Applications shortcut present"

if [ "$APP_OK" != true ]; then
  echo "❌ Pane.app missing from final DMG"
  exit 1
fi
echo "✅ Pane.app present"

# All checks passed — this is the only point in the script that ever writes
# to the final, real DMG name. `mv` within the same directory is an atomic
# rename on a single filesystem, so there is no window where a half-made or
# unverified file exists at $FINAL_DMG_PATH.
mv -f "$STAGING_DMG_PATH" "$FINAL_DMG_PATH"
STAGING_DMG_PATH=""

# #3049: final assertion. This alone would have caught the swap-step
# regression regardless of which earlier step broke — nothing exits 0
# without a real, non-empty DMG actually sitting at the final path. If this
# somehow fails *after* the mv (e.g. a race or a filesystem quirk), the bad
# artifact is removed from the real, consumer-facing name rather than left
# there looking like a good build — the trap only tracks scratch/staging
# paths, not $FINAL_DMG_PATH, so this is the one place that has to clean it
# up itself.
if [ ! -s "$FINAL_DMG_PATH" ]; then
  echo "❌ Final DMG missing or empty after mv: $FINAL_DMG_PATH"
  rm -f "$FINAL_DMG_PATH"
  exit 1
fi

echo ""
echo "✅ DMG created and verified: $FINAL_DMG_PATH ($(stat -f %z "$FINAL_DMG_PATH") bytes)"
echo "⚠️  UNSIGNED DEV BUILD: not notarized, will be Gatekeeper-blocked on any other Mac. Do not distribute this DMG. Signed builds come only from .github/workflows/release-publish.yml (#3239)."
