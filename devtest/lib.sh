#!/bin/bash
# The one library of the app scenarios, for Android and for Windows. What differs lives in
# android.sh and windows.sh; everything here reads the page and presses what a person presses.
#
#   PLATFORM=windows   the desktop app in a window off-screen that never takes the focus, on its
#                      own test profile. Safe next to the real app and next to a game.
#   PLATFORM=android   the test build (com.fidderr.whatsmine.test) on one explicit device, never
#                      the real app. SERIAL says which, an emulator unless told otherwise.
#
# Nothing waits longer than it has to: what can be asked is asked until it is true.

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
PLATFORM="${PLATFORM:-windows}"
OUT="${OUT:-devtest-out/$PLATFORM}"
mkdir -p "$OUT"

# ---- Waiting ----
# Run a check again until it holds, for at most `$1` seconds.
wait_for() {
    local limit=$((SECONDS + $1))
    shift
    until "$@"; do
        [ "$SECONDS" -ge "$limit" ] && return 1
        sleep 0.1
    done
}

. "devtest/$PLATFORM.sh"

# ---- The page ----
# `js <script>` comes from the platform: it runs in the page and prints what the script returns.
# A busy page gets asked again, for what only looks.
ask_page() {
    local out try
    for try in 1 2 3; do
        out=$(js "$1")
        case "$out" in
            *"no answer"*) sleep 0.5 ;;
            *) break ;;
        esac
    done
    echo "$out"
}
is_there() { [ "$(js "return document.querySelector('$1') ? 'y' : 'n'")" = y ]; }
is_not_there() { [ "$(js "return document.querySelector('$1') ? 'y' : 'n'")" = n ]; }
page_is_up() { [ "$(js "return 'y'")" = y ]; }

# Right now, and until it shows or is gone: a screen takes a moment to change. A first load can
# take most of a minute when the system is busy with something else, once per scenario.
PAGE_WAITED=""
on_screen() {
    wait_for "${2:-6}" is_there "$1" && return 0
    [ -z "$PAGE_WAITED" ] || return 1
    PAGE_WAITED=1
    page_is_up && return 1
    local t0=$SECONDS
    wait_for 60 page_is_up || return 1
    note "the page was not up for another $((SECONDS - t0))s"
    wait_for "${2:-6}" is_there "$1"
}
gone() { wait_for "${2:-6}" is_not_there "$1"; }
read_text() { ask_page "var e = document.querySelector('$1'); return e ? e.textContent : ''"; }

# click selector [which of them] [seconds to wait]
click() {
    local said
    said=$(js "var e = document.querySelectorAll('$1')[${2:-0}]; if (!e) return 'nothing like $1'; e.click(); return 'ok'")
    sleep "${3:-0.2}"
    # A click whose answer got lost still happened: what comes next says whether it worked.
    case "$said" in
        "nothing like"*)
            note "click $1: $said"
            return 1
            ;;
    esac
}
# By what a person reads on it: a button or link with this text.
click_text() {
    local said
    said=$(js "var want = '$1'.toLowerCase(); var e = [...document.querySelectorAll('button, a, [role=button], .sheet-option')].find(b => b.textContent.trim().toLowerCase() === want); if (!e) return 'no button says $1'; e.click(); return 'ok'")
    sleep "${2:-0.2}"
    case "$said" in
        "no button says"*)
            note "click_text $1: $said"
            return 1
            ;;
    esac
}
# What a person types goes through the page's own handlers.
type_in() { # selector text
    js "var e = document.querySelector('$1'); if (!e) return 'no field $1'; e.focus(); var set = Object.getOwnPropertyDescriptor(e.constructor.prototype, 'value').set; set.call(e, '$2'); e.dispatchEvent(new Event('input', { bubbles: true })); return 'ok'" > /dev/null
    sleep 0.1
}

# ---- Looking at it ----
# What scrolls: the biggest thing on screen that can.
SCROLLER="var s = [...document.querySelectorAll('*')].filter(e => e.clientHeight > 40 && e.scrollHeight > e.clientHeight + 2 && /auto|scroll/.test(getComputedStyle(e).overflowY)).sort((a, b) => b.clientHeight * b.clientWidth - a.clientHeight * a.clientWidth)[0]; if (!s && document.scrollingElement.scrollHeight > document.scrollingElement.clientHeight + 2) s = document.scrollingElement;"

