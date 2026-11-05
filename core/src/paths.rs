//! Where WhatsMine keeps its data on this device.

use std::path::{Path, PathBuf};

/// The generation of what is kept on disk. A new one means the old data cannot be used any more
/// (another way of being someone, another protocol): it goes, once.
pub const DATA_GENERATION: u32 = 5;
const GENERATION_FILE: &str = "generation";

/// Before anything opens what is in `dir`: removes the data of an earlier generation. Returns
/// whether there was any, for the app to say so once. Only names what the core itself wrote,
/// because on Android this folder also holds other people's files.
pub fn clean_start(dir: &Path) -> anyhow::Result<bool> {
    let marker = dir.join(GENERATION_FILE);
    let seen = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok());
    if seen == Some(DATA_GENERATION) {
        return Ok(false);
    }
    let key = dir.join(crate::identity::KEY_FILE);
    let db = dir.join(crate::DB_FILE);
    let had_data = key.exists() || db.exists();
    if had_data {
        let _ = std::fs::remove_file(dir.join("vault.key"));
        for name in ["-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(dir.join(format!("{}{name}", crate::DB_FILE)));
        }
        let _ = std::fs::remove_file(&db);
        let _ = std::fs::remove_file(dir.join("avatar.jpg"));
        let _ = std::fs::remove_dir_all(crate::files::media_dir(dir));
        // Last: without it the next start would make a new identity anyway, and a crash halfway
        // through finds the rest again.
        std::fs::remove_file(&key).or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(e)
            }
        })?;
    }
    std::fs::write(marker, DATA_GENERATION.to_string())?;
    Ok(had_data)
}

/// The data directory. `profile` lets several identities live side by side
/// on one machine (handy for testing two instances on one PC).
pub fn data_dir(profile: Option<&str>) -> anyhow::Result<PathBuf> {
    let base = base_dir()?;
    let dir = match profile.filter(|p| !p.is_empty()) {
        Some(p) => {
            anyhow::ensure!(
                p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "profile names may only use letters, digits, - and _"
            );
            base.join("profiles").join(p)
        }
        None => base,
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(not(target_os = "android"))]
fn base_dir() -> anyhow::Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "fidderr", "WhatsMine")
        .ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    Ok(dirs.data_dir().to_path_buf())
}

/// Android: the app's private files directory (`Context.getFilesDir()`).
#[cfg(target_os = "android")]
fn base_dir() -> anyhow::Result<PathBuf> {
    use jni::JavaVM;
    use jni::objects::{JObject, JString};

    let ctx = ndk_context::android_context();
    // SAFETY: the pointers come from the Android activity that hosts us.
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let context = unsafe { JObject::from_raw(ctx.context().cast()) };
    let dir = env
        .call_method(&context, "getFilesDir", "()Ljava/io/File;", &[])?
        .l()?;
    let path = env
        .call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?
        .l()?;
    let path: String = env.get_string(&JString::from(path))?.into();
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old_data(dir: &Path) {
        std::fs::write(dir.join(crate::identity::KEY_FILE), [1u8; 32]).unwrap();
        std::fs::write(dir.join(crate::DB_FILE), b"old").unwrap();
        std::fs::write(dir.join(format!("{}-wal", crate::DB_FILE)), b"old").unwrap();
        std::fs::write(dir.join("avatar.jpg"), b"old").unwrap();
        let media = crate::files::media_dir(dir);
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("photo.jpg"), b"old").unwrap();
    }

    #[test]
    fn old_data_goes_once_and_what_is_not_ours_stays() {
        let dir = tempfile::tempdir().unwrap();
        old_data(dir.path());
        std::fs::write(dir.path().join("someone-elses"), b"keep").unwrap();

        assert!(clean_start(dir.path()).unwrap());
        assert!(!dir.path().join(crate::identity::KEY_FILE).exists());
        assert!(!dir.path().join(crate::DB_FILE).exists());
        assert!(!dir.path().join(format!("{}-wal", crate::DB_FILE)).exists());
        assert!(!dir.path().join("avatar.jpg").exists());
        assert!(!crate::files::media_dir(dir.path()).exists());
        assert!(dir.path().join("someone-elses").exists());

        // The new generation's data is left alone from then on.
        old_data(dir.path());
        assert!(!clean_start(dir.path()).unwrap());
        assert!(dir.path().join(crate::identity::KEY_FILE).exists());
    }

    #[test]
    fn a_first_start_has_nothing_to_say() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!clean_start(dir.path()).unwrap());
        assert!(dir.path().join(GENERATION_FILE).exists());
    }

    #[test]
    fn an_older_generation_also_goes() {
        let dir = tempfile::tempdir().unwrap();
        old_data(dir.path());
        std::fs::write(dir.path().join(GENERATION_FILE), "1").unwrap();
        assert!(clean_start(dir.path()).unwrap());
    }
}
