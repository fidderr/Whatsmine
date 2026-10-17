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
    let img = image::ImageReader::open(src)?
        .with_guessed_format()?
        .decode()?;
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
    ctx: &ReceiveCtx,
    progress: impl Fn(&str, u64, u64),
) -> (Option<String>, Received) {
    let mut header = [0u8; 4 + 16 + 8];
    let read = tokio::time::timeout(Duration::from_secs(20), recv.read_exact(&mut header)).await;
    if !matches!(read, Ok(Ok(()))) || &header[..4] != FILE_STREAM_MAGIC {
        let _ = recv.stop(1u32.into());
        return (None, Received::Rejected);
    }
    let id = id_from_bytes(header[4..20].try_into().expect("16 bytes"));
    let offset = u64::from_be_bytes(header[20..28].try_into().expect("8 bytes"));

    // Must be an incoming file from this peer that we are still waiting for.
    let expected = ctx.db.message(&id).and_then(|m| {
        let a = m.attachment?;
        (!m.outgoing && m.contact_id == ctx.peer_id && a.state == TransferState::Transferring)
            .then_some(a)
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
