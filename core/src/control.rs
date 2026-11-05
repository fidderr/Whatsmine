//! Letting someone control this PC, during a call. Everything that decides whether input may be
//! used is here, with no window or device in sight, so every rule can be proved by a test.
//!
//! What this is built against: someone talked into it by a scammer, a stolen or hacked contact, a
//! bug or a made-up packet that sends input nobody agreed to, and whoever is in control approving
//! things on the owner's behalf. So it is off until someone turns it on, a person says yes every
//! time, it ends by itself, the owner's own hand always wins, and anything that does not fit ends it.
//!
//! What this leaves to the app: moving the real pointer, and checking on the real desktop where an
//! event lands. See `control_win.rs` in the app.

use serde::{Deserialize, Serialize};

use crate::model::Millis;

/// How long someone must have been a contact. A scammer on the phone does not wait this long. Nothing
/// skips it: safety words and a scanned code are both things a scammer can talk someone through.
pub const KNOWN_FOR: Millis = 7 * 24 * 3600 * 1000;
/// How long a session lasts. Longer takes the owner's own hand.
pub const SESSION: Millis = 15 * 60 * 1000;
/// How long the one in control waits after the owner touched the mouse or a key.
pub const OWNER_WINS: Millis = 2_000;
/// Nothing from the one in control for this long, not even "still here": over.
pub const SILENCE: Millis = 5_000;
/// How long a question waits for an answer.
pub const ASK_WAITS: Millis = 60_000;
/// After a no, or no answer: nobody gets to keep asking until the owner gives in.
pub const NO_HOLDS: Millis = 2 * 60_000;
/// Before Allow works: time to read what it says.
pub const READ_FIRST: Millis = 5_000;
/// The most events per second, far above what a hand can do.
pub const MOST_PER_SECOND: u32 = 200;
/// Wrong input in a session before it ends.
pub const STRIKES: u32 = 3;
/// The most events in one batch, and the most characters in one piece of text.
pub const MOST_IN_BATCH: usize = 32;
pub const MOST_TEXT: usize = 64;

/// What is being controlled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    /// Only the window being shown, and what belongs to its program.
    Window,
    WholeScreen,
}

/// One thing the one in control did. Coordinates run from 0 to 65535 over what is shown, and are
/// mapped to the screen by the side that is being controlled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Input {
    Move {
        x: u16,
        y: u16,
    },
    Button {
        which: MouseButton,
        down: bool,
    },
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// A key, by its place on the keyboard: its scan code, and whether it is an extended one.
    Key {
        code: u16,
        extended: bool,
        down: bool,
    },
    /// Text as it is, from a phone's keyboard.
    Text(String),
    /// Nothing happened, and I am still here.
    StillHere,
    /// Never from the wire: the owner's own hand is on the PC, and whatever is held down goes up.
    LetGo,
}

impl Input {
    /// Something that goes up. Those always count, or something stays held down on someone's PC.
    fn is_release(&self) -> bool {
        matches!(
            self,
            Self::Button { down: false, .. } | Self::Key { down: false, .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// What someone who does not follow the rules could put on the stream. For tests.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub enum RawFrame {
    Sealed(Batch),
    WrongKey(Batch),
    Bytes(Vec<u8>),
}

/// A numbered batch. A number that does not go up is someone sending it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub number: u64,
    pub events: Vec<Input>,
}

const CONTEXT: &[u8] = b"whatsmine-control-v1";

/// A batch as it goes over the wire, sealed with the key of this session alone.
pub fn seal_batch(key: &[u8; 32], batch: &Batch) -> Option<Vec<u8>> {
    let plain = postcard::to_stdvec(batch).ok()?;
    Some(crate::vault::seal_with(key, &plain, CONTEXT))
}

/// A batch off the wire. `None` for anything that was not sealed with this key, or is not a batch,
/// or holds more than a batch may.
pub fn open_batch(key: &[u8; 32], sealed: &[u8]) -> Option<Batch> {
    let plain = crate::vault::open_with(key, sealed, CONTEXT)?;
    let batch: Batch = postcard::from_bytes(&plain).ok()?;
    let fits = batch.events.len() <= MOST_IN_BATCH
        && batch.events.iter().all(|e| match e {
            Input::Text(text) => text.chars().count() <= MOST_TEXT,
            Input::LetGo => false,
            _ => true,
        });
    fits.then_some(batch)
}

/// Why it cannot be done. What the one asking is told, and the owner, in words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// The owner did not turn this on.
    TurnedOff,
    /// A contact for too short a time.
    TooNew,
    /// Only between two people.
    NotOneToOne,
    /// Nothing is being shown.
    NotSharing,
    /// This device cannot do it safely now: it runs as administrator, it is no PC, or what is being
    /// shown cannot be told for certain.
    NotPossible,
    /// Someone is asking already, or in control.
    Busy,
    /// The owner said no, or nobody answered.
    SaidNo,
}

impl Refusal {
    /// What the one asking gets to hear. Why exactly is for the owner: "wait a week" and "ask them to
    /// turn it on" are the next steps of a scam.
    pub fn for_them(self) -> Self {
        match self {
            Self::SaidNo | Self::Busy => self,
            _ => Self::NotPossible,
        }
    }
}

/// What has to be true before anyone can even ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checks {
    /// Turned on before this call began. During a call it does not count: that is when someone is
    /// being talked into it.
    pub turned_on: bool,
    /// How long they have been a contact.
    pub known_for: Millis,
    pub one_to_one: bool,
    pub sharing: bool,
    /// This device can do it safely: see [`Refusal::NotPossible`].
    pub possible: bool,
}

impl Checks {
    pub fn verdict(&self) -> Result<(), Refusal> {
        if !self.possible {
            return Err(Refusal::NotPossible);
        }
        if !self.turned_on {
            return Err(Refusal::TurnedOff);
        }
        if !self.one_to_one {
            return Err(Refusal::NotOneToOne);
        }
        if !self.sharing {
            return Err(Refusal::NotSharing);
        }
        if self.known_for < KNOWN_FOR {
            return Err(Refusal::TooNew);
        }
        Ok(())
    }
}

