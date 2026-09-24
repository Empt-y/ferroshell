//! Live window previews drawn by DWM into one of our windows.

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY,
    DWM_TNP_VISIBLE, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties,
};

use crate::{Hwnd, Rect};

/// A registered thumbnail; unregistered on drop.
pub struct Thumbnail(isize);

impl Thumbnail {
    /// Show `source`'s live contents inside `dest` (positioned with [`Thumbnail::show`]).
    pub fn register(dest: Hwnd, source: Hwnd) -> Option<Self> {
        unsafe { DwmRegisterThumbnail(dest.raw(), source.raw()).ok().map(Self) }
    }

    pub fn source_size(&self) -> Option<(i32, i32)> {
        unsafe { DwmQueryThumbnailSourceSize(self.0).ok().map(|s: SIZE| (s.cx, s.cy)) }
    }

    /// Place the preview at `rect` (physical pixels, relative to `dest`'s client area).
    pub fn show(&self, rect: Rect, opacity: u8) {
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY | DWM_TNP_OPACITY,
            rcDestination: rect.to_raw(),
            fVisible: true.into(),
            fSourceClientAreaOnly: false.into(),
            opacity,
            ..Default::default()
        };
        unsafe {
            let _ = DwmUpdateThumbnailProperties(self.0, &props);
        }
    }
}

impl Drop for Thumbnail {
    fn drop(&mut self) {
        unsafe {
            let _ = DwmUnregisterThumbnail(self.0);
        }
    }
}
