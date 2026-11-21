#!/usr/bin/env bash
# G8 fixture: isolated path-shape-safety check. These lines must NEVER match
# G8's anchored rm -rf patterns, guarded or not — they are not the dangerous
# ~/Pane / $HOME/Pane shape at all (DMG-mount build artifacts: build.sh:313,
# build-gateway-dmg.sh:115-shaped scratch removals).
MOUNT_POINT=$(mktemp -d)
rm -rf "$MOUNT_POINT/Pane.app"
rm -rf "$STAGING_DIR"
rm -rf "/tmp/pane-dmg-mount.XXXXXX/PaneGateway_1.0.1_aarch64.dmg"
