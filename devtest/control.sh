#!/bin/bash
# Two real app windows: one lets the other control its screen. Nothing real moves: in a test run the
# app writes down what it would have done (debug_control.txt) instead of touching the mouse or the
# keyboard. The screen is a painted picture, and the windows stay off-screen.
#
#   bash devtest/control.sh
PLATFORM=windows
. "$(dirname "$0")/lib.sh"

SHOT_DIR="$OUT/control"
mkdir -p "$SHOT_DIR"
export WEBVIEW2_EXTRA="--use-fake-device-for-media-stream --use-fake-ui-for-media-stream"
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
        "$EXE" >> "$OUT/control_$profile.log" 2>&1 &
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

# Which window the next `js`, `picture` and `core` talk to. A owns the PC, B controls it.
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

# What the owner's PC would have done.
did() { cat "$A_DATA/debug_control.txt" 2> /dev/null; }
did_lines() { did | wc -l | tr -d ' '; }
did_since() { did | tail -n +"$(($1 + 1))"; }
last_line_is() { [ "$(did | tail -1)" = "$1" ]; }

# On B: what a hand on the picture would do. The picture may have bars around it.
hand() { # kind x-fraction y-fraction button
    js "var t = document.querySelector('.wm-screen'), v = t.querySelector('video'), b = v.getBoundingClientRect(), s = Math.min(b.width / v.videoWidth, b.height / v.videoHeight), w = v.videoWidth * s, h = v.videoHeight * s; t.dispatchEvent(new PointerEvent('$1', { bubbles: true, cancelable: true, pointerId: 1, pointerType: 'mouse', button: ${4:-0}, clientX: b.left + (b.width - w) / 2 + w * $2, clientY: b.top + (b.height - h) / 2 + h * $3 })); return 'ok'" > /dev/null
}
press_key() { # keydown or keyup, code
    js "document.dispatchEvent(new KeyboardEvent('$1', { code: '$2', bubbles: true, cancelable: true })); return 'ok'" > /dev/null
}

start_call_and_share() {
    window_a
    js "wmCall.usePaintedScreen(true); return 'ok'" > /dev/null
    open_chat() { js "var r = [...document.querySelectorAll('.chat-row')].find(x => x.textContent.includes('Noor')); if (!r) return 'no'; r.click(); return 'ok'"; }
    chat_row_there() { is_there '[data-t="call-video"]' || [ "$(open_chat)" = ok ]; }
    wait_for 30 chat_row_there || { fail "no chat"; return 1; }
    on_screen '[data-t="call-video"]' 20 || { fail "no call button"; return 1; }
    click '[data-t="call-video"]' 0 0.3
    window_b
    on_screen '[data-t="call-accept"]' 30 || { fail "B never rang"; return 1; }
    click '[data-t="call-accept"]' 0 0.5
    on_screen '[data-t="call-share"]' 30 || { fail "no call on B"; return 1; }
    window_a
    on_screen '[data-t="call-share"]' 30 || { fail "no call on A"; return 1; }
    click '[data-t="call-share"]' 0 0.3
    on_screen '[data-t="share-go"]' 5
    click '[data-t="share-go"]' 0 0.5
    on_screen '[data-t="call-sharing"]' 15 || { fail "not sharing"; return 1; }
    window_b
    has_picture() { [ "$(js "var i = wmCall.screenInfo(); return i && i.colors > 3 ? 'y' : 'n'")" = y ]; }
    wait_for 30 has_picture || { fail "no picture on B"; return 1; }
}

hang_up() {
    window_a
    click '[data-t="call-hangup"]' 0 0.5
    gone '[data-t="call-hangup"]' 10
    window_b
    gone '[data-t="call-hangup"]' 15
}

start_window "$A_PROFILE" "$A_PORT" "name=Roy theme=dark locale=en"
A_PID=$!
start_window "$B_PROFILE" "$B_PORT" "name=Noor theme=light locale=en"
B_PID=$!

