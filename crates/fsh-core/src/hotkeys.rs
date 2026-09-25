//! The Windows-key shortcuts Explorer provides, which Ferroshell takes over when it's the
//! login shell, and refreshing the environment when Windows says it changed.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellHotkey {
    /// Win+E
    FileExplorer,
    /// Win+R
    Run,
    /// Win+D
    ShowDesktop,
    /// Win+M
    MinimiseAll,
    /// Win+Shift+M
    RestoreMinimised,
    /// Win+I
    Settings,
    /// Win+S, Win+Q
    Search,
    /// Win+N
    Notifications,
    /// Win+X
    QuickLinks,
    /// Win+Shift+S
    ScreenClip,
    /// Win+1 … Win+9: the nth task (0-based).
    Task(u8),
}

/// A Windows-key shortcut: `vk` is the virtual-key code, pressed with Win (and Shift).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub action: ShellHotkey,
    pub vk: u32,
    pub shift: bool,
}

const fn key(action: ShellHotkey, c: u8, shift: bool) -> Binding {
    Binding { action, vk: c as u32, shift }
}

/// Every shortcut, in a fixed order (a binding's index is its hotkey id).
pub fn bindings() -> Vec<Binding> {
    use ShellHotkey::*;
    let mut v = vec![
        key(FileExplorer, b'E', false),
        key(Run, b'R', false),
        key(ShowDesktop, b'D', false),
        key(MinimiseAll, b'M', false),
        key(RestoreMinimised, b'M', true),
        key(Settings, b'I', false),
        key(Search, b'S', false),
        key(Search, b'Q', false),
        key(Notifications, b'N', false),
        key(QuickLinks, b'X', false),
        key(ScreenClip, b'S', true),
    ];
    v.extend((0..9u8).map(|n| key(Task(n), b'1' + n, false)));
    v
}

/// The shortcut with this id (its index in [`bindings`]).
pub fn from_id(id: i32) -> Option<ShellHotkey> {
    usize::try_from(id).ok().and_then(|i| bindings().get(i).map(|b| b.action))
}

/// How to update this process's environment when the user environment moved from `old` to
/// `new` (both as Windows builds it from the registry): variables to set, and ones to remove
/// because they were deleted. Names compare case-insensitively, as on Windows; variables
/// the registry never had (set by whoever started us) are left alone.
pub fn environment_changes(old: &[(String, String)], new: &[(String, String)]) -> (Vec<(String, String)>, Vec<String>) {
    let index = |vars: &[(String, String)]| -> HashMap<String, String> {
        vars.iter().map(|(k, v)| (k.to_ascii_uppercase(), v.clone())).collect()
    };
    let old_map = index(old);
    let new_map = index(new);
    let set = new
        .iter()
        .filter(|(k, v)| !k.starts_with('=') && old_map.get(&k.to_ascii_uppercase()) != Some(v))
        .cloned()
        .collect();
    let remove = old
        .iter()
        .filter(|(k, _)| !k.starts_with('=') && !new_map.contains_key(&k.to_ascii_uppercase()))
        .map(|(k, _)| k.clone())
        .collect();
    (set, remove)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_are_unique() {
        let all = bindings();
        for (i, b) in all.iter().enumerate() {
            assert_eq!(from_id(i as i32), Some(b.action));
        }
        assert_eq!(from_id(-1), None);
        assert_eq!(from_id(all.len() as i32), None);
        let mut keys: Vec<(u32, bool)> = all.iter().map(|b| (b.vk, b.shift)).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), all.len(), "no key combination is bound twice");
    }

    #[test]
    fn number_keys_pick_tasks() {
        let all = bindings();
        let one = all.iter().find(|b| b.vk == u32::from(b'1')).unwrap();
        assert_eq!(one.action, ShellHotkey::Task(0));
        let nine = all.iter().find(|b| b.vk == u32::from(b'9')).unwrap();
        assert_eq!(nine.action, ShellHotkey::Task(8));
        assert!(all.iter().any(|b| b.vk == u32::from(b'S') && b.shift && b.action == ShellHotkey::ScreenClip));
    }

    fn vars(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn environment_diff() {
        let old = vars(&[("Path", r"C:\a"), ("TEMP", r"C:\t"), ("Gone", "1"), ("=C:", r"C:\")]);
        let new = vars(&[("PATH", r"C:\a;C:\b"), ("TEMP", r"C:\t"), ("NewVar", "x"), ("=D:", r"D:\")]);
        let (set, remove) = environment_changes(&old, &new);
        assert_eq!(set, vars(&[("PATH", r"C:\a;C:\b"), ("NewVar", "x")]));
        assert_eq!(remove, vec!["Gone".to_string()]);
        let (set, remove) = environment_changes(&new, &new);
        assert!(set.is_empty() && remove.is_empty());
    }
}
