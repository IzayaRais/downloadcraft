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

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;

/// True while the window is parked off-screen ("in the tray").
static PARKED: AtomicBool = AtomicBool::new(false);
/// Native window handle of the main window (set from the eframe frame).
static HWND: AtomicIsize = AtomicIsize::new(0);
/// Set by the tray's Exit item, consumed by the frame loop.
static QUIT: AtomicBool = AtomicBool::new(false);
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

/// True once, when the tray's Exit item was chosen.
pub fn take_quit() -> bool {
    QUIT.swap(false, Ordering::SeqCst)
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
pub fn request_quit() {
    QUIT.store(true, Ordering::SeqCst);
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
    /// Far outside any monitor: the window keeps painting, nobody sees it.
    pub const OFFSCREEN: i32 = -32000;
}
