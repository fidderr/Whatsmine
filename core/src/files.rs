//! Files: where they live, moving their bytes, and profile pictures.
//!
//! A file message travels in two parts. The `FileOffer` frame (name, size,
//! SHA-256) rides the normal control stream and is re-sent until delivered,
//! like a text message. The bytes go on their own QUIC uni stream, opened by
//! the sender when the receiver asks with `FileWant { offset }`:
//!
//! ```text
//! "WMF1" | message id (16) | offset (u64 BE) | bytes offset..size
//! ```
//!
//! The receiver appends to `media/partial/<id>`. A broken transfer resumes
//! from the partial file's length on the next `FileWant`. Only a complete
//! file whose hash matches is moved into place and acknowledged.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::endpoint::{Connection, RecvStream};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::db::Database;
use crate::model::TransferState;
use crate::protocol::{FILE_STREAM_MAGIC, MAX_AVATAR, id_from_bytes};

const CHUNK: usize = 64 * 1024;

/// Every uni stream starts with this many header bytes: 4 magic, a 16-byte id
/// and 8 more (file offset, or call-video timestamp and flags).
pub(crate) const STREAM_HEADER: usize = 4 + 16 + 8;
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// `<data>/media`: received and sent files, partial downloads, avatars.
pub fn media_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("media")
}

pub fn partial_path(media: &Path, message_id: &str) -> PathBuf {
    media.join("partial").join(message_id)
}

pub fn avatar_dir(media: &Path) -> PathBuf {
    media.join("avatars")
}

/// Final location of an attachment: `<media>/<id>.<ext>`. The extension
/// comes from the (untrusted) original name, so it is sanitised.
pub fn final_path(media: &Path, message_id: &str, name: &str) -> PathBuf {
    let ext: String = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    if ext.is_empty() {
        media.join(message_id)
    } else {
        media.join(format!("{message_id}.{ext}"))
    }
}

/// A received name is display-only: no paths, no control characters.
pub fn clean_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect::<String>()
        .trim()
        .to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "file".into()
    } else {
        cleaned
    }
}

/// A MIME type we pass on to viewers: `type/subtype`, plain characters.
pub fn clean_mime(mime: &str) -> String {
    let ok = mime.len() <= 100
        && mime.split('/').count() == 2
        && mime
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/.+-_".contains(c));
    if ok {
        mime.to_ascii_lowercase()
    } else {
        "application/octet-stream".into()
    }
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Copies `src` to `dest` and hashes it on the way. Returns (size, sha256).
pub fn copy_and_hash(src: &Path, dest: &Path) -> std::io::Result<(u64, [u8; 32])> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut input = std::fs::File::open(src)?;
    let mut output = std::fs::File::create(dest)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut size = 0u64;
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        output.write_all(&buf[..n])?;
        size += n as u64;
    }
    output.flush()?;
    Ok((size, hasher.finalize().into()))
}

/// Brings `src` into `dest` and returns its size and SHA-256. With
/// `take_ownership` the file is renamed (instant, no second copy) when that
/// works, else copied.
pub fn import(src: &Path, dest: &Path, take_ownership: bool) -> std::io::Result<(u64, [u8; 32])> {
    if take_ownership {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::rename(src, dest).is_ok() {
            let size = std::fs::metadata(dest)?.len();
            return Ok((size, hash_file(dest)?));
        }
    }
    copy_and_hash(src, dest)
}

pub fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut input = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// Turns any picture into a square 256 px JPEG for a profile picture.
pub fn make_avatar(src: &Path) -> anyhow::Result<Vec<u8>> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(src)?
        .with_guessed_format()?
        .into_decoder()?;
    // Phone cameras store the picture sideways plus a "rotate me" note.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    let square = img.resize_to_fill(256, 256, image::imageops::FilterType::Lanczos3);
    let mut out = Vec::new();
    let mut quality = 85;
    loop {
        out.clear();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
        enc.encode_image(&square.to_rgb8())?;
        if out.len() <= MAX_AVATAR || quality <= 40 {
            break;
        }
        quality -= 15;
    }
    anyhow::ensure!(out.len() <= MAX_AVATAR, "avatar too large");
    Ok(out)
}

/// Pictures larger than this are sent as they are.
const MAX_CLEAN: u64 = 64 * 1024 * 1024;

