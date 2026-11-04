//! Nothing is ever silently stuck: every message arrives once, through kills and restarts, or says
//! plainly that it did not. Real cores over loopback QUIC.

use std::collections::HashMap;
use std::time::Duration;

use whatsmine_core::power::{Device, Network};
use whatsmine_core::records::LocalRecords;
use whatsmine_core::{ContactState, Core, CoreEvent, MessageStatus, NetMode, Settings};

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let fut = async {
        while !cond() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(90), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

fn start(dir: &std::path::Path, net: &LocalRecords) -> Core {
    Core::start(dir, NetMode::Local(net.clone())).unwrap()
}

async fn befriend(a: &Core, b: &Core) {
    a.add_contact(&b.invite_link(), None).unwrap();
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

fn on_mobile_data() -> Device {
    Device {
        network: Network::Mobile,
        charging: false,
        power_save: false,
        low_battery: false,
    }
}

async fn resting(core: &Core) {
    let fut = async {
        while core.connection_report().await.unwrap().endpoint_up {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(70), fut)
        .await
        .expect("the endpoint to go down");
}

/// Sender and receiver get killed at random moments, and every message arrives exactly once.
#[tokio::test(flavor = "multi_thread")]
async fn every_message_arrives_once_through_restarts() {
    let net = LocalRecords::default();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = start(dir_a.path(), &net);
    let mut b = start(dir_b.path(), &net);
    befriend(&a, &b).await;
    let (ka, kb) = (a.my_key(), b.my_key());

    let mut sent_by_a = Vec::new();
    let mut sent_by_b = Vec::new();
    // A fixed pattern that still lands on every kind of moment: messages in flight, sessions
    // half made, an encrypted session just used.
    for round in 0..12u64 {
        for n in 0..3 {
            sent_by_a.push(a.send(&kb, &format!("a {round} {n}")).unwrap().id);
            sent_by_b.push(b.send(&ka, &format!("b {round} {n}")).unwrap().id);
        }
        tokio::time::sleep(Duration::from_millis(round * 37 % 90)).await;
        if round % 3 == 0 {
            b.shutdown();
            drop(b);
            b = start(dir_b.path(), &net);
        }
        if round % 4 == 1 {
            a.shutdown();
            drop(a);
            a = start(dir_a.path(), &net);
        }
    }

    until("everything of Alice's to reach Bob", || {
        let got: Vec<String> = b
            .messages(&ka, None, 1000)
            .into_iter()
            .map(|m| m.id)
            .collect();
        sent_by_a.iter().all(|id| got.contains(id))
    })
    .await;
    until("everything of Bob's to reach Alice", || {
        let got: Vec<String> = a
            .messages(&kb, None, 1000)
            .into_iter()
            .map(|m| m.id)
            .collect();
        sent_by_b.iter().all(|id| got.contains(id))
    })
    .await;
    for (core, peer, sent) in [(&b, &ka, &sent_by_a), (&a, &kb, &sent_by_b)] {
        let mut seen: HashMap<String, u32> = HashMap::new();
        for m in core.messages(peer, None, 1000) {
            if !m.outgoing {
                *seen.entry(m.id).or_default() += 1;
            }
        }
        assert_eq!(seen.len(), sent.len(), "nothing extra");
        assert!(seen.values().all(|n| *n == 1), "nothing twice");
    }
    until("Alice to see all delivered", || {
        sent_by_a.iter().all(|id| {
            a.message(id)
                .is_some_and(|m| m.status >= MessageStatus::Delivered)
        })
    })
    .await;
    a.shutdown();
    b.shutdown();
}

/// Both asleep on mobile data: a short text still gets through, sealed in the DHT.
#[tokio::test(flavor = "multi_thread")]
async fn both_asleep_a_short_text_travels_in_a_knock() {
    let net = LocalRecords::default();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(dir_a.path(), &net);
    let b = start(dir_b.path(), &net);
    befriend(&a, &b).await;
    let (ka, kb) = (a.my_key(), b.my_key());
    // Encrypted sessions exist from here on, also after the restarts.
    let first = a.send(&kb, "first").unwrap();
    until("the first message", || {
        a.message(&first.id)
            .is_some_and(|m| m.status >= MessageStatus::Delivered)
    })
    .await;
    let second = b.send(&ka, "second").unwrap();
    until("the reply", || {
        b.message(&second.id)
            .is_some_and(|m| m.status >= MessageStatus::Delivered)
    })
    .await;
    a.shutdown();
    b.shutdown();
    drop((a, b));

    // Bob is asleep with nothing held.
    let b = start(dir_b.path(), &net);
    b.set_device(on_mobile_data());
    resting(&b).await;
    let a = start(dir_a.path(), &net);
    let m = a.send(&kb, "see you at 7").unwrap();
    // Alice tries, finds nobody, and knocks. Then she goes to sleep too: no connection is possible.
    until("the knock to be left", || {
        a.delivery_info(&m.id).is_some_and(|i| i.knocked)
    })
    .await;
    a.shutdown();
    drop(a);

    // Bob's next check finds it.
    b.wake();
    until("Bob to read it", || {
        b.messages(&ka, None, 10)
            .iter()
            .any(|x| x.id == m.id && x.body == "see you at 7")
    })
    .await;
    b.shutdown();
}

/// Past the time set, a message says it did not get there, and can be tried again.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_gives_up_when_told_and_can_be_retried() {
    let net = LocalRecords::default();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(dir_a.path(), &net);
    let b = start(dir_b.path(), &net);
    befriend(&a, &b).await;
    let (ka, kb) = (a.my_key(), b.my_key());
    b.shutdown();
    drop(b);
    let mut events = a.subscribe();
    let m = a.send(&kb, "are you there").unwrap();
    assert_eq!(a.message(&m.id).unwrap().status, MessageStatus::Queued);

    a.give_up_now();
    until("it to say so", || {
        a.message(&m.id).unwrap().status == MessageStatus::NotDelivered
    })
    .await;
    let told = async {
        loop {
            if let Ok(CoreEvent::MessageStatus {
                message_id, status, ..
            }) = events.recv().await
                && message_id == m.id
                && status == MessageStatus::NotDelivered
            {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), told)
        .await
        .expect("the screen to be told");
    let info = a.delivery_info(&m.id).unwrap();
    assert_eq!(info.next_try_in_ms, None);

    // Giving up means nobody is dialled for it any more. Trying again brings it back.
    a.retry_message(&m.id);
    assert_eq!(a.message(&m.id).unwrap().status, MessageStatus::Queued);
    let b = start(dir_b.path(), &net);
    until("it to arrive after all", || {
        b.messages(&ka, None, 10).iter().any(|x| x.id == m.id)
    })
    .await;
    until("it to say delivered", || {
        a.message(&m.id).unwrap().status >= MessageStatus::Delivered
    })
    .await;
    a.shutdown();
    b.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn it_can_be_asked_when_the_next_try_is_and_why() {
    let net = LocalRecords::default();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(dir_a.path(), &net);
    let b = start(dir_b.path(), &net);
    befriend(&a, &b).await;
    let kb = b.my_key();
    b.shutdown();
    drop(b);
    let m = a.send(&kb, "hello?").unwrap();
    until("a next try to be known", || {
        a.delivery_info(&m.id)
            .is_some_and(|i| i.next_try_in_ms.is_some() && !i.connected)
    })
    .await;
    a.shutdown();
}

/// A clock that is wrong on the other side must not put a message in the future.
#[tokio::test(flavor = "multi_thread")]
async fn a_keep_trying_setting_is_kept() {
    let net = LocalRecords::default();
    let dir = tempfile::tempdir().unwrap();
    let a = start(dir.path(), &net);
    assert_eq!(a.settings().keep_trying_days, 7);
    a.save_settings(&Settings {
        keep_trying_days: 30,
        ..a.settings()
    })
    .unwrap();
    assert_eq!(a.settings().keep_trying_days, 30);
    a.shutdown();
}