/// Why a session ended. Each of these lets go of whatever was held down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ended {
    /// The owner pressed Stop or Esc.
    Owner,
    /// The one in control stopped.
    Them,
    TimeUp,
    /// Nothing heard from them.
    Silence,
    /// The call ended, or the sharing stopped.
    CallOrSharing,
    /// The screen locked or a secure prompt came up, what was shown went away, or they were blocked.
    NotSafeAnyMore,
    /// Too much that did not fit.
    Misbehaved,
}

impl Ended {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Them => "them",
            Self::TimeUp => "time",
            Self::Silence => "silence",
            Self::CallOrSharing => "sharing",
            Self::NotSafeAnyMore => "unsafe",
            Self::Misbehaved => "misbehaved",
        }
    }

    pub fn from_word(text: &str) -> Self {
        match text {
            "owner" => Self::Owner,
            "them" => Self::Them,
            "time" => Self::TimeUp,
            "silence" => Self::Silence,
            "sharing" => Self::CallOrSharing,
            "misbehaved" => Self::Misbehaved,
            _ => Self::NotSafeAnyMore,
        }
    }
}

/// One session, for the list in Privacy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub who: String,
    pub whole_screen: bool,
    pub from: Millis,
    pub to: Millis,
    pub why: Ended,
}

/// What to do with what came in.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// These may go on to the desktop, where the app checks where they land.
    Use(Vec<Input>),
    /// Not now, but nothing wrong: the owner is busy.
    Waiting,
    /// Not this one.
    Dropped,
    Ended(Ended),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Off,
    Asked { from: String, at: Millis },
    On(Session),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Session {
    id: [u8; 16],
    who: String,
    scope: Scope,
    until: Millis,
    /// Nothing from them until then: the owner is using the PC.
    waiting_until: Millis,
    last_heard: Millis,
    last_number: Option<u64>,
    strikes: u32,
    second: Millis,
    in_this_second: u32,
}

/// The side being controlled: who may send what, right now. Time always comes from outside, and is
/// a clock that only goes forward, so setting the PC's clock back gives nobody more time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controlled {
    state: State,
    /// No new questions until then.
    no_asks_until: Millis,
}

impl Default for Controlled {
    fn default() -> Self {
        Self {
            state: State::Off,
            no_asks_until: 0,
        }
    }
}

/// What the owner agreed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grant {
    pub id: [u8; 16],
    pub scope: Scope,
    pub until: Millis,
}

impl Controlled {
    pub fn is_on(&self) -> bool {
        matches!(self.state, State::On(_))
    }

    /// Who is in control, if anyone.
    pub fn who(&self) -> Option<&str> {
        match &self.state {
            State::On(session) => Some(&session.who),
            _ => None,
        }
    }

    pub fn until(&self) -> Option<Millis> {
        match &self.state {
            State::On(session) => Some(session.until),
            _ => None,
        }
    }

    pub fn scope(&self) -> Option<Scope> {
        match &self.state {
            State::On(session) => Some(session.scope),
            _ => None,
        }
    }

    /// Who is waiting for an answer, if anyone.
    pub fn asked_by(&self) -> Option<&str> {
        match &self.state {
            State::Asked { from, .. } => Some(from),
            _ => None,
        }
    }

    /// Someone asks. Nobody gets further than this question without all the rules being met, and
    /// nothing has changed yet when this says yes: only the owner's answer does that.
    pub fn ask(&mut self, from: &str, checks: &Checks, now: Millis) -> Result<(), Refusal> {
        checks.verdict()?;
        match &self.state {
            State::Off => {}
            State::Asked { at, .. } if now - at > ASK_WAITS => {
                self.no_asks_until = now + NO_HOLDS;
                self.state = State::Off;
            }
            _ => return Err(Refusal::Busy),
        }
        if now < self.no_asks_until {
            return Err(Refusal::SaidNo);
        }
        self.state = State::Asked {
            from: from.to_owned(),
            at: now,
        };
        Ok(())
    }

    /// The owner's answer, on this device, to whoever asked. `id` is new for each session. A yes
    /// before there was time to read it changes nothing: the question stays.
    pub fn answer(&mut self, yes: bool, scope: Scope, id: [u8; 16], now: Millis) -> Option<Grant> {
        let State::Asked { from, at } = &self.state else {
            return None;
        };
        if yes && now - at < READ_FIRST {
            return None;
        }
        if !yes || now - at > ASK_WAITS {
            self.no_asks_until = now + NO_HOLDS;
            self.state = State::Off;
            return None;
        }
        let until = now + SESSION;
        self.state = State::On(Session {
            id,
            who: from.clone(),
            scope,
            until,
            waiting_until: 0,
            last_heard: now,
            last_number: None,
            strikes: 0,
            second: now,
            in_this_second: 0,
        });
        Some(Grant { id, scope, until })
    }

    /// The owner's own hand on the mouse or keyboard: they wait.
    pub fn owner_busy(&mut self, now: Millis) {
        if let State::On(session) = &mut self.state {
            session.waiting_until = now + OWNER_WINS;
        }
    }

    /// More time, because the owner said so with their own hand.
    pub fn extend(&mut self, now: Millis) -> Option<Millis> {
        let State::On(session) = &mut self.state else {
            return None;
        };
        session.until = now + SESSION;
        Some(session.until)
    }

    /// Whatever the reason: over. Returns whether there was anything to end.
    pub fn end(&mut self) -> bool {
        let was = self.is_on();
        self.state = State::Off;
        was
    }

    /// Something came in that was not sealed for this session. A few and it is over.
    pub fn strike(&mut self) -> Option<Ended> {
        let State::On(session) = &mut self.state else {
            return None;
        };
        session.strikes += 1;
        if session.strikes >= STRIKES {
            self.state = State::Off;
            return Some(Ended::Misbehaved);
        }
        None
    }

    /// Time passing: what ends by itself.
    pub fn tick(&mut self, now: Millis) -> Option<Ended> {
        match &self.state {
            State::Asked { at, .. } if now - at > ASK_WAITS => {
                self.no_asks_until = now + NO_HOLDS;
                self.state = State::Off;
                None
            }
            State::On(session) if now >= session.until => {
                self.state = State::Off;
                Some(Ended::TimeUp)
            }
            State::On(session) if now - session.last_heard > SILENCE => {
                self.state = State::Off;
                Some(Ended::Silence)
            }
            _ => None,
        }
    }

