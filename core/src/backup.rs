//! A backup: all chats, contacts and keys, without the files, in one file that only the 12 words
//! open. Coming back takes a restart: the database is in use while the app runs, so what is to be
//! restored waits here, sealed, for the next start.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::identity::Identity;
use crate::vault::{Vault, open_with, seal_with};

const PENDING: &str = "restore.pending";
const CONTEXT: &[u8] = b"whatsmine-backup-v1";
/// What such a file starts with, to say plainly when something else was picked.
const MAGIC: &[u8; 5] = b"WMBK1";

#[derive(Serialize, Deserialize)]
struct Contents {
    vault_key: [u8; 32],
    db: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    words: String,
    backup: Option<Vec<u8>>,
}

/// What came back at this start.
pub enum Restored {
    Nothing,
    /// Who you are: from the words alone, with nothing else.
    Words(String),
    /// Everything that was in the file.
    Backup,
}

pub fn seal(me: &Identity, vault_key: &[u8; 32], db: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let contents = postcard::to_stdvec(&Contents {
        vault_key: *vault_key,
        db,
    })?;
    let mut out = MAGIC.to_vec();
    out.extend(seal_with(&me.backup_key(), &contents, CONTEXT));
    Ok(out)
}

fn open(me: &Identity, file: &[u8]) -> Option<Contents> {
    let sealed = file.strip_prefix(MAGIC)?;
    let plain = open_with(&me.backup_key(), sealed, CONTEXT)?;
    postcard::from_bytes(&plain).ok()
}

/// Checks that the words fit and the file opens with them, and leaves it for the next start.
pub fn leave_for_next_start(
    dir: &Path,
    vault: &Vault,
    words: &str,
    backup: Option<Vec<u8>>,
) -> anyhow::Result<()> {
    let Ok(me) = Identity::from_words(words) else {
        anyhow::bail!("wrong_words");
    };
    if let Some(file) = &backup
        && open(&me, file).is_none()
    {
        anyhow::bail!("wrong_backup");
    }
    let plain = postcard::to_stdvec(&Pending {
        words: words.to_owned(),
        backup,
    })?;
    let unfinished = dir.join("restore.tmp");
    std::fs::write(&unfinished, vault.seal(&plain))?;
    std::fs::rename(unfinished, dir.join(PENDING))?;
    Ok(())
}

/// Before anything opens the database: puts back what was left here.
pub fn apply_if_left(dir: &Path, vault: &Vault) -> anyhow::Result<Restored> {
    let path = dir.join(PENDING);
    let Ok(sealed) = std::fs::read(&path) else {
        return Ok(Restored::Nothing);
    };
    let left = vault
        .open(&sealed)
        .and_then(|plain| postcard::from_bytes::<Pending>(&plain).ok());
    // Whatever was there is gone from disk either way: it holds the words.
    std::fs::remove_file(&path)?;
    let Some(left) = left else {
        return Ok(Restored::Nothing);
    };
    for name in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(dir.join(format!("{}{name}", crate::DB_FILE)));
    }
    let Some(file) = left.backup else {
        return Ok(Restored::Words(left.words));
    };
    let me = Identity::from_words(&left.words)?;
    let contents = open(&me, &file).ok_or_else(|| anyhow::anyhow!("the backup does not open"))?;
    std::fs::write(dir.join(crate::DB_FILE), contents.db)?;
    Vault::replace_key(dir, &contents.vault_key)?;
    Ok(Restored::Backup)
}
