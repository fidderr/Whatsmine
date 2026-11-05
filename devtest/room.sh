#!/bin/bash
# Two real app windows in a drop-in room. The microphones are made up, and the windows stay off-screen.
#
#   bash devtest/room.sh
PLATFORM=windows
. "$(dirname "$0")/lib.sh"

SHOT_DIR="$OUT/room"
mkdir -p "$SHOT_DIR"
export WEBVIEW2_EXTRA="--use-fake-device-for-media-stream"
rm -f "$OUT/results.txt"

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
        "$EXE" >> "$OUT/room_$profile.log" 2>&1 &
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
open_room() {
    js "var r = [...document.querySelectorAll('.chat-row')].find(x => x.textContent.includes('Evening chat')); if (!r) return 'no'; r.click(); return 'ok'"
}

start_window "$A_PROFILE" "$A_PORT" "name=Roy theme=dark locale=en"
A_PID=$!
start_window "$B_PROFILE" "$B_PORT" "name=Noor theme=light locale=en"
B_PID=$!

scenario a_room_is_made
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
core "add $LINK_B" > /dev/null
window_b
requests_seen() { [ -n "$(core requests)" ]; }
wait_for 300 requests_seen || fail "the request never came"
core "accept $KEY_A" > /dev/null
window_a
online_b() { [ "$(core "online $KEY_B")" = true ]; }
wait_for 60 online_b || fail "they never connected"
core "room $KEY_B" > /dev/null
room_there() { [ "$(open_room)" = ok ]; }
if wait_for 20 room_there && on_screen '[data-t="room-banner"]' 10; then
    window_b
    if wait_for 40 room_there && on_screen '[data-t="room-banner"]' 10; then
        picture banner_empty
        pass
    else
        fail "B has no room"
    fi
else
    fail "A has no room"
fi

scenario coming_in_rings_nobody
window_a
click '[data-t="room-join"]' 0 0.3
on_screen '[data-t="room-hand"]' 20 || fail "A is not in"
sleep 1
picture alone_in_the_room
window_b
someone_in() { [ "$(js "var b = document.querySelector('[data-t=\"room-banner\"]'); return b && b.textContent.includes('Roy') ? 'y' : 'n'")" = y ]; }
wait_for 20 someone_in || fail "B does not see who is in"
is_there '[data-t="call-accept"]' && fail "it rang"
picture banner_someone_in
pass

scenario two_in_the_room_hear_each_other
window_b
click '[data-t="room-join"]' 0 0.3
on_screen '[data-t="room-hand"]' 20 || fail "B is not in"
heard() { [ "$(js "return wmCall.stats().aDec > 20 ? 'y' : 'n'")" = y ]; }
wait_for 20 heard && pass || fail "nothing heard: $(js "return JSON.stringify(wmCall.stats())")"

scenario who_talks_is_outlined
outlined() { [ "$(js "return wmCall.stats().speaking > 0 ? 'y' : 'n'")" = y ]; }
if wait_for 15 outlined; then
    # The made-up microphone beeps: catch it in the act.
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
        is_there '.wm-tile.speaking:not(.me)' && break
        sleep 0.1
    done
    picture someone_talks
    pass
else
    fail "never outlined: $(js "return JSON.stringify(wmCall.stats())")"
fi

scenario a_hand_goes_up_and_is_seen
click '[data-t="room-hand"]' 0 0.3
window_a
if on_screen '.wm-tile.hand' 10; then
    picture hand_up
    window_b
    click '[data-t="room-hand"]' 0 0.3
    window_a
    gone '.wm-tile.hand' 10 && pass || fail "the hand stayed up"
else
    fail "no hand"
fi

scenario the_chat_with_the_room_going_on
window_a
click '[data-t="room-chat"]' 0 0.5
if on_screen '[data-t="call-bar"]' 6; then
    on_screen '[data-t="chat-more"]' 6
    picture chat_with_a_bar
    a0=$(js "return wmCall.stats().aDec")
    sleep 2
    a1=$(js "return wmCall.stats().aDec")
    [ "$a1" -gt "$a0" ] || fail "it went quiet"
    click '[data-t="call-bar"]' 0 0.4
    on_screen '[data-t="room-hand"]' 6 && pass || fail "cannot get back"
else
    fail "no bar"
fi

scenario leaving
window_a
click '[data-t="call-hangup"]' 0 0.5
gone '[data-t="room-hand"]' 8 || fail "A is still in"
window_b
sleep 1
is_there '[data-t="room-hand"]' || fail "the room ended for B when A left"
picture alone_again
click '[data-t="call-hangup"]' 0 0.5
gone '[data-t="room-hand"]' 8 && pass || fail "B is still in"

summary
