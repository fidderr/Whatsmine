#!/bin/bash
# WhatsMine — one build command.
#
#   ./build.sh                      # desktop app, release (target/dx/whatsmine/release/...)
#   ./build.sh desktop-debug        # desktop app, debug
#   ./build.sh android              # Android debug APK
#   ./build.sh android install      # ... and install on the connected device
#   ./build.sh android install SERIAL
#   ./build.sh android-release      # Android release APK (unsigned)
#   ./build.sh cli                  # headless test peer (target/release/whatsmine-cli)
#   ./build.sh test                 # fmt check, clippy, all tests
#
# Android needs ANDROID_HOME, ANDROID_NDK_HOME and JAVA_HOME. If they are not
# set, the defaults below (this machine's Android Studio layout) are used.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"
export PATH="$HOME/.cargo/bin:$PATH"

android_env() {
  : "${ANDROID_HOME:=${LOCALAPPDATA}\\Android\\Sdk}"
  : "${ANDROID_NDK_HOME:=${ANDROID_HOME}\\ndk\\28.2.13676358}"
  : "${JAVA_HOME:=C:\\Program Files\\Microsoft\\jdk-17.0.20.101-hotspot}"
  export ANDROID_HOME ANDROID_NDK_HOME JAVA_HOME
  ADB="${ANDROID_HOME}/platform-tools/adb.exe"
}

MODE="${1:-desktop}"
case "$MODE" in
  desktop)
    (cd app && dx build --platform desktop --release)
    ;;
  desktop-debug)
    (cd app && dx build --platform desktop)
    ;;
  android | android-release)
    android_env
    RELEASE=""
    [ "$MODE" = "android-release" ] && RELEASE="--release"
    (cd app && dx build --platform android --target aarch64-linux-android --package whatsmine $RELEASE)
    if [ "${2:-}" = "install" ]; then
      [ -n "$RELEASE" ] && { echo "install is only supported for debug builds"; exit 1; }
      APK="target/dx/whatsmine/debug/android/app/app/build/outputs/apk/debug/app-debug.apk"
      if [ -n "${3:-}" ]; then
        "$ADB" -s "$3" install -r "$APK"
      else
        "$ADB" install -r "$APK"
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
    sed -n '2,13p' "$0"
    exit 1
    ;;
esac
