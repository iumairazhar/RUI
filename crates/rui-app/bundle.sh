#!/usr/bin/env bash
# Wrap target/debug/rui-app into target/RUI.app so macOS treats it as an app.
# Usage: bash crates/rui-app/bundle.sh [--open]
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$root/target/RUI.app"
bin="$root/target/debug/rui-app"

cargo build -p rui-app --manifest-path "$root/Cargo.toml"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/rui-app"
cp "$root/crates/rui-app/assets/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.rui.app</string>
  <key>CFBundleName</key><string>RUI</string>
  <key>CFBundleExecutable</key><string>rui-app</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
echo "Bundled $app"

if [[ "${1:-}" == "--open" ]]; then
  open "$app"
fi
