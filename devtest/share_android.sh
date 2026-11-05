#!/bin/bash
# An Android emulator calls a window on this PC. Proves what only a real Android can: the microphone
# goes on after Home, and the screen is shown with Android's own capture, also while the app is not
# in front. The PC's window stays off-screen, with a made-up camera and microphone.
#
#   SERIAL=emulator-5570 bash devtest/share_android.sh
PLATFORM=android
. "$(dirname "$0")/lib.sh"

SHOT_DIR="$OUT/share_android"
mkdir -p "$SHOT_DIR"
rm -f "$OUT/results.txt"
WIN_EXE="${WIN_EXE:-target/dx/whatsmine/debug/windows/app/whatsmine.exe}"
W_PROFILE=devtest-w
W_PORT=9363
W_DATA="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$W_PROFILE"
W_PID=""

APK="${APK:-target/dx/whatsmine/debug/android/app/app/build/outputs/apk/debug/app-debug.apk}"
built_fresh "$APK" || exit 1
built_fresh "$WIN_EXE" || exit 1
device_ready || { echo "STOP  the device is not there"; exit 1; }
install_app "$APK" || exit 1

w_start() {
    rm -rf "$W_DATA"
    mkdir -p "$W_DATA"
    printf 'name=Noor theme=dark locale=en\n' > "$W_DATA/debug_seed.txt"
    env WHATSMINE_PROFILE="$W_PROFILE" WHATSMINE_TEST_WINDOW="412x880" \
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=$W_PORT --use-fake-device-for-media-stream" \
        WEBVIEW2_USER_DATA_FOLDER="$(cygpath -w "$(cygpath -u "$LOCALAPPDATA")/fidderr/WhatsMine/devtest-webview-$W_PROFILE")" \
        "$WIN_EXE" >> "$OUT/share_window.log" 2>&1 &
    W_PID=$!
}
w_stop() {
    env WHATSMINE_PROFILE="$W_PROFILE" WHATSMINE_TEST_WINDOW="412x880" "$WIN_EXE" --quit > /dev/null 2>&1
    sleep 1
    [ -n "$W_PID" ] && kill "$W_PID" 2> /dev/null
}
trap w_stop EXIT

w_ask() { # file prefix, what
    local i=0 out
    rm -f "$W_DATA/debug_script_result.txt"
    printf '%s' "$1" > "$W_DATA/debug_action.tmp" && mv -f "$W_DATA/debug_action.tmp" "$W_DATA/debug_action.txt"
    while [ ! -e "$W_DATA/debug_script_result.txt" ] && [ "$i" -lt 500 ]; do
        sleep 0.01
        i=$((i + 1))
    done
    out=$(cat "$W_DATA/debug_script_result.txt" 2> /dev/null)
    echo "${out#=}"
}
w_js() { w_ask "js:$1"; }
w_core() { w_ask "core:$1"; }
w_there() { [ "$(w_js "return document.querySelector('$1') ? 'y' : 'n'")" = y ]; }
w_click() { w_js "var e = document.querySelector('$1'); if (!e) return 'no'; e.click(); return 'ok'" > /dev/null; }

# On Android the same question goes to a file in the app's private folder.
a_core() {
    local out
    # In a subshell: the shell on the device stays open, and with it where it went.
    out=$(as_app "(cd files && rm -f debug_script_result.txt && print -r -- 'core:$1' > debug_action.tmp && mv debug_action.tmp debug_action.txt && i=0; while [ ! -e debug_script_result.txt ] && [ \$i -lt 100 ]; do sleep 0.05; i=\$((i+1)); done; cat debug_script_result.txt)")
    echo "${out#=}"
}

w_start

scenario they_become_contacts
fresh "name=Roy theme=dark locale=en"
on_screen .bottom-nav 90 || fail "no app on the device"
a_core front > /dev/null
wait_for 60 w_there .bottom-nav || fail "no window"
w_core front > /dev/null
KEY_A=$(a_core key)
KEY_W=$(w_core key)
LINK_W=$(w_core invite)
LINK_A=$(a_core invite)
note "adding: $(a_core "add $LINK_W" | cut -c1-30)"
has_request_w() { [ -n "$(w_core requests)" ]; }
has_request_a() { [ -n "$(a_core requests)" ]; }
online() { [ "$(a_core "online $KEY_W")" = true ]; }
if wait_for 90 has_request_w; then
    note "the window saw the request of the emulator"
    w_core "accept $KEY_A" > /dev/null
