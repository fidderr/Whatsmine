#!/bin/bash
# WhatsMine — one build command.
#
#   ./build.sh                      # desktop app, release (target/dx/whatsmine/release/...)
#   ./build.sh desktop-debug        # desktop app, debug
#   ./build.sh desktop-test         # desktop app with the test hooks (devtest/)
#   ./build.sh android              # Android debug APK
#   ./build.sh android install      # ... and install (as an update) on the connected device
#   ./build.sh android install SERIAL
#   ./build.sh android-test         # debug APK with the test hooks, under its own app id,
#                                   # so a real install and its data are never touched
#   ./build.sh android-test install SERIAL
#   ./build.sh android-release      # Android release APK (unsigned)
#   ./build.sh aab                  # release bundle for Play; then ./sign-aab.sh
#   ./build.sh cli                  # headless test peer (target/release/whatsmine-cli)
#   ./build.sh test                 # fmt check, clippy, all tests
#
# Phones are arm64, the only ABI the app ships for. Emulators need another:
#   TARGET=x86_64-linux-android ./build.sh android-test
#
# A release the test scenarios can drive (same R8 and optimisation as the real thing, but
# debuggable, signed with the debug key, and with the test hooks and test app id):
#   RELEASE_TEST=1 ./build.sh android-release
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
TARGET="${TARGET:-aarch64-linux-android}"
TEST_ID="com.fidderr.whatsmine.test"

android_env() {
  : "${ANDROID_HOME:=${LOCALAPPDATA}\Android\Sdk}"
  : "${ANDROID_NDK_HOME:=${ANDROID_HOME}\ndk\28.2.13676358}"
  : "${JAVA_HOME:=C:\Program Files\Microsoft\jdk-17.0.20.101-hotspot}"
  export ANDROID_HOME ANDROID_NDK_HOME JAVA_HOME
  ADB="${ANDROID_HOME}/platform-tools/adb.exe"
}

# The Gradle project dx made, for what its build type needs and dx cannot be told.
patch_release_gradle() {
  local gradle="target/dx/whatsmine/release/android/app/app/build.gradle.kts"
  [ -f "$gradle" ] || { echo "ERROR: $gradle not found" >&2; exit 1; }

  # dx hardcodes a low versionCode; Play needs a higher one for every upload. Pin one in
  # Dioxus.toml as `version_code = N`, or it follows the Cargo version.
  local code version major minor patch
  code=$(sed -nE 's/^[[:space:]]*version_code[[:space:]]*=[[:space:]]*([0-9]+).*/\1/p' "$APP_DIR/Dioxus.toml" | head -1)
  version=$(sed -nE 's/^version = "([0-9.]+)".*/\1/p' Cargo.toml | head -1)
  if [ -z "$code" ]; then
    IFS=. read -r major minor patch <<<"$version"
    code=$((major * 10000 + minor * 100 + patch))
  fi
  sed -i -E "s/^( *versionCode *= *).*/\1$code/" "$gradle"
  sed -i -E "s/^( *versionName *= *).*/\1\"$version\"/" "$gradle"
  echo "  [play] versionCode=$code versionName=$version"

  # Full native symbols in the bundle's metadata, for Play's crash reports.
  grep -q "debugSymbolLevel" "$gradle" ||
    sed -i '/getByName("release") {/a\            ndk {\n                debugSymbolLevel = "FULL"\n            }' "$gradle"
  # Ship a stripped libmain.so. dx keeps the symbols in, which users would download.
  sed -i '/jniLibs.keepDebugSymbols.add/d' "$gradle"
  # AGP strips with its default NDK unless told, and ships it unstripped if that one is missing.
  local ndk
  ndk="$(basename "${ANDROID_NDK_HOME//\\//}")"
  grep -q "ndkVersion" "$gradle" || sed -i "s/^android {/android {\n    ndkVersion = \"$ndk\"/" "$gradle"

  if [ -n "${RELEASE_TEST:-}" ] && ! sed -n '/getByName("release")/,$p' "$gradle" | grep -q "isDebuggable = true"; then
    sed -i '/getByName("release") {/a\            isDebuggable = true\n            signingConfig = signingConfigs.getByName("debug")' "$gradle"
  fi
}

# A test build under its own id and name, with its own link scheme, next to a real install.
# Both files are back as they were after this script, also when it fails.
use_test_identity() {
  cp "$APP_DIR/Dioxus.toml" "$APP_DIR/Dioxus.toml.real"
  cp "$APP_DIR/sentinel.toml" "$APP_DIR/sentinel.toml.real"
  trap restore_identity EXIT
  sed -i "s/^identifier = .*/identifier = \"$TEST_ID\"/" "$APP_DIR/Dioxus.toml"
  sed -i 's/^link_scheme = .*/link_scheme = "whatsminetest"/' "$APP_DIR/sentinel.toml"
}
restore_identity() {
  mv -f "$APP_DIR/Dioxus.toml.real" "$APP_DIR/Dioxus.toml"
  mv -f "$APP_DIR/sentinel.toml.real" "$APP_DIR/sentinel.toml"
}

# Refuse an APK that would not be what was asked for: the wrong ABI, or a page size Play refuses.
check_apk() {
  local apk="$1" abi
  case "$TARGET" in
    aarch64*) abi=arm64-v8a ;;
    x86_64*) abi=x86_64 ;;
    *) return 0 ;;
  esac
  local libs
  libs=$(unzip -l "$apk" | grep -o 'lib/[^/]*/libmain.so' || true)
  [ "$libs" = "lib/$abi/libmain.so" ] || { echo "ERROR: expected only lib/$abi/libmain.so in $apk, found: $libs" >&2; exit 1; }
}

