#!/bin/bash
# WhatsMine — one build command.
#
#   ./build.sh                      # desktop app, release (target/dx/whatsmine/release/...)
#   ./build.sh desktop-debug        # desktop app, debug
#   ./build.sh android              # Android debug APK
#   ./build.sh android install      # ... and install (as an update) on the connected device
#   ./build.sh android install SERIAL
#   ./build.sh android-release      # Android release APK (unsigned)
#   ./build.sh cli                  # headless test peer (target/release/whatsmine-cli)
#   ./build.sh test                 # fmt check, clippy, all tests
#
# Android is built like AlarmFree: `dx build` makes the Rust library and the
# Gradle project, then mobile-sentinel's `build_sentinel` wires in the Kotlin
# for exactly the capabilities the app enables (apps/whatsmine/Cargo.toml),
# copies the icon, and runs Gradle. It uses the sibling ../mobile-sentinel
# checkout, pinned to the published version.
#
# Android needs ANDROID_HOME, ANDROID_NDK_HOME and JAVA_HOME. If they are not
# set, the defaults below (this machine's Android Studio layout) are used.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"
export PATH="$HOME/.cargo/bin:$PATH"

APP_DIR="apps/whatsmine"
SENTINEL_DIR="$SCRIPT_DIR/../mobile-sentinel"

android_env() {
  : "${ANDROID_HOME:=${LOCALAPPDATA}\Android\Sdk}"
  : "${ANDROID_NDK_HOME:=${ANDROID_HOME}\ndk\28.2.13676358}"
  : "${JAVA_HOME:=C:\Program Files\Microsoft\jdk-17.0.20.101-hotspot}"
  export ANDROID_HOME ANDROID_NDK_HOME JAVA_HOME
  ADB="${ANDROID_HOME}/platform-tools/adb.exe"
}

MODE="${1:-desktop}"
case "$MODE" in
  desktop)
    (cd "$APP_DIR" && dx build --platform desktop --release)
    ;;
  desktop-debug)
    (cd "$APP_DIR" && dx build --platform desktop)
    ;;
  android | android-release)
    android_env
    if [ ! -f "$SENTINEL_DIR/Cargo.toml" ]; then
      echo "ERROR: mobile-sentinel not found at $SENTINEL_DIR (expected as a sibling of this repo)." >&2
      exit 1
    fi
    RELEASE=""
    [ "$MODE" = "android-release" ] && RELEASE="--release"

    # An icon in another format left in the generated project by an earlier run
    # would clash with the one dx writes (duplicate resources). Ours is .webp.
    rm -f target/dx/whatsmine/*/android/app/app/src/main/res/mipmap-*/ic_launcher.png

    echo "=== [1/2] Building the Rust library (dx build) ==="
    (cd "$APP_DIR" && dx build --platform android --target aarch64-linux-android --package whatsmine $RELEASE)

    echo "=== [2/2] Wiring mobile-sentinel + icon + building the APK ==="
    # shellcheck disable=SC2086
    cargo run --manifest-path "$SENTINEL_DIR/Cargo.toml" --bin build_sentinel -- --app whatsmine $RELEASE

    if [ -n "$RELEASE" ]; then
      echo "Release APK: target/dx/whatsmine/release/android/app/app/build/outputs/apk/release/"
    else
      APK="target/dx/whatsmine/debug/android/app/app/build/outputs/apk/debug/app-debug.apk"
      echo "Debug APK: $APK"
      if [ "${2:-}" = "install" ]; then
        # -r keeps the app's data (chats, identity) when updating.
        if [ -n "${3:-}" ]; then
          "$ADB" -s "$3" install -r "$APK"
        else
          "$ADB" install -r "$APK"
        fi
      fi
    fi
    ;;
  cli)
    cargo build --release -p whatsmine-cli
    ;;
  test)
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --features whatsmine/desktop -- -D warnings
    cargo test --workspace
    ;;
  *)
    sed -n '2,12p' "$0"
    exit 1
    ;;
esac
