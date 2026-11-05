#!/bin/bash
# Two real app windows on this PC call each other, and one shows its screen. The screen is a painted
# picture and the camera and microphone are made up, so nothing real is captured or filmed, and the
# windows stay off-screen.
#
#   bash devtest/share.sh
PLATFORM=windows
. "$(dirname "$0")/lib.sh"

SHOT_DIR="$OUT/share"
export WEBVIEW2_EXTRA="--use-fake-device-for-media-stream --use-fake-ui-for-media-stream"

built_fresh "$EXE" || exit 1

A_PROFILE=devtest-a
B_PROFILE=devtest-b
A_DATA="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$A_PROFILE"
B_DATA="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$B_PROFILE"
A_PORT=9361
B_PORT=9362
A_PID=""
B_PID=""

start_window() { # profile port seed
    local profile=$1 port=$2 data
    data="$(cygpath -u "$APPDATA")/fidderr/WhatsMine/data/profiles/$profile"
    rm -rf "$data"
    mkdir -p "$data"
    printf '%s\n' "$3" > "$data/debug_seed.txt"
    env WHATSMINE_PROFILE="$profile" WHATSMINE_TEST_WINDOW="412x880" \
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=$port $WEBVIEW2_EXTRA" \
        WEBVIEW2_USER_DATA_FOLDER="$(cygpath -w "$(cygpath -u "$LOCALAPPDATA")/fidderr/WhatsMine/devtest-webview-$profile")" \
        "$EXE" >> "$OUT/share_$profile.log" 2>&1 &
}
stop_window() { # profile pid
    env WHATSMINE_PROFILE="$1" WHATSMINE_TEST_WINDOW="412x880" "$EXE" --quit > /dev/null 2>&1
    sleep 1
    kill "$2" 2> /dev/null
}
finish() {
    stop_window "$A_PROFILE" "$A_PID"
    stop_window "$B_PROFILE" "$B_PID"
}
trap finish EXIT

# Which window the next `js`, `picture` and `core` talk to.
window_a() { DATA="$A_DATA"; CDP_PORT=$A_PORT; }
window_b() { DATA="$B_DATA"; CDP_PORT=$B_PORT; }

core() {
    local i=0
    rm -f "$DATA/debug_script_result.txt"
    printf 'core:%s' "$1" > "$DATA/debug_action.tmp" && mv -f "$DATA/debug_action.tmp" "$DATA/debug_action.txt"
    while [ ! -e "$DATA/debug_script_result.txt" ] && [ "$i" -lt 500 ]; do
        sleep 0.01
        i=$((i + 1))
    done
    local out
    out=$(cat "$DATA/debug_script_result.txt" 2> /dev/null)
    echo "${out#=}"
}

start_window "$A_PROFILE" "$A_PORT" "name=Roy theme=dark locale=en"
A_PID=$!
start_window "$B_PROFILE" "$B_PORT" "name=Noor theme=light locale=en"
B_PID=$!

scenario two_windows_become_contacts
window_a
on_screen .bottom-nav 60 || fail "window A did not come up"
core front > /dev/null
KEY_A=$(core key)
window_b
on_screen .bottom-nav 60 || fail "window B did not come up"
core front > /dev/null
KEY_B=$(core key)
LINK_B=$(core invite)
window_a
note "adding: $(core "add $LINK_B" | cut -c1-40)"
window_b
requests_seen() { [ -n "$(core requests)" ]; }
if wait_for 180 requests_seen; then
    core "accept $KEY_A" > /dev/null
    window_a
    online_b() { [ "$(core "online $KEY_B")" = true ]; }
    wait_for 60 online_b && pass || fail "they never connected"
else
    fail "the request never came"
fi

scenario a_call_starts
window_a
js "wmCall.usePaintedScreen(true); return 'ok'" > /dev/null
open_chat() { # name
    js "var r = [...document.querySelectorAll('.chat-row')].find(x => x.textContent.includes('$1')); if (!r) return 'no'; r.click(); return 'ok'"
}
chat_row_there() { [ "$(open_chat Noor)" = ok ]; }
wait_for 30 chat_row_there || fail "no chat with Noor"
on_screen '[data-t="call-video"]' 20
tap_call() { click '[data-t="call-video"]' 0 0.3; }
tap_call
window_b
if on_screen '[data-t="call-accept"]' 30; then
    click '[data-t="call-accept"]' 0 0.5
    if on_screen '[data-t="call-share"]' 30; then
        pass
    else
        fail "no call on B"
    fi
else
    fail "B never rang"
fi

scenario a_screen_is_shown
window_a
on_screen '[data-t="call-share"]' 30
click '[data-t="call-share"]' 0 0.3
picture share_sheet
click '[data-t="share-go"]' 0 0.5
if on_screen '[data-t="call-sharing"]' 15; then
    picture sharing
    window_b
    has_picture() { [ "$(js "var i = wmCall.screenInfo(); return i && i.colors > 3 ? 'y' : 'n'")" = y ]; }
    if wait_for 30 has_picture; then
        note "watching: $(js "return JSON.stringify(wmCall.screenInfo())")"
        note "stats: $(js "var s = wmCall.stats(); return 'decoded ' + s.sDec + ', received ' + s.sRecv + ', gaps ' + s.sGaps + ', asked ' + s.sKeys")"
        sleep 3
        picture watching
        pass
    else
        fail "no picture on B: $(js "return JSON.stringify(wmCall.stats())")"
    fi
else
    fail "not sharing: $(js "return JSON.stringify(wmCall.stats())")"
fi

scenario it_keeps_coming_and_says_what_it_costs
window_a
A1=$(js "var s = wmCall.stats(); return s.sSent + ' ' + s.sBytes")
sleep 5
A2=$(js "var s = wmCall.stats(); return s.sSent + ' ' + s.sBytes")
read -r f1 b1 <<< "$A1"
read -r f2 b2 <<< "$A2"
note "sent $((f2 - f1)) frames in 5 s, $(((b2 - b1) * 8 / 5000)) kbit/s"
window_b
B1=$(js "return wmCall.stats().sDec")
sleep 3
B2=$(js "return wmCall.stats().sDec")
[ "$B2" -gt "$B1" ] && pass "still decoding" || fail "it stopped"

scenario stopping_ends_it_on_both_sides
window_a
click '[data-t="call-share"]' 0 0.5
gone '[data-t="call-sharing"]' 10 || fail "still sharing"
window_b
if gone '.wm-screen' 15; then
    pass
else
    fail "the picture stayed"
fi

scenario hanging_up
window_a
click '[data-t="call-hangup"]' 0 0.5
gone '[data-t="call-hangup"]' 10
window_b
gone '[data-t="call-hangup"]' 15 && pass || fail "the call stayed on B"

summary
