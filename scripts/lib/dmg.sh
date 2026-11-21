#!/usr/bin/env bash
# =============================================================================
# scripts/lib/dmg.sh — shared hdiutil attach/detach helpers (#3049)
# =============================================================================
# Both scripts/build.sh (the real DMG swap) and
# scripts/tests/dmg-swap/test-swap-path.sh (the CI regression test for that
# swap) mount and unmount disk images the exact same way. Per the
# one-function-per-operation rule, this is the single place that decides
# what "mounted successfully" and "detached successfully" mean — fixed once
# here, every caller gets the fix.
#
# Root cause this exists to close (#3049): build.sh's swap step ran
# `hdiutil attach -readwrite` directly on a copy of Tauri's finished DMG,
# which is UDZO (read-only compressed) — a format that can never mount
# read-write, on any machine. The attach call failed, but under `-quiet`
# the real hdiutil error was swallowed, and the only failure-detection in
# the caller (a guard checking file contents *inside* the mount) tests a
# side effect of a successful mount, not the mount itself — so a mount that
# silently produced an empty directory could, in principle, read as "fine"
# until a later step broke in a more confusing way. These helpers make
# mount-success and detach-success first-class, explicit checks everywhere
# a dmg is attached or detached, instead of relying on a later side-effect
# test or on `-quiet`-swallowed exit codes.
#
# Usage: source this file, then call:
#   dmg_attach_or_die <dmg-path> <mountpoint> [extra hdiutil attach flags...]
#   dmg_detach_or_die <mountpoint>
#
# Neither helper ever passes -quiet to hdiutil: a disk-image failure must be
# diagnosable from the log it fails in, not swallowed.
# =============================================================================

# dmg_attach_or_die — attach a dmg and fail loudly (exit 1) unless the
# attach both (a) exits 0 AND (b) actually produces a live mount at
# <mountpoint>. Neither signal alone is reliable: (a) can be defeated by
# `-quiet` hiding *why* a non-zero exit happened, and (b) is the check that
# was missing entirely before #3049 — a command that reports success without
# actually mounting anything would otherwise sail through undetected until
# whatever ran next broke in a more confusing way.
dmg_attach_or_die() {
  local dmg="$1" mountpoint="$2"
  shift 2
  local output resolved_mountpoint
  if ! output=$(hdiutil attach "$dmg" -mountpoint "$mountpoint" -nobrowse "$@" 2>&1); then
    echo "❌ hdiutil attach failed for $dmg -> $mountpoint:" >&2
    echo "$output" >&2
    exit 1
  fi
  # macOS's /tmp is a symlink to /private/tmp (and mountpoints are commonly
  # under /tmp) — `mount`'s own output always shows the resolved, canonical
  # path, never the symlinked one. Resolve before comparing, or this check
  # false-negatives (reports "not mounted") on every /tmp-based mountpoint.
  resolved_mountpoint=$(cd "$mountpoint" 2>/dev/null && pwd -P || echo "$mountpoint")
  if ! mount | grep -qF " on ${resolved_mountpoint} "; then
    echo "❌ hdiutil attach reported success (exit 0) but $mountpoint is not an active mount:" >&2
    echo "$output" >&2
    exit 1
  fi
}

# dmg_detach_or_die — detach a dmg and fail loudly (exit 1) on a non-zero
# exit, with the real hdiutil error surfaced. Callers that are already
# unwinding from an earlier failure (an EXIT trap doing best-effort cleanup)
# should call `hdiutil detach ... || true` directly instead — this helper is
# for the main, happy-path flow where a detach failure is itself a new,
# reportable problem and must not be silently ignored.
dmg_detach_or_die() {
  local mountpoint="$1"
  local output
  if ! output=$(hdiutil detach "$mountpoint" 2>&1); then
    echo "❌ hdiutil detach failed for $mountpoint:" >&2
    echo "$output" >&2
    exit 1
  fi
}

# dmg_convert_or_die — convert a dmg to a new format and fail loudly unless
# the output lands at *exactly* the path given.
#
# Root cause this exists to close (#3049, fast-follow): `hdiutil convert -o
# <path>` silently appends `.dmg` to <path> if it doesn't already end in
# `.dmg`. build.sh asked for a staging file named
# `.Pane_1.0.61.dmg.building` — hdiutil actually wrote
# `.Pane_1.0.61.dmg.building.dmg` instead, so every later check for the
# staging file at the name it was given found nothing there. Both of
# build.sh's `hdiutil convert` call sites, and both of
# test-swap-path.sh's, had their own copy of this call with their own file
# names — four unshared copies, none of which had this check, which is
# exactly how two separate scripts missed the same bug: the test's own
# names happened to already end in `.dmg`, so the bug never had a chance
# to show up there.
#
# Same two-signal shape as dmg_attach_or_die: an exit code alone isn't
# enough (hdiutil convert can exit 0 while writing to a different path
# than the one asked for), so this also independently verifies the output
# actually exists at the exact path given. That single assertion is what
# makes this bug structurally impossible to reintroduce — any caller that
# ever passes a non-`.dmg`-suffixed output path, or hits a convert that
# silently lands elsewhere, fails loudly here instead of leaving a missing
# file for some later, unrelated check to discover.
dmg_convert_or_die() {
  local src="$1" format="$2" out="$3"
  local output
  case "$out" in
    *.dmg) ;;
    *)
      echo "❌ dmg_convert_or_die: output path must end in .dmg — hdiutil convert -o auto-appends .dmg when it doesn't, which lands the file at the wrong path (this is exactly #3049's staging-file bug): $out" >&2
      exit 1
      ;;
  esac
  if ! output=$(hdiutil convert "$src" -format "$format" -ov -o "$out" 2>&1); then
    echo "❌ hdiutil convert failed for $src -> $out ($format):" >&2
    echo "$output" >&2
    exit 1
  fi
  if [ ! -s "$out" ]; then
    echo "❌ hdiutil convert reported success but $out is missing or empty — the output likely landed somewhere else:" >&2
    echo "$output" >&2
    exit 1
  fi
}

