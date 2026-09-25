//! Desktop icons: the grid, where each icon goes (auto-arranged or where the user put it),
//! selection, and keyboard navigation. Pure; the shell draws and handles the rest.

use std::collections::{HashMap, HashSet};

/// A cell on the icon grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Cell {
    pub col: u32,
    pub row: u32,
}

/// Icon sizes Windows offers on the desktop (View > Small/Medium/Large icons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSize {
    Small,
    Medium,
    Large,
}

impl IconSize {
    /// From Windows' `IconSize` (pixels) in the desktop's view settings.
    pub fn from_pixels(px: u32) -> Self {
        match px {
            0..=40 => Self::Small,
            41..=72 => Self::Medium,
            _ => Self::Large,
        }
    }

    pub fn pixels(self) -> u32 {
        match self {
            Self::Small => 32,
            Self::Medium => 48,
            Self::Large => 96,
        }
    }

    /// Grid cell size in logical pixels (icon plus up to two lines of label).
    pub fn cell(self) -> (f64, f64) {
        match self {
            Self::Small => (76.0, 76.0),
            Self::Medium => (84.0, 98.0),
            Self::Large => (132.0, 148.0),
        }
    }
}

/// The icon grid over a monitor's work area (logical pixels, relative to the desktop
/// window's top-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub origin_x: f64,
    pub origin_y: f64,
    pub cell_w: f64,
    pub cell_h: f64,
    pub cols: u32,
    pub rows: u32,
}

const MARGIN: f64 = 4.0;

impl Grid {
    /// The grid for a work area of `width` × `height` at (`x`, `y`) within the window.
    pub fn new(x: f64, y: f64, width: f64, height: f64, size: IconSize) -> Self {
        let (cell_w, cell_h) = size.cell();
        let cols = (((width - 2.0 * MARGIN) / cell_w).floor() as u32).max(1);
        let rows = (((height - 2.0 * MARGIN) / cell_h).floor() as u32).max(1);
        Self { origin_x: x + MARGIN, origin_y: y + MARGIN, cell_w, cell_h, cols, rows }
    }

    pub fn capacity(&self) -> usize {
        (self.cols * self.rows) as usize
    }

    /// The top-left of a cell.
    pub fn position(&self, c: Cell) -> (f64, f64) {
        (self.origin_x + f64::from(c.col) * self.cell_w, self.origin_y + f64::from(c.row) * self.cell_h)
    }

    /// The cell under a point, clamped to the grid.
    pub fn cell_at(&self, x: f64, y: f64) -> Cell {
        let clamp = |v: f64, n: u32| (v.max(0.0).floor() as u32).min(n - 1);
        Cell {
            col: clamp((x - self.origin_x) / self.cell_w, self.cols),
            row: clamp((y - self.origin_y) / self.cell_h, self.rows),
        }
    }

    pub fn contains(&self, c: Cell) -> bool {
        c.col < self.cols && c.row < self.rows
    }

    /// Cells in the order Windows fills them: down each column, then the next column.
    pub fn fill_order(&self) -> impl Iterator<Item = Cell> + '_ {
        (0..self.cols).flat_map(move |col| (0..self.rows).map(move |row| Cell { col, row }))
    }
}

/// Where every item goes. `items` are keys in display order (the sort order). Saved
/// positions are kept when they're on the grid and not taken; everything else (and
/// everything, with `auto_arrange`) fills free cells column by column. Items that don't
/// fit are stacked in the last cell, as Windows does.
pub fn place(items: &[String], saved: &HashMap<String, Cell>, grid: &Grid, auto_arrange: bool) -> Vec<Cell> {
    let mut taken: HashSet<Cell> = HashSet::new();
    let mut out: Vec<Option<Cell>> = vec![None; items.len()];
    if !auto_arrange {
        for (i, key) in items.iter().enumerate() {
            if let Some(&c) = saved.get(key)
                && grid.contains(c)
                && taken.insert(c)
            {
                out[i] = Some(c);
            }
        }
    }
    let mut free = grid.fill_order().filter(|c| !taken.contains(c)).collect::<Vec<_>>().into_iter();
    let last = Cell { col: grid.cols - 1, row: grid.rows - 1 };
    out.into_iter().map(|c| c.unwrap_or_else(|| free.next().unwrap_or(last))).collect()
}

