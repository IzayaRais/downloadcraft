//! Tray behaviour: park the window off-screen instead of hiding it.
//!
//! eframe only runs a frame when the window repaints, and a *hidden* window
//! never receives `WM_PAINT` (winit's own comment: "the window technically has
//! to be visible to receive WM_PAINT messages"). `ViewportCommand::Visible(false)`
//! therefore froze the entire app while it sat in the tray: transfer progress
//! and completions were never reported, the browser inbox was never drained
//! (so captures looked stuck), the liveness heartbeat went stale and the tray
//! menu - polled from the frame - did nothing at all.
//!
//! "In the tray" is therefore a window that stays `WS_VISIBLE` (so it keeps
//! painting and the app keeps working) but is moved far off-screen and marked
//! `WS_EX_TOOLWINDOW` so it has no taskbar button. Un-traying restores the
//! remembered placement and takes the foreground back.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::Mutex;

/// True while the window is parked off-screen ("in the tray").
static PARKED: AtomicBool = AtomicBool::new(false);
/// Native window handle of the main window (set from the eframe frame).
static HWND: AtomicIsize = AtomicIsize::new(0);
/// Set by the tray's Exit item, consumed by the frame loop.
static QUIT: AtomicBool = AtomicBool::new(false);
/// Set once a quit is in flight: nothing may un-park the window on the way out.
static QUITTING: AtomicBool = AtomicBool::new(false);
/// Frame counter, bumped by every `update` (see [`start_keepalive`]).
static FRAMES: AtomicU64 = AtomicU64::new(0);
/// Placement to restore on un-tray (position, size, maximized state).
static PLACEMENT: Mutex<Option<win32::WindowPlacement>> = Mutex::new(None);

const TRAY_TIP: &str = "Download Craft";

pub fn set_hwnd(hwnd: isize) {
    if hwnd != 0 {
        HWND.store(hwnd, Ordering::SeqCst);
    }
}

pub fn is_parked() -> bool {
    PARKED.load(Ordering::SeqCst)
}

/// True once the tray's Exit has been chosen and the window is on its way out.
pub fn quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

/// True once, when the tray's Exit item was chosen.
pub fn take_quit() -> bool {
    QUIT.swap(false, Ordering::SeqCst)
}

/// Count one frame and keep the paint pump armed.
///
/// eframe only runs `update` when the window paints, and Windows stops
/// generating `WM_PAINT` on its own for a window the compositor has nothing to
/// draw for: parked far off-screen (and, less often, fully covered by other
/// windows). eframe's own redraw requests go through an internal-paint flag
/// that such a window never turns into a paint. The app then looks alive - the
/// window is visible, the tray icon answers right-clicks, the menu opens - while
/// every frame-driven job is dead: progress reporting, the browser inbox, the
/// liveness heartbeat and the tray menu items, whose "Show manager" and "Exit"
/// simply did nothing. Marking the window dirty (see [`nudge_paint`]) produces a
/// genuine `WM_PAINT`, which is exactly what the frame loop waits for.
pub fn frame_tick() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// Force one real paint of the main window; safe from any thread.
pub fn nudge_paint() {
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        win32::InvalidateRect(hwnd, std::ptr::null(), 0);
        win32::RedrawWindow(hwnd, std::ptr::null(), 0, win32::RDW_INVALIDATE);
    }
}

/// Watchdog for the frame loop: if no frame has run half a second after the
/// last check, ask for a paint. A healthy app (its own repaint request runs
/// every 100-250 ms) never gets nudged, so this costs nothing in the common
/// case; a stalled one is restarted, which is what keeps tray actions - and
/// everything else the frame loop does - working while parked.
pub fn start_keepalive() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let mut seen = FRAMES.load(Ordering::Relaxed);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let now = FRAMES.load(Ordering::Relaxed);
            if now == seen {
                nudge_paint();
            }
            seen = now;
        }
    });
}

