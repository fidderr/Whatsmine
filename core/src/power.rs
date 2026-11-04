//! How much of the network to keep up, right now. A pure function of what the device tells us,
//! so every case can be tested without a device, a clock or a network.
//!
//! An open iroh connection sends a heartbeat every 5 s on each path, and a relay connection
//! pings every 15-20 s from both ends. So a connection is never cheap on a mobile radio, and the
//! only way to let that radio rest is to hold nothing: the endpoint itself goes down, and comes
//! back for a check, for something to send, or when the app is used.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A chat stays "in use" this long after the last message either way.
pub const RECENT_ACTIVITY: Duration = Duration::from_secs(5 * 60);
/// What a connection without traffic gets when the device could be resting.
const IDLE_CLOSE_REACHABLE: Duration = RECENT_ACTIVITY;
/// When holding nothing: once a check has caught up, there is no reason to stay.
const IDLE_CLOSE_RESTING: Duration = Duration::from_secs(20);
const CHECK_RESTING: Duration = Duration::from_secs(5 * 60);
/// Sooner for a while after a conversation: an answer is likely.
const CHECK_AFTER_CONVERSATION: Duration = Duration::from_secs(2 * 60);
const CONVERSATION_LASTS: Duration = Duration::from_secs(30 * 60);
const CHECK_SAVER: Duration = Duration::from_secs(30 * 60);

/// The network the device is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Network {
    None,
    /// Paid by the byte, and a radio that costs battery to keep awake.
    Mobile,
    Wifi,
    /// A PC on a cable, or anything else that is not a phone's radio.
    #[default]
    Wired,
}

/// The setting: how fast against how much battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SpeedMode {
    /// Instant on Wi-Fi and in use, within minutes on sleeping mobile data.
    #[default]
    Automatic,
    /// Stays reachable on mobile data too, for more battery use.
    AlwaysInstant,
    /// Checks now and then.
    BatterySaver,
}

/// What the device says about itself. On a PC nothing ever changes this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub network: Network,
    pub charging: bool,
    /// The system's battery saver.
    pub power_save: bool,
    pub low_battery: bool,
}

impl Default for Device {
    fn default() -> Self {
        Self {
            network: Network::Wired,
            charging: true,
            power_save: false,
            low_battery: false,
        }
    }
}

/// Everything the decision rests on.
#[derive(Debug, Clone, Copy)]
pub struct Conditions {
    pub device: Device,
    pub mode: SpeedMode,
    /// The app is on screen.
    pub in_front: bool,
    /// A call, or a file moving.
    pub busy: bool,
    /// Since the last message either way. `None`: not since the start.
    pub since_activity: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// No network: nothing to do until there is one.
    NoNetwork,
    /// The app is used: connections stay open, typing and online work.
    InUse,
    /// Costs little to stay reachable, so we do.
    Reachable,
    /// Holds nothing, checks every few minutes.
    Resting,
    /// Holds nothing, checks rarely.
    Saver,
}

/// What to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub level: Level,
    /// Keep the endpoint up, reachable through a public address or a relay.
    pub endpoint_up: bool,
    /// Close a connection without traffic after this. `None`: keep it.
    pub idle_close: Option<Duration>,
    /// Bring the endpoint up and look around this often.
    pub check_every: Option<Duration>,
}

