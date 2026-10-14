#!/usr/bin/env bash
# Local Apple-silicon build with native HDR preview, Dolby Vision 8.4 export and multilingual Whisper.
# Add --install to replace /Applications/FilmCraft.app after a successful build/sign.
set -euo pipefail

INSTALL=0
case "${1:-}" in
  "") ;;
  --install) INSTALL=1 ;;
  *) echo "Usage: bash packaging/macos/build-iphone-hdr.sh [--install]" >&2; exit 2 ;;
esac
if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  echo "This script is for an Apple-silicon Mac." >&2
  exit 1
fi

export FILMCRAFT_VERSION=0.7.2-iphone-hdr.9
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
export CRAFT_FONTS_DIR="${CRAFT_FONTS_DIR:-$ROOT/../craft-fonts}"
export CRAFT_FONTS_REQUIRED=1
if [ ! -f "$CRAFT_FONTS_DIR/fonts/roboto/Roboto-BlackItalic.ttf" ] || \
   [ ! -f "$CRAFT_FONTS_DIR/fonts/roboto/LICENSE.txt" ] || \
   [ ! -f "$CRAFT_FONTS_DIR/fonts/manifest.txt" ]; then
  echo "Не найден шрифт субтитров Roboto Black Italic в $CRAFT_FONTS_DIR. Выполните bash packaging/macos/prepare-iphone-fonts.sh." >&2
  exit 1
fi
export PATH="$HOME/.cargo/bin:$PATH"
export MACOSX_DEPLOYMENT_TARGET=11.0
cd "$ROOT"
cargo build --release --locked -p filmcraft -p filmcraft-cli \
  --features filmcraft/whisper,filmcraft-cli/whisper

STAGE="$(mktemp -d "$CARGO_TARGET_DIR/iphone-hdr.XXXXXX")"
INSTALL_STAGE=""
BACKUP=""
cleanup() {
  if [ -n "$BACKUP" ] && [ -d "$BACKUP" ] && [ ! -e /Applications/FilmCraft.app ]; then
    mv "$BACKUP" /Applications/FilmCraft.app || true
  fi
  rm -rf "$STAGE"
  if [ -n "$INSTALL_STAGE" ]; then rm -rf "$INSTALL_STAGE"; fi
}
trap cleanup EXIT
APP="$STAGE/FilmCraft.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/Licenses/egui-wgpu"
cp "$CARGO_TARGET_DIR/release/filmcraft" "$APP/Contents/MacOS/FilmCraft"
cp "$ROOT/assets/app-icon/filmcraft.icns" "$APP/Contents/Resources/FilmCraft.icns"
cp "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$APP/Contents/Resources/Licenses/"
cp "$ROOT/vendor/egui-wgpu/LICENSE-MIT" "$ROOT/vendor/egui-wgpu/LICENSE-APACHE" "$APP/Contents/Resources/Licenses/egui-wgpu/"
copy_font_licences "$APP/Contents/Resources/Licenses"
cp "$CRAFT_FONTS_DIR/fonts/roboto/LICENSE.txt" "$APP/Contents/Resources/Licenses/LICENSE-Roboto.txt"
cp "$CRAFT_FONTS_DIR/fonts/roboto/Roboto-BlackItalic.ttf.attribution" "$APP/Contents/Resources/Licenses/Roboto-BlackItalic.attribution"
sed -e "s/@VERSION@/$VERSION/g" -e 's/@SHORT_VERSION@/0.7.2/g' \
  -e "s/@BUILD_SHA@/$FILMCRAFT_BUILD_SHA-iphone-hdr.9/g" \
  "$ROOT/packaging/macos/Info.plist.in" > "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleVersion 0.7.2.9' "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"
codesign --force --sign - --timestamp=none --options runtime \
  --entitlements "$ROOT/packaging/macos/entitlements.plist" "$APP/Contents/MacOS/FilmCraft"
codesign --force --sign - --timestamp=none --options runtime \
  --entitlements "$ROOT/packaging/macos/entitlements.plist" "$APP"
codesign --verify --strict "$APP"

OUTPUT="$DIST/FilmCraft-iPhone-HDR.app"
if [ -e "$OUTPUT" ]; then rm -rf "$OUTPUT"; fi
ditto "$APP" "$OUTPUT"
if [ "$INSTALL" = 1 ]; then
  INSTALL_STAGE="$(mktemp -d /Applications/.filmcraft-hdr.XXXXXX)"
  ditto "$APP" "$INSTALL_STAGE/FilmCraft.app"
  if [ -e /Applications/FilmCraft.app ]; then
    mkdir -p "$HOME/.Trash"
    BACKUP="$HOME/.Trash/FilmCraft-before-HDR-$(date +%Y%m%d-%H%M%S)-$$.app"
    mv /Applications/FilmCraft.app "$BACKUP"
  fi
  mv "$INSTALL_STAGE/FilmCraft.app" /Applications/FilmCraft.app
  echo "Готово: /Applications/FilmCraft.app заменён. Откройте FilmCraft заново."
else
  echo "Готово: $OUTPUT"
fi