    /// A batch came in. Only what fits every rule gets through.
    pub fn input(
        &mut self,
        from: &str,
        session_id: &[u8; 16],
        batch: Batch,
        now: Millis,
    ) -> Verdict {
        if let Some(ended) = self.tick(now) {
            return Verdict::Ended(ended);
        }
        let State::On(session) = &mut self.state else {
            return Verdict::Dropped;
        };
        if session.who != from || session.id != *session_id {
            return Verdict::Dropped;
        }
        // The same or an older number: sent again by someone, or something is wrong.
        if session.last_number.is_some_and(|last| batch.number <= last) {
            session.strikes += 1;
            if session.strikes >= STRIKES {
                self.state = State::Off;
                return Verdict::Ended(Ended::Misbehaved);
            }
            return Verdict::Dropped;
        }
        session.last_number = Some(batch.number);
        session.last_heard = now;
        if now - session.second >= 1000 {
            session.second = now;
            session.in_this_second = 0;
        }
        session.in_this_second += batch.events.len() as u32;
        if session.in_this_second > MOST_PER_SECOND {
            // Too much, but what goes up still goes up.
            let up: Vec<Input> = batch.events.into_iter().filter(Input::is_release).collect();
            return if up.is_empty() {
                Verdict::Dropped
            } else {
                Verdict::Use(up)
            };
        }
        if now < session.waiting_until {
            return Verdict::Waiting;
        }
        let events: Vec<Input> = batch
            .events
            .into_iter()
            .filter(|e| !matches!(e, Input::StillHere | Input::LetGo))
            .collect();
        Verdict::Use(events)
    }
}

// ---- keys ----

/// Scan codes of the keys that matter here.
pub mod scan {
    pub const ESC: u16 = 0x01;
    pub const CTRL: u16 = 0x1D;
    pub const LEFT_SHIFT: u16 = 0x2A;
    pub const RIGHT_SHIFT: u16 = 0x36;
    pub const ALT: u16 = 0x38;
    pub const TAB: u16 = 0x0F;
    pub const DELETE: u16 = 0x53;
    /// With `extended` set.
    pub const LEFT_WIN: u16 = 0x5B;
    pub const RIGHT_WIN: u16 = 0x5C;
    pub const MENU: u16 = 0x5D;
}

/// The extended keys there are on a normal keyboard: Enter on the number pad, right Ctrl, divide,
/// Print Screen, right Alt, the arrows and what is around them, and the menu key.
const EXTENDED_KEYS: [u16; 16] = [
    0x1C, 0x1D, 0x35, 0x37, 0x38, 0x47, 0x48, 0x49, 0x4B, 0x4D, 0x4F, 0x50, 0x51, 0x52, 0x53, 0x5D,
];
/// F12.
const LAST_PLAIN_KEY: u16 = 0x58;

/// Which modifier keys the one in control is holding down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Held {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Held {
    pub fn note(&mut self, code: u16, down: bool) {
        match code {
            scan::CTRL => self.ctrl = down,
            scan::LEFT_SHIFT | scan::RIGHT_SHIFT => self.shift = down,
            scan::ALT => self.alt = down,
            _ => {}
        }
    }
}

/// May this key reach the PC? The Windows key and what opens the system's own tools do not, and
/// never mind what is being shown: those are how someone gets out of what they were let into.
pub fn key_allowed(code: u16, extended: bool, held: Held, scope: Scope) -> bool {
    // Only what is on a normal keyboard. Not the keys that start the mail program or the calculator,
    // and nothing that only looks different from the keys blocked below.
    if extended {
        if !EXTENDED_KEYS.contains(&code) {
            return false;
        }
    } else if code == 0 || code > LAST_PLAIN_KEY {
        return false;
    }
    if extended && matches!(code, scan::LEFT_WIN | scan::RIGHT_WIN) {
        return false;
    }
    if code == scan::ESC && held.ctrl {
        // Ctrl+Esc opens Start, Ctrl+Shift+Esc the Task Manager.
        return false;
    }
    if code == scan::DELETE && held.ctrl && held.alt {
        return false;
    }
    if scope == Scope::Window && held.alt && matches!(code, scan::TAB | scan::ESC) {
        // Alt+Tab and Alt+Esc go to another window.
        return false;
    }
    true
}

// ---- where input lands ----

/// The part of the real desktop this has to do with: a test uses a made-up one.
pub trait Desk {
    /// The normal desktop has the input: the screen is not locked, and there is no secure prompt.
    fn input_is_ours(&self) -> bool;
    /// The part of the screen being shown, in pixels. `None`: it is gone, or cannot be told.
    fn shown_area(&self) -> Option<Area>;
    /// Whose window is at this point of the screen.
    fn window_at(&self, x: i32, y: i32) -> Landing;
    /// Whose window gets what is typed.
    fn foreground(&self) -> Landing;
    fn mouse_to(&mut self, x: i32, y: i32);
    fn button(&mut self, which: MouseButton, down: bool);
    fn wheel(&mut self, dx: i16, dy: i16);
    fn key(&mut self, code: u16, extended: bool, down: bool);
    fn text(&mut self, text: &str);
}

/// Part of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Area {
    /// From 0 to 65535 over what is shown, to a pixel. Always inside it.
    pub fn point(&self, x: u16, y: u16) -> (i32, i32) {
        let along = |size: i32, at: u16| {
            let last = i64::from(size.max(1) - 1);
            (last * i64::from(at) / 65535) as i32
        };
        (
            self.x + along(self.width, x),
            self.y + along(self.height, y),
        )
    }
}

/// Where something would end up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    /// This app itself. Nobody clicks Allow, More time or Stop for the owner.
    Own,
    /// The window being shown, or something of its own, like a dialog or a menu.
    Shown,
    /// Anything else.
    Other,
    /// Cannot be told.
    Unknown,
}