/// Prepares a JPEG photo for sending: turns it upright if the camera stored
/// it sideways (the viewer on the other side then needs no EXIF handling) and
/// removes the metadata (location, camera, time) without touching the
/// pixels. Writes the result to `tmp` and returns true, or returns false when
/// the file is not a JPEG or there is nothing to change.
pub fn clean_photo(src: &Path, tmp: &Path) -> anyhow::Result<bool> {
    if std::fs::metadata(src)?.len() > MAX_CLEAN {
        return Ok(false);
    }
    let bytes = std::fs::read(src)?;
    let Some(scan) = scan_jpeg(&bytes) else {
        return Ok(false);
    };
    let turn = scan
        .orientation
        .and_then(image::metadata::Orientation::from_exif)
        .filter(|o| *o != image::metadata::Orientation::NoTransforms);
    let out = if let Some(o) = turn {
        let mut img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)?;
        img.apply_orientation(o);
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 92)
            .encode_image(&img.to_rgb8())?;
        out
    } else if scan.stripped.len() != bytes.len() {
        scan.stripped
    } else {
        return Ok(false);
    };
    if let Some(parent) = tmp.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(tmp, out)?;
    Ok(true)
}

struct JpegScan {
    /// The EXIF orientation (1-8), if the photo says one.
    orientation: Option<u8>,
    /// The file without its metadata segments.
    stripped: Vec<u8>,
}

/// Walks the JPEG segments up to the image data: reads the EXIF orientation
/// and copies everything except the metadata (EXIF/XMP and Photoshop blocks).
fn scan_jpeg(b: &[u8]) -> Option<JpegScan> {
    if !b.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut out = vec![0xff, 0xd8];
    let mut orientation = None;
    let mut i = 2;
    while i + 4 <= b.len() && b[i] == 0xff {
        let marker = b[i + 1];
        // Image data (or the end) starts here: the rest is copied as it is.
        if marker == 0xda || marker == 0xd9 {
            break;
        }
        // Padding and standalone markers have no length.
        if marker == 0xff {
            out.push(0xff);
            i += 1;
            continue;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            out.extend_from_slice(&b[i..i + 2]);
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        let end = i + 2 + len;
        if len < 2 || end > b.len() {
            return None;
        }
        let seg = &b[i..end];
        match marker {
            0xe1 => {
                if seg.get(4..10) == Some(b"Exif\0\0".as_slice()) {
                    orientation = orientation.or(exif_orientation(&seg[10..]));
                }
            }
            0xed => {}
            _ => out.extend_from_slice(seg),
        }
        i = end;
    }
    out.extend_from_slice(&b[i.min(b.len())..]);
    Some(JpegScan {
        orientation,
        stripped: out,
    })
}

/// The orientation tag (0x0112) from an EXIF block's TIFF header.
fn exif_orientation(t: &[u8]) -> Option<u8> {
    let big = match t.get(..2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let v: [u8; 2] = t.get(o..o + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(v)
        } else {
            u16::from_le_bytes(v)
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let v: [u8; 4] = t.get(o..o + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(v)
        } else {
            u32::from_le_bytes(v)
        })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    (0..count).find_map(|n| {
        let entry = ifd + 2 + n * 12;
        if u16_at(entry)? == 0x0112 {
            u16_at(entry + 8).map(|v| v as u8)
        } else {
            None
        }
    })
}

/// Quick check that received avatar bytes are a JPEG or PNG we can show.
pub fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff]) || bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

// ---- moving bytes ----

/// What a receive stream ended with.
#[derive(Debug)]
pub(crate) enum Received {
    /// Complete and verified, moved to this path.
    Done(PathBuf),
    /// Complete but the hash did not match; the partial file is gone.
    Corrupt,
    /// The stream ended early; the partial file keeps what arrived.
    Interrupted,
    /// Not a transfer we expect (unknown id, wrong peer, already running).
    Rejected,
}

pub(crate) struct ReceiveCtx {
    pub peer_id: String,
    pub db: Arc<Database>,
    pub media: PathBuf,
    /// Message ids currently being received, so one file never has two
    /// writers.
    pub active: Arc<Mutex<HashSet<String>>>,
}

/// Receives one file stream. `progress(done, total)` is throttled.
pub(crate) async fn receive(
    mut recv: RecvStream,
    header: [u8; STREAM_HEADER],
    ctx: &ReceiveCtx,
    progress: impl Fn(&str, u64, u64),
) -> (Option<String>, Received) {
    if &header[..4] != FILE_STREAM_MAGIC {
        let _ = recv.stop(1u32.into());
        return (None, Received::Rejected);
    }
    let id = id_from_bytes(header[4..20].try_into().expect("16 bytes"));
    let offset = u64::from_be_bytes(header[20..28].try_into().expect("8 bytes"));

    // Must be an incoming file from this peer that we are still waiting for.
    let expected = ctx.db.message(&id).and_then(|m| {
        let a = m.attachment?;
        let from_peer =
            m.contact_id == ctx.peer_id || m.sender.as_deref() == Some(ctx.peer_id.as_str());
        (!m.outgoing && from_peer && a.state == TransferState::Transferring).then_some(a)
    });
    let Some(att) = expected else {
        let _ = recv.stop(1u32.into());
        return (Some(id), Received::Rejected);
    };
    if !ctx
        .active
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(id.clone())
    {
        let _ = recv.stop(1u32.into());
        return (Some(id), Received::Rejected);
    }
    let result = receive_into(&mut recv, ctx, &id, offset, &att, progress).await;
    ctx.active
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&id);
    (Some(id), result)
}

