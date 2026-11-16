#!/usr/bin/env bash
# Build a Debian package from an already compiled release binary.
# Usage: packaging/linux/build_deb.sh [version]
set -euo pipefail

VERSION="${1:-1.2.0}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN="$ROOT/src-tauri/target/release/splashhound"
STAGE="$ROOT/dist/deb-root"

if [[ ! -f "$BIN" ]]; then
  echo "missing binary: $BIN (run cargo build --release first)" >&2
  exit 1
fi

rm -rf "$STAGE"
mkdir -p "$STAGE/DEBIAN" \
         "$STAGE/usr/bin" \
         "$STAGE/usr/share/applications" \
         "$STAGE/usr/share/icons/hicolor/128x128/apps" \
         "$STAGE/usr/share/icons/hicolor/256x256/apps" \
         "$STAGE/usr/share/icons/hicolor/512x512/apps"

install -Dm755 "$BIN" "$STAGE/usr/bin/splashhound"
install -Dm644 "$ROOT/packaging/linux/splashhound.desktop" \
  "$STAGE/usr/share/applications/splashhound.desktop"
install -Dm644 "$ROOT/src-tauri/icons/128x128.png" \
  "$STAGE/usr/share/icons/hicolor/128x128/apps/splashhound.png"
install -Dm644 "$ROOT/src-tauri/icons/128x128@2x.png" \
  "$STAGE/usr/share/icons/hicolor/256x256/apps/splashhound.png"
install -Dm644 "$ROOT/src-tauri/icons/icon.png" \
  "$STAGE/usr/share/icons/hicolor/512x512/apps/splashhound.png"

INSTALLED_SIZE="$(du -sk "$STAGE" | cut -f1)"

cat > "$STAGE/DEBIAN/control" <<EOF
Package: splashhound
Version: ${VERSION}
Section: video
Priority: optional
Architecture: amd64
Installed-Size: ${INSTALLED_SIZE}
Depends: libwebkit2gtk-4.1-0, libgtk-3-0t64 | libgtk-3-0, libsoup-3.0-0, libayatana-appindicator3-1
Recommends: ffmpeg
Suggests: xdg-utils
Maintainer: Leetdog <buddyLeetdog@users.noreply.github.com>
Homepage: https://github.com/buddyLeetdog/splashhound
Description: Local-first video library categorizer and viewer
 Splashhound indexes a folder of videos into SQLite, generates preview
 frames, and supports tags, toys, flags, star ratings, duplicate detection
 and local playback.
EOF

mkdir -p "$ROOT/dist"
DEB="$ROOT/dist/splashhound_${VERSION}_amd64.deb"
dpkg-deb --build --root-owner-group "$STAGE" "$DEB"
echo "built $DEB"
