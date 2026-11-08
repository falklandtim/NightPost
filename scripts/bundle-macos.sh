#!/usr/bin/env bash
# Build target/release/OwlPost.app: a double-clickable macOS app with the
# OwlPost icon (Finder, Dock, Launchpad, ⌘-Tab).
#
#   scripts/bundle-macos.sh            # build the bundle
#   scripts/bundle-macos.sh --install  # …and copy it to /Applications
set -euo pipefail
cd "$(dirname "$0")/.."

ASSETS=crates/owlpost/assets
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
APP=target/release/OwlPost.app
WORK=target/release/bundle-work

echo "Building OwlPost $VERSION (release)…"
cargo build --release -p owlpost

# Padded artwork (macOS icons sit inside a margin); regenerate if the source is newer.
if [[ ! -f $ASSETS/icon-macos-1024.png || $ASSETS/icon-1024.png -nt $ASSETS/icon-macos-1024.png ]]; then
  cargo run -q --release -p owlpost --example icon_assets
fi

rm -rf "$WORK" "$APP"
mkdir -p "$WORK/OwlPost.iconset" "$APP/Contents/MacOS" "$APP/Contents/Resources"
for size in 16 32 128 256 512; do
  sips -z $size $size "$ASSETS/icon-macos-1024.png" --out "$WORK/OwlPost.iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z $double $double "$ASSETS/icon-macos-1024.png" --out "$WORK/OwlPost.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$WORK/OwlPost.iconset" -o "$APP/Contents/Resources/OwlPost.icns"

cp target/release/owlpost "$APP/Contents/MacOS/owlpost"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>OwlPost</string>
  <key>CFBundleDisplayName</key><string>OwlPost</string>
  <key>CFBundleIdentifier</key><string>co.timson.OwlPost</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>owlpost</string>
  <key>CFBundleIconFile</key><string>OwlPost</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSContactsUsageDescription</key><string>OwlPost shows the photos of people in your Contacts next to their emails.</string>
</dict>
</plist>
PLIST

# Sign with the local certificate (scripts/make-signing-cert.sh) so every
# build counts as the same app and keeps its Keychain access. Without it,
# fall back to an ad-hoc signature, which macOS treats as a new app each time.
IDENTITY="OwlPost Local Signing"
if security find-identity -v -p codesigning 2>/dev/null | grep -q "$IDENTITY"; then
  codesign --force --deep --sign "$IDENTITY" --identifier co.timson.OwlPost "$APP"
  echo "Signed with \"$IDENTITY\""
else
  codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || echo "note: codesign failed; the app may need right-click → Open the first time"
  echo "note: signed ad hoc, so macOS will ask for Keychain access again after installing."
  echo "      Run scripts/make-signing-cert.sh once to stop that."
fi
rm -rf "$WORK"
echo "Built $APP"

if [[ "${1:-}" == "--install" ]]; then
  rm -rf /Applications/OwlPost.app
  cp -R "$APP" /Applications/
  echo "Installed /Applications/OwlPost.app"
  echo "MCP: claude mcp add owlpost -- /Applications/OwlPost.app/Contents/MacOS/owlpost mcp"
fi
