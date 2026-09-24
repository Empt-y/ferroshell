//! The widget system: built-in assets, widget package discovery, manifests with typed
//! config schemas, and the composer that turns a panel's widget list into a Slint
//! component via `slint-interpreter`. Built-in widgets go through exactly the same path as
//! user widgets, so anything built in can be overridden.

#![forbid(unsafe_code)]

pub mod assets;
pub mod composer;
pub mod manifest;
pub mod registry;
pub mod theme_binding;

pub use assets::Layout;
pub use composer::{Composer, PanelSpec, WidgetStatus};
pub use manifest::{ConfigField, FieldKind, Manifest, PopupSpec, Scope};
pub use registry::{Origin, Registry, WidgetPackage};
