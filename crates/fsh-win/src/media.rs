//! "Now playing": the media session Windows considers current (Spotify, a browser tab, …),
//! via `GlobalSystemMediaTransportControlsSessionManager`, and its transport controls.
//!
//! Calls block briefly on WinRT async operations, so use this from a worker thread with
//! COM initialised (multi-threaded apartment).

use anyhow::Context as _;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NowPlaying {
    /// The playing app's AppUserModelID (e.g. `Spotify.exe` or a packaged app id).
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub can_play_pause: bool,
    pub can_next: bool,
    pub can_previous: bool,
}

pub struct Media {
    manager: Manager,
}

#[derive(Debug, Clone, Copy)]
pub enum Control {
    PlayPause,
    Next,
    Previous,
}

impl Media {
    pub fn new() -> anyhow::Result<Self> {
        let manager = Manager::RequestAsync().and_then(|op| op.join()).context("media session manager")?;
        Ok(Self { manager })
    }

    fn current(&self) -> Option<Session> {
        self.manager.GetCurrentSession().ok()
    }

    /// `None` when nothing is playing or paused.
    pub fn now_playing(&self) -> Option<NowPlaying> {
        let s = self.current()?;
        let info = s.GetPlaybackInfo().ok()?;
        let controls = info.Controls().ok();
        let props = s.TryGetMediaPropertiesAsync().and_then(|op| op.join()).ok();
        let text = |f: &dyn Fn() -> windows::core::Result<windows::core::HSTRING>| f().map(|h| h.to_string()).unwrap_or_default();
        Some(NowPlaying {
            app_id: text(&|| s.SourceAppUserModelId()),
            title: props.as_ref().map(|p| text(&|| p.Title())).unwrap_or_default(),
            artist: props.as_ref().map(|p| text(&|| p.Artist())).unwrap_or_default(),
            album: props.as_ref().map(|p| text(&|| p.AlbumTitle())).unwrap_or_default(),
            playing: info.PlaybackStatus().is_ok_and(|st| st == Status::Playing),
            can_play_pause: controls.as_ref().is_some_and(|c| c.IsPlayPauseToggleEnabled().unwrap_or(false)),
            can_next: controls.as_ref().is_some_and(|c| c.IsNextEnabled().unwrap_or(false)),
            can_previous: controls.as_ref().is_some_and(|c| c.IsPreviousEnabled().unwrap_or(false)),
        })
    }

    /// The current track's cover art as encoded image bytes (PNG/JPEG), if the app gave one.
    pub fn artwork(&self) -> Option<Vec<u8>> {
        let s = self.current()?;
        let props = s.TryGetMediaPropertiesAsync().and_then(|op| op.join()).ok()?;
        let stream = props.Thumbnail().ok()?.OpenReadAsync().and_then(|op| op.join()).ok()?;
        let size = u32::try_from(stream.Size().ok()?).ok().filter(|&n| n > 0 && n < 16 << 20)?;
        let reader = DataReader::CreateDataReader(&stream).ok()?;
        reader.LoadAsync(size).and_then(|op| op.join()).ok()?;
        let mut buf = vec![0u8; size as usize];
        reader.ReadBytes(&mut buf).ok()?;
        Some(buf)
    }

    pub fn control(&self, c: Control) {
        let Some(s) = self.current() else { return };
        let op = match c {
            Control::PlayPause => s.TryTogglePlayPauseAsync(),
            Control::Next => s.TrySkipNextAsync(),
            Control::Previous => s.TrySkipPreviousAsync(),
        };
        if let Err(e) = op.and_then(|op| op.join()) {
            tracing::warn!("media {c:?}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "reads the live media session"]
    fn live_read_only() {
        let _com = crate::com::ComGuard::mta();
        let media = Media::new().unwrap();
        println!("{:#?}", media.now_playing());
        println!("artwork bytes: {:?}", media.artwork().map(|b| b.len()));
    }
}
