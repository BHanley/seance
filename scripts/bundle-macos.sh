#!/usr/bin/env bash
# Build seance and wrap it in a macOS .app so it launches from Finder,
# Spotlight, and the Dock — not just from a terminal.
#
#   ./scripts/bundle-macos.sh              # build + install to /Applications
#   ./scripts/bundle-macos.sh --user       # install to ~/Applications instead
#   ./scripts/bundle-macos.sh --no-build   # bundle whatever is in target/release
#   ./scripts/bundle-macos.sh --dest DIR   # install somewhere else entirely
#   ./scripts/bundle-macos.sh --cli-dir DIR # CLI link (default ~/.local/bin)
#   ./scripts/bundle-macos.sh --with-skills # install ChatGPT/Codex + Claude skill
#   ./scripts/bundle-macos.sh --app-only DIR # only write DIR/Seance.app: no install,
#                                            # no links, no LaunchServices (release-macos.sh)
#
# This is the mac build command: the bundle holds a *copy* of the binary, so a
# bare `cargo build --release` leaves the app stale. Re-run this instead — with
# a warm target dir the whole thing is a few seconds.
#
# A copy, not a symlink into target/, because the executable of a signed bundle
# has to live inside it. Ad-hoc signing is what lets the app keep its identity
# (and its Keychain/TCC grants) across reinstalls.
#
# The icon is generated from assets/icons/seance-macos-1024.png with stock
# sips + iconutil — macOS ships no SVG rasterizer, so the PNG is committed
# rather than rendered here.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "bundle-macos.sh only runs on macOS (this is $(uname -s))" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="Seance"
BUNDLE_ID="xyz.ham.seance"
ICON_SRC="$ROOT/assets/icons/seance-macos-1024.png"

DO_BUILD=1
DEST="/Applications"
CLI_DIR="$HOME/.local/bin"
INSTALL_SKILLS=0
APP_ONLY=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-build) DO_BUILD=0; shift ;;
    --user)     DEST="$HOME/Applications"; shift ;;
    --dest)     DEST="${2:?--dest needs a directory}"; shift 2 ;;
    --cli-dir)  CLI_DIR="${2:?--cli-dir needs a directory}"; shift 2 ;;
    --with-skills) INSTALL_SKILLS=1; shift ;;
    --app-only) APP_ONLY="${2:?--app-only needs a directory}"; shift 2 ;;
    -h|--help)  sed -n '2,19p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

# The workspace version — the first `version = "…"` in Cargo.toml, which is
# [workspace.package]. The [package] below it inherits with version.workspace.
VERSION="$(awk -F'"' '/^version = "/ {print $2; exit}' "$ROOT/Cargo.toml")"
if [[ -z "$VERSION" ]]; then
  echo "could not read version out of Cargo.toml" >&2
  exit 1
fi

if [[ $DO_BUILD -eq 1 ]]; then
  echo "building seance $VERSION (release)…"
  (cd "$ROOT" && cargo build --release)
fi

BIN="$ROOT/target/release/seance"
if [[ ! -x "$BIN" ]]; then
  echo "no release binary at $BIN — drop --no-build, or run cargo build --release" >&2
  exit 1
fi

# Assemble in a staging dir and swap at the end, so a failure halfway through
# can't leave a half-written app where a working one used to be.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
APP="$STAGE/$APP_NAME.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

echo "rendering icon…"
ICONSET="$STAGE/seance.iconset"
mkdir -p "$ICONSET"
# name:pixels — the sizes iconutil expects, @2x included.
for spec in \
  16x16:16 16x16@2x:32 32x32:32 32x32@2x:64 \
  128x128:128 128x128@2x:256 256x256:256 256x256@2x:512 \
  512x512:512 512x512@2x:1024
do
  sips -z "${spec#*:}" "${spec#*:}" "$ICON_SRC" \
    --out "$ICONSET/icon_${spec%:*}.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/seance.icns"

cp "$BIN" "$APP/Contents/MacOS/seance"
mkdir -p "$APP/Contents/Resources/skills"
cp -R "$ROOT/skills/seance-control" "$APP/Contents/Resources/skills/"

