//! The notification-area icon list, built from `Shell_NotifyIcon` commands. Pure logic:
//! icon pixels are stored elsewhere under [`TrayIcon::icon_key`].

use fsh_win::tray::{NIF_ICON, NIF_MESSAGE, NIF_STATE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIS_HIDDEN, NotifyCommand};

/// An icon is identified by its GUID if it has one, else by (owner window, id).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TrayKey {
    Guid([u8; 16]),
    Id(isize, u32),
}

impl TrayKey {
    pub fn of(cmd: &NotifyCommand) -> Self {
        match cmd.guid {
            Some(g) => TrayKey::Guid(g),
            None => TrayKey::Id(cmd.owner, cmd.uid),
        }
    }

    /// Stable text form, used as the id the UI hands back on clicks.
    pub fn id(&self) -> String {
        match self {
            TrayKey::Guid(g) => g.iter().map(|b| format!("{b:02x}")).collect(),
            TrayKey::Id(h, u) => format!("{h}:{u}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayIcon {
    pub key: TrayKey,
    pub owner: isize,
    pub uid: u32,
    pub callback: u32,
    pub version: u32,
    pub tip: String,
    pub hidden: bool,
    /// Changes whenever the icon image changes.
    pub icon_rev: u64,
}

impl TrayIcon {
    pub fn icon_key(&self) -> String {
        format!("tray:{}#{}", self.key.id(), self.icon_rev)
    }
}

#[derive(Debug, Default)]
pub struct TrayModel {
    icons: Vec<TrayIcon>,
    rev: u64,
}

impl TrayModel {
    /// Apply one command. Returns true if anything visible changed.
    pub fn apply(&mut self, cmd: &NotifyCommand) -> bool {
        let key = TrayKey::of(cmd);
        let pos = self.icons.iter().position(|i| i.key == key);
        match (cmd.message, pos) {
            (NIM_DELETE, Some(p)) => {
                self.icons.remove(p);
                true
            }
            (NIM_DELETE, None) => false,
            // Re-adding (e.g. after a TaskbarCreated broadcast) updates in place.
            (NIM_ADD | NIM_MODIFY, Some(p)) => {
                self.rev += 1;
                update(&mut self.icons[p], cmd, self.rev);
                true
            }
            (NIM_ADD, None) => {
                self.rev += 1;
                let mut icon = TrayIcon {
                    key,
                    owner: cmd.owner,
                    uid: cmd.uid,
                    callback: 0,
                    version: 0,
                    tip: String::new(),
                    hidden: false,
                    icon_rev: self.rev,
                };
                update(&mut icon, cmd, self.rev);
                self.icons.push(icon);
                true
            }
            (NIM_SETVERSION, Some(p)) => {
                self.icons[p].version = cmd.version;
                false
            }
            _ => false,
        }
    }

    /// Drop icons whose owner window no longer exists (the app crashed without removing
    /// them). Returns true if any were removed.
    pub fn remove_dead(&mut self, alive: impl Fn(isize) -> bool) -> bool {
        let before = self.icons.len();
        self.icons.retain(|i| alive(i.owner));
        self.icons.len() != before
    }

    pub fn icons(&self) -> &[TrayIcon] {
        &self.icons
    }

    pub fn find(&self, id: &str) -> Option<&TrayIcon> {
        self.icons.iter().find(|i| i.key.id() == id)
    }
}

fn update(icon: &mut TrayIcon, cmd: &NotifyCommand, rev: u64) {
    if cmd.owner != 0 {
        icon.owner = cmd.owner;
    }
    icon.uid = cmd.uid;
    if cmd.flags & NIF_MESSAGE != 0 {
        icon.callback = cmd.callback;
    }
    if cmd.flags & NIF_TIP != 0 {
        icon.tip = cmd.tip.clone();
    }
    if cmd.flags & NIF_ICON != 0 {
        icon.icon_rev = rev;
    }
    if cmd.flags & NIF_STATE != 0 && cmd.state_mask & NIS_HIDDEN != 0 {
        icon.hidden = cmd.state & NIS_HIDDEN != 0;
    }
}

/// Should an icon be hidden by the user's `hidden` patterns? Each pattern matches
/// (case-insensitively, as a substring) the tooltip or the owning program's file name.
pub fn matches_hidden(patterns: &[String], tip: &str, exe_name: &str) -> bool {
    let (tip, exe) = (tip.to_lowercase(), exe_name.to_lowercase());
    patterns.iter().map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).any(|p| tip.contains(&p) || exe.contains(&p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(message: u32, owner: isize, uid: u32, flags: u32) -> NotifyCommand {
        NotifyCommand {
            message,
            owner,
            uid,
            flags,
            callback: 0x8000,
            icon: 1,
            tip: "tip".into(),
            state: 0,
            state_mask: 0,
            version: 0,
            guid: None,
        }
    }

    #[test]
    fn add_modify_delete_lifecycle() {
        let mut m = TrayModel::default();
        assert!(m.apply(&cmd(NIM_ADD, 10, 1, NIF_MESSAGE | NIF_ICON | NIF_TIP)));
        let rev0 = m.icons()[0].icon_rev;
        assert_eq!((m.icons()[0].callback, m.icons()[0].tip.as_str()), (0x8000, "tip"));

        let mut tip_only = cmd(NIM_MODIFY, 10, 1, NIF_TIP);
        tip_only.tip = "new".into();
        assert!(m.apply(&tip_only));
        assert_eq!(m.icons()[0].tip, "new");
        assert_eq!(m.icons()[0].icon_rev, rev0, "icon unchanged without NIF_ICON");
        assert!(m.apply(&cmd(NIM_MODIFY, 10, 1, NIF_ICON)));
        assert_ne!(m.icons()[0].icon_rev, rev0);

        assert!(!m.apply(&cmd(NIM_MODIFY, 99, 1, NIF_TIP)), "modify of unknown icon is ignored");
        let mut v = cmd(NIM_SETVERSION, 10, 1, 0);
        v.version = 4;
        m.apply(&v);
        assert_eq!(m.icons()[0].version, 4);

        assert!(m.apply(&cmd(NIM_DELETE, 10, 1, 0)));
        assert!(m.icons().is_empty());
    }

    #[test]
    fn readding_keeps_order_and_guid_identity() {
        let mut m = TrayModel::default();
        m.apply(&cmd(NIM_ADD, 10, 1, NIF_TIP));
        let mut g = cmd(NIM_ADD, 20, 5, NIF_TIP);
        g.guid = Some([7; 16]);
        m.apply(&g);
        m.apply(&cmd(NIM_ADD, 10, 1, NIF_TIP));
        assert_eq!(m.icons().len(), 2);
        // A GUID icon can move to a new window and stays the same icon.
        g.message = NIM_MODIFY;
        g.owner = 30;
        m.apply(&g);
        assert_eq!(m.icons()[1].owner, 30);
        assert_eq!(m.find(&TrayKey::Guid([7; 16]).id()).unwrap().uid, 5);
    }

    #[test]
    fn hidden_state_and_dead_owners() {
        let mut m = TrayModel::default();
        let mut c = cmd(NIM_ADD, 10, 1, NIF_STATE);
        c.state = NIS_HIDDEN;
        c.state_mask = NIS_HIDDEN;
        m.apply(&c);
        assert!(m.icons()[0].hidden);
        m.apply(&cmd(NIM_ADD, 11, 1, 0));
        assert!(m.remove_dead(|h| h == 11));
        assert_eq!(m.icons().len(), 1);
        assert_eq!(m.icons()[0].owner, 11);
    }

    #[test]
    fn hidden_patterns() {
        let p = vec!["onedrive".to_owned(), " ".to_owned()];
        assert!(matches_hidden(&p, "", "OneDrive.exe"));
        assert!(matches_hidden(&p, "OneDrive - Personal", "x.exe"));
        assert!(!matches_hidden(&p, "Volume", "explorer.exe"));
    }
}