# What is wrong with what is on screen. At the end of what scrolls also what lies under something
# else, as in between a bar that stays in place is meant to cover what goes by.
layout_check() { # end?
    local script="var bad = [], w = innerWidth;
        var name = e => (e.textContent.trim().slice(0, 24) || e.className || e.tagName);
        document.querySelectorAll('body *').forEach(e => {
            var r = e.getBoundingClientRect();
            if (r.width < 1 || r.height < 1 || getComputedStyle(e).visibility === 'hidden' || e.closest('[id^=dx-toast], [class^=dx-toast]')) return;
            if ((r.right > w + 1 || r.left < -1) && !e.closest('[data-scrolls-sideways]')) bad.push('sticks out: ' + name(e));
            if (e.children.length || !e.textContent.trim()) return;
            if (e.clientWidth && e.scrollWidth > e.clientWidth + 1 && getComputedStyle(e).textOverflow !== 'ellipsis') bad.push('cut off: ' + name(e));
            if (!$1) return;
            var x = r.left + r.width / 2, y = r.top + r.height / 2;
            if (y < 0 || y > innerHeight) return;
            for (var up = e.parentElement; up; up = up.parentElement) {
                if (getComputedStyle(up).overflowY === 'visible') continue;
                var c = up.getBoundingClientRect();
                if (y < c.top || y > c.bottom) return;
            }
            var on = document.elementFromPoint(x, y);
            if (on && on !== e && !e.contains(on) && !on.contains(e) && !on.closest('svg')) bad.push(name(e) + ' under ' + (on.className || on.tagName));
        });
        return [...new Set(bad)].slice(0, 6).join(', ');"
    ask_page "${script//$'\n'/ }"
}

SHOT_N=0
SHOT_DIR="$OUT/tour${VARIANT:-}"
problem() {
    mkdir -p "$SHOT_DIR"
    echo "$SHOT_N  $1" >> "$SHOT_DIR/problems.txt"
    PROBLEMS=$((PROBLEMS + 1))
}
PROBLEMS=0
# The picture is for a person to look at: only that finds what looks wrong.
picture() {
    SHOT_N=$((SHOT_N + 1))
    mkdir -p "$SHOT_DIR"
    take_picture "$SHOT_DIR/$(printf %03d "$SHOT_N")_$1.png"
}
# Every stop of what is on screen: a check and a picture, and the next one by scrolling. It has
# to move when it says there is more.
look_through() { # name
    local name=$1 stop=0 before after end found
    js "$SCROLLER if (s) s.scrollTop = 0; return 'ok';" > /dev/null
    sleep 0.1
    while [ "$stop" -lt 8 ]; do
        stop=$((stop + 1))
        read -r before end <<< "$(ask_page "$SCROLLER return !s ? '0 true' : Math.round(s.scrollTop) + ' ' + (s.scrollTop + s.clientHeight >= s.scrollHeight - 2);")"
        found=$(layout_check "$end")
        [ -z "$found" ] || problem "$name: $found"
        picture "${name}_$stop"
        [ "$end" = true ] && break
        scroll_screen
        after=$(js "$SCROLLER return s ? Math.round(s.scrollTop) : 0;")
        if [ "$after" = "$before" ]; then
            problem "$name: does not scroll"
            break
        fi
    done
}

# ---- Results ----
RESULTS="$OUT/results.txt"
CURRENT=""
SCENARIO_START=$SECONDS
scenario() {
    PAGE_WAITED=""
    CURRENT=$1${VARIANT:-}
    SCENARIO_START=$SECONDS
    echo
    echo "=== $CURRENT"
    device_ok || {
        echo "STOP  $CURRENT  the device does not respond" | tee -a "$RESULTS"
        exit 2
    }
    before_scenario
}
took() { echo "$((SECONDS - SCENARIO_START))s"; }
pass() { echo "PASS  $CURRENT  [$(took)] $1" | tee -a "$RESULTS"; }
skip() { echo "SKIP  $CURRENT  [$(took)] $1" | tee -a "$RESULTS"; }
fail() {
    echo "FAIL  $CURRENT  [$(took)] $1" | tee -a "$RESULTS"
    write_log "$OUT/$CURRENT.log"
}
note() { echo "      $1"; }
summary() {
    echo
    echo "===== SUMMARY $PLATFORM"
    grep -E '^(FAIL|STOP|SKIP)' "$RESULTS" 2> /dev/null
    echo "passed: $(grep -c '^PASS' "$RESULTS" 2> /dev/null)  failed: $(grep -c '^FAIL' "$RESULTS" 2> /dev/null)  layout problems: $PROBLEMS"
}

# A build that fails leaves the last one where it was: a run would then test something older.
built_fresh() { # file
    [ -n "${OLD_BUILD_IS_FINE:-}" ] && return 0
    local built repo
    built=$(stat -c %Y "$1") || return 1
    for repo in . ../mobile-sentinel; do
        if [ "$built" -lt "$(git -C "$repo" log -1 --format=%ct)" ]; then
            echo "$1 is older than the last commit in $repo: build first (or OLD_BUILD_IS_FINE=1)"
            return 1
        fi
    done
}