/// Park the main window off-screen: still visible (so the frame loop keeps
/// running), but invisible to the user and without a taskbar button.
pub fn park() {
    if PARKED.swap(true, Ordering::SeqCst) {
        return;
    }
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        let mut placement = win32::WindowPlacement {
            length: std::mem::size_of::<win32::WindowPlacement>() as u32,
            ..Default::default()
        };
        let saved = if win32::GetWindowPlacement(hwnd, &mut placement) != 0 {
            // Coming back *minimized* would freeze the app again.
            if placement.show_cmd == win32::SW_SHOWMINIMIZED {
                placement.show_cmd = win32::SW_SHOWNORMAL as u32;
            }
            Some(placement)
        } else {
            None
        };
        if let Ok(mut slot) = PLACEMENT.lock() {
            *slot = saved;
        }
        // Un-minimize/un-maximize so the move off-screen sticks (a maximized
        // window ignores SetWindowPos), then hide -> retag -> show so the
        // taskbar drops the button.
        win32::ShowWindow(hwnd, win32::SW_RESTORE);
        win32::ShowWindow(hwnd, win32::SW_HIDE);
        let ex = win32::GetWindowLongW(hwnd, win32::GWL_EXSTYLE);
        win32::SetWindowLongW(
            hwnd,
            win32::GWL_EXSTYLE,
            (ex & !win32::WS_EX_APPWINDOW) | win32::WS_EX_TOOLWINDOW,
        );
        win32::SetWindowPos(
            hwnd,
            0,
            win32::OFFSCREEN,
            win32::OFFSCREEN,
            0,
            0,
            win32::SWP_NOSIZE
                | win32::SWP_NOZORDER
                | win32::SWP_NOACTIVATE
                | win32::SWP_SHOWWINDOW,
        );
    }
}

/// Bring the main window back on screen and forward (tray click / Show).
pub fn show() {
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    if !PARKED.swap(false, Ordering::SeqCst) {
        // Already on screen: just raise it.
        unsafe { force_foreground(hwnd) };
        return;
    }
    unsafe {
        win32::ShowWindow(hwnd, win32::SW_HIDE);
        let ex = win32::GetWindowLongW(hwnd, win32::GWL_EXSTYLE);
        win32::SetWindowLongW(
            hwnd,
            win32::GWL_EXSTYLE,
            (ex & !win32::WS_EX_TOOLWINDOW) | win32::WS_EX_APPWINDOW,
        );
        let placement = PLACEMENT.lock().ok().and_then(|g| *g);
        match placement {
            Some(placement) => {
                win32::SetWindowPlacement(hwnd, &placement);
            }
            None => {
                win32::SetWindowPos(
                    hwnd,
                    0,
                    140,
                    110,
                    0,
                    0,
                    win32::SWP_NOSIZE | win32::SWP_NOZORDER | win32::SWP_NOACTIVATE,
                );
            }
        }
        force_foreground(hwnd);
    }
}

/// Same as [`show`] but without stealing the foreground (settings changes).
pub fn restore() {
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 || !PARKED.swap(false, Ordering::SeqCst) {
        return;
    }
    unsafe {
        win32::ShowWindow(hwnd, win32::SW_HIDE);
        let ex = win32::GetWindowLongW(hwnd, win32::GWL_EXSTYLE);
        win32::SetWindowLongW(
            hwnd,
            win32::GWL_EXSTYLE,
            (ex & !win32::WS_EX_TOOLWINDOW) | win32::WS_EX_APPWINDOW,
        );
        let placement = PLACEMENT.lock().ok().and_then(|g| *g);
        match placement {
            Some(placement) => {
                win32::SetWindowPlacement(hwnd, &placement);
            }
            None => {
                win32::SetWindowPos(
                    hwnd,
                    0,
                    140,
                    110,
                    0,
                    0,
                    win32::SWP_NOSIZE | win32::SWP_NOZORDER | win32::SWP_NOACTIVATE,
                );
            }
        }
        win32::ShowWindow(hwnd, win32::SW_SHOWNOACTIVATE);
    }
}

/// Ask the frame loop to quit cleanly (tray's Exit item).
///
/// The request is honoured from the menu click itself rather than from the
/// frame loop, so Exit works even when the frame loop has gone to sleep, and
/// a watchdog covers the one case where no paint can happen at all (a locked
/// session): leaving a dead tray icon behind is worse than a forced exit.
pub fn request_quit() {
    QUITTING.store(true, Ordering::SeqCst);
    QUIT.store(true, Ordering::SeqCst);
    nudge_paint(); // wake the frame loop now, not half a second from now
    arm_quit_watchdog();
}

/// See [`request_quit`]. Five seconds is far longer than a clean shutdown
/// needs; if the app is still here by then it never got a frame.
fn arm_quit_watchdog() {
    static ARMED: AtomicBool = AtomicBool::new(false);
    if ARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(5));
        std::process::exit(0);
    });
}

