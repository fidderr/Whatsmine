//! Native clipboard: `arboard` on desktop, `ClipboardManager` on Android.

#[cfg(not(target_os = "android"))]
mod imp {
    use std::sync::Mutex;

    // Kept alive: on Linux the clipboard contents belong to this object.
    static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

    fn with<R>(f: impl FnOnce(&mut arboard::Clipboard) -> Option<R>) -> Option<R> {
        let mut guard = CLIPBOARD.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            *guard = arboard::Clipboard::new().ok();
        }
        guard.as_mut().and_then(f)
    }

    pub fn set(text: &str) {
        with(|c| c.set_text(text).ok());
    }

    pub fn get() -> Option<String> {
        with(|c| c.get_text().ok())
    }
}

#[cfg(target_os = "android")]
mod imp {
    use jni::objects::{JObject, JString};
    use jni::{JNIEnv, JavaVM};

    fn with_manager<R>(
        f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<Option<R>>,
    ) -> Option<R> {
        let ctx = ndk_context::android_context();
        // SAFETY: the pointers come from the Android activity hosting us.
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        let context = unsafe { JObject::from_raw(ctx.context().cast()) };
        let result = (|| {
            let name = env.new_string("clipboard")?;
            let manager = env
                .call_method(
                    &context,
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[(&name).into()],
                )?
                .l()?;
            if manager.is_null() {
                return Ok(None);
            }
            f(&mut env, &manager)
        })();
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        result.ok().flatten()
    }

    pub fn set(text: &str) {
        with_manager(|env, manager| {
            let label = env.new_string("WhatsMine")?;
            let value = env.new_string(text)?;
            let clip = env
                .call_static_method(
                    "android/content/ClipData",
                    "newPlainText",
                    "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                    &[(&label).into(), (&value).into()],
                )?
                .l()?;
            env.call_method(
                manager,
                "setPrimaryClip",
                "(Landroid/content/ClipData;)V",
                &[(&clip).into()],
            )?;
            Ok(Some(()))
        });
    }

    pub fn get() -> Option<String> {
        with_manager(|env, manager| {
            let clip = env
                .call_method(
                    manager,
                    "getPrimaryClip",
                    "()Landroid/content/ClipData;",
                    &[],
                )?
                .l()?;
            if clip.is_null() {
                return Ok(None);
            }
            let item = env
                .call_method(
                    &clip,
                    "getItemAt",
                    "(I)Landroid/content/ClipData$Item;",
                    &[0i32.into()],
                )?
                .l()?;
            if item.is_null() {
                return Ok(None);
            }
            let chars = env
                .call_method(&item, "getText", "()Ljava/lang/CharSequence;", &[])?
                .l()?;
            if chars.is_null() {
                return Ok(None);
            }
            let s = env
                .call_method(&chars, "toString", "()Ljava/lang/String;", &[])?
                .l()?;
            let s: String = env.get_string(&JString::from(s))?.into();
            Ok(Some(s))
        })
    }
}

pub use imp::{get, set};
