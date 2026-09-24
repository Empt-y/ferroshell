//! Icons as straight-alpha RGBA pixels, from windows (WM_GETICON) or from shell items
//! (executables, shortcuts, `shell:AppsFolder\<AUMID>` for Store apps).

use windows::Win32::Foundation::{LPARAM, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GetDIBits, GetObjectW, HBITMAP, HGDIOBJ,
};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY};
use windows::Win32::UI::WindowsAndMessaging::{
    GCLP_HICON, GetClassLongPtrW, GetIconInfo, HICON, ICONINFO, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW,
    WM_GETICON,
};
use windows::core::PCWSTR;

use crate::{Hwnd, wide};

#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// Straight (not premultiplied) RGBA, row-major, top row first.
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RgbaImage({}x{})", self.width, self.height)
    }
}

const ICON_BIG: usize = 1;
const ICON_SMALL2: usize = 2;

/// The icon a window shows in the taskbar. Asks the window with a short timeout, so a
/// hung app costs at most `timeout_ms` and never blocks indefinitely.
pub fn window_icon(hwnd: Hwnd, timeout_ms: u32) -> Option<RgbaImage> {
    for kind in [ICON_BIG, ICON_SMALL2] {
        let mut result = 0usize;
        let ok = unsafe {
            SendMessageTimeoutW(
                hwnd.raw(),
                WM_GETICON,
                WPARAM(kind),
                LPARAM(0),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                timeout_ms,
                Some(&mut result),
            )
        };
        if ok.0 == 0 {
            break; // timed out or hung: don't ask again
        }
        if result != 0
            && let Some(img) = hicon_to_rgba(HICON(result as *mut _))
        {
            return Some(img);
        }
    }
    let class_icon = unsafe { GetClassLongPtrW(hwnd.raw(), GCLP_HICON) };
    if class_icon != 0 {
        return hicon_to_rgba(HICON(class_icon as *mut _));
    }
    None
}

/// Icon for a file path, shortcut, or shell parsing name at roughly `size` pixels.
/// Requires COM on the calling thread.
pub fn shell_item_icon(parsing_name: &str, size: i32) -> Option<RgbaImage> {
    let w = wide(parsing_name);
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        let hbm = factory.GetImage(SIZE { cx: size, cy: size }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK).ok()?;
        let img = bitmap_pixels(hbm).map(|(w, h, mut px)| {
            unpremultiply(&mut px);
            RgbaImage { width: w, height: h, pixels: px }
        });
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        img
    }
}

/// Read a bitmap as top-down BGRA → RGBA (alpha untouched).
fn bitmap_pixels(hbm: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(HGDIOBJ(hbm.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        if w <= 0 || h <= 0 || w > 1024 || h > 1024 {
            return None;
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(dc, hbm, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        if lines == 0 {
            return None;
        }
        for p in px.as_chunks_mut::<4>().0 {
            p.swap(0, 2); // BGRA -> RGBA
        }
        Some((w as u32, h as u32, px))
    }
}

fn hicon_to_rgba(hicon: HICON) -> Option<RgbaImage> {
    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(hicon, &mut ii).ok()?;
        let color = (!ii.hbmColor.is_invalid()).then(|| bitmap_pixels(ii.hbmColor)).flatten();
        let mask = (!ii.hbmMask.is_invalid()).then(|| bitmap_pixels(ii.hbmMask)).flatten();
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
        }
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
        }
        let (w, h, mut px) = color?;
        // Old-style icons have no alpha channel; derive it from the AND mask
        // (mask black = opaque).
        if px.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
            match mask {
                Some((mw, mh, m)) if mw == w && mh >= h => {
                    for (p, mp) in px.as_chunks_mut::<4>().0.iter_mut().zip(m.as_chunks::<4>().0) {
                        p[3] = if mp[0] == 0 { 255 } else { 0 };
                    }
                }
                _ => px.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255),
            }
        }
        Some(RgbaImage { width: w, height: h, pixels: px })
    }
}

fn unpremultiply(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}
