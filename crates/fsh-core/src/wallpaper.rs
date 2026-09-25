//! The desktop background when Ferroshell is the login shell: Windows' settings say what
//! it should be (a picture, a slideshow or Spotlight), and Explorer normally runs the
//! slideshow and Spotlight. These are the rules for doing that ourselves.

use std::path::{Path, PathBuf};

/// `HKCU\...\Explorer\Wallpapers` `BackgroundType`, as Settings > Personalisation sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackgroundType {
    Picture,
    SolidColour,
    Slideshow,
    Spotlight,
}

impl BackgroundType {
    pub fn from_registry(v: u32) -> Option<Self> {
        Some(match v {
            0 => Self::Picture,
            1 => Self::SolidColour,
            2 => Self::Slideshow,
            3 => Self::Spotlight,
            _ => return None,
        })
    }
}

fn base64_value(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for group in bytes.chunks(4) {
        let mut v = 0u32;
        for &c in group {
            v = (v << 6) | base64_value(c)?;
        }
        out.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
    }
    Some(out)
}

/// Decodes a slideshow folder as Windows stores it (`slideshow.ini` `ImagesRootPIDL`,
/// `SlideshowDirectoryPath<n>`): base64 written back to front, of a little-endian `u16`
/// length followed by an ITEMIDLIST, the whole thing byte-reversed. Returns the ID list,
/// up to and including its terminating zero `cb`.
pub fn decode_slideshow_pidl(s: &str) -> Option<Vec<u8>> {
    let mut reversed: String = s.trim().chars().rev().collect();
    // Reversing moved the short final group to the front: pad it there.
    let pad = (4 - reversed.len() % 4) % 4;
    reversed.insert_str(0, &"A".repeat(pad));
    let mut raw = base64_decode(&reversed)?;
    raw.reverse();
    let body = raw.get(2..)?;
    // Walk the SHITEMIDs to the zero terminator.
    let mut at = 0usize;
    loop {
        let cb = u16::from_le_bytes([*body.get(at)?, *body.get(at + 1)?]) as usize;
        if cb == 0 {
            return Some(body[..at + 2].to_vec());
        }
        if cb < 2 {
            return None;
        }
        at += cb;
    }
}

/// The picture types Windows' slideshow shows.
pub fn is_slideshow_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "jfif" | "png" | "bmp" | "gif" | "tif" | "tiff")
    })
}

/// The slideshow's next picture: the one after `current` in name order (wrapping), or a
/// different one picked with `random` (any number) when shuffling. `images` needn't be sorted.
pub fn next_slide(images: &[PathBuf], current: Option<&Path>, shuffle: bool, random: u64) -> Option<PathBuf> {
    let mut sorted: Vec<&PathBuf> = images.iter().collect();
    sorted.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    let pos = current.and_then(|c| sorted.iter().position(|p| p.as_path() == c));
    let next = if shuffle && n > 1 {
        // Any picture but the current one.
        let mut i = (random % (n as u64 - u64::from(pos.is_some()))) as usize;
        if let Some(p) = pos
            && i >= p
        {
            i += 1;
        }
        i
    } else {
        pos.map_or(0, |p| (p + 1) % n)
    };
    Some(sorted[next].clone())
}

/// One picture from the Spotlight feed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpotlightItem {
    pub image_url: String,
    pub title: String,
    pub copyright: String,
    pub description: String,
    /// "Learn more" (a web page), without Windows' `microsoft-edge:` prefix.
    pub learn_more: Option<String>,
}

/// The pictures in a Spotlight feed response (`batchrsp.items[].item`, each a JSON string).
pub fn parse_spotlight(json: &str) -> Vec<SpotlightItem> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    let Some(items) = v.pointer("/batchrsp/items").and_then(|i| i.as_array()) else { return vec![] };
    items
        .iter()
        .filter_map(|i| serde_json::from_str::<serde_json::Value>(i.get("item")?.as_str()?).ok())
        .filter_map(|item| {
            let ad = item.get("ad")?;
            let text = |k: &str| ad.get(k).and_then(|v| v.as_str()).unwrap_or_default().trim().to_owned();
            let image_url = ad.pointer("/landscapeImage/asset")?.as_str()?.to_owned();
            let learn_more = ad
                .get("ctaUri")
                .and_then(|v| v.as_str())
                .map(|u| u.strip_prefix("microsoft-edge:").unwrap_or(u).to_owned())
                .filter(|u| u.starts_with("https://"));
            Some(SpotlightItem { image_url, title: text("title"), copyright: text("copyright"), description: text("description"), learn_more })
        })
        .collect()
}

/// The feed Windows' desktop Spotlight reads, for a locale such as `en-GB`.
pub fn spotlight_url(locale: &str) -> String {
    let country = locale.rsplit_once('-').map_or("US", |(_, c)| c);
    format!("https://fd.api.iris.microsoft.com/v4/api/selection?&placement=88000820&bcnt=4&country={country}&locale={locale}&fmt=json")
}

