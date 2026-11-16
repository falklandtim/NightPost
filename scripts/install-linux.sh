#!/usr/bin/env bash
# Build NightPost and install it for the current user on Linux: the program
# in ~/.local/bin, an app-menu launcher and the icon. Run it again to update.
#
#   scripts/install-linux.sh
#
# Needs Rust plus the OpenSSL, D-Bus and WebKitGTK development files:
#   Fedora:  sudo dnf install openssl-devel dbus-devel pkgconf-pkg-config gtk4-devel webkitgtk6.0-devel libsoup3-devel hunspell-en
#   Arch:    sudo pacman -S --needed openssl dbus pkgconf gtk4 webkitgtk-6.0 hunspell-en_us
set -euo pipefail
cd "$(dirname "$0")/.."

PREFIX="${PREFIX:-$HOME/.local}"
APP_ID=co.timson.NightPost

echo "Building NightPost (release)…"
cargo build --release -p nightpost

install -Dm755 target/release/nightpost "$PREFIX/bin/nightpost"

# The icon, at the sizes desktops look for (resized when ImageMagick is
# installed; otherwise the full-size picture serves every size).
SRC=crates/nightpost/assets/icon-1024.png
RESIZE=$(command -v magick || command -v convert || true)
for size in 64 128 256 512; do
  dest="$PREFIX/share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
  mkdir -p "$(dirname "$dest")"
  if [ -n "$RESIZE" ]; then "$RESIZE" "$SRC" -resize "${size}x${size}" "$dest"; else cp "$SRC" "$dest"; fi
done

mkdir -p "$PREFIX/share/applications"
cat > "$PREFIX/share/applications/$APP_ID.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=NightPost
GenericName=Mail Client
Comment=A fast, native email client
Exec=$PREFIX/bin/nightpost
Icon=$APP_ID
Terminal=false
Categories=Network;Email;
Keywords=mail;email;imap;inbox;
StartupWMClass=$APP_ID
DESKTOP

command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" || true

echo "Installed $PREFIX/bin/nightpost and the NightPost launcher."
case ":$PATH:" in *":$PREFIX/bin:"*) ;; *) echo "note: $PREFIX/bin isn't on your PATH; the launcher works anyway." ;; esac
echo "MCP: claude mcp add nightpost -- $PREFIX/bin/nightpost mcp"
