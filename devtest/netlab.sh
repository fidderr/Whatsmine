#!/bin/bash
# Measures this device's network with tools/netlab. Results as JSON lines in devtest-out/netlab/.
#
#   bash devtest/netlab.sh pc                  this PC: NAT type, DHT, connecting to itself
#   SERIAL=emulator-5554 bash devtest/netlab.sh device
#                                              the program goes over adb, needs no app, leaves
#                                              only itself in /data/local/tmp, and a PC peer
#                                              answers it through the internet
#
# A phone only when its owner says so: SERIAL=<phone> and I_ASKED=1.
cd "$(dirname "$0")/.." || exit 1
export MSYS_NO_PATHCONV=1
OUT=devtest-out/netlab
mkdir -p "$OUT"
ADB="${ADB:-$LOCALAPPDATA/Android/Sdk/platform-tools/adb.exe}"
ROUNDS="${ROUNDS:-5}"
DHT_ROUNDS="${DHT_ROUNDS:-10}"
PC_BIN=target/release/netlab.exe
NDK="${ANDROID_NDK_HOME:-$LOCALAPPDATA/Android/Sdk/ndk/28.2.13676358}"

build_pc() { cargo build --release -p netlab; }

build_for() { # rust target triple, api level
    local triple=$1 clang up bin
    bin="$(cygpath -u "$NDK")/toolchains/llvm/prebuilt/windows-x86_64/bin"
    clang="$bin/${triple}31-clang.cmd"
    [ -e "$clang" ] || { echo "no $clang"; return 1; }
    up=$(printf '%s' "$triple" | tr 'a-z-' 'A-Z_')
    env "CARGO_TARGET_${up}_LINKER=$(cygpath -w "$clang")" \
        "CC_${triple//-/_}=$(cygpath -w "$clang")" \
        "AR_${triple//-/_}=$(cygpath -w "$bin/llvm-ar.exe")" \
        cargo build --release -p netlab --target "$triple"
}

# The peer on this PC, for as long as this script runs. Prints nothing but its ticket here.
TICKET=""
PEER=""
peer_start() { # flags for listen
    "$PC_BIN" listen "$@" > "$OUT/peer.jsonl" 2>> "$OUT/peer.err" &
    PEER=$!
    local i=0
    while [ "$i" -lt 100 ]; do
        TICKET=$(sed -nE 's/.*"ticket":"([^"]+)".*/\1/p' "$OUT/peer.jsonl" 2> /dev/null | head -1)
        [ -n "$TICKET" ] && return 0
        sleep 0.5
        i=$((i + 1))
    done
    echo "the peer on this PC gave no ticket"
    return 1
}
peer_stop() { [ -z "$PEER" ] || kill "$PEER" 2> /dev/null; PEER=""; }
trap peer_stop EXIT

case "${1:-}" in
    pc)
        build_pc || exit 1
        stamp=$(date +%Y%m%d-%H%M)
        file="$OUT/pc-$stamp.jsonl"
        "$PC_BIN" net | tee -a "$file"
        "$PC_BIN" self "$ROUNDS" | tee -a "$file"
        "$PC_BIN" dht "$DHT_ROUNDS" | tee -a "$file"
        echo "in $file"
        ;;
    device)
        SERIAL="${SERIAL:-emulator-5554}"
        case "$SERIAL" in
            emulator-*) ;;
            *) [ -n "${I_ASKED:-}" ] || { echo "$SERIAL is someone's phone: say so with I_ASKED=1"; exit 1; } ;;
        esac
        adb_() { timeout 300 "$ADB" -s "$SERIAL" "$@"; }
        abi=$(adb_ shell getprop ro.product.cpu.abi | tr -d '\r')
        case "$abi" in
            arm64-v8a) triple=aarch64-linux-android ;;
            x86_64) triple=x86_64-linux-android ;;
            *) echo "unknown ABI $abi"; exit 1 ;;
        esac
        build_pc || exit 1
        build_for "$triple" || exit 1
        adb_ push "target/$triple/release/netlab" /data/local/tmp/netlab > /dev/null || exit 1
        adb_ shell chmod 755 /data/local/tmp/netlab
        there() { adb_ shell /data/local/tmp/netlab "$@" | tr -d '\r'; }
        stamp=$(date +%Y%m%d-%H%M)
        file="$OUT/$SERIAL-$stamp.jsonl"
        there net | tee -a "$file"
        there dht "$DHT_ROUNDS" | tee -a "$file"
        peer_start || exit 1
        echo "{\"note\":\"dial the PC, with a relay\"}" >> "$file"
        there dial "$TICKET" "$ROUNDS" | tee -a "$file"
        peer_stop
        adb_ shell rm -f /data/local/tmp/netlab
        echo "in $file"
        ;;
    *)
        sed -n '2,9p' "$0"
        exit 1
        ;;
esac
