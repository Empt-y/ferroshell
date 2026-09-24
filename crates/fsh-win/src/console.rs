//! Console control events (Ctrl+C, closing the console window, logoff, shutdown).

use std::sync::OnceLock;

use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::core::BOOL;

static HANDLER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Run `f` when the process receives a console control event, then let the default
/// handling (termination) proceed. `f` runs on a thread Windows creates for the event and
/// should finish quickly — for a closing console Windows allows only a few seconds.
pub fn on_console_close(f: impl Fn() + Send + Sync + 'static) {
    if HANDLER.set(Box::new(f)).is_ok() {
        unsafe {
            let _ = SetConsoleCtrlHandler(Some(handler), true);
        }
    }
}

unsafe extern "system" fn handler(_event: u32) -> BOOL {
    if let Some(f) = HANDLER.get() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    }
    BOOL(0)
}