/// Act on the tray menu the moment it is clicked instead of waiting for the
/// next frame (muda calls this handler from the menu's own message loop, which
/// keeps running even when the frame loop does not). `Show` therefore brings
/// the window back and `Exit` starts the shutdown regardless of frames; the
/// frame loop keeps its own handling as the fallback.
pub fn install_menu_handler() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    tray_icon::menu::MenuEvent::set_event_handler(Some(|ev: tray_icon::menu::MenuEvent| {
        if ev.id() == "tray-show" {
            show();
            nudge_paint();
        } else if ev.id() == "tray-exit" {
            request_quit();
        }
    }));
}

pub fn tooltip() -> &'static str {
    TRAY_TIP
}

/// Raise the window and take the foreground. Clicking the tray already gave
/// us the user's intent, but Windows' foreground lock still applies, so attach
/// to the current foreground thread for the switch (the classic recipe).
unsafe fn force_foreground(hwnd: isize) {
    win32::ShowWindow(hwnd, win32::SW_SHOW);
    win32::BringWindowToTop(hwnd);
    let foreground = win32::GetForegroundWindow();
    let foreground_thread = if foreground != 0 {
        win32::GetWindowThreadProcessId(foreground, std::ptr::null_mut())
    } else {
        0
    };
    let me = win32::GetCurrentThreadId();
    let attached = foreground_thread != 0
        && foreground_thread != me
        && win32::AttachThreadInput(me, foreground_thread, 1) != 0;
    win32::SetForegroundWindow(hwnd);
    if attached {
        win32::AttachThreadInput(me, foreground_thread, 0);
    }
}

#[cfg(windows)]
mod win32 {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    /// `WINDOWPLACEMENT` (without the trailing `rcDevice` field newer SDKs add;
    /// the API honours `length`).
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct WindowPlacement {
        pub length: u32,
        pub flags: u32,
        pub show_cmd: u32,
        pub min_position: Point,
        pub max_position: Point,
        pub normal_position: Rect,
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        pub fn SetWindowPos(
            hwnd: isize,
            insert_after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        pub fn GetWindowLongW(hwnd: isize, index: i32) -> i32;
        pub fn SetWindowLongW(hwnd: isize, index: i32, value: i32) -> i32;
        pub fn GetWindowPlacement(hwnd: isize, placement: *mut WindowPlacement) -> i32;
        pub fn SetWindowPlacement(hwnd: isize, placement: *const WindowPlacement) -> i32;
        pub fn SetForegroundWindow(hwnd: isize) -> i32;
        pub fn BringWindowToTop(hwnd: isize) -> i32;
        pub fn GetForegroundWindow() -> isize;
        pub fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        pub fn AttachThreadInput(attach: u32, attach_to: u32, attach_flag: i32) -> i32;
        pub fn InvalidateRect(hwnd: isize, rect: *const Rect, erase: i32) -> i32;
        pub fn RedrawWindow(hwnd: isize, rect: *const Rect, region: isize, flags: u32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetCurrentThreadId() -> u32;
    }

    pub const SW_HIDE: i32 = 0;
    pub const SW_SHOWNORMAL: i32 = 1;
    pub const SW_SHOWMINIMIZED: u32 = 2;
    pub const SW_SHOWNOACTIVATE: i32 = 4;
    pub const SW_SHOW: i32 = 5;
    pub const SW_RESTORE: i32 = 9;
    pub const GWL_EXSTYLE: i32 = -20;
    pub const WS_EX_TOOLWINDOW: i32 = 0x0000_0080;
    pub const WS_EX_APPWINDOW: i32 = 0x0004_0000;
    pub const SWP_NOSIZE: u32 = 0x0001;
    pub const SWP_NOZORDER: u32 = 0x0004;
    pub const SWP_NOACTIVATE: u32 = 0x0010;
    pub const SWP_SHOWWINDOW: u32 = 0x0040;
    /// `RedrawWindow`: the window itself (not `RDW_INTERNALPAINT`, which a
    /// parked window never turns into a paint) must be marked dirty.
    pub const RDW_INVALIDATE: u32 = 0x0001;
    /// Far outside any monitor: the window keeps painting, nobody sees it.
    pub const OFFSCREEN: i32 = -32000;
}
