//! Shell logic that doesn't depend on the UI toolkit, kept pure where possible so it can
//! be unit-tested without a desktop.

pub mod audio;
pub mod bluetooth;
pub mod calendar;
pub mod network;
pub mod notifications;
pub mod power;
pub mod session;
pub mod startup;
pub mod geometry;
pub mod hotkeys;
pub mod launcher;
pub mod tasks;
pub mod tray;
pub mod wallpaper;
