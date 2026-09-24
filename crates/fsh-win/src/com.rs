//! COM initialisation for threads that use shell APIs (icons, shortcuts, app ids).

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};

/// Initialises COM (single-threaded apartment) on the current thread until dropped.
pub struct ComGuard {
    initialised: bool,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl ComGuard {
    pub fn new() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        // S_FALSE (already initialised) still needs a matching CoUninitialize.
        Self { initialised: hr.is_ok(), _not_send: std::marker::PhantomData }
    }
}

impl Default for ComGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialised {
            unsafe { CoUninitialize() };
        }
    }
}
