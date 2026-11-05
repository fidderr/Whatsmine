//! Serves attachment bytes and profile pictures to the WebView.
//!
//! The page asks for `/media/<message id>` and `/avatar/<contact id>`; the
//! path on disk always comes from our own database, never from the URL.
//! Video and audio need HTTP `Range` requests (seeking, streaming), and a
//! 2 GB file must not be loaded into memory, so every ranged answer is capped
//! at [`CHUNK`] bytes: the player simply asks for the next piece.
//!
//! `/capture/<ext>` receives a photo or video the in-app camera recorded in
//! the page (Windows) and stores it, answering with its path.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use whatsmine_core::Core;

/// Largest piece sent in answer to one request.
pub const CHUNK: u64 = 4 * 1024 * 1024;
/// Files up to this size may be sent whole when no range is asked for.
const WHOLE_FILE_MAX: u64 = 64 * 1024 * 1024;
/// Upper bound for a camera upload.
const MAX_CAPTURE: usize = 512 * 1024 * 1024;

/// Android's WebView treats the stream it is given as the *whole* file and
/// applies the `Range` header to it itself. Answering with an already-sliced
/// range makes it skip the start offset a second time (short or empty bodies,
/// failed seeks). So on Android we return the entire file and let it slice, and
/// because that body lives in memory, only files up to this size are shown
/// inline there; bigger ones are a file card (Open plays them in the system
/// player). Other hosts (WebView2) honour ranges properly.
pub const ANDROID_INLINE_MAX: u64 = 64 * 1024 * 1024;

/// Does this host leave range handling to us?
pub const HOST_HONOURS_RANGES: bool = !cfg!(target_os = "android");

/// Can a file of `size` be shown inline (image, video, audio) on this host?
pub fn inline_ok(size: u64) -> bool {
    HOST_HONOURS_RANGES || size <= ANDROID_INLINE_MAX
}

/// An answer, independent of the WebView's HTTP types.
#[derive(Debug, PartialEq)]
pub struct Served {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Served {
    fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Range {
    /// No (usable) Range header.
    None,
    Bytes(u64, u64),
    Unsatisfiable,
}

/// Parses `bytes=a-b`, `bytes=a-` and `bytes=-n` against a file of `size`.
fn parse_range(header: Option<&str>, size: u64) -> Range {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return Range::None;
    };
    if spec.contains(',') {
        return Range::None; // multi-range: just send the file normally
    }
    let Some((a, b)) = spec.split_once('-') else {
        return Range::None;
    };
    let (a, b) = (a.trim(), b.trim());
    let (start, end) = if a.is_empty() {
        // Suffix: the last n bytes.
        let Ok(n) = b.parse::<u64>() else {
            return Range::None;
        };
        if n == 0 || size == 0 {
            return Range::Unsatisfiable;
        }
        (size.saturating_sub(n), size - 1)
    } else {
        let Ok(start) = a.parse::<u64>() else {
            return Range::None;
        };
        let end = if b.is_empty() {
            size.saturating_sub(1)
        } else {
            match b.parse::<u64>() {
                Ok(e) => e.min(size.saturating_sub(1)),
                Err(_) => return Range::None,
            }
        };
        (start, end)
    };
    if start >= size || start > end {
        return Range::Unsatisfiable;
    }
    Range::Bytes(start, end)
}

/// Only media types a viewer element can show keep their type; everything
/// else is sent as opaque bytes and never rendered by the page.
fn safe_mime(mime: &str) -> &str {
    let base = mime.split(';').next().unwrap_or("").trim();
    if base.starts_with("image/") || base.starts_with("video/") || base.starts_with("audio/") {
        base
    } else {
        "application/octet-stream"
    }
}

pub fn serve_file(path: &Path, mime: &str, range: Option<&str>) -> Served {
    serve_file_for(path, mime, range, HOST_HONOURS_RANGES)
}

/// [`serve_file`] with the host's range behaviour spelled out (for tests).
pub fn serve_file_for(
    path: &Path,
    mime: &str,
    range: Option<&str>,
    host_honours_ranges: bool,
) -> Served {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Served::empty(404);
    };
    let Ok(size) = file.metadata().map(|m| m.len()) else {
        return Served::empty(404);
    };
    let mut headers = vec![
        ("Content-Type", safe_mime(mime).to_string()),
        ("Accept-Ranges", "bytes".to_string()),
        ("X-Content-Type-Options", "nosniff".to_string()),
        ("Cache-Control", "private, max-age=3600".to_string()),
    ];