scenario they_become_contacts
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
if wait_for 120 requests_seen; then
    core "accept $KEY_A" > /dev/null
    window_a
    online_b() { [ "$(core "online $KEY_B")" = true ]; }
    wait_for 60 online_b && pass || fail "they never connected"
else
    fail "the request never came"
fi

scenario turned_off_nothing_happens
if start_call_and_share; then
    window_b
    on_screen '[data-t="control-ask"]' 10 || fail "no way to ask"
    click '[data-t="control-ask"]' 0 0.2
    sleep 1.5
    picture off_asker
    window_a
    is_there '[data-t="control-sheet"]' && fail "asked while it is off"
    picture off_owner
    window_b
    if is_there '[data-t="control-stop"]'; then
        fail "let in while it is off"
    else
        pass
    fi
fi
hang_up

scenario turned_on_but_known_too_short
window_a
core control_on > /dev/null
if start_call_and_share; then
    window_b
    on_screen '[data-t="control-ask"]' 10
    click '[data-t="control-ask"]' 0 0.2
    sleep 1.5
    window_a
    is_there '[data-t="control-sheet"]' && fail "asked for someone new"
    picture too_new_owner
    pass
fi
hang_up

scenario the_owner_is_asked_and_has_to_read_it
window_a
core "known_long $KEY_B" > /dev/null
if start_call_and_share; then
    window_b
    on_screen '[data-t="control-ask"]' 10
    click '[data-t="control-ask"]' 0 0.2
    window_a
    if on_screen '[data-t="control-sheet"]' 10; then
        picture asked
        [ "$(js "return document.querySelector('[data-t=\"control-allow\"]').disabled ? 'y' : 'n'")" = y ] || fail "Allow works at once"
        click '[data-t="control-tick"]' 0 0.2
        [ "$(js "return document.querySelector('[data-t=\"control-allow\"]').disabled ? 'y' : 'n'")" = y ] || fail "ticking is not enough"
        enabled() { [ "$(js "return document.querySelector('[data-t=\"control-allow\"]').disabled ? 'y' : 'n'")" = n ]; }
        wait_for 8 enabled || fail "Allow never came"
        picture ready
        js "return document.querySelector('[data-t=\"control-allow\"]').click(), 'ok'" > /dev/null
        if on_screen '[data-t="control-owner-line"]' 10; then
            sleep 0.5
            picture owner_while_controlled
            window_b
            on_screen '[data-t="control-stop"]' 10 || fail "B does not know it is in control"
            picture driving
            pass
        else
            fail "not on"
        fi
    else
        fail "the owner was never asked"
    fi
fi

scenario what_they_do_arrives
window_b
n0=$(did_lines)
hand pointermove 0.5 0.5
sleep 0.2
hand pointerdown 0.5 0.5
hand pointerup 0.5 0.5
js "document.querySelector('.wm-screen').dispatchEvent(new WheelEvent('wheel', { deltaY: 100, bubbles: true, cancelable: true })); return 'ok'" > /dev/null
press_key keydown KeyA
press_key keyup KeyA
press_key keydown ArrowUp
press_key keyup ArrowUp
arrived() { last_line_is "key 72e up"; }
if wait_for 5 arrived; then
    note "$(did_since "$n0" | tr '\n' ';' | cut -c1-200)"
    # Before each press and the wheel, the pointer goes back to the place that was checked.
    want="to 499,499;to 499,499;to 499,499;Left down;Left up;to 499,499;wheel 0,-120;key 30 down;key 30 up;key 72e down;key 72e up;"
    [ "$(did_since "$n0" | tr '\n' ';')" = "$want" ] && pass || fail "not what was done"
else
    fail "did not arrive: $(did_since "$n0" | tr '\n' ';' | cut -c1-200)"
fi