MODE="${1:-desktop}"
case "$MODE" in
  desktop)
    (cd "$APP_DIR" && dx build --platform desktop --release)
    ;;
  desktop-debug)
    (cd "$APP_DIR" && dx build --platform desktop)
    ;;
  desktop-test)
    (cd "$APP_DIR" && dx build --platform desktop --features test-hooks)
    ;;
  android | android-test | android-release | aab)
    android_env
    if [ ! -f "$SENTINEL_DIR/Cargo.toml" ]; then
      echo "ERROR: mobile-sentinel not found at $SENTINEL_DIR (expected as a sibling of this repo)." >&2
      exit 1
    fi
    RELEASE=""
    SENTINEL_FLAGS=""
    FEATURES=""
    [ "$MODE" = "android-release" ] && RELEASE="--release" && SENTINEL_FLAGS="--release"
    [ "$MODE" = "aab" ] && RELEASE="--release" && SENTINEL_FLAGS="--release --aab"
    if [ "$MODE" = "android-test" ] || [ -n "${RELEASE_TEST:-}" ]; then
      FEATURES="--features test-hooks"
      use_test_identity
    fi

    # An icon in another format left in the generated project by an earlier run
    # would clash with the one dx writes (duplicate resources). Ours is .webp.
    rm -f target/dx/whatsmine/*/android/app/app/src/main/res/mipmap-*/ic_launcher.png
    # The library of another ABI, left by an earlier build for it, would ship along.
    rm -rf target/dx/whatsmine/*/android/app/app/src/main/jniLibs/*

    echo "=== [1/2] Building the Rust library (dx build, $TARGET) ==="
    # shellcheck disable=SC2086
    (cd "$APP_DIR" && dx build --platform android --target "$TARGET" --package whatsmine $RELEASE $FEATURES)

    if [ -n "$FEATURES" ]; then
      PROFILE=debug
      [ -n "$RELEASE" ] && PROFILE=release
      sed -i 's/>Whatsmine</>WhatsMine test</' "target/dx/whatsmine/$PROFILE/android/app/app/src/main/res/values/strings.xml"
    fi
    if [ -n "$RELEASE" ]; then
      patch_release_gradle
    fi

    echo "=== [2/2] Wiring mobile-sentinel + icon + building the app ==="
    # shellcheck disable=SC2086
    cargo run --manifest-path "$SENTINEL_DIR/Cargo.toml" --bin build_sentinel -- --app whatsmine $SENTINEL_FLAGS

    case "$MODE" in
      aab)
        echo "Bundle: target/dx/whatsmine/release/android/app/app/build/outputs/bundle/release/app-release.aab"
        echo "Sign it with ./sign-aab.sh"
        ;;
      android-release)
        APK="target/dx/whatsmine/release/android/app/app/build/outputs/apk/release/app-release-unsigned.apk"
        [ -n "${RELEASE_TEST:-}" ] && APK="target/dx/whatsmine/release/android/app/app/build/outputs/apk/release/app-release.apk"
        check_apk "$APK"
        echo "Release APK: $APK"
        ;;
      *)
        APK="target/dx/whatsmine/debug/android/app/app/build/outputs/apk/debug/app-debug.apk"
        check_apk "$APK"
        echo "Debug APK: $APK"
        if [ "${2:-}" = "install" ]; then
          # -r keeps the app's data (chats, identity) when updating.
          if [ -n "${3:-}" ]; then
            "$ADB" -s "$3" install -r "$APK"
          else
            "$ADB" install -r "$APK"
          fi
        fi
        ;;
    esac
    ;;
  cli)
    cargo build --release -p whatsmine-cli
    ;;
  test)
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --features whatsmine/desktop,whatsmine/test-hooks -- -D warnings
    cargo test --workspace
    ;;
  *)
    sed -n '2,20p' "$0"
    exit 1
    ;;
esac
