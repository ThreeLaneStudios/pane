#!/bin/bash
set -e

# Sign native libraries before Tauri bundles them
# This ensures notarization succeeds for all embedded binaries

if [ -z "$APPLE_SIGNING_IDENTITY" ]; then
  echo "⚠️  APPLE_SIGNING_IDENTITY not set - skipping dylib signing"
  exit 0
fi

echo "🔏 Signing native libraries..."

# Sign crsqlite.dylib
if [ -f "src-tauri/resources/crsqlite.dylib" ]; then
  codesign --force --sign "$APPLE_SIGNING_IDENTITY" \
    --timestamp \
    --options runtime \
    src-tauri/resources/crsqlite.dylib
  echo "✅ Signed crsqlite.dylib"
else
  echo "⚠️  crsqlite.dylib not found"
fi