    // A host that slices ranges itself gets the whole file, never a range.
    if !host_honours_ranges {
        if size > ANDROID_INLINE_MAX {
            return Served::empty(413);
        }
        headers.retain(|(k, _)| *k != "Accept-Ranges");
    }
    let range = if host_honours_ranges { range } else { None };

    let (status, start, end) = match parse_range(range, size) {
        Range::Unsatisfiable => {
            let mut s = Served::empty(416);
            s.headers.push(("Content-Range", format!("bytes */{size}")));
            return s;
        }
        Range::Bytes(a, b) => (206, a, b.min(a + CHUNK - 1)),
        Range::None if size <= WHOLE_FILE_MAX => (200, 0, size.saturating_sub(1)),
        Range::None => (206, 0, CHUNK.min(size) - 1),
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    let mut body = vec![0u8; len as usize];
    if len > 0
        && (file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut body).is_err())
    {
        return Served::empty(500);
    }
    if status == 206 {
        headers.push(("Content-Range", format!("bytes {start}-{end}/{size}")));
    }
    headers.push(("Content-Length", len.to_string()));
    Served {
        status,
        headers,
        body,
    }
}

fn text(status: u16, body: &str) -> Served {
    Served {
        status,
        headers: vec![("Content-Type", "text/plain".to_string())],
        body: body.as_bytes().to_vec(),
    }
}

/// An id from the URL: only the characters ids are made of.
fn clean_id(raw: &str) -> Option<&str> {
    let ok = !raw.is_empty()
        && raw.len() <= 64
        && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then_some(raw)
}

fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or("")
}

pub fn serve_media(core: &Core, path: &str, range: Option<&str>) -> Served {
    let Some(id) = clean_id(last_segment(path)) else {
        return Served::empty(400);
    };
    let Some(att) = core.message(id).and_then(|m| m.attachment) else {
        return Served::empty(404);
    };
    match att.path {
        Some(p) => serve_file(Path::new(&p), &att.mime, range),
        None => Served::empty(404),
    }
}

pub fn serve_avatar(core: &Core, path: &str) -> Served {
    let Some(id) = clean_id(last_segment(path)) else {
        return Served::empty(400);
    };
    let file = if id == "me" {
        core.settings().avatar
    } else {
        core.contact(id).and_then(|c| c.avatar)
    };
    match file {
        Some(p) => {
            let mut s = serve_file(Path::new(&p), "image/jpeg", None);
            // The URL carries a version (?v=…), so it may be cached.
            s.headers.retain(|(k, _)| *k != "Cache-Control");
            s.headers
                .push(("Cache-Control", "private, max-age=86400".to_string()));
            s
        }
        None => Served::empty(404),
    }
}

/// A look's photo. It lies sealed in the database, and comes from there.
pub fn serve_look_photo(core: &Core, path: &str) -> Served {
    let id = path
        .trim_start_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    match core.look_photo(id) {
        Some(body) => Served {
            status: 200,
            headers: vec![
                ("Content-Type", "image/jpeg".to_string()),
                ("X-Content-Type-Options", "nosniff".to_string()),
                ("Cache-Control", "private, max-age=86400".to_string()),
            ],
            body,
        },
        None => Served::empty(404),
    }
}