/// Moving items by dragging: `moved` (indices) shift by the same number of cells as the
/// dragged item, clamped to the grid; an occupied target cell bumps the moved item to the
/// nearest free cell after it in fill order.
pub fn move_items(cells: &[Cell], moved: &[usize], from: Cell, to: Cell, grid: &Grid) -> Vec<Cell> {
    let dc = i64::from(to.col) - i64::from(from.col);
    let dr = i64::from(to.row) - i64::from(from.row);
    let moving: HashSet<usize> = moved.iter().copied().collect();
    let mut out = cells.to_vec();
    let mut taken: HashSet<Cell> = cells.iter().enumerate().filter(|(i, _)| !moving.contains(i)).map(|(_, c)| *c).collect();
    let order: Vec<Cell> = grid.fill_order().collect();
    for &i in moved {
        let Some(c) = cells.get(i) else { continue };
        let target = Cell {
            col: (i64::from(c.col) + dc).clamp(0, i64::from(grid.cols) - 1) as u32,
            row: (i64::from(c.row) + dr).clamp(0, i64::from(grid.rows) - 1) as u32,
        };
        let cell = if taken.contains(&target) {
            let start = order.iter().position(|x| *x == target).unwrap_or(0);
            order[start..].iter().chain(order[..start].iter()).copied().find(|x| !taken.contains(x)).unwrap_or(target)
        } else {
            target
        };
        taken.insert(cell);
        out[i] = cell;
    }
    out
}

/// Which item an arrow key moves to from `current`: the nearest item in that direction
/// (by grid distance, preferring the same row/column).
pub fn navigate(cells: &[Cell], current: Option<usize>, dir: Direction) -> Option<usize> {
    let Some(cur) = current.and_then(|i| cells.get(i).map(|c| (i, *c))) else {
        // Nothing selected yet: the first item in fill order.
        return cells.iter().enumerate().min_by_key(|(_, c)| (c.col, c.row)).map(|(i, _)| i);
    };
    let (ci, c) = cur;
    let (cc, cr) = (i64::from(c.col), i64::from(c.row));
    cells
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != ci)
        .filter_map(|(i, o)| {
            let (dc, dr) = (i64::from(o.col) - cc, i64::from(o.row) - cr);
            let (along, across) = match dir {
                Direction::Up => (-dr, dc),
                Direction::Down => (dr, dc),
                Direction::Left => (-dc, dr),
                Direction::Right => (dc, dr),
            };
            (along > 0).then_some((i, (across.abs() * 100 + along, along)))
        })
        .min_by_key(|(_, k)| *k)
        .map(|(i, _)| i)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Items whose cells overlap a rubber-band rectangle (logical pixels, any corner order).
pub fn in_rectangle(cells: &[Cell], grid: &Grid, a: (f64, f64), b: (f64, f64)) -> Vec<usize> {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    cells
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            let (x, y) = grid.position(**c);
            // The icon and label occupy the middle of the cell.
            let (ix0, ix1, iy0, iy1) = (x + 8.0, x + grid.cell_w - 8.0, y + 2.0, y + grid.cell_h - 8.0);
            ix0 < x1 && ix1 > x0 && iy0 < y1 && iy1 > y0
        })
        .map(|(i, _)| i)
        .collect()
}

/// A click on item `i`: plain selects only it, Ctrl toggles it, Shift selects the range
/// from `anchor` (in display order).
pub fn click(selected: &HashSet<usize>, anchor: Option<usize>, i: usize, ctrl: bool, shift: bool) -> HashSet<usize> {
    match (ctrl, shift, anchor) {
        (_, true, Some(a)) => {
            let (lo, hi) = (a.min(i), a.max(i));
            let mut s: HashSet<usize> = if ctrl { selected.clone() } else { HashSet::new() };
            s.extend(lo..=hi);
            s
        }
        (true, _, _) => {
            let mut s = selected.clone();
            if !s.remove(&i) {
                s.insert(i);
            }
            s
        }
        _ => HashSet::from([i]),
    }
}

/// How items are sorted (View's "Sort by").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Size,
    Type,
    Modified,
}

