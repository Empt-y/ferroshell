//! The tray thread: owns our hidden `Shell_TrayWnd` (see `fsh_win::tray`). It runs only
//! while some panel has a system tray widget.
//!
//! Apps block on `Shell_NotifyIcon` until the tray answers, so this thread does nothing
//! slow: decode, convert the icon, hand it to the UI thread, forward to Explorer.

use std::rc::Rc;
use std::time::Duration;

use fsh_win::Hwnd;
use fsh_win::icon::RgbaImage;
use fsh_win::tray::{NotifyCommand, TrayHost};
use fsh_win::window::{self, MessageWindow, WM_APP, WM_TIMER};

use crate::app;

const WM_STOP: u32 = WM_APP + 1;
const TIMER_RAISE: usize = 1;
const RAISE_EVERY_MS: u32 = 2000;

pub struct TrayThread {
    control: Hwnd,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TrayThread {
    pub fn start() -> anyhow::Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new().name("tray".into()).spawn(move || {
            if let Err(e) = run(&tx) {
                tracing::error!("system tray unavailable: {e:#}");
            }
        })?;
        let control = rx.recv_timeout(Duration::from_secs(5)).map_err(|_| anyhow::anyhow!("tray thread did not start"))?;
        Ok(Self { control, thread: Some(thread) })
    }
}

impl Drop for TrayThread {
    fn drop(&mut self) {
        self.control.post(WM_STOP, 0, 0);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        tracing::info!("system tray host stopped");
    }
}

fn run(ready: &std::sync::mpsc::Sender<Hwnd>) -> anyhow::Result<()> {
    let host = Rc::new(TrayHost::new(|cmd: NotifyCommand, icon: Option<RgbaImage>| {
        let _ = slint::invoke_from_event_loop(move || {
            app::with(|a| a.on_tray_command(cmd, icon));
        });
    })?);

    let h = host.clone();
    let control = MessageWindow::new(
        "Ferroshell Tray Control",
        Box::new(move |_, msg, wparam, _| match msg {
            WM_STOP => {
                window::post_quit(0);
                Some(0)
            }
            WM_TIMER if wparam == TIMER_RAISE => {
                // Explorer can put its own tray window back in front (e.g. after it
                // restarts); keep ours first so new icons keep coming to us.
                if !h.is_first() {
                    tracing::debug!("re-raising tray window");
                    h.raise();
                }
                Some(0)
            }
            _ => None,
        }),
    )?;
    control.set_timer(TIMER_RAISE, RAISE_EVERY_MS);

    // Existing icons were registered with Explorer; ask apps to register them again
    // (they'll now reach us first).
    fsh_win::tray::broadcast_taskbar_created();
    tracing::info!("system tray host running (our window is first: {})", host.is_first());
    let _ = ready.send(control.hwnd());
    window::run_message_loop();
    Ok(())
}