/// The fonts that come with the app, by name. Nothing else can be asked for.
fn bundled_font(key: &str) -> Option<&'static [u8]> {
    const FONTS: &[(&str, &[u8])] = &[
        ("inter", include_bytes!("../assets/fonts/inter.woff2")),
        ("atkinson", include_bytes!("../assets/fonts/atkinson.woff2")),
        ("lexend", include_bytes!("../assets/fonts/lexend.woff2")),
        ("nunito", include_bytes!("../assets/fonts/nunito.woff2")),
        ("lora", include_bytes!("../assets/fonts/lora.woff2")),
        (
            "jetbrains",
            include_bytes!("../assets/fonts/jetbrains.woff2"),
        ),
        ("caveat", include_bytes!("../assets/fonts/caveat.woff2")),
        (
            "opendyslexic",
            include_bytes!("../assets/fonts/opendyslexic.woff2"),
        ),
        (
            "opendyslexic-bold",
            include_bytes!("../assets/fonts/opendyslexic-bold.woff2"),
        ),
    ];
    FONTS.iter().find(|(k, _)| *k == key).map(|(_, b)| *b)
}

/// A font that came with the app, or one someone added: the WebView checks it before it uses it.
pub fn serve_font(core: &Core, path: &str) -> Served {
    let key = path
        .trim_start_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let (body, mime) = if let Some(id) = key.strip_prefix("own-") {
        let Some(file) = core.own_font_path(id) else {
            return Served::empty(404);
        };
        let Ok(body) = std::fs::read(file) else {
            return Served::empty(404);
        };
        (body, "font/ttf")
    } else if let Some(body) = bundled_font(key) {
        (body.to_vec(), "font/woff2")
    } else {
        return Served::empty(404);
    };
    Served {
        status: 200,
        headers: vec![
            ("Content-Type", mime.to_string()),
            ("X-Content-Type-Options", "nosniff".to_string()),
            ("Cache-Control", "private, max-age=86400".to_string()),
        ],
        body,
    }
}

/// Stores an upload from the in-app camera under `<media>/captures/`.
pub fn store_capture(core: &Core, path: &str, method: &str, body: &[u8]) -> Served {
    if method != "POST" {
        return Served::empty(405);
    }
    let ext = last_segment(path);
    if !["jpg", "png", "webm", "mp4"].contains(&ext) {
        return Served::empty(400);
    }
    if body.is_empty() || body.len() > MAX_CAPTURE {
        return Served::empty(400);
    }
    let dir = core.media_dir().join("captures");
    let file = dir.join(format!("{}.{ext}", uuid::Uuid::new_v4()));
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(&file, body).is_err() {
        return Served::empty(500);
    }
    text(200, &file.to_string_lossy())
}

