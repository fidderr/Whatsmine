//! "Back" for everything that can sit on top of a screen.
//!
//! Anything that covers the screen (a photo, a menu, a dialog, the camera)
//! renders a [`BackLayer`] while it is open. Back (the Android gesture or
//! button, Esc, the mouse's back button) first closes the topmost layer; only
//! when there is none does it go back a screen, and from the first screen to
//! the background or out of the app.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use dioxus::prelude::*;

type Layers = Rc<RefCell<Vec<(u64, EventHandler<()>)>>>;

/// The open layers, bottom first. Provided once, near the root.
#[derive(Clone, Default)]
pub struct BackStack(Layers);

impl BackStack {
    /// Closes the topmost layer. False if there was none.
    pub fn close_top(&self) -> bool {
        let top = self.0.borrow().last().map(|(_, h)| *h);
        match top {
            Some(handler) => {
                handler.call(());
                true
            }
            None => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Put this inside an overlay: while it is on screen, Back calls `on_back`
/// (which should close the overlay) instead of leaving the screen.
#[component]
pub fn BackLayer(on_back: EventHandler<()>) -> Element {
    let stack = use_context::<BackStack>();
    let id = use_hook(|| NEXT_ID.fetch_add(1, Ordering::Relaxed));
    use_hook({
        let stack = stack.clone();
        move || stack.0.borrow_mut().push((id, on_back))
    });
    use_drop(move || stack.0.borrow_mut().retain(|(i, _)| *i != id));
    rsx! {}
}