# Invoke (the phone chat), optional. Its code is not part of this repository:
# the builder points SEANCE_INVOKE_DIR at a folder holding `seance-chat` and
# `seance-chat-web/`, and SEANCE_PYTHON_DIR at a runtime made by
# scripts/prepare-invoke-python.sh. Unset, the app is built without it. Set but
# wrong, the build stops here rather than ship an app that silently lacks it.
if [[ -n "${SEANCE_INVOKE_DIR:-}${SEANCE_PYTHON_DIR:-}" ]]; then
  [[ -f "${SEANCE_INVOKE_DIR:-}/seance-chat" ]] || {
    echo "SEANCE_INVOKE_DIR='${SEANCE_INVOKE_DIR:-}' has no seance-chat file" >&2; exit 1; }
  [[ -d "$SEANCE_INVOKE_DIR/seance-chat-web" ]] || {
    echo "SEANCE_INVOKE_DIR='$SEANCE_INVOKE_DIR' has no seance-chat-web folder" >&2; exit 1; }
  [[ -x "${SEANCE_PYTHON_DIR:-}/bin/python3" ]] || {
    echo "SEANCE_PYTHON_DIR='${SEANCE_PYTHON_DIR:-}' has no bin/python3 (run scripts/prepare-invoke-python.sh)" >&2; exit 1; }
  INV="$APP/Contents/Resources/invoke"
  mkdir -p "$INV"
  cp -R "$SEANCE_PYTHON_DIR" "$INV/python"
  cp "$SEANCE_INVOKE_DIR/seance-chat" "$INV/seance-chat"
  cp -R "$SEANCE_INVOKE_DIR/seance-chat-web" "$INV/seance-chat-web"
  chmod +x "$INV/seance-chat"
  echo "bundled Invoke ($(du -sh "$INV" | cut -f1))"
fi
printf 'APPL????' > "$APP/Contents/PkgInfo"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>              <string>$APP_NAME</string>
  <key>CFBundleDisplayName</key>       <string>$APP_NAME</string>
  <key>CFBundleIdentifier</key>        <string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key>        <string>seance</string>
  <key>CFBundleIconFile</key>          <string>seance</string>
  <key>CFBundlePackageType</key>       <string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key>           <string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key>    <string>11.0</string>
  <key>LSApplicationCategoryType</key> <string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key>   <true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key>  <string>MIT · github.com/zackham/seance</string>
</dict>
</plist>
PLIST

# Release signing (optional). With SEANCE_SIGN_IDENTITY (a certificate name made
# by scripts/create-signing-identity.sh) and its keychain, every build carries
# the SAME code identity, so macOS keeps its permission grants across updates.
# Unset, the app is ad-hoc signed as below. The keychain's password is read
# from SEANCE_SIGN_KEYCHAIN_PASSWORD_FILE (default
# ~/.config/seance-signing/keychain-password); nothing secret is in this repo.
SIGNED=0
if [[ -n "${SEANCE_SIGN_IDENTITY:-}" ]]; then
  SIGN_KC="${SEANCE_SIGN_KEYCHAIN:-$HOME/Library/Keychains/seance-signing.keychain-db}"
  SIGN_PW_FILE="${SEANCE_SIGN_KEYCHAIN_PASSWORD_FILE:-$HOME/.config/seance-signing/keychain-password}"
  [[ -f "$SIGN_KC" ]] || { echo "SEANCE_SIGN_IDENTITY is set but there is no keychain at $SIGN_KC" >&2; exit 1; }
  [[ -f "$SIGN_PW_FILE" ]] || { echo "no keychain password file at $SIGN_PW_FILE" >&2; exit 1; }
  security unlock-keychain -p "$(cat "$SIGN_PW_FILE")" "$SIGN_KC"
  # codesign only finds an identity in the user's keychain search list, so the
  # signing keychain joins it for this run and the list is put back on exit.
  KC_LIST=()
  while IFS= read -r k; do k="${k#"${k%%[![:space:]]*}"}"; k="${k//\"/}"; [[ -n "$k" ]] && KC_LIST+=("$k"); done \
    < <(security list-keychains -d user)
  security list-keychains -d user -s "$SIGN_KC" ${KC_LIST[@]+"${KC_LIST[@]}"}
  trap 'security list-keychains -d user -s ${KC_LIST[@]+"${KC_LIST[@]}"}; rm -rf "$STAGE"' EXIT
  LEAF="$(security find-certificate -c "$SEANCE_SIGN_IDENTITY" -Z "$SIGN_KC" | awk '/SHA-1 hash:/ {print $3}')"
  [[ -n "$LEAF" ]] || { echo "no certificate named '$SEANCE_SIGN_IDENTITY' in $SIGN_KC" >&2; exit 1; }
  # Stable identity: the designated requirement is the identifier plus the
  # certificate's own hash, not this build's hash.
  sign_one() {   # sign_one PATH IDENTIFIER
    codesign --force --keychain "$SIGN_KC" -s "$SEANCE_SIGN_IDENTITY" --identifier "$2" \
      -r="designated => identifier \"$2\" and certificate leaf = H\"$LEAF\"" --timestamp=none "$1"
  }
  echo "signing with '$SEANCE_SIGN_IDENTITY' ($LEAF)…"
  # Invoke's Python runtime: every Mach-O inside it.
  if [[ -d "$APP/Contents/Resources/invoke" ]]; then
    while IFS= read -r f; do
      file "$f" | grep -q 'Mach-O' && sign_one "$f" "$BUNDLE_ID.invoke.$(basename "$f")"
    done < <(find "$APP/Contents/Resources/invoke" -type f)
  fi
  # The daemon: signed on its own (a bundle's main executable is bound to the
  # bundle's Info.plist, so a loose copy of that would not verify), and copied
  # out of here by the app. Same identifier and requirement as the app.
  cp "$BIN" "$APP/Contents/Resources/seance-daemon"
  sign_one "$APP/Contents/Resources/seance-daemon" "$BUNDLE_ID"
  sign_one "$APP" "$BUNDLE_ID"
  codesign --verify --deep --strict "$APP"
  codesign -d -r- "$APP" 2>&1 | sed -n 's/^# //p' | sed 's/^/  /'
  SIGNED=1
