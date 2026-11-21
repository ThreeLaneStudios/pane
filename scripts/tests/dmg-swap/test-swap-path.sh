#!/usr/bin/env bash
# =============================================================================
# test-swap-path.sh — regression test for the DMG UDRW swap (#3049)
# =============================================================================
# Exercises the exact sequence scripts/build.sh runs against the real DMG,
# against a throwaway dummy image instead — no Apple keys, no `tauri build`,
# nothing outside /tmp:
#
#   create dummy UDZO image -> convert to UDRW -> attach read-write ->
#   swap a file -> detach -> convert back to UDZO -> verify the swap landed
#
# Root cause (#3049): build.sh's swap step mounted Tauri's finished DMG
# -readwrite *directly*. That DMG is UDZO (read-only compressed) — a format
# that can never mount read-write, on any machine. The UDRW conversion
# below is the fix; this test fails without it (see the skip switch below).
#
# Uses the same scripts/lib/dmg.sh attach/detach helpers build.sh itself
# uses — one function per operation, so this test is exercising the exact
# mount-safety logic the real build runs, not a reimplementation of it.
#
# Testing seams:
#   DMG_SWAP_TEST_SKIP_UDRW_CONVERT=1 — skip the UDRW conversion step and
#     attach the UDZO image -readwrite directly. Reproduces #3049's
#     original bug exactly.
#   DMG_SWAP_TEST_BREAK_STAGING_NAME=1 — ask the final UDZO convert for an
#     output path that doesn't end in .dmg (mirrors build.sh's original
#     ".Pane_${VERSION}.dmg.building", which isn't .dmg-suffixed either).
#     Reproduces the #3049 fast-follow's first bug: `hdiutil convert -o`
#     silently appends .dmg to a path that doesn't already end in it, so
#     the file lands somewhere other than the name every later check
#     looks for. dmg_convert_or_die's exact-path assertion is what's
#     supposed to turn that silent miss into a loud failure here.
#   DMG_SWAP_TEST_BREAK_TRAP=1 — reinstate the pre-fast-follow EXIT trap
#     shape (a bare `[ -n "$VAR" ] && rm -f "$VAR"` chain with no captured
#     exit code) instead of this script's own fixed trap below. Reproduces
#     the #3049 fast-follow's second bug: that trap shape makes a fully
#     successful run exit non-zero, because its last statement evaluates
#     to 1 whenever the final scratch var happens to be empty — which, on
#     a clean run, it is.
# All three are how this test proves it actually catches the regressions
# it exists to catch, rather than passing vacuously (see
# scripts/tests/dmg-swap/prove-catches-regression.sh, and
# scripts/tests/check-spec-first/run-tests.sh for the same proof shape on a
# different check).
#
# Called by: .github/workflows/dmg-swap-test.yml (self-hosted macOS runner,
# gated to PRs that touch scripts/build.sh, scripts/lib/dmg.sh, or this
# test directory)
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
# shellcheck source=../../lib/dmg.sh
source "$REPO_ROOT/scripts/lib/dmg.sh"

SKIP_UDRW="${DMG_SWAP_TEST_SKIP_UDRW_CONVERT:-0}"
BREAK_STAGING_NAME="${DMG_SWAP_TEST_BREAK_STAGING_NAME:-0}"
BREAK_TRAP="${DMG_SWAP_TEST_BREAK_TRAP:-0}"

WORKDIR=$(mktemp -d "/tmp/dmg-swap-test.XXXXXX")
MOUNT_POINT=""
NOTHING=""
cleanup() {
  # Must be the absolute first statement in this function, before even the
  # BREAK_TRAP check below — `$?` reflects the exit status of whatever ran
  # right before this trap fired for exactly one statement, and the `if`
  # test itself would overwrite it otherwise. (Unused in the BREAK_TRAP
  # branch below — that branch deliberately reproduces the old bug, which
  # never captured this at all.)
  local exit_code=$?

  if [ "$BREAK_TRAP" = "1" ]; then
    # Deliberately reinstating the pre-fast-follow trap shape (#3049
    # fast-follow, bug 2): no captured exit code, and a trailing
    # `[ -n "$VAR" ] && rm -f "$VAR"` chain whose last line's exit status
    # becomes this whole trap's exit status — which, under `set -e`,
    # becomes the *script's* exit status. $NOTHING is always empty here on
    # purpose, so this line always evaluates to 1, exactly like
    # $STAGING_DMG_PATH did in build.sh on every successful run.
    if [ -n "$MOUNT_POINT" ] && [ -d "$MOUNT_POINT" ]; then
      hdiutil detach "$MOUNT_POINT" -force >/dev/null 2>&1 || true
      rmdir "$MOUNT_POINT" 2>/dev/null || true
    fi
    rm -rf "$WORKDIR"
    [ -n "$NOTHING" ] && rm -f "$NOTHING"
    return
  fi

  # Fixed shape (#3049 fast-follow): exit_code was captured above before
  # anything had a chance to overwrite `$?`; re-exit with it explicitly as
  # this function's LAST statement, so none of this cleanup's own exit
  # statuses can leak out as the script's. Same pattern as build.sh's
  # cleanup_scratch; see dmg_rm_scratch in lib/dmg.sh for the full
  # root-cause writeup.
  dmg_cleanup_mount "$MOUNT_POINT"
  rm -rf "$WORKDIR"
  exit "$exit_code"
}
trap cleanup EXIT

