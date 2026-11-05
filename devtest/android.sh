#!/bin/bash
# The Android side of devtest/lib.sh. Everything goes through adb against one explicit device,
# and only ever touches the test build (./build.sh android-test), which has its own app id and
# its own data, so a real install next to it is never touched.

export MSYS_NO_PATHCONV=1
ADB="${ADB:-$LOCALAPPDATA/Android/Sdk/platform-tools/adb.exe}"
SERIAL="${SERIAL:-emulator-5554}"
PKG=com.fidderr.whatsmine.test
ACTIVITY=dev.dioxus.main.MainActivity
AVD="${AVD:-wm36}"
EMULATOR="${EMULATOR:-$LOCALAPPDATA/Android/Sdk/emulator/emulator.exe}"

on_emulator() { case "$SERIAL" in emulator-*) return 0 ;; *) return 1 ;; esac; }

# Nothing may hang a run: an unresponsive device fails instead of waiting forever.
adb_() { timeout 120 "$ADB" -s "$SERIAL" "$@"; }
device_ok() { [ "$(timeout 20 "$ADB" -s "$SERIAL" shell echo ok 2> /dev/null | tr -d '\r')" = ok ]; }

# ---- The emulator ----
emulator_start() {
    powershell.exe -NoProfile -Command "Start-Process -WindowStyle Hidden -FilePath '$(cygpath -w "$EMULATOR")' -ArgumentList '-avd $AVD -no-window -no-audio -no-boot-anim -no-snapshot -gpu ${EMULATOR_GPU:-host} -port ${SERIAL#emulator-}'"
}
# This emulator, and not another one running next to it.
emulator_stop() {
    powershell.exe -NoProfile -Command "Get-CimInstance Win32_Process | Where-Object { \$_.Name -match 'emulator|qemu' -and \$_.CommandLine -match '-port ${SERIAL#emulator-}' } | ForEach-Object { Stop-Process -Id \$_.ProcessId -Force -ErrorAction SilentlyContinue }"
    sleep 1
}
# Awake, Asleep or Dozing. A picture of an asleep screen is black and says nothing, and touches
# go nowhere.
wakefulness() { adb_ shell "dumpsys power | grep -m1 mWakefulness=" 2> /dev/null | tr -d '\r' | sed 's/.*=//'; }
no_keyguard() {
    adb_ shell "dumpsys window displays | grep -q 'isKeyguardShowing=false'" > /dev/null 2>&1 && return 0
    adb_ shell wm dismiss-keyguard > /dev/null 2>&1
    return 1
}
# An emulator can put its setting back after booting, and go dark soon after: so this comes before
# every scenario. A phone is left as it is: it is someone's, with a PIN no script knows.
before_scenario() {
    on_emulator || return 0
    if [ "$(adb_ shell settings get system screen_off_timeout | tr -d '\r')" != 86400000 ]; then
        adb_ shell settings put system screen_off_timeout 86400000
    fi
    if [ "$(wakefulness)" != Awake ]; then
        note "the screen was dark: woken"
        adb_ shell input keyevent KEYCODE_WAKEUP
    fi
    wait_for 5 no_keyguard || note "the lock screen stays"
}

# Up, and past its lock screen.
device_ready() {
    if device_ok; then
        :
    elif on_emulator; then
        emulator_start
        timeout 240 "$ADB" -s "$SERIAL" wait-for-device || return 1
    else
        echo "$SERIAL is not connected"
        return 1
    fi
    timeout 240 "$ADB" -s "$SERIAL" shell 'while [ -z "$(getprop sys.boot_completed)" ]; do sleep 0.5; done' || return 1
    open_shells
    before_scenario
}

# Two shells on the device that stay open, one as adb and one as the app. Starting adb for every
# call takes 110 ms, a call in an open shell 30, and a scenario makes hundreds. Open them from the
# script itself, not from a subshell: what a subshell opens goes with it.
DEV_PID=""
APP_PID=""
open_shells() {
    { coproc DEV_SH { "$ADB" -s "$SERIAL" shell -T sh 2>&1; }; } 2> /dev/null
    exec 5>&"${DEV_SH[1]}" 6<&"${DEV_SH[0]}"
    DEV_PID=$DEV_SH_PID
    open_app_shell
}
# Right after an install or a clear, run-as can refuse for a moment: on a phone for several seconds.
open_app_shell() {
    local try
    for try in $(seq 1 30); do
        { coproc APP_SH { "$ADB" -s "$SERIAL" shell -T "run-as $PKG sh" 2>&1; }; } 2> /dev/null
        # A shell that ended at once (run-as refusing) has nothing to hold on to: try again.
        if ! { exec 7>&"${APP_SH[1]}" 8<&"${APP_SH[0]}"; } 2> /dev/null; then
            sleep 0.5
            continue
        fi
        APP_PID=$APP_SH_PID
        [ "$(ask "$APP_PID" 7 8 'echo ok' 2> /dev/null)" = ok ] && return 0
        kill "$APP_PID" 2> /dev/null
        sleep 0.5
    done
    echo "no shell as the app"
    return 1
}

