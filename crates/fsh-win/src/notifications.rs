//! Other apps' notifications, through `UserNotificationListener`: read the notifications
//! currently in Windows' notification centre, and dismiss them.
//!
//! Only works in a process with package identity (see `packaging/identity/`) whose user has
//! allowed notification access. WinRT: use from a worker thread in a multi-threaded COM
//! apartment ([`crate::com::ComGuard::mta`]).

use windows::Foundation::Size;
use windows::Storage::Streams::DataReader;
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// No package identity: the listener isn't available to this process at all.
    NoIdentity,
    /// The user hasn't been asked yet.
    Unspecified,
    Allowed,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub id: u32,
    /// The sending app's AppUserModelID (what `shell:AppsFolder\<id>` launches).
    pub app_id: String,
    pub app_name: String,
    /// Seconds since the Unix epoch.
    pub created: i64,
    /// The first text line is the title; the rest the body.
    pub title: String,
    pub body: String,
}

fn listener() -> Option<UserNotificationListener> {
    UserNotificationListener::Current().ok()
}

pub fn access() -> Access {
    if crate::identity::package_full_name().is_none() {
        return Access::NoIdentity;
    }
    match listener().and_then(|l| l.GetAccessStatus().ok()) {
        Some(UserNotificationListenerAccessStatus::Allowed) => Access::Allowed,
        Some(UserNotificationListenerAccessStatus::Denied) => Access::Denied,
        Some(_) => Access::Unspecified,
        None => Access::NoIdentity,
    }
}

/// Ask the user for notification access (Windows shows its consent prompt the first time).
pub fn request_access() -> Access {
    if crate::identity::package_full_name().is_none() {
        return Access::NoIdentity;
    }
    match listener().and_then(|l| l.RequestAccessAsync().and_then(|op| op.join()).ok()) {
        Some(UserNotificationListenerAccessStatus::Allowed) => Access::Allowed,
        Some(UserNotificationListenerAccessStatus::Denied) => Access::Denied,
        _ => access(),
    }
}

/// Windows `DateTime` (100 ns ticks since 1601) as Unix seconds.
fn unix_seconds(ticks: i64) -> i64 {
    (ticks - 116_444_736_000_000_000) / 10_000_000
}

fn to_toast(n: &UserNotification) -> Option<Toast> {
    let app = n.AppInfo().ok();
    let texts: Vec<String> = n
        .Notification()
        .and_then(|x| x.Visual())
        .and_then(|v| v.GetBinding(&KnownNotificationBindings::ToastGeneric()?))
        .and_then(|b| b.GetTextElements())
        .map(|t| t.into_iter().filter_map(|e| e.Text().ok()).map(|s| s.to_string()).filter(|s| !s.trim().is_empty()).collect())
        .unwrap_or_default();
    Some(Toast {
        id: n.Id().ok()?,
        app_id: app.as_ref().and_then(|a| a.AppUserModelId().ok()).map(|s| s.to_string()).unwrap_or_default(),
        app_name: app.as_ref().and_then(|a| a.DisplayInfo().ok()).and_then(|d| d.DisplayName().ok()).map(|s| s.to_string()).unwrap_or_default(),
        created: n.CreationTime().map(|t| unix_seconds(t.UniversalTime)).unwrap_or_default(),
        title: texts.first().cloned().unwrap_or_default(),
        body: texts.iter().skip(1).cloned().collect::<Vec<_>>().join("\n"),
    })
}

/// The toasts currently in the notification centre. Empty without access.
pub fn toasts() -> Vec<Toast> {
    let Some(l) = listener() else { return vec![] };
    l.GetNotificationsAsync(NotificationKinds::Toast)
        .and_then(|op| op.join())
        .map(|list| list.into_iter().filter_map(|n| to_toast(&n)).collect())
        .unwrap_or_default()
}

/// Dismiss one notification from Windows' notification centre.
pub fn remove(id: u32) {
    if let Some(l) = listener() {
        let _ = l.RemoveNotification(id);
    }
}

/// Dismiss every notification from Windows' notification centre.
pub fn clear() {
    if let Some(l) = listener() {
        let _ = l.ClearNotifications();
    }
}

/// Raise a notification as this app (needs package identity). For testing the applet and
/// banners; the text is escaped into the toast XML.
pub fn send_test(title: &str, body: &str) -> windows::core::Result<()> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let xml = XmlDocument::new()?;
    xml.LoadXml(&windows::core::HSTRING::from(format!(
        "<toast><visual><binding template='ToastGeneric'><text>{}</text><text>{}</text></binding></visual></toast>",
        esc(title),
        esc(body)
    )))?;
    ToastNotificationManager::CreateToastNotifier()?.Show(&ToastNotification::CreateToastNotification(&xml)?)
}

/// The app's logo for notification `id` as encoded image bytes (usually PNG).
pub fn app_logo(id: u32, size: f32) -> Option<Vec<u8>> {
    let n = listener()?.GetNotification(id).ok()?;
    let reference = n.AppInfo().ok()?.DisplayInfo().ok()?.GetLogo(Size { Width: size, Height: size }).ok()?;
    let stream = reference.OpenReadAsync().and_then(|op| op.join()).ok()?;
    let len = u32::try_from(stream.Size().ok()?).ok().filter(|&n| n > 0 && n < 4 << 20)?;
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(len).and_then(|op| op.join()).ok()?;
    let mut buf = vec![0u8; len as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_time_to_unix() {
        // 2025-09-25T00:00:00Z
        assert_eq!(unix_seconds(134_032_320_000_000_000), 1_758_758_400);
        assert_eq!(unix_seconds(116_444_736_000_000_000), 0);
    }

    #[test]
    fn test_processes_have_no_access() {
        assert_eq!(access(), Access::NoIdentity);
    }
}
