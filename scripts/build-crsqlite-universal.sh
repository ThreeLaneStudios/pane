#!/bin/bash
set -euo pipefail

echo "🔨 Building universal crsqlite.dylib (arm64 + x86_64)"
echo ""

# Set up reproducible build environment
# Note: cr-sqlite commit 891fe9e0 uses concat_idents feature removed in Rust 1.90.0
# Using nightly-2024-06-15 (1.81.0-nightly, before 1.90.0 removal)
export RUSTUP_TOOLCHAIN=nightly-2024-06-15

# Capture Pane repo location before changing directories
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$SCRIPT_DIR/.."
RESOURCES_DIR="$REPO_ROOT/src-tauri/resources"

# Create temp directory for builds
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

cd "$TEMP_DIR"

# Clone cr-sqlite at pinned commit
echo "📦 Cloning cr-sqlite..."
git clone --quiet https://github.com/vlcn-io/cr-sqlite.git
cd cr-sqlite
git checkout 891fe9e0
echo "✅ Checked out commit 891fe9e0"
echo ""

echo "📦 Initializing submodules..."
# Convert SSH URLs to HTTPS for submodules
sed -i '' 's|git@github.com:|https://github.com/|g' .gitmodules
git submodule update --init --recursive --quiet
echo "✅ Submodules initialized"
echo ""

# Build arm64
echo "🏗️  Building arm64..."
cd core
CI_MAYBE_TARGET=aarch64-apple-darwin make loadable
cp dist/crsqlite.dylib "$TEMP_DIR/crsqlite-arm64.dylib"
echo "✅ arm64 build complete"
echo ""

# Clean and build x86_64
echo "🧹 Cleaning..."
make clean
echo ""

echo "🏗️  Building x86_64..."
CI_MAYBE_TARGET=x86_64-apple-darwin make loadable
cp dist/crsqlite.dylib "$TEMP_DIR/crsqlite-x86_64.dylib"
echo "✅ x86_64 build complete"
echo ""

# Create universal binary
echo "🔗 Creating universal binary..."
cd "$TEMP_DIR"
lipo -create crsqlite-arm64.dylib crsqlite-x86_64.dylib -output crsqlite-universal.dylib

# Verify
echo ""
echo "🔍 Verifying architectures..."
lipo -info crsqlite-universal.dylib

if ! lipo -info crsqlite-universal.dylib | grep -q "arm64"; then
  echo "❌ ERROR: arm64 architecture missing"
  exit 1
fi

if ! lipo -info crsqlite-universal.dylib | grep -q "x86_64"; then
  echo "❌ ERROR: x86_64 architecture missing"
  exit 1
fi

echo "✅ Universal binary verified"
echo ""

# Copy to Pane resources
mkdir -p "$RESOURCES_DIR"
cp crsqlite-universal.dylib "$RESOURCES_DIR/crsqlite.dylib"

echo "📦 Copied to src-tauri/resources/crsqlite.dylib"
echo ""

# Sign with Developer ID to match app signing identity
echo "🔏 Signing with Developer ID..."
codesign --force --sign "Developer ID Application: Matthew Carano (FL493VAUAL)" "$RESOURCES_DIR/crsqlite.dylib"

# Verify signing
echo "🔍 Verifying code signature..."
codesign -dv "$RESOURCES_DIR/crsqlite.dylib" 2>&1
TEAM_ID=$(codesign -dv "$RESOURCES_DIR/crsqlite.dylib" 2>&1 | grep TeamIdentifier | cut -d= -f2)
if [ "$TEAM_ID" != "FL493VAUAL" ]; then
  echo "❌ ERROR: TeamIdentifier mismatch — expected FL493VAUAL, got $TEAM_ID"
  exit 1
fi
echo "✅ Code signature verified (TeamIdentifier=FL493VAUAL)"
echo ""

# Compute and display SHA256
echo "🔐 SHA256:"
shasum -a 256 "$RESOURCES_DIR/crsqlite.dylib"
echo ""
echo "✅ Universal crsqlite.dylib build complete"