/// Registers the handlers for this window. A no-op in builds without a
/// WebView host (plain `cargo test`).
pub fn use_media_server(core: Core) {
    #[cfg(any(feature = "desktop", feature = "mobile"))]
    {
        #[cfg(feature = "desktop")]
        use dioxus::desktop as host;
        #[cfg(all(feature = "mobile", not(feature = "desktop")))]
        use dioxus::mobile as host;

        fn reply(responder: host::RequestAsyncResponder, s: Served) {
            let mut b = host::wry::http::Response::builder().status(s.status);
            for (k, v) in s.headers {
                b = b.header(k, v);
            }
            if let Ok(resp) = b.body(s.body) {
                responder.respond(resp);
            }
        }

        let c = core.clone();
        host::use_asset_handler("media", move |req, responder| {
            let range = req.headers().get("range").and_then(|v| v.to_str().ok());
            reply(responder, serve_media(&c, req.uri().path(), range));
        });
        let c = core.clone();
        host::use_asset_handler("look-photo", move |req, responder| {
            reply(responder, serve_look_photo(&c, req.uri().path()));
        });
        let c = core.clone();
        host::use_asset_handler("font", move |req, responder| {
            reply(responder, serve_font(&c, req.uri().path()));
        });
        let c = core.clone();
        host::use_asset_handler("avatar", move |req, responder| {
            reply(responder, serve_avatar(&c, req.uri().path()));
        });
        host::use_asset_handler("capture", move |req, responder| {
            let served = store_capture(&core, req.uri().path(), req.method().as_str(), req.body());
            reply(responder, served);
        });
    }
    #[cfg(not(any(feature = "desktop", feature = "mobile")))]
    let _ = core;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_font_is_there_and_nothing_else_can_be_asked_for() {
        for font in whatsmine_core::look::Font::ALL {
            let bytes = bundled_font(font.key()).unwrap_or_else(|| panic!("{}", font.key()));
            assert!(bytes.starts_with(b"wOF2"), "{}", font.key());
        }
        assert!(bundled_font("../../secret").is_none());
        assert!(bundled_font("").is_none());
    }

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range(None, 100), Range::None);
        assert_eq!(parse_range(Some("bytes=0-9"), 100), Range::Bytes(0, 9));
        assert_eq!(parse_range(Some("bytes=90-"), 100), Range::Bytes(90, 99));
        assert_eq!(parse_range(Some("bytes=-10"), 100), Range::Bytes(90, 99));
        assert_eq!(parse_range(Some("bytes=5-500"), 100), Range::Bytes(5, 99));
        assert_eq!(parse_range(Some("bytes=100-"), 100), Range::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=9-5"), 100), Range::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), 100), Range::None);
        assert_eq!(parse_range(Some("items=0-1"), 100), Range::None);
    }

    #[test]
    fn serving_files_whole_ranged_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        let data: Vec<u8> = (0..(CHUNK as usize * 2 + 10))
            .map(|i| (i % 253) as u8)
            .collect();
        std::fs::write(&p, &data).unwrap();

        let h = |s: &Served, k: &str| {
            s.headers
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.clone())
        };

        let whole = serve_file(&p, "video/mp4", None);
        assert_eq!((whole.status, whole.body.len()), (200, data.len()));
        assert_eq!(h(&whole, "Accept-Ranges").as_deref(), Some("bytes"));

        let part = serve_file(&p, "video/mp4", Some("bytes=100-199"));
        assert_eq!(part.status, 206);
        assert_eq!(part.body, data[100..200]);
        assert_eq!(
            h(&part, "Content-Range").unwrap(),
            format!("bytes 100-199/{}", data.len())
        );

        // "from here to the end" is answered in capped pieces.
        let open = serve_file(&p, "video/mp4", Some("bytes=0-"));
        assert_eq!((open.status, open.body.len() as u64), (206, CHUNK));

        let bad = serve_file(&p, "video/mp4", Some("bytes=999999999-"));
        assert_eq!(bad.status, 416);
        assert_eq!(
            serve_file(&dir.path().join("nope"), "x/y", None).status,
            404
        );
    }

    #[test]
    fn android_style_hosts_get_the_whole_file_and_a_size_cap() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("v.mp4");
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();

        // The host slices ranges itself, so a Range header must not be applied twice.
        for range in [
            None,
            Some("bytes=100-199"),
            Some("bytes=9000-"),
            Some("bytes=-50"),
        ] {
            let s = serve_file_for(&p, "video/mp4", range, false);
            assert_eq!(
                (s.status, s.body.as_slice()),
                (200, data.as_slice()),
                "{range:?}"
            );
            assert!(
                s.headers
                    .iter()
                    .all(|(k, _)| *k != "Content-Range" && *k != "Accept-Ranges")
            );
        }
        // Too big to hold in memory: refused (the UI shows a file card instead).
        let big = dir.path().join("big.bin");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(ANDROID_INLINE_MAX + 1).unwrap();
        assert_eq!(serve_file_for(&big, "video/mp4", None, false).status, 413);
        assert_eq!(
            serve_file_for(&big, "video/mp4", Some("bytes=0-9"), true).status,
            206
        );
        assert!(HOST_HONOURS_RANGES || !inline_ok(ANDROID_INLINE_MAX + 1));
    }

    #[test]
    fn only_media_types_keep_their_type() {
        assert_eq!(safe_mime("image/png"), "image/png");
        assert_eq!(safe_mime("audio/mp4; codecs=x"), "audio/mp4");
        assert_eq!(safe_mime("text/html"), "application/octet-stream");
        assert_eq!(safe_mime("application/pdf"), "application/octet-stream");
        assert_eq!(clean_id("../../etc"), None);
        assert_eq!(clean_id("0190-abcd"), Some("0190-abcd"));
    }
}
