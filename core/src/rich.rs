//! What a message can be besides text, and what can happen to it later: replies, reactions, edits,
//! polls, lists, plans, places and contact cards. Each change is one small event that reaches
//! everyone in the chat in any order, so each is written to end up the same whatever came first.

use serde::{Deserialize, Serialize};

use crate::model::Millis;

/// What a person answered to a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanAnswer {
    Going,
    Maybe,
    No,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Rich {
    Poll {
        question: String,
        options: Vec<String>,
        /// Who chose what: person key, option number.
        votes: Vec<(String, u32)>,
        multiple: bool,
    },
    Checklist {
        title: String,
        items: Vec<ChecklistItem>,
    },
    Plan {
        title: String,
        at: Millis,
        place: String,
        answers: Vec<(String, PlanAnswer)>,
    },
    /// Opens in the maps app of whoever taps it. Nothing of it goes anywhere else.
    Place {
        lat: f64,
        lon: f64,
        label: String,
        /// For a live one, until when.
        live_until: Option<Millis>,
    },
    /// Someone's invitation, to pass them on.
    ContactCard { name: String, invite: String },
    /// A copy of a look. Nothing changes for anyone until they choose to use it.
    Look(Box<crate::look::Look>),
    /// A line this device wrote in the chat itself. Never leaves this device.
    Note(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub text: String,
    pub done: bool,
}

/// A change to a rich message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RichChange {
    /// `on`: chosen or no longer chosen.
    Vote {
        option: u32,
        on: bool,
    },
    Check {
        item: u32,
        done: bool,
    },
    Answer(PlanAnswer),
    Moved {
        lat: f64,
        lon: f64,
    },
}

impl Rich {
    /// What `who` changes. Whatever does not fit is ignored: it may come from a newer version.
    pub fn apply(&mut self, who: &str, change: &RichChange) {
        match (self, change) {
            (
                Rich::Poll {
                    options,
                    votes,
                    multiple,
                    ..
                },
                RichChange::Vote { option, on },
            ) => {
                if *option as usize >= options.len() {
                    return;
                }
                if *on && !*multiple {
                    votes.retain(|(person, _)| person != who);
                }
                votes.retain(|(person, chosen)| !(person == who && chosen == option));
                if *on {
                    votes.push((who.to_owned(), *option));
                }
            }
            (Rich::Checklist { items, .. }, RichChange::Check { item, done }) => {
                if let Some(entry) = items.get_mut(*item as usize) {
                    entry.done = *done;
                }
            }
            (Rich::Plan { answers, .. }, RichChange::Answer(answer)) => {
                answers.retain(|(person, _)| person != who);
                answers.push((who.to_owned(), *answer));
            }
            (
                Rich::Place { lat, lon, .. },
                RichChange::Moved {
                    lat: to_lat,
                    lon: to_lon,
                },
            ) => {
                *lat = *to_lat;
                *lon = *to_lon;
            }
            _ => {}
        }
    }

    /// One line for a chat list.
    pub fn summary(&self) -> &str {
        match self {
            Rich::Poll { question, .. } => question,
            Rich::Checklist { title, .. } | Rich::Plan { title, .. } => title,
            Rich::Place { label, .. } => label,
            Rich::ContactCard { name, .. } => name,
            Rich::Look(look) => &look.name,
            Rich::Note(text) => text,
        }
    }
}

/// Everything about a message besides its text and where it is.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageExtra {
    pub reply_to: Option<String>,
    pub edited: bool,
    /// Taken back by whoever wrote it: the text is gone, this stays in its place.
    pub unsent: bool,
    /// Person key, emoji. One each.
    pub reactions: Vec<(String, String)>,
    pub rich: Option<Rich>,
    /// On your own list, with these words.
    pub saved: bool,
    pub tags: Vec<String>,
    /// People named with an @.
    pub mentions: Vec<String>,
    pub forwarded: bool,
    /// No sound on their side.
    pub silent: bool,
    /// New messages in this chat disappeared this long after arriving, when this one was sent.
    pub disappear_ms: Option<i64>,
    /// Can be opened once.
    pub view_once: bool,
    /// Opened, for one that can be opened once.
    pub viewed: bool,
    /// From a newer version, with something this one cannot show.
    pub unknown: bool,
    /// As it came, with the signature: what it takes to pass it on in a group.
    pub proof: Option<GroupProof>,
}

