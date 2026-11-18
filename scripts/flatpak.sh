#!/usr/bin/env bash
# Build NightPost as a Flatpak and install it for the current user
# (docs/flatpak.md). Run it on Linux, from anywhere in the repository:
#
#   scripts/flatpak.sh            build and install
#   scripts/flatpak.sh --bundle   also write a .flatpak file to copy to
#                                 another machine or attach to a release
#
# Needs flatpak and the Flathub remote. The first run installs the builder
# (org.flatpak.Builder) and the GNOME runtime it needs, for this user only.
# Build output goes in target/flatpak/, which is safe to delete.
set -euo pipefail
cd "$(dirname "$0")/.."

APP_ID=co.timson.NightPost
MANIFEST=flatpak/$APP_ID.yml
OUT=target/flatpak
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
ARCH=$(flatpak --default-arch)

bundle=false
[ "${1:-}" = "--bundle" ] && bundle=true

# A crate list older than Cargo.lock would fail half-way through the build.
if [ Cargo.lock -nt flatpak/cargo-sources.json ]; then
  echo "Cargo.lock changed: run scripts/flatpak-cargo-sources.py and commit the result." >&2
  exit 1
fi

if command -v flatpak-builder >/dev/null; then
  BUILDER=(flatpak-builder)
else
  flatpak info --user org.flatpak.Builder >/dev/null 2>&1 || flatpak install --user -y flathub org.flatpak.Builder
  BUILDER=(flatpak run org.flatpak.Builder)
fi

mkdir -p "$OUT"
echo "Building NightPost $VERSION ($ARCH) as a Flatpak…"
"${BUILDER[@]}" --user --install --force-clean --install-deps-from=flathub \
  --state-dir="$OUT/state" --repo="$OUT/repo" \
  "$OUT/build" "$MANIFEST"

echo "Installed. Start it from the app menu, or: flatpak run $APP_ID"
echo "MCP: claude mcp add nightpost -- flatpak run $APP_ID mcp"

if $bundle; then
  file="$OUT/nightpost-$VERSION-$ARCH.flatpak"
  flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo "$OUT/repo" "$file" "$APP_ID"
  echo "Bundle: $file (install with: flatpak install --user $(basename "$file"))"
fi
