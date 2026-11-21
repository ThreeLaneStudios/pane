#!/bin/bash
#
# build-gateway-dmg.sh
#
# Builds, signs, notarizes, and packages the Pane Gateway (src-gateway/)
# as a DMG, then uploads it as a GitHub release.
#
# This mirrors what `tauri build` + `scripts/release.sh` do for the main
# Pane app, but for the standalone `pane-gateway` CLI binary (no .app
# bundle — it's a plain executable).
#
# Usage:
#   ./scripts/build-gateway-dmg.sh
#   ./scripts/build-gateway-dmg.sh --version 1.0.0
#   ./scripts/build-gateway-dmg.sh --skip-build
#   ./scripts/build-gateway-dmg.sh --dry-run
#
set -euo pipefail

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
SIGNING_IDENTITY="Developer ID Application: Matthew Carano (FL493VAUAL)"
GATEWAY_MANIFEST="src-gateway/Cargo.toml"
GATEWAY_BINARY_PATH="src-gateway/target/release/pane-gateway"
BUILDS_DIR="$HOME/Shared/Pane/builds"
GH_REPO="ThreeLaneStudios/pane"
ENV_FILE="$HOME/Projects/pane/.env"

# ---------------------------------------------------------------------------
# Args
# ---------------------------------------------------------------------------
VERSION=""
SKIP_BUILD=0
DRY_RUN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      if [[ -z "$VERSION" ]]; then
        echo "[GATEWAY-DMG] Error: --version requires a value" >&2
        exit 1
      fi
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      grep '^#' "$0" | sed 's/^#//'
      exit 0
      ;;
    *)
      echo "[GATEWAY-DMG] Error: unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

# ---------------------------------------------------------------------------
# Must run from repo root
# ---------------------------------------------------------------------------
if [[ ! -f "$GATEWAY_MANIFEST" ]]; then
  echo "[GATEWAY-DMG] Error: must be run from the pane repo root (missing ${GATEWAY_MANIFEST})" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Resolve version (from Cargo.toml if not passed via --version)
# ---------------------------------------------------------------------------
if [[ -z "$VERSION" ]]; then
  VERSION=$(grep '^version' "$GATEWAY_MANIFEST" | head -1 | sed 's/.*"\(.*\)".*/\1/')
  if [[ -z "$VERSION" ]]; then
    echo "[GATEWAY-DMG] Error: could not determine version from ${GATEWAY_MANIFEST}" >&2
    exit 1
  fi
  echo "[GATEWAY-DMG] Using version from Cargo.toml: ${VERSION}"
else
  echo "[GATEWAY-DMG] Using version from --version flag: ${VERSION}"
fi

DMG_NAME="PaneGateway_${VERSION}_aarch64.dmg"

# ---------------------------------------------------------------------------
# Load Apple credentials from .env if not already in environment
# ---------------------------------------------------------------------------
if [[ -z "${APPLE_ID:-}" || -z "${APPLE_PASSWORD:-}" || -z "${APPLE_TEAM_ID:-}" ]]; then
  if [[ -f "$ENV_FILE" ]]; then
    echo "[GATEWAY-DMG] Loading Apple credentials from ${ENV_FILE}"
    set -a
    # shellcheck disable=SC1090
    source "$ENV_FILE"
    set +a
  fi
fi

if [[ -z "${APPLE_ID:-}" || -z "${APPLE_PASSWORD:-}" || -z "${APPLE_TEAM_ID:-}" ]]; then
  echo "[GATEWAY-DMG] Error: APPLE_ID, APPLE_PASSWORD, and APPLE_TEAM_ID must be set (env or ${ENV_FILE})" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Temp staging dir + cleanup trap
