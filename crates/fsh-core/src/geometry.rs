//! Where panels go. All results are in physical pixels.

use fsh_config::{Edge, MonitorSel};
use fsh_win::Rect;
use fsh_win::monitor::Monitor;

/// Physical pixels to reserve along the edge: the panel's thickness, plus the gap on both
/// sides of a floating panel.
pub fn reserve_px(thickness: u32, floating: bool, floating_margin: f32, scale: f32) -> i32 {
    let logical = thickness as f32 + if floating { 2.0 * floating_margin } else { 0.0 };
    (logical * scale).round() as i32
}

/// The panel window's rectangle inside the strip the app bar was granted.
pub fn window_rect(granted: Rect, floating: bool, floating_margin: f32, scale: f32) -> Rect {
    if !floating {
        return granted;
    }
    let m = (floating_margin * scale).round() as i32;
    let r = Rect::new(granted.left + m, granted.top + m, granted.right - m, granted.bottom - m);
    // Never produce an inverted rectangle, whatever the settings.
    if r.width() < 1 || r.height() < 1 { granted } else { r }
}

/// Which monitors a panel appears on, as indices into `monitors`.
pub fn select_monitors(sel: MonitorSel, monitors: &[Monitor]) -> Vec<usize> {
    if monitors.is_empty() {
        return vec![];
    }
    match sel {
        MonitorSel::All => (0..monitors.len()).collect(),
        MonitorSel::Primary => vec![monitors.iter().position(|m| m.primary).unwrap_or(0)],
        // A missing monitor (e.g. laptop undocked) means the panel isn't shown, not an error.
        MonitorSel::Index(i) => ((i as usize) < monitors.len()).then_some(i as usize).into_iter().collect(),
    }
}

pub fn appbar_edge(edge: Edge) -> fsh_win::appbar::Edge {
    match edge {
        Edge::Top => fsh_win::appbar::Edge::Top,
        Edge::Bottom => fsh_win::appbar::Edge::Bottom,
        Edge::Left => fsh_win::appbar::Edge::Left,
        Edge::Right => fsh_win::appbar::Edge::Right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(left: i32, primary: bool) -> Monitor {
        Monitor {
            handle: 0,
            device: String::new(),
            rect: Rect::new(left, 0, left + 1920, 1080),
            work: Rect::new(left, 0, left + 1920, 1080),
            primary,
            dpi: 120,
        }
    }

    #[test]
    fn reserve_scales_and_includes_floating_gaps() {
        assert_eq!(reserve_px(44, false, 6.0, 1.25), 55);
        assert_eq!(reserve_px(44, true, 6.0, 1.25), 70);
        assert_eq!(reserve_px(44, false, 6.0, 1.0), 44);
    }

    #[test]
    fn floating_window_is_inset() {
        let strip = Rect::new(0, 1010, 1920, 1080);
        assert_eq!(window_rect(strip, false, 6.0, 1.25), strip);
        assert_eq!(window_rect(strip, true, 6.0, 1.25), Rect::new(8, 1018, 1912, 1072));
        // Absurd margins fall back to the whole strip.
        assert_eq!(window_rect(strip, true, 500.0, 1.0), strip);
    }

    #[test]
    fn monitor_selection() {
        let ms = [mon(-1920, false), mon(0, true), mon(1920, false)];
        assert_eq!(select_monitors(MonitorSel::All, &ms), vec![0, 1, 2]);
        assert_eq!(select_monitors(MonitorSel::Primary, &ms), vec![1]);
        assert_eq!(select_monitors(MonitorSel::Index(2), &ms), vec![2]);
        assert_eq!(select_monitors(MonitorSel::Index(3), &ms), Vec::<usize>::new());
        assert_eq!(select_monitors(MonitorSel::Primary, &[]), Vec::<usize>::new());
    }
}
