#!/bin/bash
# The one test that moves the REAL mouse, with the real screen picker. Run it only with someone at this
# PC who agreed to it. Everything else about control is tested with a made-up desktop (control.sh).
#
#   bash devtest/control_real.sh window    # share one Notepad window: it clicks and types in there
#   bash devtest/control_real.sh screen    # share the whole screen: it only moves the pointer
#
# Window A is a normal window on your screen. Window B stays off-screen and plays the other person.
# You press Share, choose in the picker, tick the box and press Allow yourself. Esc, or Stop on the bar
# at the top of the screen, ends it at any moment.
MODE="${1:-window}"
PLATFORM=windows
. "$(dirname "$0")/lib.sh"

SHOT_DIR="$OUT/control_real"
mkdir -p "$SHOT_DIR"

built_fresh "$EXE" || exit 1

A_PROFILE=devtest-a
B_PROFILE=devtest-b
A_DATA="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$A_PROFILE"
B_DATA="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$B_PROFILE"

start_window() { # profile port seed size-or-empty flags
    local profile=$1 port=$2 data
    data="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$profile"
    rm -rf "$data"
    mkdir -p "$data"
    printf '%s\n' "$3" > "$data/debug_seed.txt"
    env WHATSMINE_PROFILE="$profile" ${4:+WHATSMINE_TEST_WINDOW="$4"} \
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=$port $5" \
        WEBVIEW2_USER_DATA_FOLDER="$(cygpath -w "$(cygpath -u "$LOCALAPPDATA")/fidderr/WhatsMine/devtest-webview-$profile")" \
        "$EXE" >> "$OUT/control_real_$profile.log" 2>&1 &
}
stop_window() { # profile pid size-or-empty
    env WHATSMINE_PROFILE="$1" ${3:+WHATSMINE_TEST_WINDOW="$3"} "$EXE" --quit > /dev/null 2>&1
    sleep 1
    kill "$2" 2> /dev/null
}
finish() {
    stop_window "$A_PROFILE" "$A_PID" ""
    stop_window "$B_PROFILE" "$B_PID" "412x880"
}
trap finish EXIT

window_a() { DATA="$A_DATA"; CDP_PORT=9361; }
window_b() { DATA="$B_DATA"; CDP_PORT=9362; }
core() {
    local i=0 out
    rm -f "$DATA/debug_script_result.txt"
    printf 'core:%s' "$1" > "$DATA/debug_action.tmp" && mv -f "$DATA/debug_action.tmp" "$DATA/debug_action.txt"
    while [ ! -e "$DATA/debug_script_result.txt" ] && [ "$i" -lt 500 ]; do
        sleep 0.01
        i=$((i + 1))
    done
    out=$(cat "$DATA/debug_script_result.txt" 2> /dev/null)
    echo "${out#=}"
}
hand() { # kind x-fraction y-fraction
    js "var t = document.querySelector('.wm-screen'), v = t.querySelector('video'), b = v.getBoundingClientRect(), s = Math.min(b.width / v.videoWidth, b.height / v.videoHeight), w = v.videoWidth * s, h = v.videoHeight * s; t.dispatchEvent(new PointerEvent('$1', { bubbles: true, cancelable: true, pointerId: 1, pointerType: 'mouse', button: 0, clientX: b.left + (b.width - w) / 2 + w * $2, clientY: b.top + (b.height - h) / 2 + h * $3 })); return 'ok'" > /dev/null
}
press() { # code
    js "document.dispatchEvent(new KeyboardEvent('keydown', { code: '$1', bubbles: true, cancelable: true })); document.dispatchEvent(new KeyboardEvent('keyup', { code: '$1', bubbles: true, cancelable: true })); return 'ok'" > /dev/null
}
say() { echo ">>> $1"; }

# A gets a made-up microphone, and the real screen picker.
start_window "$A_PROFILE" 9361 "name=Roy theme=dark locale=en" "" "--use-fake-device-for-media-stream"
A_PID=$!
start_window "$B_PROFILE" 9362 "name=Noor theme=light locale=en" "412x880" "--use-fake-device-for-media-stream"
B_PID=$!

