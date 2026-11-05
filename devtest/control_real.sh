#!/bin/bash
# The one test that moves the REAL mouse. Run it only with someone sitting at this PC, who agreed to
# it: about a minute. Everything else about control is tested with a made-up desktop (control.sh).
#
#   bash devtest/control_real.sh
#
# Window A is a normal window on your screen. Window B stays off-screen and plays the other person.
# You tick the box and press Allow yourself. Then B moves the pointer in a small square. You can stop
# it at any moment with Esc, or the Stop button on the bar at the top of the screen.
PLATFORM=windows
. "$(dirname "$0")/lib.sh"

SHOT_DIR="$OUT/control_real"
mkdir -p "$SHOT_DIR"
EXTRA="--use-fake-device-for-media-stream --use-fake-ui-for-media-stream"

built_fresh "$EXE" || exit 1

A_PROFILE=devtest-a
B_PROFILE=devtest-b
A_DATA="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$A_PROFILE"
B_DATA="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$B_PROFILE"
A_PORT=9361
B_PORT=9362

start_window() { # profile port seed size-or-empty
    local profile=$1 port=$2 data
    data="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$profile"
    rm -rf "$data"
    mkdir -p "$data"
    printf '%s\n' "$3" > "$data/debug_seed.txt"
    env WHATSMINE_PROFILE="$profile" ${4:+WHATSMINE_TEST_WINDOW="$4"} \
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=$port $EXTRA" \
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

window_a() { DATA="$A_DATA"; CDP_PORT=$A_PORT; }
window_b() { DATA="$B_DATA"; CDP_PORT=$B_PORT; }
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

start_window "$A_PROFILE" "$A_PORT" "name=Roy theme=dark locale=en" ""
A_PID=$!
start_window "$B_PROFILE" "$B_PORT" "name=Noor theme=light locale=en" "412x880"
B_PID=$!

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
wait_for 120 requests_seen || { echo "the request never came"; exit 1; }
core "accept $KEY_A" > /dev/null
window_a
online_b() { [ "$(core "online $KEY_B")" = true ]; }
wait_for 60 online_b || { echo "they never connected"; exit 1; }
core "known_long $KEY_B" > /dev/null

open_chat() { js "var r = [...document.querySelectorAll('.chat-row')].find(x => x.textContent.includes('Noor')); if (!r) return 'no'; r.click(); return 'ok'"; }
chat_there() { is_there '[data-t="call-video"]' || [ "$(open_chat)" = ok ]; }
wait_for 30 chat_there || { echo "no chat"; exit 1; }
on_screen '[data-t="call-video"]' 20
click '[data-t="call-video"]' 0 0.3
window_b
on_screen '[data-t="call-accept"]' 30 || { echo "B never rang"; exit 1; }
click '[data-t="call-accept"]' 0 0.5
window_a
on_screen '[data-t="call-share"]' 30 || { echo "no call"; exit 1; }
click '[data-t="call-share"]' 0 0.3
on_screen '[data-t="share-go"]' 5
click '[data-t="share-go"]' 0 0.5
on_screen '[data-t="call-sharing"]' 15 || { echo "not sharing"; exit 1; }
window_b
has_picture() { [ "$(js "var i = wmCall.screenInfo(); return i && i.colors > 3 ? 'y' : 'n'")" = y ]; }
wait_for 30 has_picture || { echo "no picture"; exit 1; }
on_screen '[data-t="control-ask"]' 10 || { echo "control is not offered: the shared screen could not be told for certain"; exit 1; }
click '[data-t="control-ask"]' 0 0.2

echo
echo ">>> In the WhatsMine window: read it, tick the box, and press Allow. You have 60 seconds."
window_a
on_screen '[data-t="control-owner-line"]' 70 || { echo "not allowed: nothing moved"; exit 0; }
echo ">>> Allowed. Is there a bar at the top of your screen with Noor's name and a Stop button?"
sleep 3
echo ">>> The pointer now moves in a small square for 10 seconds. Move your own mouse: it should pause."
echo ">>> Esc, or Stop on the bar, ends it at once."
window_b
for round in 1 2 3 4 5 6 7 8 9 10 11 12 13; do
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
    echo ">>> It stopped from the other side."
else
    echo ">>> It was stopped from your side."
fi
sleep 1
window_a
is_there '[data-t="control-owner-line"]' && echo ">>> WRONG: it still says someone is in control" || echo ">>> Over."
echo ">>> Tell Claude: did the pointer move, did your mouse pause it, did Esc stop it, and was the bar there?"
