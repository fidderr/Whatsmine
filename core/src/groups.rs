//! Groups: ids, and the admin's signature on a group's state.
//!
//! A group has no server and no owner of the data. One member, the admin
//! (its creator), decides who is in it: every change is a new signed
//! [`GroupState`] with a higher version, sent to all members. Because the
//! signature can be checked by anyone, members may also pass the latest
//! state on to each other, and a member who was offline catches up from
//! whoever they meet first.

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

/// True if `sig` is the admin's signature over `state`.
pub fn verify(state: &GroupState, sig: &[u8]) -> bool {
    let Some(admin) = identity::parse_key(&state.admin) else {
        return false;
    };
    verify_with(&admin, state, sig)
}

fn verify_with(admin: &PublicKey, state: &GroupState, sig: &[u8]) -> bool {
    let Ok(bytes) = <[u8; 64]>::try_from(sig) else {
        return false;
    };
    admin
        .verify(&state.signing_bytes(), &Signature::from_bytes(&bytes))
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
        }
    }

    #[test]
    fn only_the_admin_can_sign_and_changes_invalidate() {
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
