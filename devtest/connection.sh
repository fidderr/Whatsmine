#!/bin/bash
# What the connection does when the device changes under it: the real Android signals, on an
# emulator. Each step changes one thing and asks the app what it made of it.
#
#   SERIAL=emulator-5554 bash devtest/connection.sh
PLATFORM=android
. "$(dirname "$0")/lib.sh"

on_emulator || { echo "this switches the network off and on: emulators only"; exit 1; }
APK="${APK:-target/dx/whatsmine/debug/android/app/app/build/outputs/apk/debug/app-debug.apk}"
built_fresh "$APK" || exit 1
device_ready || { echo "STOP  the device is not there"; exit 1; }
install_app "$APK" || exit 1

# What the app says about itself.
report() {
    as_app "cd files && rm -f debug_report.txt && echo report > debug_action.tmp && mv debug_action.tmp debug_action.txt && i=0; while [ ! -e debug_report.txt ] && [ \$i -lt 300 ]; do sleep 0.01; i=\$((i + 1)); done; cat debug_report.txt 2> /dev/null; cd .."
}
level_is() { case "$(report)" in *"level=$1 "*) return 0 ;; *) return 1 ;; esac; }
up_is() { case "$(report)" in *"up=$1 "*) return 0 ;; *) return 1 ;; esac; }
expect() { # level up what
    if wait_for 25 level_is "$1" && wait_for 75 up_is "$2"; then
        pass "$3: $(report)"
    else
        fail "$3: wanted level=$1 up=$2, it says: $(report)"
    fi
}

wifi() { dev svc wifi "$1"; }
data() { dev svc data "$1"; }
put_back() {
    wifi enable
    data enable
    dev dumpsys battery reset > /dev/null
    dev cmd power set-mode 0 > /dev/null
    dev cmd connectivity airplane-mode disable > /dev/null 2>&1
}
trap put_back EXIT
put_back

scenario in_front
fresh "demo name=Roy theme=dark locale=en"
on_screen .bottom-nav 60 || fail "no chat list"
expect InUse true "on screen"

scenario network_gone
wifi disable
data disable
expect NoNetwork false "nothing to connect to"
if on_screen '.net-banner, .banner' 3; then note "the app says so"; fi
picture no_network

scenario network_back
wifi enable
data enable
expect InUse true "back on the network"
picture network_back

scenario to_the_background_on_wifi
dev input keyevent KEYCODE_HOME
dev dumpsys battery unplug > /dev/null
expect Reachable true "in the background on Wi-Fi"

scenario to_mobile_data_in_the_background
wifi disable
expect Resting false "in the background on mobile data: holds nothing"
if adb_ shell "dumpsys alarm | grep -q SENTINEL_WAKE"; then
    pass "the next check is with the system"
else
    fail "no wake-up asked of the system"
fi

scenario charger
dev dumpsys battery set ac 1 > /dev/null
dev dumpsys battery set status 2 > /dev/null
expect Reachable true "on the charger: reachable again"
dev dumpsys battery unplug > /dev/null
expect Resting false "off the charger: resting again"

scenario battery_saver
wifi enable
expect Reachable true "back on Wi-Fi"
dev cmd power set-mode 1 > /dev/null
expect Saver false "the system's battery saver"
dev cmd power set-mode 0 > /dev/null
expect Reachable true "battery saver off"

scenario open_again
start_app
expect InUse true "opened again"
wifi disable
data enable
expect InUse true "from Wi-Fi to mobile data with the app open"
wifi enable

summary
stop_app
[ -z "${KEEP_EMULATOR:-}" ] && emulator_stop
exit 0