/// Puts what may be used on the desktop, after one more look at where it would land. Remembers what
/// is held down, because whatever way it ends, nothing may stay stuck.
pub struct Applier<D: Desk> {
    desk: D,
    scope: Scope,
    keys: Vec<(u16, bool)>,
    buttons: Vec<MouseButton>,
    pointer: Option<(i32, i32)>,
}

impl<D: Desk> Applier<D> {
    pub fn new(desk: D, scope: Scope) -> Self {
        Self {
            desk,
            scope,
            keys: Vec::new(),
            buttons: Vec::new(),
            pointer: None,
        }
    }

    pub fn desk(&self) -> &D {
        &self.desk
    }

    /// Is it still safe, also when nothing is coming in?
    pub fn look(&mut self) -> Result<(), Ended> {
        if self.desk.input_is_ours() && self.desk.shown_area().is_some() {
            Ok(())
        } else {
            self.let_go();
            Err(Ended::NotSafeAnyMore)
        }
    }

    fn may_land(&self, on: Landing) -> bool {
        match on {
            Landing::Shown => true,
            Landing::Other => self.scope == Scope::WholeScreen,
            Landing::Own | Landing::Unknown => false,
        }
    }

    /// What is down right now, from what really is. Left and right are each a key of their own: one
    /// going up says nothing about the other.
    fn held(&self) -> Held {
        let mut held = Held::default();
        for (code, _) in &self.keys {
            held.note(*code, true);
        }
        held
    }

    /// A press and the wheel land where the real pointer is, and the owner's hand may have moved it.
    /// So it goes back to the place that was checked, checked again, right before.
    fn point_again(&mut self) -> bool {
        let Some((x, y)) = self.pointer else {
            return false;
        };
        if !self.may_land(self.desk.window_at(x, y)) {
            return false;
        }
        self.desk.mouse_to(x, y);
        true
    }

    pub fn apply(&mut self, events: Vec<Input>) -> Result<(), Ended> {
        self.look()?;
        let Some(area) = self.desk.shown_area() else {
            return Err(Ended::NotSafeAnyMore);
        };
        for event in events {
            match event {
                Input::Move { x, y } => {
                    let (x, y) = area.point(x, y);
                    if self.may_land(self.desk.window_at(x, y)) {
                        self.desk.mouse_to(x, y);
                        self.pointer = Some((x, y));
                    } else {
                        // A press now would land where the pointer last was, not where they think.
                        self.pointer = None;
                    }
                }
                Input::Button { which, down: true } => {
                    if self.point_again() {
                        self.desk.button(which, true);
                        if !self.buttons.contains(&which) {
                            self.buttons.push(which);
                        }
                    }
                }
                Input::Button { which, down: false } => {
                    if self.buttons.contains(&which) {
                        self.buttons.retain(|b| *b != which);
                        self.desk.button(which, false);
                    }
                }
                Input::Wheel { dx, dy } => {
                    if self.point_again() {
                        self.desk.wheel(dx, dy);
                    }
                }
                Input::Key {
                    code,
                    extended,
                    down: true,
                } => {
                    if key_allowed(code, extended, self.held(), self.scope)
                        && self.may_land(self.desk.foreground())
                    {
                        self.desk.key(code, extended, true);
                        if !self.keys.contains(&(code, extended)) {
                            self.keys.push((code, extended));
                        }
                    }
                }
                Input::Key {
                    code,
                    extended,
                    down: false,
                } => {
                    if self.keys.contains(&(code, extended)) {
                        self.keys.retain(|k| *k != (code, extended));
                        self.desk.key(code, extended, false);
                    }
                }
                Input::Text(text) => {
                    let text: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !text.is_empty() && self.may_land(self.desk.foreground()) {
                        self.desk.text(&text);
                    }
                }
                Input::StillHere => {}
                Input::LetGo => self.let_go(),
            }
        }
        Ok(())
    }

    /// Whatever is held down goes up.
    pub fn let_go(&mut self) {
        for which in std::mem::take(&mut self.buttons) {
            self.desk.button(which, false);
        }
        for (code, extended) in std::mem::take(&mut self.keys) {
            self.desk.key(code, extended, false);
        }
        self.pointer = None;
    }
}