pub fn plan(c: &Conditions) -> Plan {
    if c.device.network == Network::None {
        return Plan {
            level: Level::NoNetwork,
            endpoint_up: false,
            idle_close: None,
            check_every: None,
        };
    }
    let recent = c.since_activity.is_some_and(|age| age < RECENT_ACTIVITY);
    if c.in_front || c.busy {
        return Plan {
            level: Level::InUse,
            endpoint_up: true,
            idle_close: None,
            check_every: None,
        };
    }
    if recent {
        return Plan {
            level: Level::InUse,
            endpoint_up: true,
            idle_close: Some(RECENT_ACTIVITY),
            check_every: None,
        };
    }
    let on_battery = !c.device.charging;
    if c.mode == SpeedMode::BatterySaver
        || (on_battery && (c.device.power_save || c.device.low_battery))
    {
        return Plan {
            level: Level::Saver,
            endpoint_up: false,
            idle_close: Some(IDLE_CLOSE_RESTING),
            check_every: Some(CHECK_SAVER),
        };
    }
    if c.mode == SpeedMode::AlwaysInstant
        || c.device.network != Network::Mobile
        || c.device.charging
    {
        return Plan {
            level: Level::Reachable,
            endpoint_up: true,
            idle_close: Some(IDLE_CLOSE_REACHABLE),
            check_every: None,
        };
    }
    let conversation = c.since_activity.is_some_and(|age| age < CONVERSATION_LASTS);
    Plan {
        level: Level::Resting,
        endpoint_up: false,
        idle_close: Some(IDLE_CLOSE_RESTING),
        check_every: Some(if conversation {
            CHECK_AFTER_CONVERSATION
        } else {
            CHECK_RESTING
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone(network: Network) -> Conditions {
        Conditions {
            device: Device {
                network,
                charging: false,
                power_save: false,
                low_battery: false,
            },
            mode: SpeedMode::Automatic,
            in_front: false,
            busy: false,
            since_activity: None,
        }
    }

    #[test]
    fn no_network_holds_nothing_and_does_not_check() {
        let p = plan(&Conditions {
            in_front: true,
            ..phone(Network::None)
        });
        assert_eq!(p.level, Level::NoNetwork);
        assert!(!p.endpoint_up);
        assert_eq!(p.check_every, None);
    }

    #[test]
    fn on_screen_keeps_everything_open() {
        for network in [Network::Mobile, Network::Wifi, Network::Wired] {
            let p = plan(&Conditions {
                in_front: true,
                ..phone(network)
            });
            assert_eq!(p.level, Level::InUse);
            assert!(p.endpoint_up);
            assert_eq!(p.idle_close, None);
        }
    }

    #[test]
    fn a_call_or_a_file_counts_as_use_with_the_screen_off() {
        let p = plan(&Conditions {
            busy: true,
            ..phone(Network::Mobile)
        });
        assert_eq!(p.level, Level::InUse);
        assert_eq!(p.idle_close, None);
    }

    #[test]
    fn a_recent_chat_stays_open_for_a_while_then_rests() {
        let at = |age| {
            plan(&Conditions {
                since_activity: Some(age),
                ..phone(Network::Mobile)
            })
        };
        let during = at(Duration::from_secs(60));
        assert_eq!(during.level, Level::InUse);
        assert_eq!(during.idle_close, Some(RECENT_ACTIVITY));
        let after = at(RECENT_ACTIVITY);
        assert_eq!(after.level, Level::Resting);
        assert_eq!(after.check_every, Some(CHECK_AFTER_CONVERSATION));
        let later = at(CONVERSATION_LASTS);
        assert_eq!(later.check_every, Some(CHECK_RESTING));
    }

    #[test]
    fn wifi_and_charging_stay_reachable_mobile_data_rests() {
        assert_eq!(plan(&phone(Network::Wifi)).level, Level::Reachable);
        assert_eq!(plan(&phone(Network::Wired)).level, Level::Reachable);
        let resting = plan(&phone(Network::Mobile));
        assert_eq!(resting.level, Level::Resting);
        assert!(!resting.endpoint_up);
        let mut charging = phone(Network::Mobile);
        charging.device.charging = true;
        assert_eq!(plan(&charging).level, Level::Reachable);
    }

    #[test]
    fn always_instant_holds_on_mobile_data_too() {
        let p = plan(&Conditions {
            mode: SpeedMode::AlwaysInstant,
            ..phone(Network::Mobile)
        });
        assert_eq!(p.level, Level::Reachable);
        assert!(p.endpoint_up);
    }

    #[test]
    fn battery_saver_wins_over_wifi_but_not_over_use() {
        let mut low = phone(Network::Wifi);
        low.device.low_battery = true;
        assert_eq!(plan(&low).level, Level::Saver);
        let mut saver = phone(Network::Wifi);
        saver.device.power_save = true;
        assert_eq!(plan(&saver).level, Level::Saver);
        assert_eq!(plan(&saver).check_every, Some(CHECK_SAVER));
        let chosen = Conditions {
            mode: SpeedMode::BatterySaver,
            ..phone(Network::Wired)
        };
        assert_eq!(plan(&chosen).level, Level::Saver);
        assert_eq!(
            plan(&Conditions {
                in_front: true,
                ..chosen
            })
            .level,
            Level::InUse
        );
        // On the charger low battery is no reason to go quiet.
        let mut charging = low;
        charging.device.charging = true;
        assert_eq!(plan(&charging).level, Level::Reachable);
    }

    /// A resting device always has a way back; one that holds the endpoint needs none.
    #[test]
    fn nothing_held_always_means_a_check_is_planned() {
        for network in [Network::Mobile, Network::Wifi, Network::Wired] {
            for mode in [
                SpeedMode::Automatic,
                SpeedMode::AlwaysInstant,
                SpeedMode::BatterySaver,
            ] {
                for charging in [false, true] {
                    for power_save in [false, true] {
                        let mut c = phone(network);
                        c.mode = mode;
                        c.device.charging = charging;
                        c.device.power_save = power_save;
                        let p = plan(&c);
                        assert_eq!(p.endpoint_up, p.check_every.is_none(), "{c:?}");
                        assert!(p.endpoint_up || p.idle_close.is_some(), "{c:?}");
                    }
                }
            }
        }
    }
}