# ---------------------------------------------------------------------------
STAGING_DIR=""
cleanup() {
  if [[ -n "$STAGING_DIR" && -d "$STAGING_DIR" ]]; then
    echo "[GATEWAY-DMG] Cleaning up staging dir: ${STAGING_DIR}"
    rm -rf "$STAGING_DIR"
  fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# 1. Build
# ---------------------------------------------------------------------------
if [[ "$SKIP_BUILD" -eq 1 ]]; then
  echo "[GATEWAY-DMG] --skip-build set, reusing existing binary"
  if [[ ! -f "$GATEWAY_BINARY_PATH" ]]; then
    echo "[GATEWAY-DMG] Error: --skip-build set but no binary found at ${GATEWAY_BINARY_PATH}" >&2
    exit 1
  fi
else
  echo "[GATEWAY-DMG] Building pane-gateway v${VERSION} (release)..."
  cargo build --release --manifest-path "$GATEWAY_MANIFEST"
fi

if [[ ! -f "$GATEWAY_BINARY_PATH" ]]; then
  echo "[GATEWAY-DMG] Error: binary not found at ${GATEWAY_BINARY_PATH} after build" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 2. Sign the binary
# ---------------------------------------------------------------------------
echo "[GATEWAY-DMG] Signing binary with hardened runtime..."
codesign --force --options runtime --sign "$SIGNING_IDENTITY" \
  --timestamp "$GATEWAY_BINARY_PATH"

echo "[GATEWAY-DMG] Verifying binary signature..."
codesign --verify --verbose "$GATEWAY_BINARY_PATH"

# ---------------------------------------------------------------------------
# 3. Staging directory
# ---------------------------------------------------------------------------
STAGING_DIR=$(mktemp -d /tmp/pane-gateway-dmg.XXXXXX)
echo "[GATEWAY-DMG] Staging dir: ${STAGING_DIR}"
cp "$GATEWAY_BINARY_PATH" "$STAGING_DIR/pane-gateway"

# ---------------------------------------------------------------------------
# 4. Create DMG
# ---------------------------------------------------------------------------
if [[ -f "$DMG_NAME" ]]; then
  echo "[GATEWAY-DMG] Removing existing ${DMG_NAME}"
  rm -f "$DMG_NAME"
fi

echo "[GATEWAY-DMG] Creating DMG: ${DMG_NAME}"
hdiutil create -volname "Pane Gateway" -srcfolder "$STAGING_DIR" -ov -format UDZO "$DMG_NAME"

if [[ ! -f "$DMG_NAME" ]]; then
  echo "[GATEWAY-DMG] Error: DMG creation failed, ${DMG_NAME} not found" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 5. Sign the DMG
# ---------------------------------------------------------------------------
echo "[GATEWAY-DMG] Signing DMG..."
codesign --force --sign "$SIGNING_IDENTITY" \
  --timestamp "$DMG_NAME"

# ---------------------------------------------------------------------------
# 6. Notarize
# ---------------------------------------------------------------------------
echo "[GATEWAY-DMG] Submitting DMG for notarization (this can take a few minutes)..."
xcrun notarytool submit "$DMG_NAME" \
  --apple-id "$APPLE_ID" \
  --password "$APPLE_PASSWORD" \
  --team-id "$APPLE_TEAM_ID" \
  --wait

echo "[GATEWAY-DMG] Stapling notarization ticket..."
xcrun stapler staple "$DMG_NAME"

# ---------------------------------------------------------------------------
# 7. Copy to builds dir
# ---------------------------------------------------------------------------
mkdir -p "$BUILDS_DIR"
cp "$DMG_NAME" "$BUILDS_DIR/"
echo "[GATEWAY-DMG] Copied DMG to ${BUILDS_DIR}/${DMG_NAME}"

# ---------------------------------------------------------------------------
# 8. GitHub release
# ---------------------------------------------------------------------------
RELEASE_URL=""
if [[ "$DRY_RUN" -eq 1 ]]; then
  echo "[GATEWAY-DMG] --dry-run set, skipping GitHub release upload"
else
  TAG="gateway-v${VERSION}"
  echo "[GATEWAY-DMG] Creating GitHub release ${TAG}..."
  gh release create "$TAG" \
    --repo "$GH_REPO" \
    --title "Pane Gateway v${VERSION}" \
    --notes "Pane Gateway v${VERSION} for macOS (Apple silicon)." \
    "$DMG_NAME"

  RELEASE_URL=$(gh release view "$TAG" --repo "$GH_REPO" --json url --jq '.url')
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "[GATEWAY-DMG] Done."
echo "[GATEWAY-DMG] DMG: ${BUILDS_DIR}/${DMG_NAME}"
if [[ -n "$RELEASE_URL" ]]; then
  echo "[GATEWAY-DMG] Release: ${RELEASE_URL}"
elif [[ "$DRY_RUN" -eq 1 ]]; then
  echo "[GATEWAY-DMG] Release: skipped (--dry-run)"
fi