scenario nothing_lands_on_the_app_itself
# In a test run, the left tenth of the screen plays the app's own window.
n0=$(did_lines)
hand pointermove 0.05 0.5
sleep 0.1
hand pointerdown 0.05 0.5
hand pointerup 0.05 0.5
sleep 0.5
if [ "$(did_lines)" = "$n0" ]; then
    # And what was held elsewhere can still be let go. A press, away, and up.
    hand pointermove 0.5 0.5
    sleep 0.1
    hand pointerdown 0.5 0.5
    sleep 0.1
    hand pointermove 0.05 0.5
    sleep 0.1
    hand pointerup 0.05 0.5
    let_go() { last_line_is "Left up"; }
    wait_for 3 let_go && pass || fail "stuck: $(did | tail -3 | tr '\n' ';')"
else
    fail "it landed: $(did_since "$n0" | tr '\n' ';')"
fi

scenario blocked_keys_do_not_go
n0=$(did_lines)
press_key keydown MetaLeft
press_key keyup MetaLeft
press_key keydown ControlLeft
press_key keydown Escape
press_key keyup Escape
press_key keyup ControlLeft
ctrl_went() { last_line_is "key 29 up"; }
if wait_for 3 ctrl_went; then
    [ "$(did_since "$n0" | tr '\n' ';')" = "key 29 down;key 29 up;" ] && pass || fail "more than Ctrl: $(did_since "$n0" | tr '\n' ';')"
else
    fail "not even Ctrl: $(did_since "$n0" | tr '\n' ';')"
fi

scenario the_owners_hand_wins
window_a
core control_busy > /dev/null
window_b
n0=$(did_lines)
hand pointermove 0.6 0.6
sleep 0.5
[ "$(did_lines)" = "$n0" ] || fail "went through while the owner was busy"
sleep 2
hand pointermove 0.7 0.7
went_on() { [ "$(did_lines)" -gt "$n0" ]; }
wait_for 3 went_on && pass || fail "never went on"

scenario when_the_screen_locks_it_ends_and_nothing_stays_held
press_key keydown ControlLeft
held() { last_line_is "key 29 down"; }
wait_for 3 held || note "ctrl did not arrive"
: > "$A_DATA/debug_locked.txt"
window_a
gone '[data-t="control-owner-line"]' 5 || fail "still on after the lock"
sleep 0.3
picture ended_by_lock
last_line_is "key 29 up" || fail "Ctrl stayed down: $(did | tail -3 | tr '\n' ';')"
window_b
gone '[data-t="control-stop"]' 5 && pass || fail "B thinks it is still on"
press_key keyup ControlLeft
rm -f "$A_DATA/debug_locked.txt"

scenario after_a_no_nobody_gets_to_ask_again
window_b
on_screen '[data-t="control-ask"]' 5
click '[data-t="control-ask"]' 0 0.2
window_a
if on_screen '[data-t="control-sheet"]' 10; then
    click '[data-t="control-no"]' 0 0.3
    gone '[data-t="control-sheet"]' 5
    window_b
    sleep 1
    picture said_no
    click '[data-t="control-ask"]' 0 0.2
    window_a
    sleep 1.5
    if is_there '[data-t="control-sheet"]'; then
        fail "asked again right after a no"
    else
        pass
    fi
else
    fail "not asked"
fi

scenario stopping_sharing_leaves_nothing_in_control_and_it_is_written_down
hang_up
window_a
on_screen '[data-t="chat-note"]' 5 || fail "no line in the chat"
picture chat_line
[ "$(core control_log)" = 1 ] && pass "1 line" || fail "log: $(core control_log)"

scenario the_list_in_privacy
window_a
go_back
on_screen '[data-t="tab-settings"]' 6
click '[data-t="tab-settings"]' 0 0.4
on_screen '[data-t="privacy"]' 6 && click '[data-t="privacy"]' 0 0.4
if on_screen '[data-t="control-log"]' 6; then
    js "document.querySelector('[data-t=\"control-log\"]').scrollIntoView(); return 'ok'" > /dev/null
    sleep 0.3
    picture privacy_log
    pass
else
    fail "no list"
fi

summary