# dmg_cleanup_mount — best-effort detach of a mountpoint during EXIT-trap
# unwind. ALWAYS returns 0. Used by both build.sh's cleanup_scratch and
# test-swap-path.sh's cleanup, so there is one definition of what
# "best-effort cleanup of a mount" means (one function per operation,
# same rationale as dmg_attach_or_die/dmg_detach_or_die above). Never
# calls `exit` (safe to call from inside an EXIT trap) and never silences
# a detach failure to /dev/null — a failed detach here is surfaced as a
# warning so a failed run's log always explains what state the mountpoint
# was left in, instead of going quiet about it.
#
# `return 0` at the end is NOT decorative — see dmg_rm_scratch below for
# why a non-zero return from a helper called inside an EXIT trap is its
# own flavor of #3049's exit-code bug (set -e aborts the trap mid-flight,
# before it reaches its own captured-exit-code `exit`).
dmg_cleanup_mount() {
  local mountpoint="$1"
  if [ -n "$mountpoint" ] && [ -d "$mountpoint" ]; then
    local output
    if ! output=$(hdiutil detach "$mountpoint" -force 2>&1); then
      echo "⚠️  cleanup: hdiutil detach failed for $mountpoint (continuing cleanup):" >&2
      echo "$output" >&2
    fi
    rmdir "$mountpoint" 2>/dev/null || true
  fi
  return 0
}

# dmg_apply_ds_store: copy a pre-staged .DS_Store asset onto an
# already-mounted, already-readwrite DMG volume, overwriting whatever
# Tauri's own bundler wrote there.
#
# Root cause this exists to close (#3247): Tauri's vendored bundle_dmg
# script unconditionally deletes any .DS_Store it finds in its OWN staging
# folder before it ever calls `hdiutil create`; that deletion is separate
# from, and unaffected by, the `--skip-jenkins` CI=true check (which only
# skips bundle_dmg's AppleScript Finder-layout step). So a .DS_Store staged
# before Tauri builds the DMG is wiped regardless of CI. The only place a
# custom .DS_Store survives is applied AFTER Tauri's bundle_dmg has already
# finished, onto the mount of the already-built DMG, in the same
# read-write mount window build.sh already opens to swap in the
# notarized/stapled app (#3049's swap-mount block). This function is a
# thin `cp`, not a new mount cycle; callers must already hold an open
# -readwrite mount at <mountpoint> (see dmg_attach_or_die) before calling
# this.
dmg_apply_ds_store() {
  local mountpoint="$1" asset_path="$2"
  if [ ! -f "$asset_path" ]; then
    echo "❌ dmg_apply_ds_store: asset not found at $asset_path" >&2
    exit 1
  fi
  if ! cp "$asset_path" "$mountpoint/.DS_Store"; then
    echo "❌ dmg_apply_ds_store: failed to copy $asset_path -> $mountpoint/.DS_Store" >&2
    exit 1
  fi
}

# dmg_rm_scratch — rm -f every non-empty path given. ALWAYS returns 0.
#
# Root cause this exists to close (#3049, fast-follow): a trap that ends
# with a chain of `[ -n "$VAR" ] && rm -f "$VAR"` lines returns the exit
# status of whichever line ran last — 1 whenever that $VAR happens to be
# empty, which on a successful build is exactly the normal case (the last
# scratch var in build.sh's old trap, STAGING_DMG_PATH, is cleared to ""
# right after a successful `mv`). Under `set -e`, an EXIT trap's own exit
# status becomes the script's exit status unless the trap explicitly
# captures and re-exits the real one — so that trailing `&& rm -f` line
# silently turned every successful build into a reported failure.
#
# `return 0` at the end is NOT decorative: `set -e` is in effect *inside*
# EXIT traps too, so if this function's own exit status came out non-zero
# (e.g. from a `for` loop whose last iteration's `[ -n "$path" ] && rm -f`
# evaluated false because that last path was empty — exactly the same
# shape as the original bug, just one level deeper), the trap calling this
# would abort right here under `set -e`, before ever reaching its own
# `exit "$exit_code"` — and the script's real exit status would be
# silently replaced by *this function's* status instead. That is bug 2,
# reintroduced through a helper instead of a hand-written line. This
# helper only removes the repeated per-path `[ -n ... ] &&` guard so a
# trap can list its scratch paths as one call instead of one line per
# path — callers still must capture `local exit_code=$?` as the FIRST
# statement of their trap function (before calling this or anything else)
# and `exit "$exit_code"` as the LAST statement.
dmg_rm_scratch() {
  local path
  for path in "$@"; do
    [ -n "$path" ] && rm -f "$path"
  done
  return 0
}
