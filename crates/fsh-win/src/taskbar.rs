//! Hiding and restoring Explorer's own taskbar while Ferroshell runs alongside it.
//!
//! The original auto-hide setting is written to a state file *before* anything changes, so
//! it can be restored by whichever process notices first — the shell on clean exit, the
//! supervisor after a crash, or the next run after a hard kill of both.

use std::path::Path;

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Shell::{ABM_GETSTATE, ABM_SETSTATE, ABS_AUTOHIDE, APPBARDATA, SHAppBarMessage};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, IsWindowVisible, SW_HIDE, SW_SHOWNA, ShowWindow,
};
use windows::core::BOOL;

use crate::Hwnd;

const PRIMARY_CLASS: &str = "Shell_TrayWnd";
const SECONDARY_CLASS: &str = "Shell_SecondaryTrayWnd";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
struct SavedState {
    autohide: bool,
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Explorer's taskbar windows: the primary one first, then one per secondary monitor.
pub fn taskbar_windows() -> Vec<Hwnd> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lparam.0 as *mut Vec<(bool, Hwnd)>) };
        match class_name(hwnd).as_str() {
            PRIMARY_CLASS => out.push((true, Hwnd::from_raw(hwnd))),
            SECONDARY_CLASS => out.push((false, Hwnd::from_raw(hwnd))),
            _ => {}
        }
        BOOL(1)
    }
    let mut found: Vec<(bool, Hwnd)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut found as *mut _ as isize));
    }
    found.sort_by_key(|(primary, _)| !primary);
    found.into_iter().map(|(_, h)| h).collect()
}

fn primary() -> Option<Hwnd> {
    taskbar_windows().into_iter().next()
}

/// Is Explorer's taskbar running at all? (False when Explorer isn't the shell or has crashed.)
pub fn explorer_taskbar_present() -> bool {
    primary().is_some()
}

fn appbar_data(tray: Hwnd) -> APPBARDATA {
    APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: tray.raw(), ..Default::default() }
}

fn autohide() -> Option<bool> {
    let tray = primary()?;
    let mut abd = appbar_data(tray);
    let state = unsafe { SHAppBarMessage(ABM_GETSTATE, &mut abd) } as u32;
    Some(state & ABS_AUTOHIDE != 0)
}

fn set_autohide(on: bool) {
    let Some(tray) = primary() else { return };
    let mut abd = appbar_data(tray);
    abd.lParam = LPARAM(if on { ABS_AUTOHIDE as isize } else { 0 });
    unsafe {
        SHAppBarMessage(ABM_SETSTATE, &mut abd);
    }
}

/// Hide Explorer's taskbar(s), recording the original state in `state_file` first.
/// Safe to call repeatedly (e.g. after Explorer restarts): the first recorded state wins.
pub fn hide(state_file: &Path) -> anyhow::Result<()> {
    if !state_file.exists()
        && let Some(orig) = autohide()
    {
        if let Some(dir) = state_file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(state_file, serde_json::to_vec(&SavedState { autohide: orig })?)?;
    }
    // Auto-hide releases the screen space Explorer reserves; hiding the windows stops them
    // sliding back in when the mouse touches the edge.
    set_autohide(true);
    enforce_hidden();
    Ok(())
}

/// Hide any taskbar window that has become visible again. Returns how many were re-hidden.
pub fn enforce_hidden() -> usize {
    let mut n = 0;
    for tray in taskbar_windows() {
        unsafe {
            if IsWindowVisible(tray.raw()).as_bool() {
                let _ = ShowWindow(tray.raw(), SW_HIDE);
                n += 1;
            }
        }
    }
    n
}

/// Is there saved state, i.e. did some process hide the taskbar without restoring it?
pub fn needs_restore(state_file: &Path) -> bool {
    state_file.exists()
}

/// Put Explorer's taskbar back the way it was and delete the state file.
/// Without a state file this only re-shows the windows.
pub fn restore(state_file: &Path) {
    let saved: Option<SavedState> =
        std::fs::read(state_file).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(saved) = saved {
        set_autohide(saved.autohide);
    }
    for tray in taskbar_windows() {
        unsafe {
            let _ = ShowWindow(tray.raw(), SW_SHOWNA);
        }
    }
    let _ = std::fs::remove_file(state_file);
}
