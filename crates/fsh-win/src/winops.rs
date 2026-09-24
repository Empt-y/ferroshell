//! Acting on other applications' windows and launching apps. Everything that targets a
//! window is asynchronous (posted, not sent) so a hung app can't block the caller.

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    IShellItem, IShellLinkW, SEE_MASK_NOASYNC, SHCreateItemFromParsingName, SHELLEXECUTEINFOW, SIGDN_NORMALDISPLAY,
    ShellExecuteExW, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, PostMessageW, SW_MINIMIZE,
    SW_RESTORE, SW_SHOWNORMAL, SetForegroundWindow, ShowWindowAsync, WM_CLOSE,
};
use windows::core::{Interface, PCWSTR, PWSTR};

use crate::{Hwnd, wide};

pub fn is_minimized(hwnd: Hwnd) -> bool {
    unsafe { IsIconic(hwnd.raw()).as_bool() }
}

/// Bring a window to the front, restoring it if minimized. Call from the thread that
/// received the user's click so Windows allows the focus change.
pub fn activate(hwnd: Hwnd) {
    unsafe {
        let h = hwnd.raw();
        if IsIconic(h).as_bool() {
            let _ = ShowWindowAsync(h, SW_RESTORE);
        }
        if SetForegroundWindow(h).as_bool() {
            return;
        }
        // Foreground lock: temporarily share input state with the current foreground
        // thread. Only as a fallback, since attaching to a hung thread can stall input.
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        if fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool() {
            let _ = SetForegroundWindow(h);
            let _ = BringWindowToTop(h);
            let _ = AttachThreadInput(me, fg_thread, false);
        }
    }
}

pub fn minimize(hwnd: Hwnd) {
    unsafe {
        let _ = ShowWindowAsync(hwnd.raw(), SW_MINIMIZE);
    }
}

pub fn restore(hwnd: Hwnd) {
    unsafe {
        let _ = ShowWindowAsync(hwnd.raw(), SW_RESTORE);
    }
}

/// Politely ask a window to close (like clicking its X).
pub fn close(hwnd: Hwnd) {
    unsafe {
        let _ = PostMessageW(Some(hwnd.raw()), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// Open a file, shortcut, executable or `shell:` path. May block briefly (the shell can
/// do DDE); call from a worker thread. Requires COM on the calling thread.
pub fn launch(target: &str, args: Option<&str>) -> anyhow::Result<()> {
    launch_verb(target, args, "open")
}

/// [`launch`] with a shell verb, e.g. `runas` (run as administrator).
pub fn launch_verb(target: &str, args: Option<&str>, verb: &str) -> anyhow::Result<()> {
    let target_w = wide(target);
    let args_w = args.map(wide);
    let verb = wide(verb);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(target_w.as_ptr()),
        lpParameters: args_w.as_ref().map_or(PCWSTR::null(), |a| PCWSTR(a.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| anyhow::anyhow!("launching {target}: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub target: Option<String>,
    pub app_id: Option<String>,
}

/// Read a `.lnk` file's target and explicit app id. Requires COM on the calling thread.
pub fn resolve_shortcut(path: &str) -> Option<Shortcut> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let w = wide(path);
        file.Load(PCWSTR(w.as_ptr()), STGM_READ).ok()?;
        let mut buf = vec![0u16; 1024];
        let target = link
            .GetPath(&mut buf, std::ptr::null_mut(), 0)
            .ok()
            .map(|_| {
                let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
                String::from_utf16_lossy(&buf[..end])
            })
            .filter(|s| !s.is_empty());
        let app_id = link.cast::<IPropertyStore>().ok().and_then(|store| {
            let value = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
            let s: PWSTR = PropVariantToStringAlloc(&value).ok()?;
            let out = s.to_string().ok();
            CoTaskMemFree(Some(s.0 as *const _));
            out.filter(|s| !s.is_empty())
        });
        Some(Shortcut { target, app_id })
    }
}

/// The name Explorer shows for a path or shell item (e.g. an app's display name for
/// `shell:AppsFolder\<AUMID>`). Requires COM on the calling thread.
pub fn display_name(parsing_name: &str) -> Option<String> {
    let w = wide(parsing_name);
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        let name = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let out = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        out
    }
}
