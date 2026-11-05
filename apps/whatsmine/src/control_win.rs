//! Letting someone control this PC: the part that touches the real desktop. What may be used is
//! decided in `whatsmine_core::control`, and this only ever sees what passed all of that.
//!
//! Things Windows does for us here: `SendInput` cannot reach programs running as administrator, or
//! the lock screen and secure prompts, and we never ask for `uiAccess`. So those stay out of reach.
//!
//! A test run never touches the real mouse or keyboard: see [`TestDesk`].

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use whatsmine_core::Core;
#[cfg_attr(not(any(windows, feature = "test-hooks")), allow(unused_imports))]
use whatsmine_core::control::{Applier, Area, Desk, Input, Landing, MouseButton, Scope};

/// What is being shown, known for certain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The only screen of this PC.
    Screen,
    Window(isize),
}

impl Target {
    pub fn scope(self) -> Scope {
        match self {
            Self::Screen => Scope::WholeScreen,
            Self::Window(_) => Scope::Window,
        }
    }
}

/// What the capture calls itself: `screen:0:0`, or `window:<handle>:0`.
pub fn parse_label(label: &str, surface: &str) -> Option<LabelSays> {
    let mut parts = label.split(':');
    let kind = parts.next()?;
    let id: i64 = parts.next()?.parse().ok()?;
    match (kind, surface) {
        ("screen", "monitor") => Some(LabelSays::Screen),
        ("window", "window") if id > 0 => Some(LabelSays::Window(id as isize)),
        // A browser tab, or what a label and its kind do not agree on.
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSays {
    Screen,
    Window(isize),
}

/// Does something this wide and high fit what it should be a picture of? The capture may be scaled
/// down, so what counts is its shape.
pub fn same_shape(width: u32, height: u32, area: Area) -> bool {
    if width == 0 || height == 0 || area.width <= 0 || area.height <= 0 {
        return false;
    }
    let shown = f64::from(width) / f64::from(height);
    let real = f64::from(area.width) / f64::from(area.height);
    (shown / real - 1.0).abs() < 0.03
}

static SHOWN: Mutex<Option<Target>> = Mutex::new(None);
static WAITING_INPUT: Mutex<Option<mpsc::Receiver<Vec<Input>>>> = Mutex::new(None);
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
/// When the session ends, for the bar.
static UNTIL: AtomicI64 = AtomicI64::new(0);

struct Running {
    stop: std::sync::Arc<AtomicBool>,
    #[cfg(windows)]
    hand_thread: Option<u32>,
}

/// We started or stopped showing something. Says whether this can be controlled at all.
pub fn shown_now(on: bool, label: &str, surface: &str, width: u32, height: u32) -> bool {
    let target = if on {
        target_of(label, surface, width, height)
    } else {
        None
    };
    *SHOWN.lock().unwrap_or_else(|p| p.into_inner()) = target;
    target.is_some()
}

pub fn shown() -> Option<Target> {
    *SHOWN.lock().unwrap_or_else(|p| p.into_inner())
}

/// Input goes here from the moment the owner says yes.
pub fn prepare(input: mpsc::Receiver<Vec<Input>>) {
    *WAITING_INPUT.lock().unwrap_or_else(|p| p.into_inner()) = Some(input);
}

/// The words on the bar.
pub struct BarWords {
    pub who: String,
    pub more: String,
    pub stop: String,
}

/// Someone was let in, or the owner gave more time.
pub fn on(core: &Core, until: i64, words: BarWords) {
    UNTIL.store(until, Ordering::Relaxed);
    let mut running = RUNNING.lock().unwrap_or_else(|p| p.into_inner());
    if running.is_some() {
        return;
    }
    let input = WAITING_INPUT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    let (Some(input), Some(target)) = (input, shown()) else {
        // Nothing to apply it with: not in control of anything.
        core.control_not_safe();
        return;
    };
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let scope = target.scope();
    let (for_worker, for_core) = (stop.clone(), core.clone());
    #[cfg(feature = "test-hooks")]
    if crate::platform::test_run() {
        std::thread::spawn(move || {
            run(
                TestDesk::new(&for_core),
                scope,
                input,
                &for_worker,
                &for_core,
            )
        });
        *running = Some(Running {
            stop,
            #[cfg(windows)]
            hand_thread: None,
        });
        return;
    }
    #[cfg(windows)]
    {
        std::thread::spawn(move || {
            real::make_pixels_real();
            run(
                real::WinDesk::new(target),
                scope,
                input,
                &for_worker,
                &for_core,
            );
        });
        let hand_thread = real::start_hand_and_bar(core.clone(), words);
        *running = Some(Running { stop, hand_thread });
    }
    #[cfg(not(windows))]
    {
        let _ = (target, input, for_worker, for_core, scope, words);
        core.control_not_safe();
    }
}

/// Over, for whatever reason.
pub fn off() {
    WAITING_INPUT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    let Some(running) = RUNNING.lock().unwrap_or_else(|p| p.into_inner()).take() else {
        return;
    };
    running.stop.store(true, Ordering::Relaxed);
    #[cfg(windows)]
    if let Some(thread) = running.hand_thread {
        real::stop_hand_and_bar(thread);
    }
}

fn target_of(label: &str, surface: &str, width: u32, height: u32) -> Option<Target> {
    #[cfg(feature = "test-hooks")]
    if crate::platform::test_run() {
        // A painted test picture has no label worth reading.
        let _ = (label, surface, width, height);
        return Some(Target::Screen);
    }
    #[cfg(windows)]
    {
        real::target_of(parse_label(label, surface)?, width, height)
    }
    #[cfg(not(windows))]
    {
        let _ = (label, surface, width, height);
        None
    }
}

/// How long to wait for more when nothing is coming in. Short, as this is someone's mouse.
const POLL: Duration = Duration::from_millis(8);
/// How often to look whether it is still safe when nothing is coming in.
const LOOK_EVERY: Duration = Duration::from_millis(250);

/// One session, until it ends in any way. Whatever is held down goes up when the applier goes.
#[cfg_attr(not(any(windows, feature = "test-hooks")), allow(dead_code))]
fn run<D: Desk>(
    desk: D,
    scope: Scope,
    mut input: mpsc::Receiver<Vec<Input>>,
    stop: &AtomicBool,
    core: &Core,
) {
    let mut applier = Applier::new(desk, scope);
    let mut looked = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match input.try_recv() {
            Ok(events) => {
                if applier.apply(events).is_err() {
                    core.control_not_safe();
                    return;
                }
            }
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {
                std::thread::sleep(POLL);
                if looked.elapsed() >= LOOK_EVERY {
                    looked = Instant::now();
                    if applier.look().is_err() {
                        core.control_not_safe();
                        return;
                    }
                }
            }
        }
    }
}

/// For a test run: writes down what would have happened, and the real mouse stays where it is. The
/// left tenth of the screen plays this app's own window, the next tenth another program.
#[cfg(feature = "test-hooks")]
struct TestDesk {
    file: std::path::PathBuf,
    lines: Vec<String>,
}

#[cfg(feature = "test-hooks")]
impl TestDesk {
    fn new(core: &Core) -> Self {
        Self {
            file: core.data_dir().join("debug_control.txt"),
            lines: Vec::new(),
        }
    }

    fn write(&mut self, line: String) {
        use std::io::Write;
        self.lines.push(line.clone());
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

#[cfg(feature = "test-hooks")]
impl Desk for TestDesk {
    fn input_is_ours(&self) -> bool {
        // The test locks the screen with a file.
        !self.file.with_file_name("debug_locked.txt").exists()
    }
    fn shown_area(&self) -> Option<Area> {
        Some(Area {
            x: 0,
            y: 0,
            width: 1000,
            height: 1000,
        })
    }
    fn window_at(&self, x: i32, _y: i32) -> Landing {
        match x {
            0..=99 => Landing::Own,
            _ => Landing::Other,
        }
    }
    fn foreground(&self) -> Landing {
        Landing::Other
    }
    fn mouse_to(&mut self, x: i32, y: i32) {
        self.write(format!("to {x},{y}"));
    }
    fn button(&mut self, which: MouseButton, down: bool) {
        self.write(format!("{which:?} {}", if down { "down" } else { "up" }));
    }
    fn wheel(&mut self, dx: i16, dy: i16) {
        self.write(format!("wheel {dx},{dy}"));
    }
    fn key(&mut self, code: u16, extended: bool, down: bool) {
        self.write(format!(
            "key {code}{} {}",
            if extended { "e" } else { "" },
            if down { "down" } else { "up" }
        ));
    }
    fn text(&mut self, text: &str) {
        self.write(format!("text {text}"));
    }
}

#[cfg(windows)]
mod real {
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;

    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
    };
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows_sys::Win32::Graphics::Gdi::{
        BeginPaint, CreateFontW, CreateSolidBrush, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
        DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FW_SEMIBOLD,
        FillRect, InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::StationsAndDesktops::{
        CloseDesktop, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcessToken,
    };
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetThreadDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE,
        MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
        MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
        MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VK_ESCAPE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CS_HREDRAW, CS_VREDRAW, CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
        DispatchMessageW, GA_ROOT, GW_OWNER, GetAncestor, GetClientRect, GetForegroundWindow,
        GetMessageW, GetSystemMetrics, GetWindow, GetWindowRect, GetWindowThreadProcessId,
        HC_ACTION, HHOOK, IDC_ARROW, IsIconic, IsWindow, IsWindowVisible, KBDLLHOOKSTRUCT,
        LLKHF_INJECTED, LLMHF_INJECTED, LoadCursorW, MSG, MSLLHOOKSTRUCT, PostQuitMessage,
        PostThreadMessageW, RegisterClassW, SM_CMONITORS, SM_CXSCREEN, SM_CXVIRTUALSCREEN,
        SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOWNOACTIVATE,
        SetTimer, SetWindowDisplayAffinity, SetWindowsHookExW, ShowWindow, TranslateMessage,
        UnhookWindowsHookEx, WDA_EXCLUDEFROMCAPTURE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_DESTROY,
        WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_PAINT, WM_QUIT, WM_TIMER, WNDCLASSW,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WindowFromPoint,
    };

    use super::{Area, BarWords, Desk, LabelSays, Landing, MouseButton, Target, UNTIL, same_shape};
    use whatsmine_core::{Core, now_ms};

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// Running as administrator could reach windows that stay out of reach otherwise. If this
    /// cannot be told, it counts as yes.
    fn elevated() -> bool {
        // SAFETY: the token handle is ours and closed here, the buffer is the size that is passed.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) == 0 {
                return true;
            }
            let mut info = TOKEN_ELEVATION { TokenIsElevated: 1 };
            let mut got = 0;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                (&raw mut info).cast(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &raw mut got,
            );
            CloseHandle(token);
            ok == 0 || info.TokenIsElevated != 0
        }
    }

    pub fn make_pixels_real() {
        // SAFETY: changes how this thread counts pixels, nothing else.
        unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    fn screen_area() -> Option<Area> {
        // SAFETY: asks for numbers.
        unsafe {
            if GetSystemMetrics(SM_CMONITORS) != 1 {
                return None;
            }
            Some(Area {
                x: 0,
                y: 0,
                width: GetSystemMetrics(SM_CXSCREEN),
                height: GetSystemMetrics(SM_CYSCREEN),
            })
        }
    }

    fn window_area(window: isize) -> Option<Area> {
        let handle = window as HWND;
        // SAFETY: a handle that is no longer a window only makes these say no.
        unsafe {
            if IsWindow(handle) == 0 || IsIconic(handle) != 0 || IsWindowVisible(handle) == 0 {
                return None;
            }
            let mut r = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let got = DwmGetWindowAttribute(
                handle,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                (&raw mut r).cast(),
                size_of::<RECT>() as u32,
            );
            if got != 0 && GetWindowRect(handle, &raw mut r) == 0 {
                return None;
            }
            Some(Area {
                x: r.left,
                y: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
            })
        }
    }

    /// What is being shown, only if that is certain. On a PC with more than one screen the label
    /// does not say for certain which one: then one window can be shown and controlled instead.
    pub fn target_of(says: LabelSays, width: u32, height: u32) -> Option<Target> {
        make_pixels_real();
        if elevated() {
            return None;
        }
        match says {
            LabelSays::Screen => {
                same_shape(width, height, screen_area()?).then_some(Target::Screen)
            }
            LabelSays::Window(window) => {
                let mut pid = 0;
                // SAFETY: a handle that is not a window gives 0.
                let thread = unsafe { GetWindowThreadProcessId(window as HWND, &raw mut pid) };
                // SAFETY: asks for a number.
                let own = pid == unsafe { GetCurrentProcessId() };
                if thread == 0 || own {
                    return None;
                }
                same_shape(width, height, window_area(window)?).then_some(Target::Window(window))
            }
        }
    }

    /// This process and whatever it started: its windows are drawn by a browser engine that runs
    /// as other processes.
    fn own_processes() -> HashSet<u32> {
        // SAFETY: asks for a number.
        let me = unsafe { GetCurrentProcessId() };
        let mut own = HashSet::from([me]);
        let mut parents = Vec::new();
        // SAFETY: the snapshot is ours and closed here; the entry has the size it says it has.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot as isize == -1 {
                return own;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &raw mut entry) != 0;
            while more {
                parents.push((entry.th32ProcessID, entry.th32ParentProcessID));
                more = Process32NextW(snapshot, &raw mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        loop {
            let before = own.len();
            for (pid, parent) in &parents {
                if own.contains(parent) {
                    own.insert(*pid);
                }
            }
            if own.len() == before {
                return own;
            }
        }
    }

    pub struct WinDesk {
        target: Target,
        own: HashSet<u32>,
        /// Program and thread of the window being shown.
        shown: Option<(u32, u32)>,
    }

    impl WinDesk {
        pub fn new(target: Target) -> Self {
            let shown = match target {
                Target::Screen => None,
                Target::Window(window) => {
                    let mut pid = 0;
                    // SAFETY: a handle that is not a window gives 0.
                    let thread = unsafe { GetWindowThreadProcessId(window as HWND, &raw mut pid) };
                    (thread != 0).then_some((pid, thread))
                }
            };
            Self {
                target,
                own: own_processes(),
                shown,
            }
        }

        fn landing(&self, window: HWND) -> Landing {
            if window.is_null() {
                return Landing::Unknown;
            }
            let mut pid = 0;
            // SAFETY: a window that went away in the meantime gives 0.
            let (root, thread) = unsafe {
                let root = GetAncestor(window, GA_ROOT);
                (root, GetWindowThreadProcessId(root, &raw mut pid))
            };
            if thread == 0 {
                return Landing::Unknown;
            }
            if self.own.contains(&pid) {
                return Landing::Own;
            }
            let Target::Window(shown_window) = self.target else {
                return Landing::Other;
            };
            let shown = shown_window as HWND;
            if root == shown || owned_by(root, shown) {
                return Landing::Shown;
            }
            // A menu or a list that opens from it. The same program is not enough: every folder
            // window and the taskbar are one program, and so are many apps from the Store.
            if self.shown == Some((pid, thread)) {
                Landing::Shown
            } else {
                Landing::Other
            }
        }
    }

    fn owned_by(window: HWND, owner: HWND) -> bool {
        let mut at = window;
        for _ in 0..16 {
            // SAFETY: gives null for anything that is not a window with an owner.
            at = unsafe { GetWindow(at, GW_OWNER) };
            if at.is_null() {
                return false;
            }
            if at == owner {
                return true;
            }
        }
        false
    }

    fn send(input: INPUT) {
        // SAFETY: one input of the size that is passed.
        unsafe {
            SendInput(1, &raw const input, size_of::<INPUT>() as i32);
        }
    }

    fn mouse(flags: u32, x: i32, y: i32, data: i32) {
        send(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: x,
                    dy: y,
                    mouseData: data as u32,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    fn keyboard(scan: u16, flags: u32) {
        send(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    impl Desk for WinDesk {
        fn input_is_ours(&self) -> bool {
            // On the lock screen and with a secure prompt, this is not ours to open.
            // SAFETY: the handle is closed here; the buffer is as long as what is passed.
            unsafe {
                let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
                if desktop.is_null() {
                    return false;
                }
                let mut name = [0u16; 64];
                let mut got = 0;
                let ok = GetUserObjectInformationW(
                    desktop,
                    UOI_NAME,
                    name.as_mut_ptr().cast(),
                    size_of_val(&name) as u32,
                    &raw mut got,
                );
                CloseDesktop(desktop);
                let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
                ok != 0 && String::from_utf16_lossy(&name[..end]) == "Default"
            }
        }

        fn shown_area(&self) -> Option<Area> {
            match self.target {
                Target::Screen => screen_area(),
                Target::Window(window) => window_area(window),
            }
        }

        fn window_at(&self, x: i32, y: i32) -> Landing {
            // SAFETY: asks which window is there.
            self.landing(unsafe { WindowFromPoint(POINT { x, y }) })
        }

        fn foreground(&self) -> Landing {
            // SAFETY: asks which window that is.
            self.landing(unsafe { GetForegroundWindow() })
        }

        fn mouse_to(&mut self, x: i32, y: i32) {
            // SAFETY: asks for numbers.
            let (left, top, width, height) = unsafe {
                (
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    GetSystemMetrics(SM_CXVIRTUALSCREEN),
                    GetSystemMetrics(SM_CYVIRTUALSCREEN),
                )
            };
            let along = |at: i32, start: i32, size: i32| {
                (i64::from(at - start) * 65535 / i64::from((size - 1).max(1))) as i32
            };
            mouse(
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                along(x, left, width),
                along(y, top, height),
                0,
            );
        }

        fn button(&mut self, which: MouseButton, down: bool) {
            let flags = match (which, down) {
                (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
                (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
                (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
                (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
            };
            mouse(flags, 0, 0, 0);
        }

        fn wheel(&mut self, dx: i16, dy: i16) {
            if dy != 0 {
                mouse(MOUSEEVENTF_WHEEL, 0, 0, i32::from(dy));
            }
            if dx != 0 {
                mouse(MOUSEEVENTF_HWHEEL, 0, 0, i32::from(dx));
            }
        }

        fn key(&mut self, code: u16, extended: bool, down: bool) {
            let mut flags = KEYEVENTF_SCANCODE;
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            keyboard(code, flags);
        }

        fn text(&mut self, text: &str) {
            for unit in text.encode_utf16() {
                keyboard(unit, KEYEVENTF_UNICODE);
                keyboard(unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
            }
        }
    }

    // ---- the owner's own hand, and the bar ----

    /// What a hook and the bar need, which get no way to carry anything along.
    struct Hand {
        core: Core,
        words: BarWords,
    }

    static HAND: Mutex<Option<Hand>> = Mutex::new(None);
    /// When the mouse was really pressed. The bar's buttons only work with that.
    static REAL_PRESS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    static LAST_TOLD: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    /// A press on the bar is only real if the hook saw it this recently.
    const PRESS_COUNTS: i64 = 5_000;
    /// How often to say the owner is busy. Less than the time this makes the other one wait.
    const TELL_EVERY: i64 = 250;

    fn with_hand(what: impl FnOnce(&Hand)) {
        if let Some(hand) = HAND.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            what(hand);
        }
    }

    fn real_hand() {
        let now = now_ms();
        if now - LAST_TOLD.load(Ordering::Relaxed) >= TELL_EVERY {
            LAST_TOLD.store(now, Ordering::Relaxed);
            with_hand(|hand| hand.core.control_owner_busy());
        }
    }

    unsafe extern "system" fn key_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            // SAFETY: for this hook and this code, that is what lparam points to.
            let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            if info.flags & LLKHF_INJECTED == 0 {
                real_hand();
                if info.vkCode == u32::from(VK_ESCAPE) && wparam == WM_KEYDOWN as usize {
                    with_hand(|hand| hand.core.control_stop());
                }
            }
        }
        // SAFETY: passes it on, as every hook must.
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }

    unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            // SAFETY: for this hook and this code, that is what lparam points to.
            let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
            if info.flags & LLMHF_INJECTED == 0 {
                real_hand();
                if wparam == WM_LBUTTONDOWN as usize {
                    REAL_PRESS.store(now_ms(), Ordering::Relaxed);
                }
            }
        }
        // SAFETY: passes it on, as every hook must.
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }

    const BAR_WIDTH: i32 = 560;
    const BAR_HEIGHT: i32 = 56;
    const BUTTON_WIDTH: i32 = 110;

    /// (more, stop), in the bar's own pixels.
    fn bar_buttons(width: i32, height: i32) -> (RECT, RECT) {
        let pad = height / 6;
        let button = width * BUTTON_WIDTH / BAR_WIDTH;
        let stop = RECT {
            left: width - pad - button,
            top: pad,
            right: width - pad,
            bottom: height - pad,
        };
        let more = RECT {
            left: stop.left - pad - button,
            top: pad,
            right: stop.left - pad,
            bottom: height - pad,
        };
        (more, stop)
    }

    fn clock(left_ms: i64) -> String {
        let s = (left_ms / 1000).max(0);
        format!("{}:{:02}", s / 60, s % 60)
    }

    unsafe extern "system" fn bar_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_TIMER => {
                // SAFETY: our own window.
                unsafe { InvalidateRect(window, std::ptr::null(), 0) };
                0
            }
            WM_PAINT => {
                // SAFETY: our own window, between BeginPaint and EndPaint. What is made is deleted.
                unsafe { paint_bar(window) };
                0
            }
            WM_LBUTTONUP => {
                let (x, y) = (
                    i32::from((lparam & 0xffff) as u16 as i16),
                    i32::from(((lparam >> 16) & 0xffff) as u16 as i16),
                );
                // Only a real press counts here, whatever else keeps anyone away from this bar.
                if now_ms() - REAL_PRESS.load(Ordering::Relaxed) < PRESS_COUNTS {
                    let mut r = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    // SAFETY: our own window.
                    unsafe { GetClientRect(window, &raw mut r) };
                    let (more, stop) = bar_buttons(r.right, r.bottom);
                    let inside =
                        |b: &RECT| x >= b.left && x < b.right && y >= b.top && y < b.bottom;
                    if inside(&stop) {
                        with_hand(|hand| hand.core.control_stop());
                    } else if inside(&more) {
                        with_hand(|hand| hand.core.control_extend());
                    }
                }
                0
            }
            WM_DESTROY => {
                // SAFETY: ends this thread's message loop.
                unsafe { PostQuitMessage(0) };
                0
            }
            // SAFETY: what is not handled here goes to the default.
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    unsafe fn paint_bar(window: HWND) {
        const fn rgb(r: u32, g: u32, b: u32) -> u32 {
            r | g << 8 | b << 16
        }
        // SAFETY: as the caller says.
        unsafe {
            let mut paint: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(window, &raw mut paint);
            let mut r = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            GetClientRect(window, &raw mut r);
            let (more, stop) = bar_buttons(r.right, r.bottom);
            let back = CreateSolidBrush(rgb(0x1c, 0x1f, 0x26));
            FillRect(dc, &raw const r, back);
            DeleteObject(back);
            let red = CreateSolidBrush(rgb(0xc6, 0x28, 0x28));
            FillRect(dc, &raw const stop, red);
            DeleteObject(red);
            let grey = CreateSolidBrush(rgb(0x3a, 0x3f, 0x4a));
            FillRect(dc, &raw const more, grey);
            DeleteObject(grey);
            let face = wide("Segoe UI");
            let font = CreateFontW(
                -(r.bottom * 3 / 10),
                0,
                0,
                0,
                FW_SEMIBOLD as i32,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                u32::from(FF_DONTCARE),
                face.as_ptr(),
            );
            let old = SelectObject(dc, font);
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, rgb(0xff, 0xff, 0xff));
            let (left, more_word, stop_word) = {
                let guard = HAND.lock().unwrap_or_else(|p| p.into_inner());
                let left_ms = UNTIL.load(Ordering::Relaxed) - now_ms();
                match guard.as_ref() {
                    Some(hand) => (
                        format!("{}  {}", hand.words.who, clock(left_ms)),
                        hand.words.more.clone(),
                        hand.words.stop.clone(),
                    ),
                    None => (String::new(), String::new(), String::new()),
                }
            };
            let mut text_area = RECT {
                left: r.bottom / 3,
                top: 0,
                right: more.left - r.bottom / 6,
                bottom: r.bottom,
            };
            let single = DT_SINGLELINE | DT_VCENTER;
            let line = wide(&left);
            DrawTextW(
                dc,
                line.as_ptr(),
                -1,
                &raw mut text_area,
                single | DT_LEFT | DT_END_ELLIPSIS,
            );
            let mut more_area = more;
            let line = wide(&more_word);
            DrawTextW(
                dc,
                line.as_ptr(),
                -1,
                &raw mut more_area,
                single | DT_CENTER,
            );
            let mut stop_area = stop;
            let line = wide(&stop_word);
            DrawTextW(
                dc,
                line.as_ptr(),
                -1,
                &raw mut stop_area,
                single | DT_CENTER,
            );
            SelectObject(dc, old);
            DeleteObject(font);
            EndPaint(window, &raw const paint);
        }
    }

    /// A thread with its own message loop: the hooks and the Stop button stay alive, whatever the
    /// app's screen is doing. Returns that thread, to end it with.
    pub fn start_hand_and_bar(core: Core, words: BarWords) -> Option<u32> {
        *HAND.lock().unwrap_or_else(|p| p.into_inner()) = Some(Hand { core, words });
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            make_pixels_real();
            // SAFETY: asks which thread this is.
            let _ = tx.send(unsafe { GetCurrentThreadId() });
            // SAFETY: everything made here belongs to this thread, and is undone before it ends.
            unsafe { hand_and_bar_loop() };
            *HAND.lock().unwrap_or_else(|p| p.into_inner()) = None;
        });
        rx.recv().ok()
    }

    pub fn stop_hand_and_bar(thread: u32) {
        // SAFETY: asks that thread's message loop to end. If it is gone already, nothing happens.
        unsafe { PostThreadMessageW(thread, WM_QUIT, 0, 0) };
    }

    unsafe fn hand_and_bar_loop() {
        // SAFETY: as the caller says.
        unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            let keys: HHOOK = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), module, 0);
            let mice: HHOOK = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
            let class = wide("WhatsMineControlBar");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.style = CS_HREDRAW | CS_VREDRAW;
            wc.lpfnWndProc = Some(bar_proc);
            wc.hInstance = module;
            wc.lpszClassName = class.as_ptr();
            wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            RegisterClassW(&raw const wc);
            let scale = GetDpiForSystem() as i32;
            let (width, height) = (BAR_WIDTH * scale / 96, BAR_HEIGHT * scale / 96);
            let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
            let bar = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                x,
                0,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                module,
                std::ptr::null(),
            );
            if !bar.is_null() {
                // Whoever is in control does not see it, or a way to it.
                SetWindowDisplayAffinity(bar, WDA_EXCLUDEFROMCAPTURE);
                ShowWindow(bar, SW_SHOWNOACTIVATE);
                SetTimer(bar, 1, 1000, None);
            }
            // Without a Stop button and without knowing about the owner's hand, this is not on.
            if bar.is_null() || keys.is_null() || mice.is_null() {
                with_hand(|hand| hand.core.control_not_safe());
            }
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
            if !bar.is_null() {
                DestroyWindow(bar);
            }
            if !keys.is_null() {
                UnhookWindowsHookEx(keys);
            }
            if !mice.is_null() {
                UnhookWindowsHookEx(mice);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_capture_calls_itself() {
        assert_eq!(
            parse_label("screen:0:0", "monitor"),
            Some(LabelSays::Screen)
        );
        assert_eq!(
            parse_label("window:131234:0", "window"),
            Some(LabelSays::Window(131_234))
        );
        // Anything that does not fit: not known for certain.
        for (label, surface) in [
            ("screen:0:0", "window"),
            ("window:131234:0", "monitor"),
            ("window:0:0", "window"),
            ("window:-5:0", "window"),
            ("web-contents-media-stream://1:2", "browser"),
            ("", ""),
            ("window:abc:0", "window"),
            ("something else", "monitor"),
        ] {
            assert_eq!(parse_label(label, surface), None, "{label}");
        }
    }

    #[test]
    fn a_picture_has_to_have_the_shape_of_what_it_shows() {
        let area = Area {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert!(same_shape(1920, 1080, area));
        assert!(same_shape(1280, 720, area), "scaled down");
        assert!(!same_shape(1080, 1920, area), "turned");
        assert!(!same_shape(800, 800, area));
        assert!(!same_shape(0, 0, area));
        let none = Area {
            width: 0,
            height: 0,
            ..area
        };
        assert!(!same_shape(1920, 1080, none));
    }
}