/// A group message as its writer signed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupProof {
    pub text: String,
    pub sent_at: Millis,
    pub wire: Vec<u8>,
    pub sig: Vec<u8>,
}

/// What goes to the others with a message. What is only for this device stays out.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
struct WireExtra {
    reply_to: Option<String>,
    /// Text, not a `Rich`: a kind from a newer version must still arrive.
    rich: Option<serde_json::Value>,
    mentions: Vec<String>,
    forwarded: bool,
    silent: bool,
    disappear_ms: Option<i64>,
    view_once: bool,
}

/// How much may come along with a message.
const MAX_WIRE_EXTRA: usize = 32 * 1024;

impl MessageExtra {
    /// What goes along with a message: nothing for a plain one.
    pub fn to_wire(&self) -> Vec<u8> {
        let wire = WireExtra {
            reply_to: self.reply_to.clone(),
            rich: self
                .rich
                .as_ref()
                .and_then(|r| serde_json::to_value(r).ok()),
            mentions: self.mentions.clone(),
            forwarded: self.forwarded,
            silent: self.silent,
            disappear_ms: self.disappear_ms,
            view_once: self.view_once,
        };
        if wire == WireExtra::default() {
            return Vec::new();
        }
        serde_json::to_vec(&wire).unwrap_or_default()
    }

    /// What came along with a message. Whatever cannot be read does not stop the message.
    pub fn from_wire(bytes: &[u8]) -> Self {
        if bytes.is_empty() {
            return Self::default();
        }
        if bytes.len() > MAX_WIRE_EXTRA {
            return Self {
                unknown: true,
                ..Self::default()
            };
        }
        let Ok(wire) = serde_json::from_slice::<WireExtra>(bytes) else {
            return Self {
                unknown: true,
                ..Self::default()
            };
        };
        let (rich, unknown) = match wire.rich {
            None => (None, false),
            Some(value) => match serde_json::from_value::<Rich>(value) {
                Ok(rich) => (Some(rich), false),
                Err(_) => (None, true),
            },
        };
        Self {
            reply_to: wire.reply_to,
            rich,
            mentions: wire.mentions,
            forwarded: wire.forwarded,
            silent: wire.silent,
            disappear_ms: wire.disappear_ms.filter(|ms| *ms > 0),
            view_once: wire.view_once,
            unknown,
            ..Self::default()
        }
    }

    pub fn react(&mut self, who: &str, emoji: Option<&str>) {
        self.reactions.retain(|(person, _)| person != who);
        if let Some(emoji) = emoji {
            self.reactions.push((who.to_owned(), emoji.to_owned()));
        }
    }
}

/// Something that happens to a message, or to a chat, after it was sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MessageChange {
    React(Option<String>),
    Edit(String),
    /// Taken back, for everyone.
    Unsend,
    Rich(RichChange),
    /// New messages in this chat disappear after this long, from now on and for both.
    ChatTimer(Option<i64>),
}

/// How a chat is treated here. Apart from the disappearing time, nothing of this leaves the device.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatPrefs {
    pub labels: Vec<String>,
    pub notes: String,
    pub favourite: bool,
    pub archived: bool,
    /// Locked behind the app lock, and not on any list until unlocked.
    pub locked: bool,
    /// Off the list and silent until then.
    pub snoozed_until: Option<Millis>,
    /// New messages disappear after this long.
    pub disappear_ms: Option<i64>,
    /// This chat's own look. Empty: what all chats have.
    pub look: String,
    pub sound: String,
    /// Name and photo shown to this person instead of the usual ones.
    pub my_name: Option<String>,
    /// Hide last seen and online from this person.
    pub hide_presence: bool,
    /// Pinned on top of this chat.
    pub pinned_messages: Vec<String>,
    /// Shown as unread until opened.
    pub marked_unread: bool,
}

/// A folder of chats, by rule and by hand.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Folder {
    pub name: String,
    pub unread: bool,
    pub groups: bool,
    pub people: bool,
    pub labels: Vec<String>,
    pub chats: Vec<String>,
    /// Quiet outside these hours, 0-24. `None`: never quiet.
    pub quiet_outside: Option<(u8, u8)>,
}

/// What one line of a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchHit {
    Person(String),
    Message { chat: String, id: String },
}

