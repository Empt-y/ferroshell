//! The "media" service: what's playing and its transport controls, on a "media" worker
//! thread. Polls once a second (and just after a control is used); cover art is fetched
//! only when the track changes and handed to the UI as a file.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use fsh_win::media::{Control, Media, NowPlaying};

use super::Update;

const POLL_EVERY: Duration = Duration::from_secs(1);
const AFTER_CONTROL: Duration = Duration::from_millis(300);
const RETRY_EVERY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub available: bool,
    pub now: Option<NowPlaying>,
    /// Cover art for `now`, written to a file (a new name per track, so the UI's image
    /// cache never shows a stale one).
    pub art: Option<PathBuf>,
}

pub struct MediaService {
    tx: Sender<Control>,
}

impl MediaService {
    pub fn spawn(on_update: impl Fn(Update) + Send + 'static) -> anyhow::Result<Self> {
        let (tx, rx) = channel();
        std::thread::Builder::new().name("media".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&rx, &on_update)));
            if result.is_err() {
                tracing::error!("media service crashed; media controls are unavailable until the shell restarts");
                on_update(Update::Media(Snapshot::default()));
            }
        })?;
        Ok(Self { tx })
    }

    pub fn send(&self, c: Control) {
        let _ = self.tx.send(c);
    }
}

fn art_dir() -> PathBuf {
    fsh_common::paths::state_dir().join("media")
}

/// A file extension for encoded image bytes, from their signature.
fn image_ext(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [0xFF, 0xD8, ..] => "jpg",
        [b'B', b'M', ..] => "bmp",
        [b'G', b'I', b'F', ..] => "gif",
        _ => "img",
    }
}

fn run(rx: &Receiver<Control>, on_update: &dyn Fn(Update)) {
    let _com = fsh_win::com::ComGuard::mta();
    let media = loop {
        match Media::new() {
            Ok(m) => break m,
            Err(e) => {
                tracing::warn!("media sessions unavailable: {e:#}");
                on_update(Update::Media(Snapshot::default()));
                if let Err(RecvTimeoutError::Disconnected) = rx.recv_timeout(RETRY_EVERY) {
                    return;
                }
            }
        }
    };
    // Old art from a previous run.
    let _ = std::fs::remove_dir_all(art_dir());
    let mut last = Snapshot { available: false, now: None, art: None };
    let mut counter = 0u32;
    let mut wait = Duration::ZERO;
    loop {
        match rx.recv_timeout(wait) {
            Ok(c) => {
                media.control(c);
                wait = AFTER_CONTROL;
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        wait = POLL_EVERY;
        let now = media.now_playing();
        let same_track = |a: &NowPlaying, b: &NowPlaying| a.app_id == b.app_id && a.title == b.title && a.artist == b.artist;
        let track_changed = match (&now, &last.now) {
            (Some(a), Some(b)) => !same_track(a, b),
            (None, None) => false,
            _ => true,
        };
        let mut art = last.art.clone();
        if track_changed || !last.available {
            if let Some(old) = art.take() {
                let _ = std::fs::remove_file(old);
            }
            if now.is_some()
                && let Some(bytes) = media.artwork()
            {
                counter = counter.wrapping_add(1);
                let path = art_dir().join(format!("art-{counter}.{}", image_ext(&bytes)));
                let _ = std::fs::create_dir_all(art_dir());
                if std::fs::write(&path, &bytes).is_ok() {
                    art = Some(path);
                }
            }
        }
        let snap = Snapshot { available: true, now, art };
        if snap != last {
            on_update(Update::Media(snap.clone()));
            last = snap;
        }
    }
    let _ = std::fs::remove_dir_all(art_dir());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_types() {
        assert_eq!(image_ext(&[0x89, b'P', b'N', b'G', 0x0D]), "png");
        assert_eq!(image_ext(&[0xFF, 0xD8, 0xFF]), "jpg");
        assert_eq!(image_ext(b"GIF89a"), "gif");
        assert_eq!(image_ext(&[]), "img");
    }
}
