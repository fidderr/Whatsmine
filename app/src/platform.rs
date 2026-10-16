//! Small platform specifics.

/// CSS variables for the space the system bars take. Android draws the app
/// edge-to-edge and the WebView reports `env(safe-area-inset-*)` as 0, so
/// read the bar heights from the system instead. Elsewhere CSS `env()` works.
pub fn safe_area_style() -> String {
    #[cfg(target_os = "android")]
    if let Some((top, bottom)) = android::bar_heights() {
        return format!("--safe-top: {top:.0}px; --safe-bottom: {bottom:.0}px;");
    }
    String::new()
}

#[cfg(target_os = "android")]
mod android {
    use jni::JavaVM;
    use jni::objects::{JObject, JValue};

    /// (status bar, navigation bar) height in CSS px (= dp).
    pub fn bar_heights() -> Option<(f32, f32)> {
        let ctx = ndk_context::android_context();
        // SAFETY: the pointers come from the Android activity hosting us.
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        let context = unsafe { JObject::from_raw(ctx.context().cast()) };
        let result = (|| -> jni::errors::Result<(f32, f32)> {
            let res = env
                .call_method(
                    &context,
                    "getResources",
                    "()Landroid/content/res/Resources;",
                    &[],
                )?
                .l()?;
            let metrics = env
                .call_method(
                    &res,
                    "getDisplayMetrics",
                    "()Landroid/util/DisplayMetrics;",
                    &[],
                )?
                .l()?;
            let density = env.get_field(&metrics, "density", "F")?.f()?.max(0.5);
            let mut dimen = |name: &str| -> jni::errors::Result<f32> {
                let name = env.new_string(name)?;
                let kind = env.new_string("dimen")?;
                let pkg = env.new_string("android")?;
                let id = env
                    .call_method(
                        &res,
                        "getIdentifier",
                        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
                        &[(&name).into(), (&kind).into(), (&pkg).into()],
                    )?
                    .i()?;
                if id == 0 {
                    return Ok(0.0);
                }
                let px = env
                    .call_method(&res, "getDimensionPixelSize", "(I)I", &[JValue::Int(id)])?
                    .i()?;
                Ok(px as f32 / density)
            };
            // Some ROMs report the 3-button bar height even with gesture
            // navigation; cap it so the composer doesn't float too high.
            let nav = dimen("navigation_bar_height")?.min(28.0);
            Ok((dimen("status_bar_height")?, nav))
        })();
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        result.ok()
    }
}