/// What a search looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchIn {
    #[default]
    All,
    People,
    Media,
    Files,
    Links,
}

/// How a call went, for the list of calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRecord {
    pub id: String,
    pub contact_id: String,
    pub outgoing: bool,
    pub video: bool,
    pub at: Millis,
    pub duration_ms: u64,
    /// Nobody answered: they tried to call, or you did.
    pub missed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_is_for_the_others_goes_along() {
        assert!(MessageExtra::default().to_wire().is_empty());
        let mine = MessageExtra {
            saved: true,
            tags: vec!["Plans".into()],
            ..MessageExtra::default()
        };
        assert!(mine.to_wire().is_empty(), "saved items stay here");
        let reply = MessageExtra {
            reply_to: Some("x".into()),
            ..MessageExtra::default()
        };
        assert_eq!(MessageExtra::from_wire(&reply.to_wire()), reply);
    }

    #[test]
    fn something_from_a_newer_version_still_arrives() {
        let newer = br#"{"reply_to":"x","rich":{"Hologram":{"size":3}},"other":1}"#;
        let extra = MessageExtra::from_wire(newer);
        assert!(extra.unknown);
        assert_eq!(extra.reply_to.as_deref(), Some("x"));
        assert!(MessageExtra::from_wire(b"not json").unknown);
    }

    fn poll(multiple: bool) -> Rich {
        Rich::Poll {
            question: "When?".into(),
            options: vec!["Thu".into(), "Fri".into()],
            votes: Vec::new(),
            multiple,
        }
    }

    fn votes(rich: &Rich) -> Vec<(String, u32)> {
        match rich {
            Rich::Poll { votes, .. } => votes.clone(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn a_vote_moves_unless_many_may_be_chosen() {
        let mut one = poll(false);
        one.apply(
            "a",
            &RichChange::Vote {
                option: 0,
                on: true,
            },
        );
        one.apply(
            "a",
            &RichChange::Vote {
                option: 1,
                on: true,
            },
        );
        assert_eq!(votes(&one), vec![("a".to_owned(), 1)]);
        let mut many = poll(true);
        many.apply(
            "a",
            &RichChange::Vote {
                option: 0,
                on: true,
            },
        );
        many.apply(
            "a",
            &RichChange::Vote {
                option: 1,
                on: true,
            },
        );
        assert_eq!(votes(&many).len(), 2);
        many.apply(
            "a",
            &RichChange::Vote {
                option: 0,
                on: false,
            },
        );
        assert_eq!(votes(&many), vec![("a".to_owned(), 1)]);
    }

    #[test]
    fn what_does_not_fit_is_ignored() {
        let mut p = poll(false);
        p.apply(
            "a",
            &RichChange::Vote {
                option: 9,
                on: true,
            },
        );
        p.apply(
            "a",
            &RichChange::Check {
                item: 0,
                done: true,
            },
        );
        assert!(votes(&p).is_empty());
    }

    #[test]
    fn a_plan_keeps_the_last_answer_and_a_list_its_marks() {
        let mut plan = Rich::Plan {
            title: "Climbing".into(),
            at: 5,
            place: "Hall".into(),
            answers: Vec::new(),
        };
        plan.apply("a", &RichChange::Answer(PlanAnswer::Maybe));
        plan.apply("a", &RichChange::Answer(PlanAnswer::Going));
        assert!(
            matches!(&plan, Rich::Plan { answers, .. } if answers == &[("a".to_owned(), PlanAnswer::Going)])
        );
        let mut list = Rich::Checklist {
            title: "Bring".into(),
            items: vec![ChecklistItem {
                text: "Chalk".into(),
                done: false,
            }],
        };
        list.apply(
            "b",
            &RichChange::Check {
                item: 0,
                done: true,
            },
        );
        assert!(matches!(&list, Rich::Checklist { items, .. } if items[0].done));
    }

    #[test]
    fn a_reaction_is_one_each_and_can_be_taken_back() {
        let mut extra = MessageExtra::default();
        extra.react("a", Some("👍"));
        extra.react("a", Some("❤️"));
        extra.react("b", Some("👍"));
        assert_eq!(extra.reactions.len(), 2);
        extra.react("a", None);
        assert_eq!(extra.reactions, vec![("b".to_owned(), "👍".to_owned())]);
    }
}
