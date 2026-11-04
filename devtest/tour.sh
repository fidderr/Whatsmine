#!/bin/bash
# Every screen and sheet, reached by clicking what a person clicks, with a picture of each stop
# and a check of what is wrong there. The pictures are for a person to look at: only that finds
# what looks wrong. They end up in <out>/tour/, numbered in the order a person would see them.
#
#   PLATFORM=windows bash devtest/tour.sh
#   PLATFORM=android bash devtest/tour.sh
#
# THEME and LANG_ make a variant of it: THEME=light LANG_=nl bash devtest/tour.sh
. "$(dirname "$0")/lib.sh"

THEME="${THEME:-dark}"
LANG_="${LANG_:-en}"
VARIANT="_${THEME}_${LANG_}"
SHOT_DIR="$OUT/tour$VARIANT"
SEED="demo name=Roy theme=$THEME locale=$LANG_"

if [ "$PLATFORM" = android ]; then
    APK="${APK:-target/dx/whatsmine/debug/android/app/app/build/outputs/apk/debug/app-debug.apk}"
    built_fresh "$APK" || exit 1
    device_ready || { echo "STOP  the device is not there"; exit 1; }
    install_app "$APK" || exit 1
else
    built_fresh "$EXE" || exit 1
fi

scenario first_run
fresh ""
if on_screen .onboarding 60; then
    look_through onboarding
    pass
else
    fail "no first-run screen"
fi

scenario chats
fresh "$SEED"
if on_screen .bottom-nav 60; then
    look_through chats
    pass
else
    fail "no chat list"
fi

scenario chat
if click '.list > *' 0 0.4 && on_screen .send-btn 6; then
    look_through chat
    type_in 'textarea, input[type=text]' 'A message being typed'
    sleep 0.3
    picture chat_typing
    click_title() { click "[title=\"$1\"]" 0 0.3; }
    click .icon-btn 0 0.4 && picture chat_attach
    press_back
    pass
else
    fail "no chat"
fi

# Edge to edge, a keyboard changes nothing the page can see: the app has to make room itself.
scenario keyboard
room_made() {
    [ "$(ask_page "var root = document.querySelector('.app-root'); return parseFloat(getComputedStyle(root).getPropertyValue('--safe-bottom')) > 100 ? 'y' : 'n';")" = y ]
}
if keyboard_can_show; then
    touch_on 'textarea, input[type=text]'
    # A keyboard that was not running yet takes a while on an emulator.
    if wait_for 20 room_made; then
        sleep 0.5
        picture chat_keyboard
        bottom=$(ask_page "var r = document.querySelector('.send-btn').getBoundingClientRect(), root = document.querySelector('.app-root'); return Math.round(r.bottom) + ' ' + Math.round(innerHeight - parseFloat(getComputedStyle(root).getPropertyValue('--safe-bottom')));")
        read -r button free <<< "$bottom"
        if [ "${button:-99999}" -le "${free:-0}" ]; then
            pass "the send button ends at $button, the keyboard starts at $free"
        else
            fail "the send button ends at $button with the keyboard at $free"
        fi
    else
        picture chat_keyboard_missing
        fail "no room was made for a keyboard: up=$(keyboard_up && echo yes || echo no)"
    fi
    keyboard_down
else
    skip "no keyboard on screen here"
fi
go_back
on_screen .bottom-nav 6 || fail "not back on the chat list"

scenario tabs
for tab in 1 2 3; do
    click .nav-item "$tab" 0.4 || fail "no tab $tab"
    look_through "tab_$tab"
done
pass
click .nav-item 0 0.4

scenario add
if click .fab 0 0.4 && on_screen .config-header 6; then
    look_through add
    pass
else
    fail "no add screen"
fi
go_back

scenario new_group
on_screen .bottom-nav 6
if click .secondary-fab 0 0.4 && on_screen .config-header 6; then
    look_through new_group
    pass
else
    fail "no new group screen"
fi
go_back

summary
stop_app
[ "$PLATFORM" = android ] && on_emulator && [ -z "${KEEP_EMULATOR:-}" ] && emulator_stop
exit 0
