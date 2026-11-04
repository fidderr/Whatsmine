#!/bin/bash
# The Windows side of devtest/lib.sh: the desktop test build, in a window off-screen that never
# takes the focus and has no taskbar button, on its own profile. It writes nothing to the
# registry, shows no toast and no tray icon, so a run can go on while the PC is in use.
# Build it with ./build.sh desktop-test.

EXE="${EXE:-target/dx/whatsmine/debug/windows/app/whatsmine.exe}"
PROFILE="${PROFILE:-devtest}"
SIZE="${SIZE:-412x880}"
CDP_PORT="${CDP_PORT:-9341}"
DATA="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$PROFILE"
WEB_DATA="$(cygpath -u "$LOCALAPPDATA")/fidderr/WhatsMine/devtest-webview-$PROFILE"
APP_PID=""

device_ok() { true; }

app_env() {
    # shellcheck disable=SC2034
    env WHATSMINE_PROFILE="$PROFILE" WHATSMINE_TEST_WINDOW="$SIZE" \
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=$CDP_PORT" \
        WEBVIEW2_USER_DATA_FOLDER="$(cygpath -w "$WEB_DATA")" "$@"
}
start_app() {
    app_env "$EXE" >> "$OUT/app.log" 2>&1 &
    APP_PID=$!
}
app_alive() { [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2> /dev/null; }
app_gone() { ! app_alive; }
stop_app() {
    app_alive || return 0
    # Asks the running copy of this profile to quit, as a second launch does. The real app has
    # another profile, so another address, and never hears of it.
    app_env "$EXE" --quit > /dev/null 2>&1
    wait_for 8 app_gone || kill "$APP_PID" 2> /dev/null
    APP_PID=""
}

# A clean app, with what `$1` asks for already there (see debug_hooks::seed_if_asked).
fresh() { # what to seed, or nothing for a first start
    stop_app
    rm -rf "$DATA"
    mkdir -p "$DATA"
    [ -z "${1:-}" ] || printf '%s\n' "$1" > "$DATA/debug_seed.txt"
    start_app
}

# In one go: the app deletes the file once it has seen it.
js() {
    local out i=0
    rm -f "$DATA/debug_script_result.txt"
    printf 'js:%s' "$1" > "$DATA/debug_action.tmp" && mv -f "$DATA/debug_action.tmp" "$DATA/debug_action.txt"
    while [ ! -e "$DATA/debug_script_result.txt" ] && [ "$i" -lt 300 ]; do
        sleep 0.01
        i=$((i + 1))
    done
    out=$(cat "$DATA/debug_script_result.txt" 2> /dev/null)
    case "$out" in
        =*)
            echo "${out#=}"
            return 0
            ;;
    esac
    # Never empty: that reads as an answer.
    echo "no answer from the page"
    return 1
}

take_picture() { node devtest/cdp.mjs "$CDP_PORT" shot "$1"; }
touch_on() { # selector
    js "var e = document.querySelector('$1'); e.focus(); e.click(); return 'ok'" > /dev/null
    sleep 0.2
}
# No keyboard on screen here.
keyboard_can_show() { return 1; }
keyboard_up() { return 1; }
keyboard_down() { :; }
# Esc closes what is on top, Alt+Left goes back a screen: what the app listens for.
press_back() {
    js "document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); return 'ok'" > /dev/null
    sleep 0.2
}
go_back() {
    js "document.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', altKey: true, bubbles: true })); return 'ok'" > /dev/null
    sleep 0.2
}
scroll_screen() {
    js "$SCROLLER if (s) s.scrollTop += s.clientHeight * 0.7; return 'ok';" > /dev/null
    sleep 0.2
}
write_log() { tail -n 200 "$OUT/app.log" > "$1" 2> /dev/null; }
