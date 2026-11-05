//! Controlling a screen between two real cores over loopback QUIC. Nothing here touches a desktop:
//! what these tests watch is what the core would hand on to it.

use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use whatsmine_core::calls::CallEvent;
use whatsmine_core::control::{
    Batch, Ended, Input, KNOWN_FOR, READ_FIRST, RawFrame, Refusal, Scope,
};
use whatsmine_core::protocol::{ControlKey, ControlReply, Frame, id_to_bytes};
use whatsmine_core::records::LocalRecords as MemoryLookup;
use whatsmine_core::{Core, CoreEvent, NetMode, Settings, now_ms};

const WAIT: Duration = Duration::from_secs(30);

fn start(dir: &std::path::Path, lookup: &MemoryLookup, name: &str, control: bool) -> Core {
    let core = Core::start(dir, NetMode::Local(lookup.clone())).unwrap();
    core.save_settings(&Settings {
        display_name: name.into(),
        onboarded: true,
        control_on_since: control.then(|| now_ms() - 1000),
        ..Settings::default()
    })
    .unwrap();
    core
}

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..600 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never: {what}");
}

async fn call_event(
    events: &mut broadcast::Receiver<CoreEvent>,
    what: &str,
    mut want: impl FnMut(&CallEvent) -> bool,
) -> CallEvent {
    let wait = async {
        loop {
            if let Ok(CoreEvent::Call(e)) = events.recv().await
                && want(&e)
            {
                return e;
            }
        }
    };
    tokio::time::timeout(WAIT, wait)
        .await
        .unwrap_or_else(|_| panic!("never: {what}"))
}

