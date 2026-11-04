#!/bin/bash
# sign-aab.sh
# Signs the unsigned AAB produced by ./build.sh aab (or workspace aab)
# and copies a clean signed AAB to the release/ folder so you can just copy it to Play Console.
#
# For internal testing on Play Store.
#
# Usage:
#   ./sign-aab.sh
#
# Requirements:
#   - keytool and jarsigner in PATH (usually from Android Studio JDK or your Java install)
#   - The AAB from a previous `./build.sh aab` (or `workspace aab`)
#
# The script will:
#   1. Find the latest unsigned app-release.aab from the dx build output.
#   2. If no upload keystore exists, generate one (upload-keystore.jks) and guide you.
#   3. Sign the AAB.
#   4. Copy the signed result to whatsmine/release/ with a timestamped name.
#
# IMPORTANT: Never commit the .jks file or passwords.
#
# WHAT IF I LOSE THE PASSWORDS / KEYSTORE?
# =========================================
# This is your UPLOAD KEY (not the final app signing key, because we use
# Google Play App Signing).
#
# - Before first upload to Play Console: Just delete the .jks and generate a new one.
#   Run the script again — it will create a fresh keystore.
#
# - After the app has been uploaded + Play App Signing is enabled:
#   You can request an "upload key reset" from Google Play support.
#   See: https://support.google.com/googleplay/android-developer/answer/7384423
#   The process involves proving ownership. It can take days to weeks and
#   you won't be able to publish new versions during that time.
#
# STRONGLY RECOMMENDED:
# - Store the .jks file in a password manager as an attachment.
# - Store the two passwords (keystore + key) in the same password manager entry.
# - Make an encrypted backup on at least two different devices/drives.
# - Write down the alias ("upload") and the fact that it was created with 2048-bit RSA.
#
# For future releases: you MUST keep using this exact same .jks file.
# Creating a "new" keystore later (even with the same password) = different certificate
# = you will need a painful upload key reset with Google support.
#
# Losing the upload key after going live is painful but recoverable.
# Losing it before going live is basically free — just generate a new one.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Support non-interactive signing via environment variables
# (useful for scripting or when you don't want to type passwords every time)
STORE_PASS="${AAB_SIGN_STOREPASS:-}"
KEY_PASS="${AAB_SIGN_KEYPASS:-}"

# Auto-detect Android Studio JDK on Windows (MINGW64 / Git Bash / MSYS)
# This makes it "just work" without manual PATH fiddling.
if ! command -v keytool >/dev/null 2>&1 || ! command -v jarsigner >/dev/null 2>&1; then
  for studio_path in \
    "/c/Program Files/Android/Android Studio/jbr/bin" \
    "/c/Program Files/Android/Android Studio/jre/bin" \
    "/c/Program Files (x86)/Android/Android Studio/jbr/bin" \
    "/c/Program Files (x86)/Android/Android Studio/jre/bin" \
    "$LOCALAPPDATA/Android/Sdk/jre/bin" \
    "$LOCALAPPDATA/Android/Sdk/ndk-bundle/prebuilt/windows-x86_64/bin" ; do
    if [ -n "$studio_path" ] && [ -d "$studio_path" ] && [ -x "$studio_path/keytool" ]; then
      export PATH="$studio_path:$PATH"
      echo "Auto-detected Android Studio JDK tools and added to PATH: $studio_path"
      break
    fi
  done
fi

# Make sure keytool / jarsigner are available
if ! command -v keytool >/dev/null 2>&1 || ! command -v jarsigner >/dev/null 2>&1; then
  echo "ERROR: keytool and/or jarsigner not found in PATH."
  echo ""
  echo "Tried to auto-detect Android Studio, but failed."
  echo ""
  echo "Please add the bin folder manually, e.g. in Git Bash / MINGW64:"
  echo "  export PATH=\"/c/Program Files/Android/Android Studio/jbr/bin:\$PATH\""
  echo "  ./sign-aab.sh"
  echo ""
  echo "Or start Android Studio first (it sometimes injects tools), then run from Git Bash."
  exit 1
fi

# Where dx + build_sentinel puts the unsigned AAB
UNSIGNED_AAB_CANDIDATES=(
  "target/dx/whatsmine/release/android/app/app/build/outputs/bundle/release/app-release.aab"
  "target/dx/whatsmine/debug/android/app/app/build/outputs/bundle/release/app-release.aab" # fallback
)