impl<D: Desk> Drop for Applier<D> {
    fn drop(&mut self) {
        self.let_go();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEK: Millis = KNOWN_FOR;
    const NOW: Millis = 1_000_000_000;

    fn fine() -> Checks {
        Checks {
            turned_on: true,
            known_for: WEEK,
            one_to_one: true,
            sharing: true,
            possible: true,
        }
    }

    fn batch(number: u64) -> Batch {
        Batch {
            number,
            events: vec![Input::Move { x: 1, y: 2 }],
        }
    }

    fn granted() -> (Controlled, [u8; 16]) {
        let mut c = Controlled::default();
        c.ask("noor", &fine(), NOW).unwrap();
        let grant = c.answer(true, Scope::Window, [7; 16], NOW + 6000).unwrap();
        assert_eq!(grant.until, NOW + 6000 + SESSION);
        (c, [7; 16])
    }

    #[test]
    fn nobody_gets_to_ask_unless_every_rule_is_met() {
        let cases = [
            (
                Checks {
                    possible: false,
                    ..fine()
                },
                Refusal::NotPossible,
            ),
            (
                Checks {
                    turned_on: false,
                    ..fine()
                },
                Refusal::TurnedOff,
            ),
            (
                Checks {
                    one_to_one: false,
                    ..fine()
                },
                Refusal::NotOneToOne,
            ),
            (
                Checks {
                    sharing: false,
                    ..fine()
                },
                Refusal::NotSharing,
            ),
            (
                Checks {
                    known_for: 1000,
                    ..fine()
                },
                Refusal::TooNew,
            ),
            (
                Checks {
                    known_for: WEEK - 1,
                    ..fine()
                },
                Refusal::TooNew,
            ),
        ];
        for (checks, refusal) in cases {
            let mut c = Controlled::default();
            assert_eq!(c.ask("noor", &checks, NOW), Err(refusal));
            assert!(!c.is_on());
            assert!(
                c.answer(true, Scope::WholeScreen, [1; 16], NOW + READ_FIRST)
                    .is_none(),
                "no question, no yes"
            );
        }
        // A week is enough, and the rest still counts.
        assert!(Controlled::default().ask("noor", &fine(), NOW).is_ok());
    }

    #[test]
    fn the_one_asking_does_not_hear_what_to_do_about_it() {
        for why in [
            Refusal::TurnedOff,
            Refusal::TooNew,
            Refusal::NotOneToOne,
            Refusal::NotSharing,
            Refusal::NotPossible,
        ] {
            assert_eq!(why.for_them(), Refusal::NotPossible);
        }
        assert_eq!(Refusal::SaidNo.for_them(), Refusal::SaidNo);
        assert_eq!(Refusal::Busy.for_them(), Refusal::Busy);
    }

    #[test]
    fn a_yes_needs_time_to_read_and_nobody_gets_to_keep_asking() {
        let mut c = Controlled::default();
        c.ask("noor", &fine(), NOW).unwrap();
        // Too soon: nothing happens, and the question is still there.
        assert!(
            c.answer(true, Scope::Window, [1; 16], NOW + READ_FIRST - 1)
                .is_none()
        );
        assert_eq!(c.asked_by(), Some("noor"));
        assert!(!c.is_on());
        // No, and they cannot ask again for a while.
        assert!(
            c.answer(false, Scope::Window, [1; 16], NOW + 1000)
                .is_none()
        );
        assert_eq!(c.ask("noor", &fine(), NOW + 2000), Err(Refusal::SaidNo));
        assert_eq!(c.asked_by(), None);
        assert_eq!(
            c.ask("noor", &fine(), NOW + 1000 + NO_HOLDS - 1),
            Err(Refusal::SaidNo)
        );
        assert!(c.ask("noor", &fine(), NOW + 1000 + NO_HOLDS).is_ok());
        // No answer counts the same.
        let t = NOW + 1000 + NO_HOLDS;
        assert_eq!(c.tick(t + ASK_WAITS + 1), None);
        assert_eq!(c.asked_by(), None);
        assert_eq!(
            c.ask("noor", &fine(), t + ASK_WAITS + 2),
            Err(Refusal::SaidNo)
        );
    }

    #[test]
    fn what_does_not_open_counts_against_them() {
        let (mut c, _) = granted();
        assert_eq!(c.strike(), None);
        assert_eq!(c.strike(), None);
        assert_eq!(c.strike(), Some(Ended::Misbehaved));
        assert!(!c.is_on());
        assert_eq!(c.strike(), None, "nothing left to end");
    }

    #[test]
    fn asking_changes_nothing_until_the_owner_says_yes() {
        let mut c = Controlled::default();
        c.ask("noor", &fine(), NOW).unwrap();
        assert!(!c.is_on());
        assert_eq!(
            c.input("noor", &[0; 16], batch(1), NOW + 1),
            Verdict::Dropped
        );
        assert_eq!(c.ask("bas", &fine(), NOW + 1), Err(Refusal::Busy));
        // No, and it is over.
        assert!(c.answer(false, Scope::Window, [1; 16], NOW + 2).is_none());
        assert!(!c.is_on());
        assert!(
            c.answer(true, Scope::Window, [1; 16], NOW + READ_FIRST + 3)
                .is_none(),
            "one question, one answer"
        );
        // Too late to answer.
        let mut c = Controlled::default();
        c.ask("noor", &fine(), NOW).unwrap();
        assert!(
            c.answer(true, Scope::Window, [1; 16], NOW + ASK_WAITS + 1)
                .is_none()
        );
        assert!(!c.is_on());
    }

    #[test]
    fn only_the_one_who_was_let_in_with_this_session_gets_through() {
        let (mut c, id) = granted();
        let t = NOW + 7000;
        assert!(matches!(c.input("noor", &id, batch(1), t), Verdict::Use(_)));
        assert_eq!(
            c.input("bas", &id, batch(2), t),
            Verdict::Dropped,
            "someone else"
        );
        assert_eq!(
            c.input("noor", &[9; 16], batch(3), t),
            Verdict::Dropped,
            "another session"
        );
    }

    #[test]
    fn the_owners_own_hand_always_wins() {
        let (mut c, id) = granted();
        let t = NOW + 7000;
        c.owner_busy(t);
        assert_eq!(c.input("noor", &id, batch(1), t + 1), Verdict::Waiting);
        assert_eq!(
            c.input("noor", &id, batch(2), t + OWNER_WINS - 1),
            Verdict::Waiting
        );
        assert!(matches!(
            c.input("noor", &id, batch(3), t + OWNER_WINS),
            Verdict::Use(_)
        ));
        assert!(c.end());
        assert_eq!(
            c.input("noor", &id, batch(4), t + OWNER_WINS + 1),
            Verdict::Dropped
        );
    }

    #[test]
    fn it_ends_by_itself() {
        // Time up.
        let (mut c, id) = granted();
        let mut t = NOW + 6000;
        for n in 1..((SESSION / 4000) as u64) {
            t += 4000;
            let _ = c.input("noor", &id, batch(n), t);
        }
        assert_eq!(c.tick(NOW + 6000 + SESSION), Some(Ended::TimeUp));
        assert!(!c.is_on());
        // Silence: the connection is gone, and nothing may stay in control.
        let (mut c, id) = granted();
        assert_eq!(c.tick(NOW + 6000 + SILENCE), None);
        assert_eq!(c.tick(NOW + 6000 + SILENCE + 1), Some(Ended::Silence));
        assert_eq!(
            c.input("noor", &id, batch(1), NOW + 6000 + SILENCE + 2),
            Verdict::Dropped
        );
        // More time only if the owner gives it.
        let (mut c, _) = granted();
        let later = NOW + 6000 + SESSION - 1000;
        assert_eq!(c.extend(later), Some(later + SESSION));
    }

    #[test]
    fn numbers_go_up_and_what_is_sent_again_ends_it() {
        let (mut c, id) = granted();
        let t = NOW + 7000;
        assert!(matches!(c.input("noor", &id, batch(5), t), Verdict::Use(_)));
        assert_eq!(c.input("noor", &id, batch(5), t), Verdict::Dropped);
        assert_eq!(c.input("noor", &id, batch(4), t), Verdict::Dropped);
        assert_eq!(
            c.input("noor", &id, batch(1), t),
            Verdict::Ended(Ended::Misbehaved)
        );
        assert!(!c.is_on());
    }

    #[test]
    fn a_flood_gets_nowhere() {
        let (mut c, id) = granted();
        let t = NOW + 7000;
        let many = Batch {
            number: 1,
            events: vec![Input::Move { x: 0, y: 0 }; MOST_PER_SECOND as usize + 1],
        };
        assert_eq!(c.input("noor", &id, many, t), Verdict::Dropped);
        // The next second it is fine again.
        assert!(matches!(
            c.input("noor", &id, batch(2), t + 1000),
            Verdict::Use(_)
        ));
    }

    #[test]
    fn still_here_keeps_it_alive_and_moves_nothing() {
        let (mut c, id) = granted();
        let t = NOW + 6000 + SILENCE - 1;
        let alive = Batch {
            number: 1,
            events: vec![Input::StillHere],
        };
        assert_eq!(c.input("noor", &id, alive, t), Verdict::Use(Vec::new()));
        assert_eq!(c.tick(t + SILENCE), None);
    }

    /// Whatever happens, in whatever order, nothing is ever used outside a session the owner agreed
    /// to, from someone else, while waiting, or after the end.
    #[test]
    fn no_order_of_events_lets_anything_through_that_should_not_pass() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2_000 {
            let mut c = Controlled::default();
            let mut t = NOW;
            let mut said_yes_for: Option<String> = None;
            let mut session = [0u8; 16];
            let mut waiting_until = 0;
            let mut number = 0;
            for _ in 0..60 {
                t += (next() % 3000) as Millis;
                let who = if next() % 2 == 0 { "noor" } else { "bas" };
                match next() % 8 {
                    0 => {
                        let good = next() % 2 == 0;
                        let checks = if good {
                            fine()
                        } else {
                            Checks {
                                turned_on: false,
                                ..fine()
                            }
                        };
                        let _ = c.ask(who, &checks, t);
                    }
                    1 => {
                        let id = [(next() % 200) as u8; 16];
                        let before = c.state.clone();
                        if let Some(grant) = c.answer(true, Scope::Window, id, t) {
                            let State::Asked { from, .. } = before else {
                                panic!("a yes without a question");
                            };
                            said_yes_for = Some(from);
                            session = grant.id;
                            waiting_until = 0;
                        }
                    }
                    2 => {
                        c.answer(false, Scope::Window, [0; 16], t);
                    }
                    3 => {
                        if c.end() {
                            said_yes_for = None;
                        }
                    }
                    4 => {
                        c.owner_busy(t);
                        if c.is_on() {
                            waiting_until = t + OWNER_WINS;
                        }
                    }
                    5 => {
                        c.tick(t);
                    }
                    _ => {
                        number += 1;
                        let use_id = if next() % 2 == 0 {
                            session
                        } else {
                            [(next() % 200) as u8; 16]
                        };
                        if let Verdict::Use(_) = c.input(who, &use_id, batch(number), t) {
                            assert_eq!(said_yes_for.as_deref(), Some(who), "someone else");
                            assert_eq!(use_id, session, "another session");
                            assert!(t >= waiting_until, "while the owner was busy");
                            assert!(c.is_on(), "after the end");
                            assert!(t < c.until().unwrap(), "after the time was up");
                        }
                    }
                }
                if !c.is_on() {
                    said_yes_for = None;
                }
            }
        }
    }

