#!/bin/bash
# Every screen and sheet, reached by clicking what a person clicks, with a picture of each stop
# and a check of what is wrong there. The pictures are for a person to look at: only that finds
# what looks wrong. They end up in <out>/tour/, numbered in the order a person would see them.
#
#   PLATFORM=windows bash devtest/tour.sh
#   PLATFORM=android bash devtest/tour.sh
#
# THEME and LANG_ make a variant of it: THEME=light LANG_=nl bash devtest/tour.sh
# PARTS="settings chat" only goes through those parts.
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

wanted() { [ -z "${PARTS:-}" ] || [[ " $PARTS " == *" $1 "* ]]; }
tap() { click "[data-t=\"$1\"]" "${2:-0}" "${3:-0.35}"; }
# Goes to a stop, looks at everything on it, and comes back.
stop_at() { # tag name
    if tap "$1"; then
        look_through "$2"
    else
        fail "no way to $2"
    fi
    press_back_or_up
}
press_back_or_up() {
    go_back
    sleep 0.25
}
sheet_at() { # tag name
    if tap "$1"; then
        picture "$2"
    else
        fail "no sheet $2"
    fi
    press_back
    sleep 0.2
}
first_chat() { click '.chat-list .chat-row' "${1:-0}" 0.4; }
chat_with() { # name
    js "var r = [...document.querySelectorAll('.chat-row')].find(x => x.textContent.includes('$1')); if (!r) return 'no'; r.click(); return 'ok'" > /dev/null
    sleep 0.4
}
home() {
    js "location.hash = ''; return 'ok'" > /dev/null
    tap tab-chats
}

scenario first_run
if wanted first_run; then
    fresh ""
    if on_screen '.start' 60; then
        look_through start_welcome
        type_in 'input[type=text]' 'Roy'
        sleep 0.2
        tap start-next 0 0.3
        if on_screen '[data-t="mode-full"]' 6; then
            look_through start_mode
            tap mode-full 0 0.3
            if on_screen '[data-t="start-done"]' 6; then
                look_through start_allow
                tap start-done 0 0.5
                if on_screen '.bottom-nav' 6; then
                    look_through empty_chats
                    pass
                else
                    fail "first run did not end in the app"
                fi
            else
                fail "no allow step"
            fi
        else
            fail "no mode step"
        fi
    else
        fail "no first-run screen"
    fi
else
    skip
fi

fresh "$SEED"
on_screen '.bottom-nav, .wide-shell' 60 || fail "no app with demo data"

scenario chats
if wanted chats; then
    look_through chats
    js "var r = document.querySelector('.chat-list .chat-row'); r.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })); return 'ok'" > /dev/null
    sleep 0.3
    picture chat_menu
    press_back
    sleep 0.2
    click '.chips.scroll .chip' 1 0.3 && picture chats_unread
    click '.chips.scroll .chip' 0 0.3
    stop_at search search_empty
    if tap search; then
        type_in '[data-t="search-input"]' 'tickets'
        sleep 0.4
        picture search_found
        press_back_or_up
    fi
    if tap folders; then
        picture folders
        tap new-folder 0 0.3 && picture folder_new
        press_back
        press_back_or_up
    fi
    pass
else
    skip
fi

scenario chat
if wanted chat; then
    tap tab-chats
    chat_with Noor
    if on_screen '[data-t="attach"]' 6; then
        look_through chat
        type_in '#composer' 'A message being typed'
        sleep 0.3
        picture chat_typing
        tap attach 0 0.3 && picture chat_attach
        press_back
        sleep 0.2
        tap send-later 0 0.2
        type_in '#composer' ''
        tap chat-more 0 0.3 && picture chat_more
        press_back
        sleep 0.2
        click '.bubble-more' 2 0.3 && picture message_menu
        tap react-more 0 0.2 && picture message_menu_emoji
        press_back
        sleep 0.2
        click '.bubble-more' 3 0.3
        tap remind 0 0.3 && picture message_remind
        press_back
        sleep 0.2
        tap ticks 0 0.4 && picture delivery_sheet
        press_back
        sleep 0.2
        pass
    else
        fail "no chat"
    fi
    press_back_or_up
else
    skip
fi

# Edge to edge, a keyboard changes nothing the page can see: the app has to make room itself.
scenario keyboard
room_made() {
    [ "$(ask_page "var root = document.querySelector('.app-root'); return parseFloat(getComputedStyle(root).getPropertyValue('--safe-bottom')) > 100 ? 'y' : 'n';")" = y ]
}
if wanted chat && keyboard_can_show; then
    tap tab-chats
    chat_with Noor
    on_screen '#composer' 6
    touch_on '#composer'
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
    press_back_or_up