UNSIGNED_AAB=""
for candidate in "${UNSIGNED_AAB_CANDIDATES[@]}"; do
  if [ -f "$candidate" ]; then
    UNSIGNED_AAB="$candidate"
    break
  fi
done

if [ -z "$UNSIGNED_AAB" ]; then
  echo "ERROR: Could not find an unsigned AAB."
  echo "Run one of these first:"
  echo "  ./build.sh aab"
  echo "  ./build.sh workspace aab"
  exit 1
fi

echo "Found unsigned AAB: $UNSIGNED_AAB"

# Keystore location (relative to whatsmine/)
KEYSTORE="upload-keystore.jks"
KEY_ALIAS="upload"

RELEASE_DIR="release"
mkdir -p "$RELEASE_DIR"

# Check if keystore exists
if [ ! -f "$KEYSTORE" ]; then
  echo ""
  echo "No upload keystore found at $KEYSTORE"
  echo "Generating a new one now (this is your Play Store 'upload key')."
  echo ""
  echo "IMPORTANT: This keystore (and the certificate inside it) will be permanently"
  echo "           tied to your app on Play Store. You MUST keep this exact .jks file"
  echo "           and its password for ALL future signed releases. Do not create new"
  echo "           keystores later unless you are prepared to do an upload key reset"
  echo "           with Google support."

  if [ -z "$STORE_PASS" ] || [ -z "$KEY_PASS" ]; then
    echo "You will be prompted for passwords and some certificate info."
    echo ""
    echo "========== CRITICAL WARNING =========="
    echo "This keystore + its passwords are the ONLY way you will be able to"
    echo "upload new versions of the app for a long time."
    echo ""
    echo "If you lose the file or the passwords AFTER your first Play Store upload:"
    echo "  → You will have to go through Google's upload key reset process"
    echo "    (https://support.google.com/googleplay/android-developer/answer/7384423)"
    echo "  → It can take days or weeks."
    echo "  → You cannot publish updates during that period."
    echo ""
    echo "BEFORE first upload: losing it is harmless — just generate a new one."
    echo ""
    echo "RECOMMENDED RIGHT NOW:"
    echo "  1. After this command finishes, immediately attach this .jks file"
    echo "     to an entry in your password manager (Bitwarden, 1Password, etc.)."
    echo "  2. Save BOTH passwords in the same password manager entry."
    echo "  3. Make at least one encrypted backup on another device or USB stick."
    echo "======================================"
    echo ""

    echo ""
    echo "About the certificate information keytool will ask for:"
    echo "  This data goes into the PUBLIC part of the certificate that signs your AAB."
    echo "  It will be visible to Google when you upload, and to anyone who"
    echo "  inspects the signature of your app bundle (e.g. with apksigner verify -v or keytool)."
    echo ""
    echo "  It does NOT appear in the Play Store listing, description, or get sent to"
    echo "  normal users' devices."
    echo ""
    echo "  It is standard for all Android apps. You do not have to use real personal"
    echo "  details. Many developers use their GitHub handle or company name."
    echo ""
    echo "  Privacy tip: Avoid putting your home street address, personal phone number,"
    echo "  or other highly sensitive info. City + country code is usually fine."
    echo ""
    echo "  Suggested values (you can press Enter on most to use something reasonable):"
    echo "    First and last name: fidderr (or your real/public name)"
    echo "    Organizational unit: Development"
    echo "    Organization: fidderr"
    echo "    City or Locality: (your city, or just press Enter)"
    echo "    State or Province: (your state/province, or press Enter)"
    echo "    Two-letter country code: XX (or US, DE, NL, etc.)"
    echo ""

    keytool -genkey -v \
      -keystore "$KEYSTORE" \
      -alias "$KEY_ALIAS" \
      -keyalg RSA \
      -keysize 2048 \
      -validity 10000

    echo ""
    echo "Keystore created: $KEYSTORE"
    echo "Alias: $KEY_ALIAS"
    echo ""
    echo ">>> ACTION REQUIRED <<<"
    echo "Right now (while you still remember):"
    echo "  - Add $KEYSTORE to your password manager as an attachment."
    echo "  - Save the two passwords you just entered."
    echo "  - Make a backup."
    echo ">>> Do this before you forget! <<<"
    echo ""
  else
    echo "Using passwords from environment variables (non-interactive mode)."
    echo "Generating keystore non-interactively..."
    echo ""
    echo "(Note: the certificate DN uses generic values. The public cert details"
    echo " will still be visible to Google and signature inspectors, which is normal.)"

    keytool -genkey -v \
      -keystore "$KEYSTORE" \
      -alias "$KEY_ALIAS" \
      -keyalg RSA \
      -keysize 2048 \
      -validity 10000 \
      -storepass "$STORE_PASS" \
      -keypass "$KEY_PASS" \
      -dname "CN=WhatsMine, OU=Development, O=fidderr, L=Unknown, ST=Unknown, C=XX"

    echo "Keystore created non-interactively: $KEYSTORE (alias: $KEY_ALIAS)"
  fi