/// A file name for a downloaded Spotlight picture: the URL's last path segment, reduced
/// to safe characters.
pub fn spotlight_file_name(url: &str) -> String {
    let last = url.rsplit('/').next().unwrap_or("spotlight.jpg");
    let last = last.split(['?', '#']).next().unwrap_or(last);
    let mut name: String = last.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
    if !name.to_ascii_lowercase().ends_with(".jpg") && !name.to_ascii_lowercase().ends_with(".png") {
        name.push_str(".jpg");
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    // This PC's slideshow.ini ImagesRootPIDL (C:\Windows\Web\Wallpaper\ThemeB).
    const ROOT: &str = "NHAFA8BUg/E0gouOpBhoYjAArADMdmBAvMkOcBAAAAAAAAAAAAAAAAAAAAAAAAgVAEDAAAAAA4NX2tHEAcVauR2b3NHAABQCAQAAv7bgYlqOezld75CAAAwFOAAAAAQAAAAAAAAAAAAAAAAAAAAAdYJiAcFApBgbAQGAvBwdAMHAAAgFAoEAxAAAAAAABilT8ABAXVmYAgDAJAABA8uvBiVR74NXMsnLAAAAGLCAAAAABAAAAAAAAAAAAAAAAAAAAQwHsBwVAUGAiBAAAIBAcBQMAAAAAAQgYpRQQAwVhxGbwFGclJHAEBQCAQAAv7bgYV0OezFD75CAAAAziAAAAAQAAAAAAAAAAAAAAAAAAAAAIfY+AcFAhBAbAwGAwBQYAAHAlBgcAAAAYAgoAEDAAAAAAEIWMtTEAQFal1WZCBAAMCQCAQAAv7bgYV0OezlT85CAAAA0iAAAAAQAAAAAAAAAAAAA8AAAAAAAfl6BBQFAoBQZA0GAlBgQAAAAABwQAoDAcBwVAkEAOBARA8EAXBwUAwFATBQeAMHA0BQZA0GAzAgMAwFA0BAaAUGAtBQZAUHApBgLAQGAsBAbAwCAtAgMAEDAxAAOAAAAWAAAAA";

    #[test]
    fn decodes_slideshow_folders() {
        let pidl = decode_slideshow_pidl(ROOT).unwrap();
        // First item: This PC ({20D04FE0-3AEA-1069-A2D8-08002B30309D}), 0x14 bytes.
        assert_eq!(&pidl[..6], &[0x14, 0x00, 0x1f, 0x50, 0xe0, 0x4f]);
        assert_eq!(&pidl[pidl.len() - 2..], &[0, 0], "ends with the terminator");
        let text: String = String::from_utf16_lossy(&pidl.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect::<Vec<_>>());
        assert!(text.contains("ThemeB") || String::from_utf8_lossy(&pidl).contains("ThemeB"));
        assert_eq!(decode_slideshow_pidl("not base64!"), None);
        assert_eq!(decode_slideshow_pidl(""), None);
    }

    #[test]
    fn slides_advance_in_name_order_and_wrap() {
        let imgs: Vec<PathBuf> = ["c.jpg", "A.jpg", "b.png"].iter().map(PathBuf::from).collect();
        assert_eq!(next_slide(&imgs, None, false, 0), Some(PathBuf::from("A.jpg")));
        assert_eq!(next_slide(&imgs, Some(Path::new("A.jpg")), false, 0), Some(PathBuf::from("b.png")));
        assert_eq!(next_slide(&imgs, Some(Path::new("c.jpg")), false, 0), Some(PathBuf::from("A.jpg")));
        assert_eq!(next_slide(&imgs, Some(Path::new("gone.jpg")), false, 0), Some(PathBuf::from("A.jpg")));
        assert_eq!(next_slide(&[], None, false, 0), None);
    }

    #[test]
    fn shuffle_never_repeats_the_current_slide() {
        let imgs: Vec<PathBuf> = ["a.jpg", "b.jpg", "c.jpg"].iter().map(PathBuf::from).collect();
        for r in 0..20 {
            let next = next_slide(&imgs, Some(Path::new("b.jpg")), true, r).unwrap();
            assert_ne!(next, PathBuf::from("b.jpg"));
        }
        let one = vec![PathBuf::from("only.jpg")];
        assert_eq!(next_slide(&one, Some(Path::new("only.jpg")), true, 7), Some(PathBuf::from("only.jpg")));
    }

    #[test]
    fn image_types() {
        assert!(is_slideshow_image(Path::new("x.JPG")));
        assert!(is_slideshow_image(Path::new("x.png")));
        assert!(!is_slideshow_image(Path::new("x.txt")));
        assert!(!is_slideshow_image(Path::new("TranscodedWallpaper")));
    }

    #[test]
    fn parses_the_spotlight_feed() {
        let items = parse_spotlight(include_str!("../testdata/spotlight-sample.json"));
        assert_eq!(items.len(), 4);
        let first = &items[0];
        assert!(first.image_url.starts_with("https://") && first.image_url.ends_with(".jpg"));
        assert!(first.image_url.contains("3840x2160"), "the landscape picture");
        assert_eq!(first.title, "A fleeting oasis");
        assert!(first.copyright.starts_with('©'));
        assert!(first.learn_more.as_deref().is_some_and(|u| u.starts_with("https://www.bing.com/")));
        assert!(parse_spotlight("{}").is_empty());
        assert!(parse_spotlight("garbage").is_empty());
    }

    #[test]
    fn spotlight_urls_and_names() {
        assert!(spotlight_url("en-GB").contains("country=GB&locale=en-GB"));
        assert!(spotlight_url("en").contains("country=US"));
        assert_eq!(spotlight_file_name("https://x/y/abc_3840x2160.jpg?x=1"), "abc_3840x2160.jpg");
        assert_eq!(spotlight_file_name("https://x/y/we!rd"), "we_rd.jpg");
    }
}
