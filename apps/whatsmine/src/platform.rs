//! Small platform specifics.

/// CSS variables for the space the system bars and the keyboard take. Android draws the app
/// edge-to-edge (under the status and navigation bars) and the WebView reports
/// `env(safe-area-inset-*)` as 0, so ask the window for its real insets (the same way AlarmFree
/// does). On Android 15+ a keyboard changes nothing the page can see either. Elsewhere CSS
/// `env()` works.
pub fn safe_area_style() -> String {
    #[cfg(target_os = "android")]
    if let Some(i) = mobile_sentinel::display::safe_area_insets() {
        return format!(
            "--safe-top: {}px; --safe-bottom: {}px; --safe-left: {}px; --safe-right: {}px;",
            i.top.round() as u32,
            i.bottom.max(i.keyboard).round() as u32,
            i.left.round() as u32,
            i.right.round() as u32
        );
    }
    String::new()
}

/// The key that seals everything on disk is held by the system: the Android Keystore, or
/// Windows' DPAPI. A copied folder is then no stolen identity and no readable chats.
pub fn install_key_protector() {
    #[cfg(target_os = "android")]
    whatsmine_core::vault::set_protector(whatsmine_core::vault::KeyProtector {
        wrap: mobile_sentinel::secure_storage::wrap,
        unwrap: mobile_sentinel::secure_storage::unwrap,
    });
    #[cfg(windows)]
    whatsmine_core::vault::set_protector(whatsmine_core::vault::KeyProtector {
        wrap: dpapi::protect,
        unwrap: dpapi::unprotect,
    });
}

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr().cast_mut(),
        }
    }

    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: Windows gave us this buffer of this length, and it is ours to free.
        unsafe {
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as HLOCAL);
            bytes
        }
    }

    pub fn protect(secret: &[u8]) -> Option<Vec<u8>> {
        let input = blob(secret);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: the input outlives the call, and the output is written by Windows.
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        (ok != 0).then(|| take(out))
    }

    pub fn unprotect(wrapped: &[u8]) -> Option<Vec<u8>> {
        let input = blob(wrapped);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: the input outlives the call, and the output is written by Windows.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        (ok != 0).then(|| take(out))
    }
}

/// Whether the system is set to dark.
pub fn system_dark() -> bool {
    #[cfg(target_os = "android")]
    {
        mobile_sentinel::display::system_dark()
    }
    #[cfg(windows)]
    {
        windows_registry::CURRENT_USER
            .open(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
            .and_then(|key| key.get_u32("AppsUseLightTheme"))
            .is_ok_and(|light| light == 0)
    }
    #[cfg(not(any(target_os = "android", windows)))]
    {
        false
    }
}

/// Whether a saved theme comes out dark right now.
pub fn is_dark(theme: &str, system_dark: bool) -> bool {
    match theme {
        "light" => false,
        "dark" => true,
        _ => system_dark,
    }
}

/// The page's background as `0xAARRGGBB`: `--bg` in style.css.
pub fn background(dark: bool) -> u32 {
    if dark { 0xFF10_1218 } else { 0xFFF5_F6F9 }
}

/// The colour the page comes up on, before it has painted anything: the saved theme's. Dioxus's
/// own is white, and a dark app flashed. Starts the core, which the first screen needs anyway.
pub fn start_background() -> (u8, u8, u8, u8) {
    #[cfg(target_os = "android")]
    crate::android::init_once();
    let theme = crate::boot::core()
        .map(|core| core.settings().theme)
        .unwrap_or_default();
    let [_, r, g, b] = background(is_dark(&theme, system_dark())).to_be_bytes();
    (r, g, b, 255)
}

/// The system bars go with the page: its colour where the app does not draw under them (up to
/// Android 14), and icons that show on it.
pub fn set_system_bars(dark: bool) {
    #[cfg(target_os = "android")]
    {
        mobile_sentinel::display::set_light_system_bars(!dark);
        mobile_sentinel::display::set_system_bar_color(background(dark));
    }
    #[cfg(not(target_os = "android"))]
    let _ = dark;
}

/// A test run on this PC (see `debug_hooks`): out of the user's way. No window that takes the
/// focus, no toast, no tray icon, nothing in the registry.
pub fn test_run() -> bool {
    #[cfg(feature = "test-hooks")]
    {
        std::env::var_os(crate::debug_hooks::TEST_WINDOW).is_some()
    }
    #[cfg(not(feature = "test-hooks"))]
    {
        false
    }
}

/// A wait of `normal`, unless a test build was told a shorter one (see `debug_hooks`).
pub fn shortened(name: &str, normal: u32) -> u32 {
    #[cfg(feature = "test-hooks")]
    if let Some(value) = crate::debug_hooks::test_value(name) {
        return value;
    }
    let _ = name;
    normal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_gives_a_protected_secret_back() {
        let wrapped = dpapi::protect(b"a secret").unwrap();
        assert!(!wrapped.windows(8).any(|w| w == b"a secret"));
        assert_eq!(dpapi::unprotect(&wrapped).unwrap(), b"a secret");
        assert_eq!(dpapi::unprotect(b"not protected by anyone"), None);
    }

    #[test]
    fn saved_theme_decides_unless_it_follows_the_system() {
        assert!(!is_dark("light", true));
        assert!(is_dark("dark", false));
        assert!(is_dark("system", true));
        assert!(!is_dark("system", false));
        assert!(is_dark("", true));
    }

    /// The window's first colour has to be the page's, or it flashes.
    #[test]
    fn background_is_the_page_background() {
        let css = include_str!("../assets/style.css");
        let bg = |dark: bool| format!("--bg: #{:06x};", background(dark) & 0xFF_FFFF);
        assert!(css.contains(&bg(true)), "{}", bg(true));
        assert!(css.contains(&bg(false)), "{}", bg(false));
        let sentinel = include_str!("../sentinel.toml").to_lowercase();
        assert!(sentinel.contains(&format!("{:06x}", background(true) & 0xFF_FFFF)));
        assert!(sentinel.contains(&format!("{:06x}", background(false) & 0xFF_FFFF)));
    }
}