else
    skip "no keyboard on screen here"
fi

scenario group
if wanted group; then
    tap tab-chats
    chat_with Family
    if on_screen '[data-t="poll"]' 6 || on_screen '[data-t="attach"]' 2; then
        look_through group_chat
        tap attach 0 0.3
        tap new-poll 0 0.3 && picture compose_poll
        press_back
        sleep 0.2
        tap attach 0 0.3
        tap new-plan 0 0.3 && picture compose_plan
        press_back
        sleep 0.2
        tap attach 0 0.3
        tap new-list 0 0.3 && picture compose_list
        press_back
        sleep 0.2
        tap attach 0 0.3
        tap new-place 0 0.3 && picture compose_place
        press_back
        sleep 0.2
        tap attach 0 0.3
        tap new-card 0 0.3 && picture compose_card
        press_back
        sleep 0.2
        if tap chat-peer; then
            look_through group_info
            press_back_or_up
        fi
        pass
    else
        fail "no group chat"
    fi
    press_back_or_up
else
    skip
fi

scenario calls
if wanted calls; then
    tap tab-calls
    look_through calls
    pass
else
    skip
fi

scenario people
if wanted people; then
    tap tab-people
    if on_screen '[data-t="add"]' 6; then
        look_through people
        if tap add; then
            look_through add
            if tap type; then
                look_through add_type
                tap read-out 0 1.5
                look_through add_type_code
                press_back_or_up
            fi
            if tap address; then
                look_through address
                tap new-link 0 0.3 && picture address_new_link
                press_back
                sleep 0.2
                tap reset 0 0.3 && picture address_reset
                press_back
                sleep 0.2
                press_back_or_up
            fi
            press_back_or_up
        fi
        tap requests-notice 0 0.3 && look_through requests && press_back_or_up
        tap new-group 0 0.3 && look_through new_group && press_back_or_up
        pass
    else
        fail "no people screen"
    fi
else
    skip
fi

scenario contact
if wanted contact; then
    tap tab-chats
    chat_with Noor
    if tap chat-peer; then
        look_through contact
        for sheet in nickname notes disappear sees words key labels; do
            tap "$sheet" 0 0.3 && picture "contact_$sheet"
            press_back
            sleep 0.2
        done
        if tap look; then
            look_through chat_look
            press_back_or_up
        fi
        if tap shared; then
            look_through shared
            press_back_or_up
        fi
        pass
        press_back_or_up
    else
        fail "no contact screen"
    fi
    press_back_or_up
else
    skip
fi

scenario settings
if wanted settings; then
    tap tab-settings
    if on_screen '[data-t="privacy"]' 6; then
        look_through settings
        tap profile 0 0.3 && picture profile
        press_back
        sleep 0.2
        for part in appearance folders saved notifications privacy speed connection devices storage help; do
            stop_at "$part" "settings_$part"
            on_screen '[data-t="privacy"]' 3 || tap tab-settings
        done
        pass
    else
        fail "no settings"
    fi
else
    skip
fi

scenario simple
if wanted simple; then
    tap tab-settings
    on_screen '[data-t="simple"]' 6
    tap simple 0 0.3
    tap tab-chats 0 0.5
    if on_screen '[data-t="big-person"]' 6; then
        look_through simple_home
        tap big-message 0 0.4
        look_through simple_chat
        press_back_or_up
        tap simple-add 0 0.4
        look_through simple_add
        press_back_or_up
        tap simple-settings 0 0.3 && picture simple_leave
        tap simple-off 0 0.4
        on_screen .bottom-nav 6 && pass || fail "could not leave simple mode"
    else
        fail "no simple home"
    fi
else
    skip
fi

scenario wide
if [ "${PARTS:-}" = wide ]; then
    # Started wide: SIZE=1100x760 PARTS=wide bash devtest/tour.sh
    if on_screen '.wide-shell' 6; then
        picture wide_main
        chat_with Noor
        sleep 0.3
        picture wide_chat
        tap tab-settings 0 0.4
        picture wide_settings
        pass
    else
        fail "no side by side at this size"
    fi
else
    skip
fi

summary
stop_app
[ "$PLATFORM" = android ] && on_emulator && [ -z "${KEEP_EMULATOR:-}" ] && emulator_stop
exit 0