/// What sorting needs to know about an item.
#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    pub name: String,
    /// Virtual items (This PC, Recycle Bin…) come first, then folders, then files.
    pub rank: u8,
    pub size: u64,
    pub type_name: String,
    pub modified: i64,
}

/// Display order for the given keys.
pub fn sort_order(keys: &[SortKey], by: SortBy) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..keys.len()).collect();
    idx.sort_by(|&a, &b| {
        let (ka, kb) = (&keys[a], &keys[b]);
        let name = || ka.name.to_lowercase().cmp(&kb.name.to_lowercase());
        ka.rank.cmp(&kb.rank).then_with(|| match by {
            SortBy::Name => name(),
            SortBy::Size => ka.size.cmp(&kb.size).then_with(name),
            SortBy::Type => ka.type_name.to_lowercase().cmp(&kb.type_name.to_lowercase()).then_with(name),
            SortBy::Modified => kb.modified.cmp(&ka.modified).then_with(name),
        })
    });
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("item{i}")).collect()
    }

    fn c(col: u32, row: u32) -> Cell {
        Cell { col, row }
    }

    #[test]
    fn grid_fits_the_work_area() {
        let g = Grid::new(0.0, 0.0, 1536.0, 816.0, IconSize::Medium);
        assert_eq!((g.cols, g.rows), (18, 8));
        assert_eq!(g.position(c(0, 0)), (4.0, 4.0));
        assert_eq!(g.position(c(1, 2)), (88.0, 200.0));
        assert_eq!(g.cell_at(90.0, 205.0), c(1, 2));
        assert_eq!(g.cell_at(-50.0, 99999.0), c(0, 7), "clamped");
        let tiny = Grid::new(0.0, 0.0, 10.0, 10.0, IconSize::Large);
        assert_eq!((tiny.cols, tiny.rows), (1, 1));
    }

    #[test]
    fn auto_arrange_fills_columns_top_to_bottom() {
        let g = Grid::new(0.0, 0.0, 300.0, 250.0, IconSize::Medium); // 3 cols × 2 rows
        let cells = place(&keys(4), &HashMap::new(), &g, true);
        assert_eq!(cells, vec![c(0, 0), c(0, 1), c(1, 0), c(1, 1)]);
    }

    #[test]
    fn saved_positions_are_kept_and_others_fill_around_them() {
        let g = Grid::new(0.0, 0.0, 300.0, 250.0, IconSize::Medium);
        let saved = HashMap::from([("item1".to_string(), c(0, 0)), ("item2".to_string(), c(9, 9))]);
        let cells = place(&keys(3), &saved, &g, false);
        assert_eq!(cells, vec![c(0, 1), c(0, 0), c(1, 0)], "off-grid saved position is ignored");
        // Auto-arrange ignores saved positions.
        assert_eq!(place(&keys(3), &saved, &g, true), vec![c(0, 0), c(0, 1), c(1, 0)]);
    }

    #[test]
    fn overflow_stacks_in_the_last_cell() {
        let g = Grid::new(0.0, 0.0, 100.0, 110.0, IconSize::Medium); // 1 × 1
        assert_eq!(place(&keys(3), &HashMap::new(), &g, true), vec![c(0, 0), c(0, 0), c(0, 0)]);
    }

    #[test]
    fn dragging_moves_the_selection_together_and_avoids_collisions() {
        let g = Grid::new(0.0, 0.0, 500.0, 500.0, IconSize::Medium); // 5 × 5
        let cells = vec![c(0, 0), c(0, 1), c(2, 2)];
        let moved = move_items(&cells, &[0, 1], c(0, 0), c(3, 0), &g);
        assert_eq!(moved, vec![c(3, 0), c(3, 1), c(2, 2)]);
        // Onto an occupied cell: the next free one in fill order.
        let bumped = move_items(&cells, &[0], c(0, 0), c(2, 2), &g);
        assert_eq!(bumped[0], c(2, 3));
        // Clamped at the edge.
        assert_eq!(move_items(&cells, &[2], c(2, 2), c(9, 9), &g)[2], c(4, 4));
    }

    #[test]
    fn arrow_keys_find_the_nearest_item() {
        let cells = vec![c(0, 0), c(0, 1), c(1, 0), c(3, 1)];
        assert_eq!(navigate(&cells, None, Direction::Down), Some(0));
        assert_eq!(navigate(&cells, Some(0), Direction::Down), Some(1));
        assert_eq!(navigate(&cells, Some(0), Direction::Right), Some(2));
        assert_eq!(navigate(&cells, Some(1), Direction::Right), Some(3), "same row beats a nearer diagonal");
        assert_eq!(navigate(&cells, Some(0), Direction::Up), None);
    }

    #[test]
    fn rubber_band_selects_overlapping_icons() {
        let g = Grid::new(0.0, 0.0, 500.0, 500.0, IconSize::Medium);
        let cells = vec![c(0, 0), c(1, 0), c(3, 3)];
        assert_eq!(in_rectangle(&cells, &g, (170.0, 60.0), (10.0, 10.0)), vec![0, 1]);
        assert!(in_rectangle(&cells, &g, (0.0, 0.0), (2.0, 2.0)).is_empty());
    }

    #[test]
    fn clicks_with_modifiers() {
        let none = HashSet::new();
        assert_eq!(click(&none, None, 2, false, false), HashSet::from([2]));
        let s = HashSet::from([1, 2]);
        assert_eq!(click(&s, Some(1), 2, true, false), HashSet::from([1]));
        assert_eq!(click(&s, Some(1), 4, true, false), HashSet::from([1, 2, 4]));
        assert_eq!(click(&s, Some(1), 4, false, true), HashSet::from([1, 2, 3, 4]));
        assert_eq!(click(&HashSet::from([7]), Some(1), 3, true, true), HashSet::from([1, 2, 3, 7]));
    }

    #[test]
    fn sorting_puts_virtual_items_then_folders_first() {
        let k = |name: &str, rank, size, t: &str, m| SortKey { name: name.into(), rank, size, type_name: t.into(), modified: m };
        let keys = vec![k("zeta.txt", 2, 5, "Text", 3), k("Recycle Bin", 0, 0, "", 0), k("Alpha", 1, 0, "Folder", 1), k("beta.mp4", 2, 50, "Video", 9)];
        assert_eq!(sort_order(&keys, SortBy::Name), vec![1, 2, 3, 0]);
        assert_eq!(sort_order(&keys, SortBy::Size), vec![1, 2, 0, 3]);
        assert_eq!(sort_order(&keys, SortBy::Modified), vec![1, 2, 3, 0], "newest first");
        assert_eq!(sort_order(&keys, SortBy::Type), vec![1, 2, 0, 3]);
    }
}

