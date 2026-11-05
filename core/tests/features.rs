//! What a chat can do: replies, reactions, edits, taking back, rich items, send later,
//! disappearing messages, search, saved items and the list of calls.

use std::time::Duration;

use whatsmine_core::model::now_ms;
use whatsmine_core::records::LocalRecords;
use whatsmine_core::rich::{ChatPrefs, PlanAnswer, Rich, RichChange, SearchHit, SearchIn};
use whatsmine_core::{ContactState, Core, MessageStatus, NetMode};

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

struct Pair {
    a: Core,
    b: Core,
    ka: String,
    kb: String,
    net: LocalRecords,
    _a: tempfile::TempDir,
    dir_b: tempfile::TempDir,
}

async fn pair() -> Pair {
    let net = LocalRecords::default();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Core::start(dir_a.path(), NetMode::Local(net.clone())).unwrap();
    let b = Core::start(dir_b.path(), NetMode::Local(net.clone())).unwrap();
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
    Pair {
        ka: a.my_key(),
        kb: b.my_key(),
        a,
        b,
        net,
        _a: dir_a,
        dir_b,
    }
}

impl Pair {
    /// Alice writes, and it arrives.
    async fn sent(&self, text: &str) -> String {
        let m = self.a.send(&self.kb, text).unwrap();
        until("it to arrive", || self.b.message(&m.id).is_some()).await;
        m.id
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_says_what_to() {
    let p = pair().await;
    let first = p.sent("Did you get the tickets?").await;
    let reply = p.b.send_reply(&p.ka, "Yes, two", Some(&first)).unwrap();
    until("the reply", || p.a.message(&reply.id).is_some()).await;
    assert_eq!(
        p.a.message(&reply.id).unwrap().extra.reply_to.as_deref(),
        Some(first.as_str())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reactions_edits_and_taking_back_reach_the_other_side() {
    let p = pair().await;
    let id = p.sent("see you at 7").await;

    p.b.react(&id, Some("👍"));
    until("the reaction", || {
        p.a.message(&id).unwrap().extra.reactions.len() == 1
    })
    .await;
    p.b.react(&id, None);
    until("the reaction to go", || {
        p.a.message(&id).unwrap().extra.reactions.is_empty()
    })
    .await;

    p.a.edit_message(&id, "see you at 8").unwrap();
    until("the edit", || {
        p.b.message(&id).unwrap().body == "see you at 8"
    })
    .await;
    assert!(p.b.message(&id).unwrap().extra.edited);

    // Only whoever wrote it can.
    assert!(p.b.edit_message(&id, "hacked").is_err());
    assert!(p.b.unsend_message(&id).is_err());

    p.a.unsend_message(&id).unwrap();
    until("it to be taken back", || {
        p.b.message(&id).unwrap().extra.unsent
    })
    .await;
    assert!(p.b.message(&id).unwrap().body.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_waits_for_someone_who_is_away() {
    let mut p = pair().await;
    let id = p.sent("meet at the station").await;
    let kb = p.kb.clone();
    p.b.shutdown();
    p.a.edit_message(&id, "meet at the bridge").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    p.b = Core::start(p.dir_b.path(), NetMode::Local(p.net.clone())).unwrap();
    assert_eq!(p.b.my_key(), kb);
    until("the edit after coming back", || {
        p.b.message(&id).unwrap().body == "meet at the bridge"
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn polls_lists_plans_and_places_work_for_both() {
    let p = pair().await;
    let poll =
        p.a.send_rich(
            &p.kb,
            Rich::Poll {
                question: "Which evening?".into(),
                options: vec!["Thursday".into(), "Friday".into()],
                votes: Vec::new(),
                multiple: false,
            },
        )
        .unwrap();
    until("the poll", || p.b.message(&poll.id).is_some()).await;
    p.b.change_rich(
        &poll.id,
        RichChange::Vote {
            option: 1,
            on: true,
        },
    );
    until("the vote", || {
        matches!(p.a.message(&poll.id).unwrap().extra.rich, Some(Rich::Poll { ref votes, .. }) if votes.len() == 1)
    })
    .await;

    let plan =
        p.a.send_rich(
            &p.kb,
            Rich::Plan {
                title: "Bouldering night".into(),
                at: now_ms() + 86_400_000,
                place: "Sterk hall".into(),
                answers: Vec::new(),
            },
        )
        .unwrap();
    until("the plan", || p.b.message(&plan.id).is_some()).await;
    p.b.change_rich(&plan.id, RichChange::Answer(PlanAnswer::Going));
    until("the answer", || {
        matches!(p.a.message(&plan.id).unwrap().extra.rich, Some(Rich::Plan { ref answers, .. }) if answers.len() == 1)
    })
    .await;

    let place =
        p.a.send_rich(
            &p.kb,
            Rich::Place {
                lat: 52.01,
                lon: 4.71,
                label: "Gouda".into(),
                live_until: None,
            },
        )
        .unwrap();
    until("the place", || p.b.message(&place.id).is_some()).await;
    assert_eq!(
        p.b.message(&place.id)
            .unwrap()
            .extra
            .rich
            .unwrap()
            .summary(),
        "Gouda"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn send_later_waits_for_its_moment() {
    let p = pair().await;
    let m =
        p.a.send_later(&p.kb, "Happy birthday!", now_ms() + 1500)
            .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(p.b.message(&m.id).is_none(), "it went too early");
    assert_eq!(p.a.scheduled().len(), 1);
    until("it to go", || p.b.message(&m.id).is_some()).await;
    until("it to leave the list", || p.a.scheduled().is_empty()).await;
    until("it to be delivered", || {
        p.a.message(&m.id).unwrap().status >= MessageStatus::Delivered
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disappearing_message_goes_on_both_sides() {
    let p = pair().await;
    p.a.set_chat_prefs(
        &p.kb,
        &ChatPrefs {
            disappear_ms: Some(1500),
            ..ChatPrefs::default()
        },
    );
    let id = p.sent("gone soon").await;
    until("it to go here", || p.a.message(&id).is_none()).await;
    until("it to go there", || p.b.message(&id).is_none()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reminder_comes_back_and_saved_items_stay() {
    let p = pair().await;
    let id = p.sent("Appointment on Thursday").await;
    let mut events = p.b.subscribe();
    p.b.remind_me(&id, now_ms() + 300);
    assert_eq!(p.b.reminders().len(), 1);
    let fired = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(whatsmine_core::CoreEvent::Reminder { message_id }) = events.recv().await {
                return message_id;
            }
        }
    })
    .await
    .expect("the reminder");
    assert_eq!(fired, id);
    until("it to leave the list", || p.b.reminders().is_empty()).await;

    p.b.save_message(&id, &["Plans".to_owned()]);
    let saved = p.b.saved_messages();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].extra.tags, ["Plans"]);
    p.b.unsave_message(&id);
    assert!(p.b.saved_messages().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn search_finds_people_and_messages() {
    let p = pair().await;
    p.b.rename(&p.ka, "Aunt Noor");
    let id = p.sent("Do they still have tickets left").await;
    p.sent("nothing to see here").await;
    let hits = p.b.search("TICKETS", SearchIn::All);
    assert!(hits.contains(&SearchHit::Message {
        chat: p.ka.clone(),
        id
    }));
    assert_eq!(hits.len(), 1);
    assert_eq!(
        p.b.search("noor", SearchIn::People),
        vec![SearchHit::Person(p.ka.clone())]
    );
    assert!(p.b.search("noor", SearchIn::Files).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_preferences_and_folders_are_kept() {
    let p = pair().await;
    p.a.set_chat_prefs(
        &p.kb,
        &ChatPrefs {
            labels: vec!["Work".into()],
            favourite: true,
            ..ChatPrefs::default()
        },
    );
    assert!(p.a.chat_prefs(&p.kb).favourite);
    let folder = whatsmine_core::rich::Folder {
        name: "Work".into(),
        labels: vec!["Work".into()],
        ..Default::default()
    };
    p.a.save_folders(std::slice::from_ref(&folder));
    assert_eq!(p.a.folders(), std::slice::from_ref(&folder));
    assert_eq!(p.a.chats_in(&folder).len(), 1);
    let other = whatsmine_core::rich::Folder {
        name: "Groups".into(),
        groups: true,
        ..Default::default()
    };
    assert!(p.a.chats_in(&other).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_that_nobody_answered_stays_on_the_list() {
    let p = pair().await;
    p.a.call_invite(&p.kb, false).unwrap();
    until("it to ring", || p.b.call_snapshot().is_some()).await;
    p.a.call_end();
    until("the list on both sides", || {
        !p.a.call_records().is_empty() && !p.b.call_records().is_empty()
    })
    .await;
    let theirs = &p.b.call_records()[0];
    assert!(theirs.missed);
    assert!(!theirs.outgoing);
    assert_eq!(theirs.contact_id, p.ka);
    assert!(p.a.call_records()[0].outgoing);
}