else
  echo ""
  echo "Reusing existing upload keystore: $KEYSTORE"
  echo "This is the correct behavior for new releases."
  echo "The certificate (including whatever details were given at creation) is fixed."
  echo "You do not need to (and should not) create a new keystore or enter new details."
  echo "Just provide the correct password when asked (or via AAB_SIGN_* env vars)."
fi

# Ask for passwords (don't echo) only if not provided via env
if [ -z "$STORE_PASS" ]; then
  echo "Enter the keystore password (storepass):"
  read -s STORE_PASS
fi
if [ -z "$KEY_PASS" ]; then
  echo "Enter the key password (keypass) for alias '$KEY_ALIAS' (usually same as storepass):"
  read -s KEY_PASS
fi

echo ""
echo "Signing the AAB with jarsigner..."
echo "(This may take a moment)"

jarsigner -verbose \
  -sigalg SHA256withRSA \
  -digestalg SHA-256 \
  -tsa http://timestamp.digicert.com \
  -keystore "$KEYSTORE" \
  -storepass "$STORE_PASS" \
  -keypass "$KEY_PASS" \
  "$UNSIGNED_AAB" \
  "$KEY_ALIAS"

echo ""
echo "Verifying signature..."
jarsigner -verify -verbose -certs "$UNSIGNED_AAB" | tail -5

echo ""
echo "NOTE about the messages you usually see here:"
echo "  - 'self-signed' + 'certificate chain is invalid' + 'PKIX path building failed'"
echo "    These are 100% normal and expected for any upload key you created with keytool."
echo "    You are using Google Play App Signing: the local .jks is only your *upload* key."
echo "    Google re-signs the final APKs with the real app signing key they manage."
echo "  - 'signatures that do not include a timestamp' (now reduced because we pass -tsa)"
echo "    We request a trusted timestamp so the signature stays valid long-term."
echo "  As long as you see 'jar signed.' followed by the SUCCESS! block below, the AAB is fine."


# Create a nicely named signed copy for easy upload
TIMESTAMP=$(date +%Y%m%d-%H%M)
SIGNED_NAME="WhatsMine-InternalTesting-${TIMESTAMP}.aab"
SIGNED_PATH="$RELEASE_DIR/$SIGNED_NAME"

cp "$UNSIGNED_AAB" "$SIGNED_PATH"

echo ""
echo "✅ SUCCESS!"
echo ""
echo "Signed AAB ready for Play Console internal testing:"
echo "  $SIGNED_PATH"
echo ""
echo "The signed file is at:"
echo "  $SIGNED_PATH"
echo ""
echo "Just open the 'release' folder in Explorer and copy the .aab file from there."
echo ""
echo "You can now copy this file and upload it to:"
echo "  Google Play Console → Your app → Internal testing → Create new release"
echo ""
echo "Remember:"
echo "  - First time: Create the app in Play Console using package name com.fidderr.whatsmine exactly."
echo "  - Enable Play App Signing (recommended)."
echo "  - Keep your $KEYSTORE file and passwords extremely safe FOREVER."
echo "  - For ALL future releases (even years from now): just run ./build.sh aab then ./sign-aab.sh again."
echo "    Do NOT create a new keystore or change the certificate details — it must be the same one."
echo ""

# Clean up sensitive vars (best effort)
unset STORE_PASS
unset KEY_PASS

echo "Done. The file is waiting in the release/ folder."