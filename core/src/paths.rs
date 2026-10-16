//! Where WhatsMine keeps its data on this device.

use std::path::PathBuf;

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