async fn receive_into(
    recv: &mut RecvStream,
    ctx: &ReceiveCtx,
    id: &str,
    offset: u64,
    att: &crate::model::Attachment,
    progress: impl Fn(&str, u64, u64),
) -> Received {
    let partial = partial_path(&ctx.media, id);
    if let Some(dir) = partial.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&partial)
        .await
    else {
        return Received::Interrupted;
    };
    let have = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    if offset > have || offset > att.size {
        let _ = recv.stop(1u32.into());
        return Received::Rejected;
    }
    if offset < have && file.set_len(offset).await.is_err() {
        return Received::Interrupted;
    }
    if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
        return Received::Interrupted;
    }

    let mut done = offset;
    let mut buf = vec![0u8; CHUNK];
    let mut last = Instant::now();
    loop {
        match recv.read(&mut buf).await {
            Ok(Some(n)) => {
                if done + n as u64 > att.size {
                    // More than announced: not the file we agreed on.
                    let _ = recv.stop(1u32.into());
                    drop(file);
                    let _ = tokio::fs::remove_file(&partial).await;
                    return Received::Corrupt;
                }
                if file.write_all(&buf[..n]).await.is_err() {
                    return Received::Interrupted;
                }
                done += n as u64;
                if last.elapsed() >= PROGRESS_EVERY {
                    last = Instant::now();
                    progress(id, done, att.size);
                }
            }
            Ok(None) => break,
            Err(_) => {
                let _ = file.flush().await;
                return Received::Interrupted;
            }
        }
    }
    if file.flush().await.is_err() || done != att.size {
        return Received::Interrupted;
    }
    drop(file);
    progress(id, done, att.size);

    let path = partial.clone();
    let hash = tokio::task::spawn_blocking(move || hash_file(&path)).await;
    let ok = matches!(hash, Ok(Ok(h)) if to_hex(&h) == att.sha256);
    if !ok {
        let _ = tokio::fs::remove_file(&partial).await;
        return Received::Corrupt;
    }
    let dest = final_path(&ctx.media, id, &att.name);
    if tokio::fs::rename(&partial, &dest).await.is_err() {
        return Received::Interrupted;
    }
    Received::Done(dest)
}

