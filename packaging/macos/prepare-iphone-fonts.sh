#!/usr/bin/env bash
# Prepare the external craft-fonts input; this script does not build or install FilmCraft.
set -euo pipefail

SOURCE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FONT_ROOT="${CRAFT_FONTS_DIR:-$SOURCE_ROOT/../craft-fonts}"
FONT_REV=38062f4b4a0be4346d07a928408da21602545e9e
CRAFT_REV=3d9ab3a132ed0eff457e424226a435a901da30d3
FONT_HASH=5ac072b1f16adecb9ab44891f756bd689b5760a02c0c1b14f9b7afe2f68ebbe9
FONT_URL="https://raw.githubusercontent.com/googlefonts/roboto-2/$FONT_REV/src/hinted/Roboto-BlackItalic.ttf"
FONT_SOURCE="https://github.com/googlefonts/roboto-2/blob/$FONT_REV/src/hinted/Roboto-BlackItalic.ttf"
MANIFEST_LINE="Roboto | Black Italic | fonts/roboto/Roboto-BlackItalic.ttf | Latn,Cyrl,Grek | Apache-2.0 | fonts/roboto/LICENSE.txt | $FONT_HASH | $FONT_SOURCE"

mkdir -p "$(dirname "$FONT_ROOT")"
FONT_STAGE="$(mktemp -d "$(dirname "$FONT_ROOT")/.filmcraft-fonts.XXXXXX")"
trap 'rm -rf "$FONT_STAGE"' EXIT

if [ ! -e "$FONT_ROOT" ]; then
  git clone https://github.com/storytold/craft-fonts.git "$FONT_STAGE/craft-fonts"
  git -C "$FONT_STAGE/craft-fonts" checkout --detach "$CRAFT_REV"
  mv "$FONT_STAGE/craft-fonts" "$FONT_ROOT"
fi
if [ ! -f "$FONT_ROOT/fonts/manifest.txt" ]; then
  echo "Missing $FONT_ROOT/fonts/manifest.txt. Set CRAFT_FONTS_DIR to a craft-fonts checkout." >&2
  exit 1
fi

FONT_FILE="$FONT_ROOT/fonts/roboto/Roboto-BlackItalic.ttf"
if [ -f "$FONT_FILE" ]; then
  ACTUAL_HASH="$(shasum -a 256 "$FONT_FILE" | awk '{print $1}')"
  if [ "$ACTUAL_HASH" != "$FONT_HASH" ]; then
    echo "Existing Roboto font differs from the pinned file; leaving it unchanged: $FONT_FILE" >&2
    exit 1
  fi
else
  curl --fail --location --proto '=https' --tlsv1.2 --retry 3 "$FONT_URL" -o "$FONT_STAGE/Roboto-BlackItalic.ttf"
  ACTUAL_HASH="$(shasum -a 256 "$FONT_STAGE/Roboto-BlackItalic.ttf" | awk '{print $1}')"
  if [ "$ACTUAL_HASH" != "$FONT_HASH" ]; then
    echo "Roboto download failed SHA-256 verification." >&2
    exit 1
  fi
fi
if grep -Fq 'fonts/roboto/Roboto-BlackItalic.ttf' "$FONT_ROOT/fonts/manifest.txt" && \
   ! grep -Fxq "$MANIFEST_LINE" "$FONT_ROOT/fonts/manifest.txt"; then
  echo "The manifest has a different Roboto entry; leaving it unchanged." >&2
  exit 1
fi

mkdir -p "$FONT_ROOT/fonts/roboto"
if [ ! -f "$FONT_FILE" ]; then
  mv "$FONT_STAGE/Roboto-BlackItalic.ttf" "$FONT_FILE"
fi
cp "$SOURCE_ROOT/packaging/fonts/LICENSE-Roboto.txt" "$FONT_ROOT/fonts/roboto/LICENSE.txt"
cp "$SOURCE_ROOT/packaging/fonts/Roboto-BlackItalic-attribution.txt" "$FONT_FILE.attribution"
if ! grep -Fxq "$MANIFEST_LINE" "$FONT_ROOT/fonts/manifest.txt"; then
  printf '\n%s\n' "$MANIFEST_LINE" >> "$FONT_ROOT/fonts/manifest.txt"
fi
if ! grep -Fq 'fonts/roboto/Roboto-BlackItalic.ttf' "$FONT_ROOT/ATTRIBUTION.md"; then
  printf '\n| `fonts/roboto/Roboto-BlackItalic.ttf` | Google Inc.; Christian Robertson | [Pinned official source](%s) | Apache-2.0 |\n' "$FONT_SOURCE" >> "$FONT_ROOT/ATTRIBUTION.md"
fi
echo "Ready: $FONT_FILE (verified SHA-256). FilmCraft has not been built or installed."
