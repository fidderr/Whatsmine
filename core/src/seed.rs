//! Test data: a device that has been in use for a while, so a screen can be looked at in its
//! real state. Only with the `test-hooks` feature, never in a build for users.

use iroh::SecretKey;

use crate::model::{Attachment, ContactState, Message, MessageStatus, TransferState, now_ms};
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
        person("Yara", ContactState::PendingIn)?;
        person("Tom", ContactState::PendingIn)?;
        person("Pushy Pete", ContactState::Blocked)?;

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
        add(message(
            &noor,
            false,
            13 * MINUTE,
            "Perfect. I'll bring the charger for your camera",
            Read,
        ))?;
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
        add(from(
            &mum,
            2 * MINUTE,
            "Lovely. See you all on Sunday!",
            false,
        ))?;
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
