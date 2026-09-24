//! Pure helpers for the audio and media services: names and volume arithmetic.

/// Core Audio's 0.0–1.0 scalar as the 0–100 the widget API uses (whole percent, like
/// Windows shows it).
pub fn to_percent(scalar: f32) -> f64 {
    (f64::from(scalar.clamp(0.0, 1.0)) * 100.0).round()
}

pub fn from_percent(percent: f64) -> f32 {
    (percent.clamp(0.0, 100.0) / 100.0) as f32
}

/// Volume after `notches` steps of `step` percent (media keys, scrolling), snapped to the
/// step grid so repeated presses land on round numbers.
pub fn step(percent: f64, notches: i32, step: f64) -> f64 {
    let step = step.clamp(1.0, 100.0);
    (((percent / step).round() + f64::from(notches)) * step).clamp(0.0, 100.0)
}

/// What to call a per-app session: the name the app gave its session, else the
/// executable's description ("Mozilla Firefox"), else the executable's name.
pub fn app_name(pid: u32, display_name: &str, description: &str, exe: &str) -> String {
    if pid == 0 {
        return "System sounds".to_owned();
    }
    for candidate in [display_name, description] {
        let c = candidate.trim();
        if !c.is_empty() {
            return c.to_owned();
        }
    }
    let stem = std::path::Path::new(exe).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if stem.is_empty() { format!("Process {pid}") } else { capitalise(&stem) }
}

/// A readable name for a media session's AppUserModelID when the app index doesn't know it:
/// `SpotifyAB.SpotifyMusic_…!Spotify` → "Spotify", `chrome.exe` → "Chrome".
pub fn media_app_name(app_id: &str) -> String {
    let id = app_id.rsplit('!').next().unwrap_or(app_id);
    let id = id.strip_suffix(".exe").or_else(|| id.strip_suffix(".EXE")).unwrap_or(id);
    let id = id.rsplit(['\\', '/']).next().unwrap_or(id);
    let id = id.rsplit('.').next().unwrap_or(id);
    capitalise(id)
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_round_trip() {
        assert_eq!(to_percent(0.4), 40.0);
        assert_eq!(to_percent(0.456), 46.0);
        assert_eq!(to_percent(1.7), 100.0);
        assert!((from_percent(40.0) - 0.4).abs() < 1e-6);
        assert_eq!(from_percent(-5.0), 0.0);
        assert_eq!(from_percent(250.0), 1.0);
    }

    #[test]
    fn steps_snap_and_clamp() {
        assert_eq!(step(40.0, 1, 5.0), 45.0);
        assert_eq!(step(43.0, 1, 5.0), 50.0, "43 snaps to 45, then one step up");
        assert_eq!(step(43.0, -1, 5.0), 40.0);
        assert_eq!(step(98.0, 1, 5.0), 100.0);
        assert_eq!(step(1.0, -3, 5.0), 0.0);
        assert_eq!(step(50.0, 2, 0.0), 52.0, "a zero step is treated as 1");
    }

    #[test]
    fn app_names() {
        assert_eq!(app_name(0, "", "", ""), "System sounds");
        assert_eq!(app_name(5, "Spotify", "Spotify Music", "C:\\x\\Spotify.exe"), "Spotify");
        assert_eq!(app_name(5, " ", "Overwatch", "C:\\x\\Overwatch.exe"), "Overwatch");
        assert_eq!(app_name(5, "", "", "C:\\x\\discord.exe"), "Discord");
        assert_eq!(app_name(5, "", "", ""), "Process 5");
    }

    #[test]
    fn media_app_names() {
        assert_eq!(media_app_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
        assert_eq!(media_app_name("chrome.exe"), "Chrome");
        assert_eq!(media_app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "ZuneMusic");
        assert_eq!(media_app_name(r"C:\Program Files\VideoLAN\VLC\vlc.exe"), "Vlc");
        assert_eq!(media_app_name(""), "");
    }
}