fi

# Ad-hoc signature. Cargo already ad-hoc signs the bare binary on Apple
# Silicon, but that signature covers the executable alone — everything added
# above is outside it, and an inconsistent bundle is what makes macOS refuse
# to launch a freshly-copied app.
if [[ $SIGNED -eq 0 ]]; then
echo "signing (ad-hoc)…"
codesign --force --sign - --timestamp=none "$APP" 2>&1 | sed 's/^/  /' || true
fi

if [[ -n "$APP_ONLY" ]]; then
  mkdir -p "$APP_ONLY"
  rm -rf "$APP_ONLY/$APP_NAME.app"
  cp -R "$APP" "$APP_ONLY/$APP_NAME.app"
  echo "wrote $APP_ONLY/$APP_NAME.app ($VERSION)"
  exit 0
fi

mkdir -p "$DEST"
TARGET="$DEST/$APP_NAME.app"
if [[ -e "$TARGET" && ! -w "$DEST" ]]; then
  echo "no write permission on $DEST — try --user (installs to ~/Applications)" >&2
  exit 1
fi
rm -rf "$TARGET"
cp -R "$APP" "$TARGET"

# Tell LaunchServices about it now rather than whenever it next rescans, so
# Spotlight and `open -a Seance` work the moment this script returns.
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
[[ -x "$LSREGISTER" ]] && "$LSREGISTER" -f "$TARGET" || true

install_link() {
  local source="$1" link="$2"
  if [[ -e "$link" && ! -L "$link" ]]; then
    echo "not replacing existing file/directory: $link" >&2
    return 1
  fi
  mkdir -p "$(dirname "$link")"
  ln -sfn "$source" "$link"
}

install_link "$TARGET/Contents/MacOS/seance" "$CLI_DIR/seance"
# Retarget the earlier cargo-build link so an existing PATH doesn't keep using it.
if [[ -L "$HOME/.cargo/bin/seance" ]] && \
   [[ "$(readlink "$HOME/.cargo/bin/seance")" == "$ROOT/target/release/seance" ]]; then
  install_link "$TARGET/Contents/MacOS/seance" "$HOME/.cargo/bin/seance"
fi
if [[ $INSTALL_SKILLS -eq 1 ]]; then
  for skill_root in "$HOME/.agents/skills" "$HOME/.claude/skills"; do
    install_link "$TARGET/Contents/Resources/skills/seance-control" "$skill_root/seance-control"
  done
fi

echo
echo "installed $TARGET ($VERSION)"
echo "cli: $CLI_DIR/seance (uses the app's saved connection, even with the GUI closed)"
echo "launch: open -a $APP_NAME   ·   or Spotlight → \"$APP_NAME\""