/// The new file name for a rename typed as `typed` over an item shown as `display` whose
/// file is `file_name`. When Explorer hides the extension (`Chrome` for `Chrome.lnk`), it's
/// kept. `None` for an empty name or one Windows can't store.
pub fn rename_target(file_name: &str, display: &str, typed: &str) -> Option<String> {
    let typed = typed.trim().trim_end_matches('.');
    if typed.is_empty() || typed.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c < ' ') {
        return None;
    }
    if display == file_name {
        return Some(typed.to_owned());
    }
    let hidden_ext = match file_name.strip_prefix(display) {
        Some(rest) if rest.starts_with('.') => rest,
        _ => file_name.rfind('.').map_or("", |i| &file_name[i..]),
    };
    Some(format!("{typed}{hidden_ext}"))
}

#[cfg(test)]
mod rename_tests {
    use super::rename_target;

    #[test]
    fn keeps_hidden_extensions() {
        assert_eq!(rename_target("Chrome.lnk", "Chrome", "Browser").as_deref(), Some("Browser.lnk"));
        assert_eq!(rename_target("notes.txt", "notes.txt", "todo.md").as_deref(), Some("todo.md"), "extension shown: as typed");
        assert_eq!(rename_target("Game", "Game", "Games").as_deref(), Some("Games"));
        assert_eq!(rename_target("setup.exe", "setup", " installer ").as_deref(), Some("installer.exe"));
        assert_eq!(rename_target("x.txt", "x", ""), None);
        assert_eq!(rename_target("x.txt", "x", "a/b"), None);
        assert_eq!(rename_target("x.txt", "x", "name."), Some("name.txt".into()), "trailing dots dropped");
    }
}
