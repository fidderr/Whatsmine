//! A device with nothing to do holds nothing: the endpoint goes down, and comes back for a check,
//! for something to send, or when the app is used. Real cores over loopback QUIC.

use std::time::Duration;

use iroh::address_lookup::MemoryLookup;
use whatsmine_core::power::{Device, Level, Network, SpeedMode};
use whatsmine_core::{ContactState, Core, CoreEvent, MessageStatus, NetMode, Settings};

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let fut = async {
        while !cond() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

fn start(dir: &std::path::Path, lookup: &MemoryLookup, name: &str) -> Core {
    let core = Core::start(dir, NetMode::Local(lookup.clone())).unwrap();
    core.save_settings(&Settings {
        display_name: name.into(),
        onboarded: true,
        ..Settings::default()
    })
    .unwrap();
    core
}

async fn befriend(a: &Core, b: &Core) {
    a.add_contact(&b.my_key(), None).unwrap();
    until("the request", || !b.requests().is_empty()).await;
    b.accept(&a.my_key()).unwrap();
    until("being accepted", || {
        a.contact(&b.my_key())
            .is_some_and(|c| c.state == ContactState::Accepted)
    })
    .await;
    until("being connected", || {
        a.is_online(&b.my_key()) && b.is_online(&a.my_key())
    })
    .await;
}

fn phone_on_mobile_data() -> Device {
    Device {
        network: Network::Mobile,
        charging: false,
        power_save: false,
        low_battery: false,
    }
}

async fn endpoint_up(core: &Core) -> bool {
    core.connection_report().await.unwrap().endpoint_up
}

fn status(core: &Core, contact: &str, id: &str) -> MessageStatus {
    core.message(id)
        .filter(|m| m.contact_id == contact)
        .expect("stored")
        .status
}

/// Waits for a report that says so.
async fn until_up(core: &Core, up: bool, what: &str) {
    let fut = async {
        while endpoint_up(core).await != up {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_device_on_mobile_data_holds_nothing_and_asks_to_be_woken() {
    let lookup = MemoryLookup::new();
    let dir = tempfile::tempdir().unwrap();
    let a = start(dir.path(), &lookup, "Alice");
    let mut events = a.subscribe();
    a.set_device(phone_on_mobile_data());
    until_up(&a, false, "the endpoint to go down").await;
    let report = a.connection_report().await.unwrap();
    assert_eq!(report.level, Level::Resting);
    assert!(report.peers.is_empty());

    // It asked for the next check.
    let asked = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(CoreEvent::WakeIn { ms }) = events.recv().await {
                return ms;
            }
        }
    })
    .await
    .expect("asked to be woken");
    assert!(asked <= 5 * 60 * 1000, "{asked} ms");
    a.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resting_device_catches_up_when_woken_and_rests_again() {
    let lookup = MemoryLookup::new();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(dir_a.path(), &lookup, "Alice");
    let b = start(dir_b.path(), &lookup, "Bob");
    befriend(&a, &b).await;
    let ka = a.my_key();
    let kb = b.my_key();

    // Bob's phone goes to sleep. With a recent chat it stays up for a while, so restart it: a
    // phone that has been asleep for long has no recent activity either.
    b.shutdown();
    drop(b);
    let b = Core::start(dir_b.path(), NetMode::Local(lookup.clone())).unwrap();
    b.set_device(phone_on_mobile_data());
    until_up(&b, false, "Bob's endpoint to go down").await;

    // Alice writes. It cannot arrive: nothing of Bob's is there.
    let m = a.send(&kb, "are you there?").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(status(&a, &kb, &m.id) < MessageStatus::Delivered);

    // A check on Bob's side finds it.
    b.wake();
    until("the message to reach Bob", || {
        b.messages(&ka, None, 10).iter().any(|x| x.id == m.id)
    })
    .await;
    until("Alice to hear it arrived", || {
        status(&a, &kb, &m.id) >= MessageStatus::Delivered
    })
    .await;

    // Nothing left to do. Incoming messages count as a conversation, so it stays up as long as
    // that lasts: a chat is likely to go on.
    let report = b.connection_report().await.unwrap();
    assert_eq!(report.level, Level::InUse);
    a.shutdown();
    b.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn writing_from_a_resting_device_brings_the_network_back() {
    let lookup = MemoryLookup::new();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(dir_a.path(), &lookup, "Alice");
    let b = start(dir_b.path(), &lookup, "Bob");
    befriend(&a, &b).await;
    let ka = a.my_key();
    let kb = b.my_key();
    b.shutdown();
    drop(b);
    let b = Core::start(dir_b.path(), NetMode::Local(lookup.clone())).unwrap();
    b.set_device(phone_on_mobile_data());
    until_up(&b, false, "Bob's endpoint to go down").await;

    let m = b.send(&ka, "on my way").unwrap();
    until("Alice to get it", || {
        a.messages(&kb, None, 10).iter().any(|x| x.id == m.id)
    })
    .await;
    assert!(endpoint_up(&b).await);
    a.shutdown();
    b.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_the_app_brings_it_all_back() {
    let lookup = MemoryLookup::new();
    let dir = tempfile::tempdir().unwrap();
    let a = start(dir.path(), &lookup, "Alice");
    a.set_device(phone_on_mobile_data());
    until_up(&a, false, "the endpoint to go down").await;
    a.set_in_front(true);
    until_up(&a, true, "the endpoint to come back").await;
    assert_eq!(a.connection_report().await.unwrap().level, Level::InUse);
    a.set_in_front(false);
    until_up(&a, false, "the endpoint to go down again").await;
    a.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn no_network_is_offline_and_a_network_brings_it_back() {
    let lookup = MemoryLookup::new();
    let dir = tempfile::tempdir().unwrap();
    let a = start(dir.path(), &lookup, "Alice");
    until_up(&a, true, "the endpoint to come up").await;
    a.set_device(Device {
        network: Network::None,
        ..phone_on_mobile_data()
    });
    until_up(&a, false, "the endpoint to go down").await;
    until("offline to show", || {
        a.net_status() == whatsmine_core::NetStatus::Offline
    })
    .await;
    assert_eq!(a.connection_report().await.unwrap().level, Level::NoNetwork);
    a.set_device(Device {
        network: Network::Wifi,
        ..phone_on_mobile_data()
    });
    until_up(&a, true, "the endpoint to come back").await;
    until("online to show", || {
        a.net_status() == whatsmine_core::NetStatus::Online
    })
    .await;
    a.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_setting_decides_on_mobile_data() {
    let lookup = MemoryLookup::new();
    let dir = tempfile::tempdir().unwrap();
    let a = start(dir.path(), &lookup, "Alice");
    a.set_device(phone_on_mobile_data());
    until_up(&a, false, "the endpoint to go down").await;
    a.save_settings(&Settings {
        speed_mode: SpeedMode::AlwaysInstant,
        ..a.settings()
    })
    .unwrap();
    until_up(&a, true, "always instant to hold it").await;
    a.save_settings(&Settings {
        speed_mode: SpeedMode::BatterySaver,
        ..a.settings()
    })
    .unwrap();
    until_up(&a, false, "battery saver to let go").await;
    assert_eq!(a.connection_report().await.unwrap().level, Level::Saver);
    a.shutdown();
}

/// A connection that is not used costs a heartbeat every few seconds, so it goes.
#[tokio::test(flavor = "multi_thread")]
async fn after_a_check_nothing_stays_open() {
    let lookup = MemoryLookup::new();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(dir_a.path(), &lookup, "Alice");
    let b = start(dir_b.path(), &lookup, "Bob");
    befriend(&a, &b).await;
    let ka = a.my_key();
    b.shutdown();
    drop(b);
    let b = Core::start(dir_b.path(), NetMode::Local(lookup.clone())).unwrap();
    b.set_device(phone_on_mobile_data());
    until_up(&b, false, "Bob's endpoint to go down").await;
    b.wake();
    until("Bob to find Alice", || b.is_online(&ka)).await;
    // Twenty seconds without traffic, and the thirty a wake-up lasts.
    until_up(&b, false, "Bob to rest again").await;
    assert!(!b.is_online(&ka));
    a.shutdown();
    b.shutdown();
}
