//! The core lives for the whole life of the process, not of a window.
//!
//! The first caller starts it (the UI, or on Android possibly the background
//! service before any screen exists); everyone after gets the same one. That
//! is what lets messages keep arriving, and notifications keep appearing,
//! while the window is hidden in the tray or the Android activity is gone.

use std::path::Path;
use std::sync::OnceLock;

use whatsmine_core::{Core, NetMode, paths};

static CORE: OnceLock<Result<Core, String>> = OnceLock::new();

fn start(dir: Result<std::path::PathBuf, String>) -> Result<Core, String> {
    let dir = dir?;
    let core = Core::start(&dir, NetMode::Internet).map_err(|e| format!("{e:#}"))?;
    #[cfg(feature = "test-hooks")]
    crate::debug_hooks::seed_if_asked(&core);
    crate::notify::start(core.clone());
    Ok(core)
}

/// The core for this process, started on first use in the default data
/// directory (or the one named by `WHATSMINE_PROFILE`).
pub fn core() -> Result<Core, String> {
    CORE.get_or_init(|| {
        let profile = std::env::var("WHATSMINE_PROFILE").ok();
        start(paths::data_dir(profile.as_deref()).map_err(|e| format!("{e:#}")))
    })
    .clone()
}

/// Like [`core`], with the data directory given (Android's background service
/// starts before any activity, so it cannot ask the activity for it).
pub fn core_in(dir: &Path) -> Result<Core, String> {
    CORE.get_or_init(|| start(Ok(dir.to_path_buf()))).clone()
}
