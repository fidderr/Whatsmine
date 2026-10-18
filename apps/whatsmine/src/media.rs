//! Getting files into a chat: pick, capture, record, send; and getting them
//! out again: open and save. All device features come from mobile-sentinel.
//!
//! Every call that can wait for the user (a system picker, the camera app)
//! blocks, so it runs on a worker thread, never on the UI thread.

use std::path::{Path, PathBuf};

use whatsmine_core::Core;

/// A file ready to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Picked {
    pub path: PathBuf,
    pub name: String,
    pub mime: String,
    pub duration_ms: Option<u64>,
}

/// Android has a system camera app; elsewhere the app records in its own
/// camera screen.
pub const NATIVE_CAMERA: bool = cfg!(target_os = "android");

pub const IMAGES: &[&str] = &["image/*"];
pub const GALLERY: &[&str] = &["image/*", "video/*"];
pub const ANY_FILE: &[&str] = &[];

/// Runs `f` on a worker thread.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

/// Files the OS or camera handed us in a scratch folder: ours to delete
/// once the core has its own copy.
fn is_scratch(path: &Path) -> bool {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .is_some_and(|n| {
            matches!(
                n,
                "sentinel_imports" | "sentinel_camera" | "captures" | "recordings"
            )
        })
}

/// Opens the system picker. `None` means it could not be shown; an empty
/// list means the user cancelled.
pub async fn pick(mimes: &'static [&'static str], multiple: bool) -> Option<Vec<Picked>> {
    blocking(move || {
        mobile_sentinel::media_picker::pick_files(mimes, multiple)
            .ok()
            .map(|files| {
                files
                    .into_iter()
                    .map(|f| Picked {
                        path: f.path.into(),
                        name: f.name,
                        mime: f.mime,
                        duration_ms: None,
                    })
                    .collect()
            })
    })
    .await
    .flatten()
}

/// Photo or video from the system camera app (Android).
pub async fn capture(video: bool) -> Option<Picked> {
    blocking(move || {
        let result = if video {
            mobile_sentinel::camera::capture_video()
        } else {
            mobile_sentinel::camera::capture_photo()
        };
        log::info!(
            "media: system camera finished: {}",
            if result.is_ok() {
                "got a file"
            } else {
                "cancelled or unavailable"
            }
        );
        let path = PathBuf::from(result.ok()?);
        let (name, mime) = if video {
            ("video.mp4", "video/mp4")
        } else {
            ("photo.jpg", "image/jpeg")
        };
        Some(Picked {
            path,
            name: name.into(),
            mime: mime.into(),
            duration_ms: None,
        })
    })
    .await
    .flatten()
}

/// Sends each file to a contact. The caption goes with the first one.
/// Returns the error key of every file that could not be sent.
pub async fn send_files(
    core: Core,
    contact: String,
    files: Vec<Picked>,
    caption: String,
) -> Vec<String> {
    log::info!("media: sending {} file(s)", files.len());
    blocking(move || {
        let mut errors = Vec::new();
        for (i, f) in files.iter().enumerate() {
            let caption = if i == 0 { caption.as_str() } else { "" };
            let scratch = is_scratch(&f.path);
            match core.send_file_from(
                &contact,
                &f.path,
                &f.name,
                &f.mime,
                caption,
                f.duration_ms,
                scratch,
            ) {
                Ok(_) => {}
                Err(e) => errors.push(e.to_string()),
            }
            if scratch {
                // Moved already on success; clean up if it was copied or failed.
                let _ = std::fs::remove_file(&f.path);
            }
        }
        errors
    })
    .await
    .unwrap_or_default()
}

/// Sets a profile picture from `file` (and removes scratch files).
pub async fn set_avatar(core: Core, file: Picked) -> bool {
    blocking(move || {
        let ok = core.set_avatar(&file.path).is_ok();
        if is_scratch(&file.path) {
            let _ = std::fs::remove_file(&file.path);
        }
        ok
    })
    .await
    .unwrap_or(false)
}

// ---- voice notes ----

/// Where voice notes are recorded before the core copies them.
pub fn recordings_dir(core: &Core) -> PathBuf {
    core.media_dir().join("recordings")
}

pub enum VoiceStart {
    Started,
    /// The microphone permission dialog was shown and not granted in time.
    NoPermission,
    Failed,
}

/// Starts recording. On Android this asks for the microphone permission
/// first and waits for the answer.
pub async fn start_voice(core: &Core) -> VoiceStart {
    if !mobile_sentinel::recorder::has_permission() {
        mobile_sentinel::recorder::request_permission();
        let mut granted = false;
        for _ in 0..80 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if mobile_sentinel::recorder::has_permission() {
                granted = true;
                break;
            }
        }
        if !granted {
            return VoiceStart::NoPermission;
        }
    }
    let dir = recordings_dir(core);
    match blocking(move || mobile_sentinel::recorder::start(&dir)).await {
        Some(Ok(())) => VoiceStart::Started,
        _ => VoiceStart::Failed,
    }
}

/// Stops the recording and turns it into something sendable.
pub async fn finish_voice() -> Option<Picked> {
    let rec = blocking(mobile_sentinel::recorder::stop).await?.ok()?;
    if rec.duration_ms < 500 {
        let _ = std::fs::remove_file(&rec.path);
        return None;
    }
    let path = PathBuf::from(&rec.path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("m4a")
        .to_string();
    let mime = if ext == "wav" {
        "audio/wav"
    } else {
        "audio/mp4"
    };
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S");
    Some(Picked {
        path,
        name: format!("Voice message {stamp}.{ext}"),
        mime: mime.into(),
        duration_ms: Some(rec.duration_ms),
    })
}

pub async fn cancel_voice() {
    let _ = blocking(mobile_sentinel::recorder::cancel).await;
}

// ---- files out ----

/// Opens a web address in the browser.
pub fn open_url(url: &str) -> bool {
    mobile_sentinel::share::open_url(url).is_ok()
}

pub fn open(path: &str, mime: &str) -> bool {
    mobile_sentinel::share::open(path, mime).is_ok()
}

/// Copies a received file to the Downloads folder.
pub async fn save_to_downloads(path: String, name: String, mime: String) -> Option<String> {
    blocking(move || mobile_sentinel::share::save_to_downloads(&path, &name, &mime).ok())
        .await
        .flatten()
}

/// Removes leftovers in the scratch folders (a camera capture or import that
/// was never sent because the app was closed or killed). Run once at start,
/// when nothing can be in flight.
pub fn clean_scratch(core: &Core) {
    let media = core.media_dir();
    let data = media.parent().map(Path::to_path_buf).unwrap_or_default();
    for dir in [
        data.join("sentinel_camera"),
        data.join("sentinel_imports"),
        media.join("captures"),
        media.join("recordings"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Total size of files, for deciding whether to say "preparing…".
pub fn total_size(files: &[Picked]) -> u64 {
    files
        .iter()
        .filter_map(|f| std::fs::metadata(&f.path).ok())
        .map(|m| m.len())
        .sum()
}
