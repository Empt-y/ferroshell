//! COM initialisation for threads that use shell APIs (icons, shortcuts, app ids).

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

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

    /// Multi-threaded apartment, for threads that wait on WinRT async operations.
    pub fn mta() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self { initialised: hr.is_ok(), _not_send: std::marker::PhantomData }
    }
}

/// Process-wide COM security for outgoing calls: authenticate, and let servers impersonate
/// us. WMI's `root\wmi` providers (e.g. screen brightness) refuse calls at COM's default
/// "identify" level, and `CoSetProxyBlanket` on individual proxies isn't enough.
///
/// Call once, early in `main`, on a thread with COM initialised and before any other COM
/// use in the process (otherwise it fails with `RPC_E_TOO_LATE`). Keep that thread's
/// [`ComGuard`] alive for the process's lifetime.
pub fn init_process_security() -> windows::core::Result<()> {
    use windows::Win32::System::Com::{CoInitializeSecurity, EOAC_NONE, RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE};
    unsafe { CoInitializeSecurity(None, -1, None, None, RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE, None) }
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
