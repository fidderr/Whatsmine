//! Test data: a device that has been in use for a while, so a screen can be looked at in its
//! real state. Only with the `test-hooks` feature, never in a build for users.

use iroh::SecretKey;

use crate::look::{Background, Base, Colors, Look, Rgb};
use crate::model::{
    Attachment, ContactKeys, ContactState, Message, MessageStatus, TransferState, Trust, now_ms,
};
use crate::rich::{CallRecord, ChatPrefs, ChecklistItem, Folder, MessageExtra, PlanAnswer, Rich};
use crate::{Core, groups, identity, protocol};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// A key that stays the same for a name. Nobody holds it: nothing ever answers there.
fn key_of(name: &str) -> String {
    let mut bytes = [7u8; 32];
    for (slot, byte) in bytes.iter_mut().zip(name.bytes()) {
        *slot = byte;
    }
    identity::key_to_string(&SecretKey::from_bytes(&bytes).public())
}

/// One message, `ago` milliseconds ago.
fn message(contact: &str, outgoing: bool, ago: i64, body: &str, status: MessageStatus) -> Message {
    let at = now_ms() - ago;
    Message {
        id: uuid::Uuid::now_v7().to_string(),
        contact_id: contact.to_string(),
        outgoing,
        body: body.to_string(),
        sent_at: at,
        received_at: at,
        status,
        read: true,
        attachment: None,
        sender: None,
        extra: Default::default(),
    }
}

