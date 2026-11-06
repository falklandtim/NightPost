#!/usr/bin/env bash
# Build target/release/SpringPost.app: a double-clickable macOS app with the
# SpringPost icon (Finder, Dock, Launchpad, ⌘-Tab).
#
#   scripts/bundle-macos.sh            # build the bundle
#   scripts/bundle-macos.sh --install  # …and copy it to /Applications
set -euo pipefail
cd "$(dirname "$0")/.."

ASSETS=crates/springpost/assets
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
APP=target/release/SpringPost.app
WORK=target/release/bundle-work

echo "Building SpringPost $VERSION (release)…"
cargo build --release -p springpost

# Padded artwork (macOS icons sit inside a margin); regenerate if the source is newer.
if [[ ! -f $ASSETS/icon-macos-1024.png || $ASSETS/icon-1024.png -nt $ASSETS/icon-macos-1024.png ]]; then
  cargo run -q --release -p springpost --example icon_assets
fi

rm -rf "$WORK" "$APP"
mkdir -p "$WORK/SpringPost.iconset" "$APP/Contents/MacOS" "$APP/Contents/Resources"
for size in 16 32 128 256 512; do
  sips -z $size $size "$ASSETS/icon-macos-1024.png" --out "$WORK/SpringPost.iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z $double $double "$ASSETS/icon-macos-1024.png" --out "$WORK/SpringPost.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$WORK/SpringPost.iconset" -o "$APP/Contents/Resources/SpringPost.icns"

cp target/release/springpost "$APP/Contents/MacOS/springpost"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>SpringPost</string>
  <key>CFBundleDisplayName</key><string>SpringPost</string>
  <key>CFBundleIdentifier</key><string>co.timson.SpringPost</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>springpost</string>
  <key>CFBundleIconFile</key><string>SpringPost</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc signature so macOS will run it locally.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || echo "note: codesign failed; the app may need right-click → Open the first time"
rm -rf "$WORK"
echo "Built $APP"

if [[ "${1:-}" == "--install" ]]; then
  rm -rf /Applications/SpringPost.app
  cp -R "$APP" /Applications/
  echo "Installed /Applications/SpringPost.app"
  echo "MCP: claude mcp add springpost -- /Applications/SpringPost.app/Contents/MacOS/springpost mcp"
fi
