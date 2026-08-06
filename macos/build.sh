#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
APP="$ROOT/target/macos/Minimacs.app"
CONTENTS="$APP/Contents"
MACOS="$CONTENTS/MacOS"

cd "$ROOT"
# Keep Rust, C grammar objects, Swift, and the bundle metadata on one minimum
# deployment target so cached parser objects never raise linker-version warnings.
export MACOSX_DEPLOYMENT_TARGET=12.0
cargo build --release --lib --no-default-features

rm -rf "$APP"
mkdir -p "$MACOS" "$CONTENTS/Resources"
cp macos/Info.plist "$CONTENTS/Info.plist"

swiftc \
    -O \
    -whole-module-optimization \
    -target "$(uname -m)-apple-macosx${MACOSX_DEPLOYMENT_TARGET}" \
    -import-objc-header macos/include/minimacs_native.h \
    macos/MinimacsApp.swift \
    target/release/libminimacs.a \
    -framework AppKit \
    -framework CoreText \
    -framework Security \
    -o "$MACOS/Minimacs"

# An ad-hoc signature makes the locally built bundle behave like a normal app.
# Distribution builds can replace this with a Developer ID signature.
codesign --force --sign - "$APP" >/dev/null
printf '%s\n' "$APP"
