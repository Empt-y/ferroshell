//! Change detection for hot reload by polling a fingerprint of file paths, sizes and
//! modification times. Polling a handful of small directories once a second is cheap and
//! has none of the edge cases of OS change notifications (editors that replace files,
//! network drives, directories created after startup).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

const MAX_DEPTH: usize = 6;
const MAX_FILES: usize = 5000;

pub fn fingerprint<P: AsRef<Path>>(roots: &[P]) -> u64 {
    let mut h = DefaultHasher::new();
    let mut budget = MAX_FILES;
    for root in roots {
        visit(root.as_ref(), 0, &mut h, &mut budget);
    }
    h.finish()
}

fn visit(path: &Path, depth: usize, h: &mut DefaultHasher, budget: &mut usize) {
    if *budget == 0 {
        return;
    }
    *budget -= 1;
    let Ok(meta) = std::fs::metadata(path) else {
        path.hash(h);
        0u8.hash(h);
        return;
    };
    path.hash(h);
    meta.len().hash(h);
    meta.modified().ok().hash(h);
    if meta.is_dir() && depth < MAX_DEPTH {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        let mut children: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        children.sort();
        for c in children {
            visit(&c, depth + 1, h, budget);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_changes_additions_and_removals() {
        let dir = std::env::temp_dir().join(format!("fsh-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let f = || fingerprint(&[&dir]);
        let missing = f();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let empty = f();
        assert_ne!(missing, empty);
        std::fs::write(dir.join("sub").join("a.slint"), "x").unwrap();
        let one = f();
        assert_ne!(empty, one);
        assert_eq!(one, f(), "stable when nothing changes");
        std::fs::write(dir.join("sub").join("a.slint"), "xy").unwrap();
        assert_ne!(one, f());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(missing, f());
    }
}