/// Sends `path` from `offset` on a new uni stream of `conn`.
pub(crate) async fn upload(
    conn: Connection,
    path: PathBuf,
    id: [u8; 16],
    offset: u64,
    size: u64,
    progress: impl Fn(u64, u64),
) -> anyhow::Result<()> {
    let mut send = conn.open_uni().await?;
    let mut header = Vec::with_capacity(28);
    header.extend_from_slice(FILE_STREAM_MAGIC);
    header.extend_from_slice(&id);
    header.extend_from_slice(&offset.to_be_bytes());
    send.write_all(&header).await?;

    let mut file = tokio::fs::File::open(&path).await?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut buf = vec![0u8; CHUNK];
    let mut done = offset;
    let mut last = Instant::now();
    while done < size {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        send.write_all(&buf[..n]).await?;
        done += n as u64;
        if last.elapsed() >= PROGRESS_EVERY {
            last = Instant::now();
            progress(done, size);
        }
    }
    send.finish()?;
    // Wait until the receiver has everything (or gives up) before we let
    // the stream go, so progress reflects what really arrived.
    let _ = send.stopped().await;
    progress(done, size);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths_are_sanitised() {
        assert_eq!(clean_file_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_file_name("C:\\x\\holiday.JPG"), "holiday.JPG");
        assert_eq!(clean_file_name(".."), "file");
        assert_eq!(clean_file_name("a\u{0}b.txt"), "ab.txt");
        let media = Path::new("/m");
        assert_eq!(final_path(media, "id", "x.JPG"), Path::new("/m/id.jpg"));
        assert_eq!(
            final_path(media, "id", "x.<script>"),
            Path::new("/m/id.script")
        );
        assert_eq!(final_path(media, "id", "noext"), Path::new("/m/id"));
        assert_eq!(clean_mime("image/png"), "image/png");
        assert_eq!(
            clean_mime("text/html; charset=x"),
            "application/octet-stream"
        );
    }

    #[test]
    fn hex_round_trip() {
        let h = [7u8; 32];
        assert_eq!(from_hex32(&to_hex(&h)), Some(h));
        assert_eq!(from_hex32("zz"), None);
    }

    #[test]
    fn copy_hashes_like_hash_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.bin");
        std::fs::write(&src, vec![42u8; 200_000]).unwrap();
        let dest = dir.path().join("sub/b.bin");
        let (size, h) = copy_and_hash(&src, &dest).unwrap();
        assert_eq!(size, 200_000);
        assert_eq!(h, hash_file(&dest).unwrap());
        assert_eq!(h, hash_file(&src).unwrap());
    }

    #[test]
    fn import_moves_scratch_files_and_copies_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("scratch.bin");
        std::fs::write(&src, vec![7u8; 100_000]).unwrap();
        let want = hash_file(&src).unwrap();

        let kept = dir.path().join("media/kept.bin");
        let (size, h) = import(&src, &kept, false).unwrap();
        assert_eq!((size, h), (100_000, want));
        assert!(src.exists(), "a normal import leaves the original alone");

        let moved = dir.path().join("media/moved.bin");
        let (size, h) = import(&src, &moved, true).unwrap();
        assert_eq!((size, h), (100_000, want));
        assert!(!src.exists(), "a scratch file is moved, not copied");
        assert_eq!(hash_file(&moved).unwrap(), want);

        assert!(import(&dir.path().join("missing"), &dir.path().join("x"), true).is_err());
    }

    /// A JPEG of `w`x`h` (red left half, dark right half) carrying an EXIF
    /// block with this orientation.
    fn jpeg_with_orientation(w: u32, h: u32, orientation: u8) -> Vec<u8> {
        let mut plain = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut plain, 90)
            .encode_image(&image::RgbImage::from_fn(w, h, |x, _| {
                image::Rgb([if x < w / 2 { 250 } else { 10 }, 40, 40])
            }))
            .unwrap();
        // EXIF: "Exif\0\0", a little-endian TIFF header, one IFD entry.
        let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
        tiff.extend_from_slice(&[
            0x12,
            0x01,
            3,
            0,
            1,
            0,
            0,
            0,
            orientation,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ]);
        let mut seg = b"Exif\0\0".to_vec();
        seg.extend_from_slice(&tiff);
        let len = (seg.len() + 2) as u16;
        let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&seg);
        out.extend_from_slice(&plain[2..]);
        out
    }

    #[test]
    fn sideways_photos_are_turned_upright_and_lose_their_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("p.jpg");
        let tmp = dir.path().join("p.clean");
        // Landscape pixels that say "rotate 90 degrees clockwise to view".
        std::fs::write(&src, jpeg_with_orientation(64, 32, 6)).unwrap();
        assert!(clean_photo(&src, &tmp).unwrap());
        let out = std::fs::read(&tmp).unwrap();
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!((img.width(), img.height()), (32, 64), "now portrait");
        assert!(scan_jpeg(&out).unwrap().orientation.is_none());
        // The red left half ended up at the top.
        let px = img.to_rgb8();
        assert!(px.get_pixel(16, 4)[0] > 200 && px.get_pixel(16, 60)[0] < 60);
    }

    #[test]
    fn upright_photos_only_lose_their_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("p.jpg");
        let tmp = dir.path().join("p.clean");
        let original = jpeg_with_orientation(64, 32, 1);
        std::fs::write(&src, &original).unwrap();
        assert!(clean_photo(&src, &tmp).unwrap());
        let out = std::fs::read(&tmp).unwrap();
        assert!(out.len() < original.len());
        assert_eq!(image::load_from_memory(&out).unwrap().width(), 64);
        // Nothing left to remove: the file is used as it is.
        assert!(!clean_photo(&tmp, &dir.path().join("again")).unwrap());
        // Not a JPEG at all.
        std::fs::write(&src, b"hello").unwrap();
        assert!(!clean_photo(&src, &tmp).unwrap());
    }

    #[test]
    fn avatars_are_small_square_jpegs() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("big.png");
        image::RgbImage::from_fn(900, 500, |x, y| image::Rgb([x as u8, y as u8, 99]))
            .save(&src)
            .unwrap();
        let bytes = make_avatar(&src).unwrap();
        assert!(looks_like_image(&bytes));
        assert!(bytes.len() <= MAX_AVATAR);
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (256, 256));
    }
}
