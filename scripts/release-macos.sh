#!/usr/bin/env bash
# Pack Seance.app into the two files people download: a DMG (drag to
# Applications) and a zip. Installs nothing.
#
#   ./scripts/release-macos.sh                 # build, write to target/release-macos
#   ./scripts/release-macos.sh --out DIR       # write somewhere else
#   ./scripts/release-macos.sh --no-build      # package whatever is in target/release
#
# Output: DIR/Seance-<version>.dmg and DIR/Seance-<version>.zip, plus
# DIR/SHA256SUMS. The version comes from Cargo.toml. The app is ad-hoc signed
# only, so a downloaded copy is blocked by Gatekeeper on first open; the
# first-open steps go with the download.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "release-macos.sh only runs on macOS (this is $(uname -s))" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/target/release-macos"
BUILD_ARGS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)      OUT="${2:?--out needs a directory}"; shift 2 ;;
    --no-build) BUILD_ARGS=(--no-build); shift ;;
    -h|--help)  sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

VERSION="$(awk -F'"' '/^version = "/ {print $2; exit}' "$ROOT/Cargo.toml")"
[[ -n "$VERSION" ]] || { echo "could not read version out of Cargo.toml" >&2; exit 1; }

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

"$ROOT/scripts/bundle-macos.sh" ${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"} --app-only "$STAGE/app"

mkdir -p "$OUT"
DMG="$OUT/Seance-$VERSION.dmg"
ZIP="$OUT/Seance-$VERSION.zip"
rm -f "$DMG" "$ZIP"

# DMG: the app next to an Applications link, so install is one drag.
mkdir -p "$STAGE/dmg"
cp -R "$STAGE/app/Seance.app" "$STAGE/dmg/Seance.app"
ln -s /Applications "$STAGE/dmg/Applications"
hdiutil create -quiet -volname "Seance $VERSION" -srcfolder "$STAGE/dmg" -ov -format UDZO "$DMG"

# zip: ditto keeps the bundle's symlinks, permissions and signature intact.
ditto -c -k --keepParent "$STAGE/app/Seance.app" "$ZIP"

(cd "$OUT" && shasum -a 256 "$(basename "$DMG")" "$(basename "$ZIP")" > SHA256SUMS)

echo
echo "release $VERSION:"
ls -lh "$DMG" "$ZIP" | awk '{print "  " $5 "  " $9}'
echo "  $OUT/SHA256SUMS"