# A command in an open shell. Its output whole, and only then: a reader that stops early must not
# leave the rest in the way of the next answer. Returns what the command returned.
ask() { # pid in out command
    local pid=$1 line text="" rest
    printf '{ %s\n} < /dev/null 2>&1; echo "__DONE__$?"\n' "$4" >&"$2"
    while :; do
        if ! IFS= read -r -t 90 line <&"$3"; then
            kill "$pid" 2> /dev/null
            return 124
        fi
        line=${line%$'\r'}
        case "$line" in
            *__DONE__*)
                rest=${line%__DONE__*}
                [ -z "$rest" ] || text="$text$rest"$'\n'
                printf '%s' "$text"
                return "${line##*__DONE__}"
                ;;
        esac
        text="$text$line"$'\n'
    done
}

# As adb. What gives pages of output goes the old way: reading it line by line is slow.
dev() {
    case "$1" in
        dumpsys | cat | uiautomator) adb_ shell "$@" | tr -d '\r' ;;
        *)
            if kill -0 "$DEV_PID" 2> /dev/null; then
                ask "$DEV_PID" 5 6 "$*"
            else
                adb_ shell "$@" | tr -d '\r'
            fi
            ;;
    esac
}
# As the app, in its private folder.
as_app() {
    kill -0 "$APP_PID" 2> /dev/null || open_app_shell || return 1
    ask "$APP_PID" 7 8 "$*"
}

install_app() { # apk
    local said
    said=$(adb_ install -r -g "$1" 2>&1)
    case "$said" in
        *Success*)
            kill "$APP_PID" 2> /dev/null
            open_app_shell
            ;;
        *)
            echo "the install failed: $said"
            return 1
            ;;
    esac
}

start_app() { dev am start -n "$PKG/$ACTIVITY" > /dev/null; }
stop_app() { dev am force-stop "$PKG" > /dev/null; }

# A clean app, with what `$1` asks for already there (see debug_hooks::seed_if_asked).
fresh() { # what to seed, or nothing for a first start
    stop_app
    # ColorOS does not let the shell clear an app: then only what the app itself owns.
    case "$(dev pm clear "$PKG" 2>&1)" in
        *Success*) ;;
        *) as_app "rm -rf ./files ./databases ./shared_prefs ./cache" ;;
    esac
    adb_ shell "pm grant $PKG android.permission.POST_NOTIFICATIONS; pm grant $PKG android.permission.CAMERA; pm grant $PKG android.permission.RECORD_AUDIO" > /dev/null 2>&1
    kill "$APP_PID" 2> /dev/null
    open_app_shell
    as_app "mkdir -p files"
    [ -z "${1:-}" ] || as_app "echo '$1' > files/debug_seed.txt"
    start_app
}

# Run JavaScript in the page and print what it returns. One program on the device, that also
# waits for the answer: under a tenth of a second.
js() {
    local out
    out=$(as_app "cd files && rm -f debug_script_result.txt && print -r -- 'js:${1//\'/\'\\\'\'}' > debug_action.tmp && mv debug_action.tmp debug_action.txt && i=0; while [ ! -e debug_script_result.txt ] && [ \$i -lt 300 ]; do sleep 0.01; i=\$((i + 1)); done; cat debug_script_result.txt 2> /dev/null; cd ..")
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

take_picture() { adb_ exec-out screencap -p > "$1"; }
# A real touch, as a person's: what only a touch brings up, like the keyboard.
touch_on() { # selector
    local x y
    read -r x y <<< "$(ask_page "var r = document.querySelector('$1').getBoundingClientRect(), f = devicePixelRatio; return Math.round((r.left + r.width / 2) * f) + ' ' + Math.round((r.top + r.height / 2) * f);")"
    dev input tap "$x" "$y"
    sleep 0.2
}
# An emulator has a hardware keyboard, and hides the one on screen unless told.
keyboard_can_show() {
    on_emulator && dev settings put secure show_ime_with_hard_keyboard 1
    return 0
}
keyboard_up() { dev "dumpsys input_method | grep -q 'mInputShown=true'" > /dev/null; }
keyboard_gone() { ! keyboard_up; }
# Only when it is there: otherwise Back leaves the screen.
keyboard_down() {
    keyboard_up || return 0
    dev input keyevent KEYCODE_BACK
    wait_for 3 keyboard_gone
    sleep 0.2
}
press_back() {
    dev input keyevent KEYCODE_BACK
    sleep 0.2
}
go_back() { press_back; }
# By touch: the same as a person's finger, with a moment for it to come to rest.
scroll_screen() {
    local top height x
    read -r top height x <<< "$(ask_page "$SCROLLER if (!s) return '0 0 0'; var r = s.getBoundingClientRect(), f = devicePixelRatio, t = Math.max(r.top, 0), b = Math.min(r.bottom, innerHeight); return Math.round(t * f) + ' ' + Math.round((b - t) * f) + ' ' + Math.round((r.left + r.width / 2) * f);")"
    dev input swipe "$x" "$((top + height * 80 / 100))" "$x" "$((top + height * 25 / 100))" 300
    sleep 0.4
}
write_log() { adb_ logcat -d -b all -v time 2> /dev/null | grep -E "WhatsMine|whatsmine|MobileSentinel|Sentinel|AndroidRuntime|FATAL|am_anr|ANR in" > "$1"; }