else
    note "90 s and the window saw nothing: the other way around"
    note "adding: $(w_core "add $LINK_A" | cut -c1-30)"
    if wait_for 120 has_request_a; then
        note "the emulator saw the request of the window"
        a_core "accept $KEY_W" > /dev/null
    else
        note "nobody saw anything"
    fi
fi
wait_for 120 online && pass || fail "they never connected"

scenario a_call
js "var r = [...document.querySelectorAll('.chat-row')].find(x => x.textContent.includes('Noor')); if (r) r.click(); return r ? 'ok' : 'no'" > /dev/null
on_screen '[data-t="call-voice"]' 30
click '[data-t="call-voice"]' 0 0.5
wait_for 60 w_there '[data-t="call-accept"]' || fail "the window never rang"
w_click '[data-t="call-accept"]'
if on_screen '[data-t="call-share"]' 40; then
    picture in_call
    pass
else
    fail "no call"
fi

heard() { w_js "return wmCall.stats().aRecv"; }
scenario the_microphone_goes_on_after_home
h1=$(heard)
sleep 3
h2=$(heard)
note "in front: $((h2 - h1)) packets in 3 s"
adb_ shell input keyevent KEYCODE_HOME
sleep 6
h3=$(heard)
sleep 5
h4=$(heard)
note "behind another app: $((h4 - h3)) packets in 5 s"
note "service: $(adb_ shell dumpsys activity services com.fidderr.whatsmine.test 2> /dev/null | grep -m2 -E 'SentinelCallService|foregroundServiceType' | tr -d '\r' | tr -s ' ' | tr '\n' ' ' | cut -c1-150)"
if [ "$((h4 - h3))" -gt 100 ]; then
    pass "still heard"
else
    fail "silent after Home"
fi
adb_ shell am start -n "$PKG/$ACTIVITY" > /dev/null
on_screen '[data-t="call-share"]' 20

scenario the_screen_is_shown
adb_ shell appops set "$PKG" PROJECT_MEDIA allow > /dev/null 2>&1
click '[data-t="call-share"]' 0 0.4
click '[data-t="share-go"]' 0 1
sleep 3
adb_ shell dumpsys media_projection 2> /dev/null | grep -m1 -E "mProjectionGrant|Media Projection" | tr -d '\r' | cut -c1-120
has_picture() { [ "$(w_js "var i = wmCall.screenInfo(); return i && i.w > 0 ? 'y' : 'n'")" = y ]; }
if wait_for 40 has_picture; then
    note "watching: $(w_js "return JSON.stringify(wmCall.screenInfo())")"
    note "stats: $(w_js "var s = wmCall.stats(); return 'received ' + s.sRecv + ', decoded ' + s.sDec + ', gaps ' + s.sGaps + ', asked ' + s.sKeys + ' ' + s.errors.join(';')")"
    pass
else
    fail "no picture: $(w_js "return JSON.stringify(wmCall.stats())")"
fi

scenario it_goes_on_from_another_app
adb_ shell input keyevent KEYCODE_HOME
sleep 3
d1=$(w_js "return wmCall.stats().sDec")
adb_ shell input swipe 500 1500 500 500 300
sleep 1
adb_ shell input swipe 500 500 500 1500 300
sleep 4
d2=$(w_js "return wmCall.stats().sDec")
note "decoded $((d2 - d1)) frames from behind another app"
node devtest/cdp.mjs "$W_PORT" shot "$SHOT_DIR/watching_the_home_screen.png"
if [ "$d2" -gt "$d1" ]; then pass; else fail "it stopped"; fi

scenario stopping
adb_ shell am start -n "$PKG/$ACTIVITY" > /dev/null
on_screen '[data-t="call-share"]' 20
click '[data-t="call-share"]' 0 0.5
gone_on_window() { [ "$(w_js "return document.querySelector('.wm-screen') ? 'y' : 'n'")" = n ]; }
wait_for 20 gone_on_window && pass || fail "the picture stayed"

scenario hanging_up
click '[data-t="call-hangup"]' 0 0.5
gone_w() { [ "$(w_js "return document.querySelector('[data-t=\"call-hangup\"]') ? 'y' : 'n'")" = n ]; }
wait_for 20 gone_w && pass || fail "the call stayed"
sleep 1
note "service after the call: $(adb_ shell dumpsys activity services com.fidderr.whatsmine.test 2> /dev/null | grep -c SentinelCallService)"

summary