    #[test]
    fn what_comes_off_the_wire_is_this_sessions_or_nothing() {
        let key = [3u8; 32];
        let good = Batch {
            number: 9,
            events: vec![Input::Text("hello".into())],
        };
        let sealed = seal_batch(&key, &good).unwrap();
        assert_eq!(open_batch(&key, &sealed), Some(good));
        assert_eq!(open_batch(&[4u8; 32], &sealed), None, "another key");
        let mut bent = sealed.clone();
        *bent.last_mut().unwrap() ^= 1;
        assert_eq!(open_batch(&key, &bent), None, "changed on the way");
        assert_eq!(open_batch(&key, b"nonsense"), None);
        assert_eq!(open_batch(&key, &[]), None);
        // Too much in one go is not a batch.
        let many = Batch {
            number: 1,
            events: vec![Input::StillHere; MOST_IN_BATCH + 1],
        };
        assert_eq!(open_batch(&key, &seal_batch(&key, &many).unwrap()), None);
        let long = Batch {
            number: 1,
            events: vec![Input::Text("x".repeat(MOST_TEXT + 1))],
        };
        assert_eq!(open_batch(&key, &seal_batch(&key, &long).unwrap()), None);
        // Whatever bytes come in: no panic, and nothing opens.
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..5_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let bytes: Vec<u8> = (0..(seed % 200))
                .map(|i| (seed >> (i % 56)) as u8)
                .collect();
            assert_eq!(open_batch(&key, &bytes), None);
        }
    }

    #[test]
    fn the_keys_that_get_someone_out_of_what_they_were_let_into_stay_here() {
        let none = Held::default();
        let ctrl = Held { ctrl: true, ..none };
        let alt = Held { alt: true, ..none };
        let ctrl_alt = Held {
            ctrl: true,
            alt: true,
            ..none
        };
        for scope in [Scope::Window, Scope::WholeScreen] {
            assert!(!key_allowed(scan::LEFT_WIN, true, none, scope));
            assert!(!key_allowed(scan::RIGHT_WIN, true, none, scope));
            assert!(!key_allowed(scan::ESC, false, ctrl, scope), "Ctrl+Esc");
            assert!(
                !key_allowed(scan::DELETE, true, ctrl_alt, scope),
                "Ctrl+Alt+Del"
            );
            assert!(key_allowed(0x1E, false, none, scope), "the letter A");
            assert!(key_allowed(0x1E, false, ctrl, scope), "Ctrl+A");
            assert!(key_allowed(scan::ESC, false, none, scope), "Esc");
        }
        assert!(
            !key_allowed(scan::TAB, false, alt, Scope::Window),
            "Alt+Tab in one window"
        );
        assert!(key_allowed(scan::TAB, false, alt, Scope::WholeScreen));
        assert!(key_allowed(scan::TAB, false, none, Scope::Window), "Tab");
    }

    #[test]
    fn what_is_held_follows_the_keys() {
        let mut held = Held::default();
        held.note(scan::CTRL, true);
        held.note(scan::ALT, true);
        assert!(held.ctrl && held.alt);
        held.note(scan::CTRL, false);
        assert!(!held.ctrl && held.alt);
        held.note(0x1E, true);
        assert!(held.alt);
    }

    /// A desktop on paper: left of x=100 is this app, between 100 and 200 another program.
    #[derive(Default)]
    struct Paper {
        locked: bool,
        gone: bool,
        typing_on: Option<Landing>,
        did: Vec<String>,
    }

    impl Desk for Paper {
        fn input_is_ours(&self) -> bool {
            !self.locked
        }
        fn shown_area(&self) -> Option<Area> {
            (!self.gone).then_some(Area {
                x: 0,
                y: 0,
                width: 1000,
                height: 1000,
            })
        }
        fn window_at(&self, x: i32, _y: i32) -> Landing {
            match x {
                0..=99 => Landing::Own,
                100..=199 => Landing::Other,
                200..=899 => Landing::Shown,
                _ => Landing::Unknown,
            }
        }
        fn foreground(&self) -> Landing {
            self.typing_on.unwrap_or(Landing::Shown)
        }
        fn mouse_to(&mut self, x: i32, y: i32) {
            self.did.push(format!("to {x},{y}"));
        }
        fn button(&mut self, which: MouseButton, down: bool) {
            self.did.push(format!("{which:?} {down}"));
        }
        fn wheel(&mut self, dx: i16, dy: i16) {
            self.did.push(format!("wheel {dx},{dy}"));
        }
        fn key(&mut self, code: u16, _extended: bool, down: bool) {
            self.did.push(format!("key {code} {down}"));
        }
        fn text(&mut self, text: &str) {
            self.did.push(format!("text {text}"));
        }
    }

    /// Where x lands on the paper desktop.
    fn at(x: u16) -> Input {
        Input::Move {
            x: (u32::from(x) * 65535 / 999) as u16,
            y: 0,
        }
    }

    fn press(which: MouseButton) -> Vec<Input> {
        vec![
            Input::Button { which, down: true },
            Input::Button { which, down: false },
        ]
    }

    fn did(a: &mut Applier<Paper>) -> Vec<String> {
        std::mem::take(&mut a.desk.did)
    }

    #[test]
    fn the_corners_of_what_is_shown_are_inside_it() {
        let area = Area {
            x: -1920,
            y: 100,
            width: 1920,
            height: 1080,
        };
        assert_eq!(area.point(0, 0), (-1920, 100));
        assert_eq!(area.point(65535, 65535), (-1, 1179));
        assert_eq!(area.point(32768, 0).0, -1920 + 959);
        // Something odd is no reason to end up elsewhere.
        assert_eq!(
            Area {
                x: 5,
                y: 5,
                width: 0,
                height: 0
            }
            .point(65535, 65535),
            (5, 5)
        );
    }

    #[test]
    fn nothing_lands_on_this_app_whatever_is_shown() {
        for scope in [Scope::Window, Scope::WholeScreen] {
            let mut a = Applier::new(Paper::default(), scope);
            a.apply(vec![at(500)]).unwrap();
            did(&mut a);
            // To a spot on the app: the pointer does not go there, and no click follows it there.
            let mut events = vec![at(50)];
            events.extend(press(MouseButton::Left));
            events.push(Input::Wheel { dx: 0, dy: 1 });
            a.apply(events).unwrap();
            assert_eq!(did(&mut a), Vec::<String>::new());
            // What is typed does not reach it either.
            a.desk.typing_on = Some(Landing::Own);
            a.apply(vec![
                Input::Key {
                    code: 0x1C,
                    extended: false,
                    down: true,
                },
                Input::Text("yes".into()),
            ])
            .unwrap();
            assert_eq!(did(&mut a), Vec::<String>::new());
        }
    }

    #[test]
    fn one_window_means_that_window() {
        let mut a = Applier::new(Paper::default(), Scope::Window);
        let mut events = vec![at(150)];
        events.extend(press(MouseButton::Left));
        a.apply(events).unwrap();
        assert_eq!(
            did(&mut a),
            Vec::<String>::new(),
            "another program in the way"
        );
        let mut events = vec![at(500)];
        events.extend(press(MouseButton::Left));
        a.apply(events).unwrap();
        assert_eq!(did(&mut a).len(), 4, "the shown window");
        a.desk.typing_on = Some(Landing::Other);
        a.apply(vec![Input::Text("x".into())]).unwrap();
        assert_eq!(
            did(&mut a),
            Vec::<String>::new(),
            "another program has the keyboard"
        );

        // With the whole screen shown, that program is fine, and this app still is not.
        let mut a = Applier::new(Paper::default(), Scope::WholeScreen);
        let mut events = vec![at(150)];
        events.extend(press(MouseButton::Left));
        a.apply(events).unwrap();
        assert_eq!(did(&mut a).len(), 4);
    }

    #[test]
    fn what_cannot_be_told_is_treated_as_no() {
        let mut a = Applier::new(Paper::default(), Scope::WholeScreen);
        let mut events = vec![at(950)];
        events.extend(press(MouseButton::Left));
        a.apply(events).unwrap();
        assert_eq!(did(&mut a), Vec::<String>::new());
        a.desk.typing_on = Some(Landing::Unknown);
        a.apply(vec![Input::Key {
            code: 0x1E,
            extended: false,
            down: true,
        }])
        .unwrap();
        assert_eq!(did(&mut a), Vec::<String>::new());
    }

    #[test]
    fn blocked_keys_stay_blocked_also_held_together() {
        let mut a = Applier::new(Paper::default(), Scope::Window);
        let key = |code, extended, down| Input::Key {
            code,
            extended,
            down,
        };
        a.apply(vec![
            key(scan::LEFT_WIN, true, true),
            key(scan::CTRL, false, true),
            key(scan::ESC, false, true),
            key(scan::CTRL, false, false),
            key(scan::ALT, false, true),
            key(scan::TAB, false, true),
            key(scan::ALT, false, false),
        ])
        .unwrap();
        let did = did(&mut a);
        assert_eq!(
            did,
            ["key 29 true", "key 29 false", "key 56 true", "key 56 false"]
        );
    }

    #[test]
    fn when_it_stops_nothing_stays_held_down() {
        let mut a = Applier::new(Paper::default(), Scope::WholeScreen);
        a.apply(vec![
            at(500),
            Input::Button {
                which: MouseButton::Left,
                down: true,
            },
            Input::Key {
                code: scan::CTRL,
                extended: false,
                down: true,
            },
            Input::Key {
                code: 0x1E,
                extended: false,
                down: true,
            },
        ])
        .unwrap();
        did(&mut a);
        // The screen locks.
        a.desk.locked = true;
        assert_eq!(a.apply(vec![at(600)]), Err(Ended::NotSafeAnyMore));
        let went_up = did(&mut a);
        assert_eq!(went_up, ["Left false", "key 29 false", "key 30 false"]);
        // And nothing more goes through after that.
        assert_eq!(a.apply(vec![at(600)]), Err(Ended::NotSafeAnyMore));
        assert_eq!(did(&mut a), Vec::<String>::new());
    }

    #[test]
    fn a_window_that_went_away_ends_it_even_when_nothing_comes_in() {
        let mut a = Applier::new(Paper::default(), Scope::Window);
        assert_eq!(a.look(), Ok(()));
        a.desk.gone = true;
        assert_eq!(a.look(), Err(Ended::NotSafeAnyMore));
    }

    #[test]
    fn up_goes_only_for_what_went_down_and_text_has_no_control_characters() {
        let mut a = Applier::new(Paper::default(), Scope::WholeScreen);
        a.apply(vec![
            Input::Button {
                which: MouseButton::Left,
                down: false,
            },
            Input::Key {
                code: 0x1E,
                extended: false,
                down: false,
            },
            Input::Text("a\nb\u{7}".into()),
            Input::Text("\r\n".into()),
        ])
        .unwrap();
        assert_eq!(did(&mut a), ["text ab"]);
    }

    #[test]
    fn both_ctrl_keys_down_and_one_up_is_still_ctrl_down() {
        let key = |code, extended, down| Input::Key {
            code,
            extended,
            down,
        };
        for (scope, modifier, blocked) in [
            (Scope::WholeScreen, scan::CTRL, scan::ESC),
            (Scope::Window, scan::ALT, scan::TAB),
            (Scope::Window, scan::ALT, scan::ESC),
        ] {
            let mut a = Applier::new(Paper::default(), scope);
            a.apply(vec![
                key(modifier, false, true),
                key(modifier, true, true),
                key(modifier, true, false),
                key(blocked, false, true),
            ])
            .unwrap();
            let went = did(&mut a);
            assert_eq!(went.len(), 3, "the blocked key did not go: {went:?}");
        }
    }

    #[test]
    fn a_press_lands_where_it_was_checked_not_where_the_owner_moved_the_mouse() {
        let mut a = Applier::new(Paper::default(), Scope::Window);
        a.apply(vec![at(500)]).unwrap();
        did(&mut a);
        // The owner moved the real pointer anywhere since. The press goes back first.
        a.apply(vec![Input::Button {
            which: MouseButton::Left,
            down: true,
        }])
        .unwrap();
        let went = did(&mut a);
        assert!(went[0].starts_with("to "), "{went:?}");
        assert_eq!(went[1], "Left true");
        a.apply(vec![Input::Wheel { dx: 0, dy: 120 }]).unwrap();
        let went = did(&mut a);
        assert!(went[0].starts_with("to "), "{went:?}");
    }

    #[test]
    fn the_owners_hand_lets_go_of_what_is_held_and_what_goes_up_is_never_lost() {
        let mut a = Applier::new(Paper::default(), Scope::WholeScreen);
        a.apply(vec![
            at(500),
            Input::Button {
                which: MouseButton::Left,
                down: true,
            },
            Input::Key {
                code: scan::CTRL,
                extended: false,
                down: true,
            },
        ])
        .unwrap();
        did(&mut a);
        a.apply(vec![Input::LetGo]).unwrap();
        assert_eq!(did(&mut a), ["Left false", "key 29 false"]);

        // Nobody can send that from outside.
        let key = [3u8; 32];
        let forged = Batch {
            number: 1,
            events: vec![Input::LetGo],
        };
        assert_eq!(open_batch(&key, &seal_batch(&key, &forged).unwrap()), None);

        // Over the limit: what goes up still goes up, and nothing else.
        let (mut c, id) = granted();
        let mut events = vec![Input::Move { x: 0, y: 0 }; MOST_PER_SECOND as usize];
        events.push(Input::Button {
            which: MouseButton::Left,
            down: true,
        });
        events.push(Input::Key {
            code: 0x1E,
            extended: false,
            down: false,
        });
        let verdict = c.input("noor", &id, Batch { number: 1, events }, NOW + 7000);
        assert_eq!(
            verdict,
            Verdict::Use(vec![Input::Key {
                code: 0x1E,
                extended: false,
                down: false
            }])
        );
    }

    #[test]
    fn only_keys_that_are_on_a_normal_keyboard() {
        let none = Held::default();
        for scope in [Scope::Window, Scope::WholeScreen] {
            // What opens other programs: calculator, mail, this PC, the browser.
            for launcher in [0x21, 0x6C, 0x6B, 0x32] {
                assert!(!key_allowed(launcher, true, none, scope), "{launcher:#x}");
            }
            // What only looks different from a blocked key.
            for sneaky in [0xE05B, 0x0101, 0x015B, 0xFFFF, 0] {
                assert!(!key_allowed(sneaky, false, none, scope), "{sneaky:#x}");
                assert!(!key_allowed(sneaky, true, none, scope), "{sneaky:#x}");
            }
            assert!(key_allowed(0x58, false, none, scope), "F12");
            assert!(!key_allowed(0x59, false, none, scope));
            assert!(key_allowed(0x48, true, none, scope), "up");
            assert!(key_allowed(0x1D, true, none, scope), "right Ctrl");
        }
    }
}