impl Core {
    /// Contacts, chats, a group, requests and a blocked person.
    pub fn seed_demo(&self) -> anyhow::Result<()> {
        use MessageStatus::{Delivered, Queued, Read, Sent};
        let db = &self.inner.db;
        let me = self.my_key();

        let person = |name: &str, state: ContactState| -> anyhow::Result<String> {
            let key = key_of(name);
            db.upsert_contact(&key, state, Some(name), None)?;
            Ok(key)
        };
        let noor = person("Noor", ContactState::Accepted)?;
        let sam = person("Sam", ContactState::Accepted)?;
        let lieke = person("Lieke", ContactState::Accepted)?;
        let mum = person("Mum", ContactState::Accepted)?;
        let bas = person("Bas", ContactState::Accepted)?;
        person("Anna", ContactState::Accepted)?;
        let yara = person("Yara", ContactState::PendingIn)?;
        let tom = person("Tom", ContactState::PendingIn)?;
        db.set_contact_keys(
            &yara,
            &ContactKeys {
                greeting: "Hi, I'm Yara from the Thursday group.".into(),
                ..ContactKeys::default()
            },
        );
        db.set_contact_keys(
            &tom,
            &ContactKeys {
                greeting: "Hello, we met at Anna's party last week.".into(),
                ..ContactKeys::default()
            },
        );
        for (who, trust) in [
            (&noor, Trust::MetInPerson),
            (&mum, Trust::MetInPerson),
            (&bas, Trust::SafetyWords),
        ] {
            let mut keys = db.contact_keys(who).unwrap_or_default();
            keys.trust = trust;
            db.set_contact_keys(who, &keys);
        }
        for who in [&noor, &mum, &sam, &lieke] {
            db.set_chat_prefs(
                who,
                &ChatPrefs {
                    favourite: true,
                    ..ChatPrefs::default()
                },
            );
        }
        person("Pushy Pete", ContactState::Blocked)?;
        {
            use crate::status::{Backdrop, LASTS, Post};
            let now = now_ms();
            let post = |id: &str, author: &str, ago: i64, text: &str, backdrop, seen| Post {
                id: format!("0190a000-0000-7000-8000-00000000{id}"),
                author: author.to_owned(),
                text: text.to_owned(),
                backdrop,
                at: now - ago,
                until: now - ago + LASTS,
                parts: 0,
                seen,
                seen_told: seen,
            };
            for p in [
                post(
                    "0001",
                    &noor,
                    720_000,
                    "Finally at the top. Worth every step.",
                    Backdrop::Dusk,
                    false,
                ),
                post(
                    "0002",
                    &sam,
                    3_600_000,
                    "New bike, who wants to ride on Sunday?",
                    Backdrop::Forest,
                    false,
                ),
                post(
                    "0003",
                    &lieke,
                    14_400_000,
                    "Coffee at mine, door is open",
                    Backdrop::Sand,
                    false,
                ),
                post(
                    "0004",
                    &mum,
                    50_000_000,
                    "The tulips are out",
                    Backdrop::Plum,
                    true,
                ),
                post("0005", &bas, 60_000_000, "Back home", Backdrop::Night, true),
            ] {
                db.add_status(&p);
            }
            let me = self.my_key();
            db.add_status(&post(
                "0006",
                &me,
                1_800_000,
                "At the lake all afternoon, come by",
                Backdrop::Sea,
                true,
            ));
            db.owe_status(
                "0190a000-0000-7000-8000-000000000006",
                std::slice::from_ref(&noor),
                now + LASTS,
            );
            db.add_status_seen_by("0190a000-0000-7000-8000-000000000006", &noor);
        }
        let sage = Look {
            id: "seed-sage".into(),
            name: "Soft sage".into(),
            base: Base::Light,
            colors: Colors {
                accent: Some(Rgb::new(0x5f_7f_5a)),
                my_bubble: Some(Rgb::new(0xa9_c4_a0)),
                ..Colors::default()
            },
            background: Background::Color(Rgb::new(0xdc_e8_d6)),
            ..Look::default()
        };
        let sea = Look {
            id: "seed-sea".into(),
            name: "Sea evening".into(),
            base: Base::Dark,
            colors: Colors {
                my_bubble: Some(Rgb::new(0x8c_c9_c1)),
                accent: Some(Rgb::new(0x8c_c9_c1)),
                ..Colors::default()
            },
            background: Background::Gradient {
                from: Rgb::new(0x1f_5a_4d),
                to: Rgb::new(0x3f_35_56),
                angle: 160,
            },
            ..Look::default()
        };
        db.save_look(&sage);
        db.save_look(&sea);
        db.set_chat_prefs(
            &lieke,
            &ChatPrefs {
                favourite: true,
                look: sea.id.clone(),
                ..ChatPrefs::default()
            },
        );

        let add = |m: Message| db.insert_message(&m);
        add(message(
            &noor,
            false,
            16 * MINUTE,
            "Did you get the tickets?",
            Read,
        ))?;
        add(message(
            &noor,
            true,
            15 * MINUTE,
            "Yes! Two for Saturday, row 12",
            Read,
        ))?;
        let tickets = message(
            &noor,
            true,
            14 * MINUTE,
            "The doors open at *half past six*",
            Read,
        );
        let tickets_id = tickets.id.clone();
        add(tickets)?;
        let mut answer = message(
            &noor,
            false,
            13 * MINUTE,
            "Perfect. I'll bring the charger for your camera",
            Read,
        );
        answer.extra = MessageExtra {
            reply_to: Some(tickets_id),
            reactions: vec![(me.clone(), "❤️".to_owned())],
            ..MessageExtra::default()
        };
        add(answer)?;
        let mut photo = message(&noor, true, 11 * MINUTE, "The place we talked about", Read);
        photo.attachment = Some(self.seed_picture()?);
        add(photo)?;
        add(message(
            &noor,
            true,
            6 * MINUTE,
            "On my way, see you at 7",
            Queued,
        ))?;
        db.set_pinned(&noor, true);

        add(message(
            &sam,
            false,
            33 * MINUTE,
            "Sent you the photos from Sunday",
            Read,
        ))?;
        add(message(
            &sam,
            true,
            30 * MINUTE,
            "Thanks, they look great",
            Delivered,
        ))?;
        add(message(
            &lieke,
            true,
            3 * HOUR,
            "Are we still on for Saturday?",
            Sent,
        ))?;
        let mut unread = message(
            &mum,
            false,
            20 * HOUR,
            "Call me when you have a moment",
            Read,
        );
        unread.read = false;
        add(unread)?;
        add(message(
            &bas,
            false,
            DAY + 2 * HOUR,
            "That was a good climb",
            Read,
        ))?;
        db.set_muted(&bas, true);

        // A group of ours.
        let mut members = vec![me.clone(), noor.clone(), sam.clone(), mum.clone()];
        members.sort();
        let state = protocol::GroupState {
            id: groups::new_group_id(),
            name: "Family".to_string(),
            admin: me.clone(),
            version: 1,
            members,
            closed: false,
            rules: protocol::GroupRules {
                signer: me.clone(),
                only_admins_add: true,
                only_admins_edit: true,
                ..protocol::GroupRules::default()
            },
        };
        let sig = groups::sign(&self.inner.key, &state);
        db.save_group(&state, &sig, true, &[], &me)?;
        let from = |who: &str, ago: i64, body: &str, read: bool| {
            let mut m = message(&state.id, false, ago, body, Read);
            m.sender = Some(who.to_string());
            m.read = read;
            m
        };
        add(from(&mum, 14 * MINUTE, "Who is bringing the cake?", false))?;
        add(from(
            &sam,
            12 * MINUTE,
            "Not me, I burned the last one",
            false,
        ))?;
        add(message(
            &state.id,
            true,
            10 * MINUTE,
            "I'll bake it tonight",
            Delivered,
        ))?;
        let mut peach = message(&noor, false, 12 * MINUTE, "Noor's peach", Read);
        peach.extra.rich = Some(Rich::Look(Box::new(Look {
            id: "their-peach".into(),
            name: "Noor's peach".into(),
            base: Base::Light,
            colors: Colors {
                background: Some(Rgb::new(0xfb_e0_cf)),
                my_bubble: Some(Rgb::new(0xf2_b0_8e)),
                ..Colors::default()
            },
            ..Look::default()
        })));
        add(peach)?;
        let mut poll = from(&sam, 9 * MINUTE, "Which evening works best?", true);
        poll.extra.rich = Some(Rich::Poll {
            question: "Which evening works best?".into(),
            options: vec!["Thursday".into(), "Friday".into(), "Saturday".into()],
            votes: vec![
                (mum.clone(), 0),
                (noor.clone(), 0),
                (me.clone(), 1),
                (sam.clone(), 0),
            ],
            multiple: false,
        });
        add(poll)?;
        let mut plan = from(&noor, 8 * MINUTE, "Bouldering night", true);
        plan.extra.rich = Some(Rich::Plan {
            title: "Bouldering night".into(),
            at: now_ms() + 2 * DAY,
            place: "Sterk hall".into(),
            answers: vec![
                (noor.clone(), PlanAnswer::Going),
                (sam.clone(), PlanAnswer::Going),
                (mum.clone(), PlanAnswer::Maybe),
            ],
        });
        add(plan)?;
        let mut list = message(&state.id, true, 7 * MINUTE, "To bring", Delivered);
        list.extra.rich = Some(Rich::Checklist {
            title: "To bring".into(),
            items: vec![
                ChecklistItem {
                    text: "Chalk".into(),
                    done: true,
                },
                ChecklistItem {
                    text: "Spare shoes".into(),
                    done: false,
                },
                ChecklistItem {
                    text: "Water".into(),
                    done: false,
                },
            ],
        });
        add(list)?;
        let mut place = from(&mum, 5 * MINUTE, "Sterk hall", true);
        place.extra.rich = Some(Rich::Place {
            lat: 52.0116,
            lon: 4.7105,
            label: "Sterk hall".into(),
            live_until: None,
        });
        add(place)?;
        let mut unsent = from(&sam, 4 * MINUTE, "", true);
        unsent.extra.unsent = true;
        add(unsent)?;
        add(from(
            &mum,
            2 * MINUTE,
            "Lovely. See you all on Sunday!",
            false,
        ))?;

        db.save_folder_seed(&[Folder {
            name: "Family".into(),
            groups: true,
            ..Folder::default()
        }]);
        for (i, (who, outgoing, video, ago, duration, missed)) in [
            (&lieke, false, false, 40 * MINUTE, 0, true),
            (&noor, false, false, 3 * HOUR, 12 * 60_000, false),
            (&state.id, false, true, 20 * HOUR, 25 * 60_000, false),
            (&sam, true, true, DAY + 3 * HOUR, 4 * 60_000, false),
            (&mum, true, false, 3 * DAY, 0, true),
        ]
        .into_iter()
        .enumerate()
        {
            db.add_call_record(&CallRecord {
                id: format!("seed-{i}"),
                contact_id: who.clone(),
                outgoing,
                video,
                at: now_ms() - ago,
                duration_ms: duration,
                missed,
            });
        }
        Ok(())
    }

