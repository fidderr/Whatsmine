//! Groups: ids, and the signature on a group's state.
//!
//! A group has no server. Its owner and admins decide who is in it: every change is a new signed
//! [`GroupState`] with a higher version, sent to all members. Because the signature can be checked
//! by anyone, members may also pass the latest state on to each other, and a member who was
//! offline catches up from whoever they meet first. Messages are signed too, so the same goes for
//! them: nobody needs the writer to be online.

use iroh::{PublicKey, SecretKey, Signature};

use crate::identity;
use crate::model::GROUP_ID_PREFIX;
use crate::protocol::GroupState;

pub fn new_group_id() -> String {
    format!("{GROUP_ID_PREFIX}{}", uuid::Uuid::now_v7())
}

/// Where a group's secret is kept.
pub fn secret_name(id: &str) -> String {
    format!("group:{id}")
}

/// Is this a plausible group id (as opposed to a key)?
pub fn is_group_id(id: &str) -> bool {
    id.starts_with(GROUP_ID_PREFIX) && id.len() <= 64
}

pub fn sign(key: &SecretKey, state: &GroupState) -> Vec<u8> {
    key.sign(&state.signing_bytes()).to_bytes().to_vec()
}

/// Who signed this state: whoever it names, or else its owner.
fn signer_of(state: &GroupState) -> &str {
    if state.rules.signer.is_empty() {
        &state.admin
    } else {
        &state.rules.signer
    }
}

/// True if `sig` is the signature over `state` of whoever it says signed it.
pub fn verify(state: &GroupState, sig: &[u8]) -> bool {
    let Some(signer) = identity::parse_key(signer_of(state)) else {
        return false;
    };
    let Ok(bytes) = <[u8; 64]>::try_from(sig) else {
        return false;
    };
    signer
        .verify(&state.signing_bytes(), &Signature::from_bytes(&bytes))
        .is_ok()
}

/// Whether this next state is one its signer may make, given the current one.
/// Whether we are in something in a space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildState {
    In,
    /// We asked, and someone there decides.
    Asked,
    Out,
}

/// Something in a space, as it is for us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceChildView {
    pub child: crate::protocol::SpaceChild,
    pub state: ChildState,
    /// For a room: who we heard is in it.
    pub people: Vec<String>,
}

pub fn may_change(current: &GroupState, next: &GroupState) -> bool {
    let signer = signer_of(next);
    if !current.members.iter().any(|m| m == signer) {
        return false;
    }
    if current.admin == signer {
        return true;
    }
    // Only the owner hands over, gives roles or ends the group.
    if next.admin != current.admin
        || next.closed != current.closed
        || sorted(&next.rules.admins) != sorted(&current.rules.admins)
        || !next.members.contains(&current.admin)
    {
        return false;
    }
    if current.is_manager(signer) {
        // Admins do not remove each other.
        return current
            .rules
            .admins
            .iter()
            .all(|a| next.members.contains(a));
    }
    let mut same_rules = next.rules.clone();
    same_rules.signer.clone_from(&current.rules.signer);
    same_rules.admins.clone_from(&current.rules.admins);
    if same_rules != current.rules {
        let only_about = {
            same_rules.about.clone_from(&current.rules.about);
            same_rules == current.rules
        };
        if !only_about || current.rules.only_admins_edit {
            return false;
        }
    }
    let same_members = sorted(&next.members) == sorted(&current.members);
    let only_added = current.members.iter().all(|m| next.members.contains(m));
    let same_name = next.name == current.name;
    if same_members {
        !current.rules.only_admins_edit || (same_name && next.rules.about == current.rules.about)
    } else {
        only_added && same_name && !current.rules.only_admins_add
    }
}

fn sorted(list: &[String]) -> Vec<&String> {
    let mut out: Vec<&String> = list.iter().collect();
    out.sort();
    out
}

fn message_bytes(group: &str, id: &[u8; 16], text: &str, sent_at: i64, extra: &[u8]) -> Vec<u8> {
    let mut out = b"whatsmine-group-message-v1".to_vec();
    out.extend(postcard::to_stdvec(&(group, id, text, sent_at, extra)).unwrap_or_default());
    out
}

/// So anyone in the group can pass this message on.
pub fn sign_message(
    key: &SecretKey,
    group: &str,
    id: &[u8; 16],
    text: &str,
    sent_at: i64,
    extra: &[u8],
) -> Vec<u8> {
    key.sign(&message_bytes(group, id, text, sent_at, extra))
        .to_bytes()
        .to_vec()
}