say "Setting up two people, this takes about a minute. Nothing to do yet."
window_a
on_screen .bottom-nav 60 || { echo "window A did not come up"; exit 1; }
core front > /dev/null
core control_on > /dev/null
KEY_A=$(core key)
window_b
on_screen .bottom-nav 60 || { echo "window B did not come up"; exit 1; }
core front > /dev/null
KEY_B=$(core key)
LINK_B=$(core invite)
window_a
core "add $LINK_B" > /dev/null
window_b
requests_seen() { [ -n "$(core requests)" ]; }
wait_for 300 requests_seen || { echo "the request never came"; exit 1; }
core "accept $KEY_A" > /dev/null
window_a
online_b() { [ "$(core "online $KEY_B")" = true ]; }
wait_for 60 online_b || { echo "they never connected"; exit 1; }
core "known_long $KEY_B" > /dev/null

open_chat() { js "var r = [...document.querySelectorAll('.chat-row')].find(x => x.textContent.includes('Noor')); if (!r) return 'no'; r.click(); return 'ok'"; }
chat_there() { is_there '[data-t="call-video"]' || [ "$(open_chat)" = ok ]; }
wait_for 30 chat_there || { echo "no chat"; exit 1; }
on_screen '[data-t="call-video"]' 20
click '[data-t="call-voice"]' 0 0.3
window_b
on_screen '[data-t="call-accept"]' 30 || { echo "B never rang"; exit 1; }
click '[data-t="call-accept"]' 0 0.5
window_a
on_screen '[data-t="call-share"]' 30 || { echo "no call"; exit 1; }

if [ "$MODE" = window ]; then
    notepad.exe &
    say "NOW: a Notepad window opened. In the WhatsMine window press Screen, then the Show button."
    say "The picker opens. Choose the WINDOWS tab, pick Notepad, and press Share. You have 2 minutes."
else
    say "NOW: in the WhatsMine window press Screen, then the Show button."
    say "The picker opens. Choose your ENTIRE SCREEN and press Share. You have 2 minutes."
fi
if ! on_screen '[data-t="call-sharing"]' 120; then
    say "Nothing was shared. Errors: $(js "return JSON.stringify(wmCall.stats().errors)")"
    exit 1
fi
say "Shared. The capture calls itself: $(js "return JSON.stringify(wmCall.shareInfo())")"
window_b
has_picture() { [ "$(js "var i = wmCall.screenInfo(); return i && i.w > 0 ? 'y' : 'n'")" = y ]; }
wait_for 30 has_picture || { say "No picture arrived on the other side."; exit 1; }
say "The other side sees it: $(js "return JSON.stringify(wmCall.screenInfo())")"
if ! on_screen '[data-t="control-ask"]' 10; then
    say "Control is not offered for this. That is what happens when it cannot be told for certain what is shown."
    exit 1
fi
click '[data-t="control-ask"]' 0 0.2

say "NOW: go to the WhatsMine window. Read it, tick the box, and press Allow after 5 seconds. You have 60 seconds."
window_a
if ! on_screen '[data-t="control-owner-line"]' 70; then
    say "Not allowed: nothing moved."
    exit 0
fi
say "Allowed. Is there a bar at the top of your screen, with Noor's name, the time and a Stop button?"
sleep 4
window_b
if [ "$MODE" = window ]; then
    say "Now it clicks in Notepad and types hello. Then the pointer moves in a square for 15 seconds."
    hand pointermove 0.5 0.5
    sleep 0.2
    hand pointerdown 0.5 0.5
    hand pointerup 0.5 0.5
    sleep 0.5
    for key in KeyH KeyE KeyL KeyL KeyO Space; do
        press "$key"
        sleep 0.1
    done
else
    say "Now the pointer moves in a square for 15 seconds. It does not click."
fi
say "Try this: move your own mouse, the pointer should wait for you for 2 seconds. Then press Esc to end it."
for round in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19; do
    is_there '[data-t="control-stop"]' || break
    hand pointermove 0.45 0.45
    sleep 0.2
    hand pointermove 0.55 0.45
    sleep 0.2
    hand pointermove 0.55 0.55
    sleep 0.2
    hand pointermove 0.45 0.55
    sleep 0.2
done
if is_there '[data-t="control-stop"]'; then
    click '[data-t="control-stop"]' 0 0.3
    say "It was stopped from the other side, the time for this test was up."
else
    say "It was stopped from your side."
fi
sleep 1
window_a
is_there '[data-t="control-owner-line"]' && say "WRONG: it still says someone is in control." || say "Over: nobody is in control."
say "Written down: $(core control_log) time(s). Done."