    /// A picture in the media folder, as a photo in a chat would have.
    fn seed_picture(&self) -> anyhow::Result<Attachment> {
        let (width, height) = (480u32, 300u32);
        let picture = image::RgbImage::from_fn(width, height, |x, y| {
            let (fx, fy) = (x as f32, y as f32);
            let hill = 210.0 - 40.0 * (fx / 70.0).sin() - fx / 12.0;
            let sun = ((fx - 330.0).powi(2) + (fy - 100.0).powi(2)).sqrt() < 42.0;
            if fy > hill {
                image::Rgb([63, 53, 86])
            } else if sun {
                image::Rgb([255, 224, 166])
            } else {
                image::Rgb([240, 168, 104])
            }
        });
        let dir = self.media_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("seed-sunset.png");
        picture.save(&path)?;
        let size = std::fs::metadata(&path)?.len();
        Ok(Attachment {
            name: "sunset.png".to_string(),
            mime: "image/png".to_string(),
            size,
            sha256: String::new(),
            path: Some(path.display().to_string()),
            state: TransferState::Done,
            duration_ms: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Core, NetMode};

    #[tokio::test]
    async fn seeding_leaves_chats_requests_and_a_group() {
        let dir = tempfile::tempdir().unwrap();
        let lookup = crate::records::LocalRecords::default();
        let core = Core::start(dir.path(), NetMode::Local(lookup)).unwrap();
        core.seed_demo().unwrap();
        let chats = core.chats();
        assert!(chats.len() >= 6, "{} chats", chats.len());
        assert_eq!(core.requests().len(), 2);
        assert_eq!(core.blocked().len(), 1);
        let family = chats
            .iter()
            .find(|c| c.contact.their_name == "Family")
            .expect("the group is a chat");
        assert_eq!(family.unread, 3);
        assert!(core.group_info(&family.contact.id).is_some());
        let noor = chats
            .iter()
            .find(|c| c.contact.their_name == "Noor")
            .expect("Noor is a chat");
        assert!(noor.contact.pinned);
        core.shutdown();
    }
}