echo "=== DMG swap-path test (#3049) ==="
echo "Workdir: $WORKDIR"
if [ "$SKIP_UDRW" = "1" ]; then
  echo "⚠️  DMG_SWAP_TEST_SKIP_UDRW_CONVERT=1 — intentionally reproducing #3049's root cause"
fi
echo ""

# ---------------------------------------------------------------------------
# 1. Create a dummy UDZO (read-only compressed) image — same format Tauri's
#    DMG bundler produces for the real build.
# ---------------------------------------------------------------------------
SRC_DIR="$WORKDIR/src"
mkdir -p "$SRC_DIR"
ln -s /Applications "$SRC_DIR/Applications"
echo "before-swap" > "$SRC_DIR/marker.txt"

UDZO_DMG="$WORKDIR/dummy.dmg"
echo "🔨 Creating dummy UDZO image..."
hdiutil create -volname DmgSwapTest -srcfolder "$SRC_DIR" -format UDZO -ov -quiet -o "$UDZO_DMG"
if [ ! -s "$UDZO_DMG" ]; then
  echo "❌ hdiutil create produced no (or empty) dummy image"
  exit 1
fi
FORMAT_LINE=$(hdiutil imageinfo "$UDZO_DMG" | grep "Format Description" || true)
echo "   $FORMAT_LINE"
if ! echo "$FORMAT_LINE" | grep -qi "read-only"; then
  echo "❌ Dummy fixture image is not read-only compressed — test fixture itself is wrong"
  exit 1
fi

# ---------------------------------------------------------------------------
# 2. Convert to UDRW (the fix under test) — unless the proof-of-regression
#    switch is set, in which case this step is skipped on purpose.
# ---------------------------------------------------------------------------
WORK_DMG="$UDZO_DMG"
if [ "$SKIP_UDRW" != "1" ]; then
  echo "🔄 Converting to UDRW..."
  RW_DMG="$WORKDIR/dummy_rw.dmg"
  dmg_convert_or_die "$UDZO_DMG" UDRW "$RW_DMG"
  echo "✅ Converted to UDRW: $RW_DMG"
  WORK_DMG="$RW_DMG"
else
  echo "⚠️  Skipping UDRW conversion — attaching the UDZO image -readwrite directly"
fi

# ---------------------------------------------------------------------------
# 3. Attach read-write, swap a file, detach.
# ---------------------------------------------------------------------------
MOUNT_POINT=$(mktemp -d "/tmp/dmg-swap-test-mount.XXXXXX")
echo "📎 Attaching $WORK_DMG read-write at $MOUNT_POINT..."
dmg_attach_or_die "$WORK_DMG" "$MOUNT_POINT" -readwrite

echo "✏️  Swapping marker.txt..."
echo "after-swap" > "$MOUNT_POINT/marker.txt"

echo "📤 Detaching..."
dmg_detach_or_die "$MOUNT_POINT"
rmdir "$MOUNT_POINT" 2>/dev/null || true
MOUNT_POINT=""

# ---------------------------------------------------------------------------
# 4. Convert back to UDZO (mirrors build.sh repacking the final DMG).
# ---------------------------------------------------------------------------
if [ "$BREAK_STAGING_NAME" = "1" ]; then
  # #3049 fast-follow bug 1, reproduced on purpose: a non-.dmg-suffixed
  # output path, same shape as build.sh's original
  # ".Pane_${VERSION}.dmg.building". dmg_convert_or_die must reject this
  # outright rather than silently writing to "$FINAL_UDZO.dmg".
  FINAL_UDZO="$WORKDIR/dummy_final.dmg.building"
  echo "⚠️  DMG_SWAP_TEST_BREAK_STAGING_NAME=1 — intentionally reproducing #3049 fast-follow bug 1 (non-.dmg output path: $FINAL_UDZO)"
else
  FINAL_UDZO="$WORKDIR/dummy_final.dmg"
fi
echo "🔄 Converting back to UDZO..."
dmg_convert_or_die "$WORK_DMG" UDZO "$FINAL_UDZO"

# ---------------------------------------------------------------------------
# 5. Verify the swap actually landed and the Applications symlink survived
#    the UDZO -> UDRW -> UDZO round-trip.
# ---------------------------------------------------------------------------
echo "🔎 Verifying the swap landed..."
MOUNT_POINT=$(mktemp -d "/tmp/dmg-swap-test-verify.XXXXXX")
dmg_attach_or_die "$FINAL_UDZO" "$MOUNT_POINT" -readonly
CONTENT=$(cat "$MOUNT_POINT/marker.txt" 2>/dev/null || echo "MISSING")
APPLICATIONS_OK=false
if [ -L "$MOUNT_POINT/Applications" ]; then
  APPLICATIONS_OK=true
fi
dmg_detach_or_die "$MOUNT_POINT"
rmdir "$MOUNT_POINT" 2>/dev/null || true
MOUNT_POINT=""

if [ "$CONTENT" != "after-swap" ]; then
  echo "❌ Swap did not land: marker.txt reads '$CONTENT' (expected 'after-swap')"
  exit 1
fi
echo "✅ Swap landed: marker.txt reads 'after-swap'"

if [ "$APPLICATIONS_OK" != true ]; then
  echo "❌ Applications symlink did not survive the UDZO -> UDRW -> UDZO round-trip"
  exit 1
fi
echo "✅ Applications symlink survived the round-trip"

echo ""
echo "✅ DMG swap-path test passed"