pub fn verify_message(
    author: &PublicKey,
    group: &str,
    id: &[u8; 16],
    text: &str,
    sent_at: i64,
    extra: &[u8],
    sig: &[u8],
) -> bool {
    let Ok(bytes) = <[u8; 64]>::try_from(sig) else {
        return false;
    };
    author
        .verify(
            &message_bytes(group, id, text, sent_at, extra),
            &Signature::from_bytes(&bytes),
        )
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(admin: &SecretKey) -> GroupState {
        GroupState {
            id: new_group_id(),
            name: "Team".into(),
            admin: identity::key_to_string(&admin.public()),
            version: 1,
            members: vec!["b".into(), "a".into()],
            closed: false,
            rules: crate::protocol::GroupRules {
                only_admins_add: true,
                only_admins_edit: true,
                ..Default::default()
            },
        }
    }

    fn signed_by(mut s: GroupState, who: &str) -> GroupState {
        s.rules.signer = who.to_owned();
        s
    }

    #[test]
    fn who_may_change_what() {
        let owner = SecretKey::generate();
        let owner_id = identity::key_to_string(&owner.public());
        let mut cur = state(&owner);
        cur.members = vec![owner_id.clone(), "adm".into(), "m".into()];
        cur.rules.admins = vec!["adm".into()];

        // An admin may rename and remove a member, not the owner, not hand over, not end it.
        let mut next = signed_by(cur.clone(), "adm");
        next.name = "New".into();
        assert!(may_change(&cur, &next));
        let mut next = signed_by(cur.clone(), "adm");
        next.members.retain(|m| m != "m");
        assert!(may_change(&cur, &next));
        let mut next = signed_by(cur.clone(), "adm");
        next.members.retain(|m| *m != owner_id);
        assert!(!may_change(&cur, &next));
        let mut next = signed_by(cur.clone(), "adm");
        next.admin = "adm".into();
        assert!(!may_change(&cur, &next));
        let mut next = signed_by(cur.clone(), "adm");
        next.closed = true;
        assert!(!may_change(&cur, &next));

        // A member may do nothing, until the rules say so.
        let mut next = signed_by(cur.clone(), "m");
        next.name = "Mine".into();
        assert!(!may_change(&cur, &next));
        let mut next = signed_by(cur.clone(), "m");
        next.members.push("new".into());
        assert!(!may_change(&cur, &next));
        let mut open = cur.clone();
        open.rules.only_admins_add = false;
        let mut next = signed_by(open.clone(), "m");
        next.members.push("new".into());
        assert!(may_change(&open, &next), "the group lets anyone add");
        let mut next = signed_by(open.clone(), "m");
        next.members.retain(|x| x != "adm");
        assert!(!may_change(&open, &next), "adding is not removing");
        let mut next = signed_by(open.clone(), "m");
        next.rules.only_admins_send = true;
        assert!(!may_change(&open, &next), "the rules are not for members");

        // Someone from outside: nothing.
        let next = signed_by(cur.clone(), "stranger");
        assert!(!may_change(&cur, &next));

        // The owner hands over.
        let mut next = signed_by(cur.clone(), &owner_id);
        next.admin = "adm".into();
        next.members.retain(|m| *m != owner_id);
        assert!(may_change(&cur, &next));
    }

    #[test]
    fn a_passed_on_message_is_checked_against_its_writer() {
        let writer = SecretKey::generate();
        let other = SecretKey::generate();
        let sig = sign_message(&writer, "g", &[1; 16], "hello", 5, b"");
        assert!(verify_message(
            &writer.public(),
            "g",
            &[1; 16],
            "hello",
            5,
            b"",
            &sig
        ));
        assert!(!verify_message(
            &other.public(),
            "g",
            &[1; 16],
            "hello",
            5,
            b"",
            &sig
        ));
        assert!(!verify_message(
            &writer.public(),
            "g",
            &[1; 16],
            "changed",
            5,
            b"",
            &sig
        ));
        assert!(!verify_message(
            &writer.public(),
            "h",
            &[1; 16],
            "hello",
            5,
            b"",
            &sig
        ));
    }

    #[test]
    fn only_the_signer_can_sign_and_changes_invalidate() {
        let admin = SecretKey::generate();
        let other = SecretKey::generate();
        let s = state(&admin);
        assert!(verify(&s, &sign(&admin, &s)));
        assert!(!verify(&s, &sign(&other, &s)), "someone else's signature");
        let mut changed = s.clone();
        changed.members.push("c".into());
        assert!(!verify(&changed, &sign(&admin, &s)), "tampered members");
        let mut reordered = s.clone();
        reordered.members.reverse();
        assert!(
            verify(&reordered, &sign(&admin, &s)),
            "member order is irrelevant"
        );
        assert!(!verify(&s, &[1, 2, 3]));
    }

    #[test]
    fn group_ids_are_not_keys() {
        let id = new_group_id();
        assert!(is_group_id(&id));
        assert!(identity::parse_key(&id).is_none());
    }
}