/// Owner and someone else, in a call, with the owner showing their screen.
struct Call {
    owner: Core,
    other: Core,
    owner_key: String,
    other_key: String,
    owner_events: broadcast::Receiver<CoreEvent>,
    other_events: broadcast::Receiver<CoreEvent>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

async fn in_a_call(turned_on: bool, known_for_long: bool, can_control: bool) -> Call {
    let lookup = MemoryLookup::default();
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let owner = start(da.path(), &lookup, "Owner", turned_on);
    let other = start(db.path(), &lookup, "Other", false);
    let (owner_key, other_key) = (owner.my_key(), other.my_key());
    let mut owner_events = owner.subscribe();
    let mut other_events = other.subscribe();
    other.add_contact(&owner.invite_link(), None).unwrap();
    let wait = async {
        loop {
            if let Ok(CoreEvent::RequestReceived { .. }) = owner_events.recv().await {
                return;
            }
        }
    };
    tokio::time::timeout(WAIT, wait).await.expect("a request");
    owner.accept(&other_key).unwrap();
    until("online", || {
        owner.is_online(&other_key) && other.is_online(&owner_key)
    })
    .await;
    if known_for_long {
        owner.known_since_as_is(&other_key, now_ms() - KNOWN_FOR - 1000);
    }
    other.call_invite(&owner_key, false).unwrap();
    call_event(&mut owner_events, "ringing", |e| {
        matches!(e, CallEvent::Incoming { .. })
    })
    .await;
    owner.call_answer(true);
    call_event(&mut other_events, "connected", |e| {
        matches!(e, CallEvent::Connected { .. })
    })
    .await;
    call_event(&mut owner_events, "connected", |e| {
        matches!(e, CallEvent::Connected { .. })
    })
    .await;
    until("in the call", || {
        owner
            .call_snapshot()
            .is_some_and(|c| c.peers.contains(&other_key))
    })
    .await;
    owner.call_screen_on(true, 1920, 1080, can_control);
    let shown = call_event(&mut other_events, "the screen", |e| {
        matches!(e, CallEvent::PeerScreen { on: true, .. })
    })
    .await;
    assert!(
        matches!(shown, CallEvent::PeerScreen { can_control: c, .. } if c == can_control),
        "told whether it could be asked"
    );
    Call {
        owner,
        other,
        owner_key,
        other_key,
        owner_events,
        other_events,
        _dirs: (da, db),
    }
}

impl Call {
    /// All the way to someone in control.
    async fn let_in(&mut self) -> mpsc::Receiver<Vec<Input>> {
        self.other.control_ask();
        call_event(&mut self.owner_events, "the question", |e| {
            matches!(e, CallEvent::ControlAsked { .. })
        })
        .await;
        tokio::time::sleep(Duration::from_millis(READ_FIRST as u64 + 100)).await;
        let input = self.owner.control_input();
        self.owner.control_answer(true, Scope::WholeScreen);
        call_event(&mut self.owner_events, "on, for the owner", |e| {
            matches!(e, CallEvent::ControlOn { driving: false, .. })
        })
        .await;
        call_event(&mut self.other_events, "on, for the other", |e| {
            matches!(e, CallEvent::ControlOn { driving: true, .. })
        })
        .await;
        input
    }
}

async fn next_input(input: &mut mpsc::Receiver<Vec<Input>>) -> Vec<Input> {
    tokio::time::timeout(WAIT, input.recv())
        .await
        .expect("input in time")
        .expect("input open")
}

fn point(x: u16) -> Input {
    Input::Move { x, y: 0 }
}

/// Nothing arrives for a moment.
async fn no_input(input: &mut mpsc::Receiver<Vec<Input>>) {
    let got = tokio::time::timeout(Duration::from_millis(600), input.recv()).await;
    assert!(
        !matches!(got, Ok(Some(_))),
        "nothing should get through: {got:?}"
    );
}

async fn refused(call: &mut Call, what: &str, why: Refusal) {
    call.other.control_ask();
    let heard = call_event(&mut call.other_events, what, |e| {
        matches!(e, CallEvent::ControlRefused { .. })
    })
    .await;
    assert!(
        matches!(
            heard,
            CallEvent::ControlRefused {
                why: Refusal::NotPossible,
                ..
            }
        ),
        "the one asking only hears that it cannot be done: {heard:?}"
    );
    let told = call_event(&mut call.owner_events, what, |e| {
        matches!(e, CallEvent::ControlAskedAnyway { .. })
    })
    .await;
    assert!(
        matches!(told, CallEvent::ControlAskedAnyway { why: w, .. } if w == why),
        "the owner hears why: {told:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn off_is_off() {
    let mut call = in_a_call(false, true, true).await;
    refused(&mut call, "turned off", Refusal::TurnedOff).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_it_on_during_the_call_does_not_count_for_that_call() {
    let mut call = in_a_call(false, true, true).await;
    let mut settings = call.owner.settings();
    settings.control_on_since = Some(now_ms());
    call.owner.save_settings(&settings).unwrap();
    refused(&mut call, "turned on too late", Refusal::TurnedOff).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_contact_has_to_wait_a_week() {
    let mut call = in_a_call(true, false, true).await;
    refused(&mut call, "too new", Refusal::TooNew).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_cannot_do_it_safely_says_so() {
    let mut call = in_a_call(true, true, false).await;
    refused(&mut call, "not possible", Refusal::NotPossible).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_means_no_and_no_nagging() {
    let mut call = in_a_call(true, true, true).await;
    call.other.control_ask();
    call_event(&mut call.owner_events, "the question", |e| {
        matches!(e, CallEvent::ControlAsked { .. })
    })
    .await;
    let mut input = call.owner.control_input();
    call.owner.control_answer(false, Scope::Window);
    let heard = call_event(&mut call.other_events, "no", |e| {
        matches!(e, CallEvent::ControlRefused { .. })
    })
    .await;
    assert!(matches!(
        heard,
        CallEvent::ControlRefused {
            why: Refusal::SaidNo,
            ..
        }
    ));
    // Asking again right after does not get to the owner.
    call.other.control_ask();
    let again = call_event(&mut call.other_events, "no again", |e| {
        matches!(e, CallEvent::ControlRefused { .. })
    })
    .await;
    assert!(matches!(
        again,
        CallEvent::ControlRefused {
            why: Refusal::SaidNo,
            ..
        }
    ));
    let told = call_event(&mut call.owner_events, "told of it", |e| {
        matches!(
            e,
            CallEvent::ControlAsked { .. } | CallEvent::ControlAskedAnyway { .. }
        )
    })
    .await;
    assert!(
        matches!(told, CallEvent::ControlAskedAnyway { .. }),
        "no new question: {told:?}"
    );
    no_input(&mut input).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn taking_a_question_back_does_not_let_anyone_keep_asking() {
    let mut call = in_a_call(true, true, true).await;
    call.other.control_ask();
    call_event(&mut call.owner_events, "the question", |e| {
        matches!(e, CallEvent::ControlAsked { .. })
    })
    .await;
    // Stopping while it is on the owner's screen.
    call.other.control_stop();
    let call_id = call.other.call_snapshot().unwrap().call_id;
    call.other.send_as_is(
        &call.owner_key,
        Frame::ControlStop {
            call: id_to_bytes(&call_id).unwrap(),
        },
    );
    call_event(&mut call.owner_events, "it goes away", |e| {
        matches!(e, CallEvent::ControlOff { .. })
    })
    .await;
    // And right away again: it does not get to the owner.
    call.other.control_ask();
    let told = call_event(&mut call.owner_events, "told of it", |e| {
        matches!(
            e,
            CallEvent::ControlAsked { .. } | CallEvent::ControlAskedAnyway { .. }
        )
    })
    .await;
    assert!(
        matches!(told, CallEvent::ControlAskedAnyway { .. }),
        "no new question on the owner's screen: {told:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owners_hand_makes_whatever_is_held_go_up_at_once() {
    let mut call = in_a_call(true, true, true).await;
    let mut input = call.let_in().await;
    call.owner.control_owner_busy();
    assert_eq!(next_input(&mut input).await, [Input::LetGo]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_yes_pressed_too_fast_does_nothing() {
    let mut call = in_a_call(true, true, true).await;
    call.other.control_ask();
    call_event(&mut call.owner_events, "the question", |e| {
        matches!(e, CallEvent::ControlAsked { .. })
    })
    .await;
    call.owner.control_answer(true, Scope::WholeScreen);
    tokio::time::sleep(Duration::from_millis(300)).await;
    call.other.control_send(vec![point(1)]);
    while let Ok(event) = call.other_events.try_recv() {
        assert!(
            !matches!(event, CoreEvent::Call(CallEvent::ControlOn { .. })),
            "not let in"
        );
    }
    // The question is still there: it takes a yes after reading it.
    tokio::time::sleep(Duration::from_millis(READ_FIRST as u64)).await;
    call.owner.control_answer(true, Scope::WholeScreen);
    call_event(&mut call.other_events, "let in", |e| {
        matches!(e, CallEvent::ControlOn { driving: true, .. })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn input_gets_through_in_order_only_while_it_is_on() {
    let mut call = in_a_call(true, true, true).await;
    let mut input = call.let_in().await;
    call.other.control_send(vec![point(1), point(2)]);
    call.other.control_send(vec![point(3)]);
    let mut got = Vec::new();
    while got.len() < 3 {
        got.extend(next_input(&mut input).await);
    }
    assert_eq!(got, [point(1), point(2), point(3)]);

    // The owner's own hand: whoever is in control waits.
    call.owner.control_owner_busy();
    assert_eq!(
        next_input(&mut input).await,
        [Input::LetGo],
        "what was held goes up"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    call.other.control_send(vec![point(4)]);
    no_input(&mut input).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    call.other.control_send(vec![point(5)]);
    assert_eq!(next_input(&mut input).await, [point(5)]);

    // More time takes the owner.
    call.owner.control_extend();
    call_event(&mut call.owner_events, "more time", |e| {
        matches!(e, CallEvent::ControlOn { driving: false, .. })
    })
    .await;

    // The owner stops it. Nothing more gets through, and it is written down.
    call.owner.control_stop();
    let ended = call_event(&mut call.owner_events, "off", |e| {
        matches!(e, CallEvent::ControlOff { .. })
    })
    .await;
    assert!(matches!(
        ended,
        CallEvent::ControlOff {
            why: Ended::Owner,
            driving: false,
            ..
        }
    ));
    call_event(&mut call.other_events, "off for them", |e| {
        matches!(e, CallEvent::ControlOff { driving: true, .. })
    })
    .await;
    call.other.control_send(vec![point(6)]);
    no_input(&mut input).await;
    let log = call.owner.control_log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].who, call.other_key);
    assert_eq!(log[0].why, Ended::Owner);
    assert!(log[0].whole_screen);
    assert!(log[0].to >= log[0].from);
}

#[tokio::test(flavor = "multi_thread")]
async fn it_ends_when_the_sharing_stops() {
    let mut call = in_a_call(true, true, true).await;
    let _input = call.let_in().await;
    call.owner.call_screen_on(false, 0, 0, false);
    let ended = call_event(&mut call.owner_events, "off", |e| {
        matches!(e, CallEvent::ControlOff { .. })
    })
    .await;
    assert!(matches!(
        ended,
        CallEvent::ControlOff {
            why: Ended::CallOrSharing,
            ..
        }
    ));
    call_event(&mut call.other_events, "off for them", |e| {
        matches!(e, CallEvent::ControlOff { driving: true, .. })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn it_ends_when_the_call_ends() {
    let mut call = in_a_call(true, true, true).await;
    let mut input = call.let_in().await;
    call.other.call_end();
    let ended = call_event(&mut call.owner_events, "off", |e| {
        matches!(e, CallEvent::ControlOff { .. })
    })
    .await;
    assert!(matches!(
        ended,
        CallEvent::ControlOff {
            why: Ended::CallOrSharing,
            ..
        }
    ));
    no_input(&mut input).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn it_ends_when_the_screen_locks() {
    let mut call = in_a_call(true, true, true).await;
    let _input = call.let_in().await;
    call.owner.control_not_safe();
    let ended = call_event(&mut call.owner_events, "off", |e| {
        matches!(e, CallEvent::ControlOff { .. })
    })
    .await;
    assert!(matches!(
        ended,
        CallEvent::ControlOff {
            why: Ended::NotSafeAnyMore,
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn it_ends_when_someone_gets_blocked() {
    let mut call = in_a_call(true, true, true).await;
    let _input = call.let_in().await;
    let other_key = call.other_key.clone();
    call.owner.block(&other_key).unwrap();
    call_event(&mut call.owner_events, "off", |e| {
        matches!(e, CallEvent::ControlOff { driving: false, .. })
    })
    .await;
}

/// Plays someone who does not follow the rules: not through `control_send`, but with what they
/// could put on the wire themselves.
#[tokio::test(flavor = "multi_thread")]
async fn nobody_gets_in_with_a_made_up_answer() {
    let mut call = in_a_call(true, true, true).await;
    let mut input = call.owner.control_input();
    let call_id = call.other.call_snapshot().unwrap().call_id;
    let id = id_to_bytes(&call_id).unwrap();
    // A yes nobody gave: goes to the other side, who never asked.
    call.owner.send_as_is(
        &call.other_key,
        Frame::ControlAnswer {
            call: id,
            answer: ControlReply::Yes(ControlKey {
                session: [1; 16],
                key: [2; 32],
                scope: Scope::WholeScreen,
            }),
        },
    );
    // Someone claims the owner agreed, by answering their own question.
    call.other.send_as_is(
        &call.owner_key,
        Frame::ControlAnswer {
            call: id,
            answer: ControlReply::Yes(ControlKey {
                session: [1; 16],
                key: [2; 32],
                scope: Scope::WholeScreen,
            }),
        },
    );
    call.other
        .send_as_is(&call.owner_key, Frame::ControlStop { call: id });
    let marker = call.other.send(&call.owner_key, "after it").unwrap();
    until("what came after", || {
        call.owner.message(&marker.id).is_some()
    })
    .await;
    for events in [&mut call.owner_events, &mut call.other_events] {
        while let Ok(event) = events.try_recv() {
            assert!(
                !matches!(event, CoreEvent::Call(CallEvent::ControlOn { .. })),
                "nobody is in control"
            );
        }
    }
    call.other.control_send(vec![point(1)]);
    no_input(&mut input).await;
    assert!(call.owner.control_log().is_empty());
}

fn batch(number: u64, x: u16) -> Batch {
    Batch {
        number,
        events: vec![point(x)],
    }
}

async fn misbehaved(call: &mut Call) {
    let ended = call_event(&mut call.owner_events, "off", |e| {
        matches!(e, CallEvent::ControlOff { .. })
    })
    .await;
    assert!(
        matches!(
            ended,
            CallEvent::ControlOff {
                why: Ended::Misbehaved,
                ..
            }
        ),
        "{ended:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_for_another_session_is_closed_and_nothing_from_it_counts() {
    let mut call = in_a_call(true, true, true).await;
    let mut input = call.let_in().await;
    call.other.control_raw_as_is(
        false,
        vec![
            RawFrame::Sealed(batch(1000, 7)),
            RawFrame::Sealed(batch(1001, 8)),
        ],
    );
    no_input(&mut input).await;
    // And the real session goes on as if nothing happened.
    call.other.control_send(vec![point(9)]);
    assert_eq!(next_input(&mut input).await, [point(9)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_was_not_sealed_with_the_session_key_never_arrives_and_ends_it() {
    let mut call = in_a_call(true, true, true).await;
    let mut input = call.let_in().await;
    call.other.control_raw_as_is(
        true,
        vec![
            RawFrame::WrongKey(batch(1000, 7)),
            RawFrame::Bytes(b"not sealed at all".to_vec()),
            RawFrame::WrongKey(batch(1001, 8)),
        ],
    );
    misbehaved(&mut call).await;
    no_input(&mut input).await;
    assert_eq!(call.owner.control_log()[0].why, Ended::Misbehaved);
}

#[tokio::test(flavor = "multi_thread")]
async fn input_sent_again_does_not_happen_again() {
    let mut call = in_a_call(true, true, true).await;
    let mut input = call.let_in().await;
    call.other.control_raw_as_is(
        true,
        vec![
            RawFrame::Sealed(batch(1_000_000, 7)),
            RawFrame::Sealed(batch(1_000_000, 7)),
            RawFrame::Sealed(batch(999_999, 8)),
            RawFrame::Sealed(batch(1, 9)),
        ],
    );
    assert_eq!(next_input(&mut input).await, [point(7)]);
    misbehaved(&mut call).await;
    no_input(&mut input).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn too_much_in_one_go_is_not_taken() {
    let mut call = in_a_call(true, true, true).await;
    let mut input = call.let_in().await;
    // Longer than any batch can be: not even read.
    call.other
        .control_raw_as_is(true, vec![RawFrame::Bytes(vec![0; 40_000])]);
    no_input(&mut input).await;
    // More events than fit in a batch.
    call.other.control_raw_as_is(
        true,
        vec![RawFrame::Sealed(Batch {
            number: 1_000_000,
            events: vec![point(1); whatsmine_core::control::MOST_IN_BATCH + 1],
        })],
    );
    no_input(&mut input).await;
}

/// A question from outside the call.
#[tokio::test(flavor = "multi_thread")]
async fn nobody_outside_the_call_gets_to_ask() {
    let mut call = in_a_call(true, true, true).await;
    call.other.send_as_is(
        &call.owner_key,
        Frame::ControlAsk {
            call: id_to_bytes(&uuid::Uuid::now_v7().to_string()).unwrap(),
        },
    );
    let marker = call.other.send(&call.owner_key, "after it").unwrap();
    until("what came after", || {
        call.owner.message(&marker.id).is_some()
    })
    .await;
    while let Ok(event) = call.owner_events.try_recv() {
        assert!(
            !matches!(
                event,
                CoreEvent::Call(
                    CallEvent::ControlAsked { .. } | CallEvent::ControlAskedAnyway { .. }
                )
            ),
            "another call's question"
        );
    }
}

/// Silence: whatever goes wrong on their side, nothing stays in control. Takes the full 5 seconds.
#[tokio::test(flavor = "multi_thread")]
async fn silence_ends_it() {
    let mut call = in_a_call(true, true, true).await;
    let _input = call.let_in().await;
    // Their app stops: the last thing they do is not to say so.
    call.other.shutdown();
    let ended = call_event(&mut call.owner_events, "off", |e| {
        matches!(e, CallEvent::ControlOff { .. })
    })
    .await;
    assert!(matches!(
        ended,
        CallEvent::ControlOff { driving: false, .. }
    ));
}
