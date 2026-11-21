#!/bin/bash

# Create DMG for Trove 3.8.0
hdiutil create -volname "Trove 3.8.0" -srcfolder src-tauri/target/release/bundle/macos/Trove.app -ov -format UDZO ~/Shared/revenue/trove/builds/v3/Trove_3.8.0_aarch64.dmg