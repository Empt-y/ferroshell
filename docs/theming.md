# Themes

A theme is a folder with a `theme.toml`, in `%APPDATA%\ferroshell\themes\<name>\`
(yours) or `%LOCALAPPDATA%\ferroshell\builtin\themes\<name>\` (built-in: `breeze-dark`,
`breeze-light`). Select it with `theme = "<name>"` in `config.toml`, or in
`fsh-settings` (which can preview before applying). Edits apply as soon as you save.

A theme only needs the values it changes; everything else keeps the default.

```toml
name = "Midnight"
backdrop = "acrylic"   # "acrylic", "mica" or "none" (Windows 11 materials behind the panel)
font = ""              # font family; empty = system UI font

[colors]               # "#rrggbb" or "#rrggbbaa"
panel-background = "#10131acc"   # alpha lets the backdrop show through
accent = "system"                # follow the Windows accent colour

[metrics]              # logical pixels
radius = 6
icon-size = 26
```

Unknown tokens and bad values are logged as warnings and ignored. A theme that doesn't
parse at all falls back to the defaults.

## Tokens

| Colour | Default | Used for |
|---|---|---|
| `panel-background` | `#202326e6` | panel fill |
| `panel-border` | `#ffffff14` | hairline on the panel's inner edge |
| `foreground` | `#fcfcfc` | text and icons |
| `foreground-dim` | `#a1a9b1` | secondary text, inactive indicators |
| `accent` | `#3daee9` | active indicators, highlights (`"system"` = Windows accent) |
| `hover` / `pressed` | `#ffffff14` / `#ffffff24` | item backgrounds |
| `highlight` | `#3daee940` | active task background (tinted with the accent) |
| `attention` | `#f67400` | apps asking for attention, warnings |
| `popup-background` | `#202326f5` | previews and popups |
| `error` | `#da4453` | broken widgets |

| Metric | Default |
|---|---|
| `radius` | 4 |
| `panel-radius` | 8 |
| `floating-margin` | 6 |
| `spacing` | 2 |
| `padding` | 3 |
| `font-size` | 13 |
| `icon-size` | 24 |

Widgets read these through the `Theme` global: `Theme.accent`, `Theme.radius`, and so on.

## Overriding widgets per theme

A theme can restyle any widget completely by including `widgets\<widget-id>\` (a full
widget package; see [widget-api.md](widget-api.md)). Lookup order is: your
`%APPDATA%\ferroshell\widgets`, then the active theme's `widgets`, then the built-ins.

## Restyling the launcher, OSD, banners and previews

These shell windows are Slint files in `%LOCALAPPDATA%\ferroshell\builtin\ferroshell\`:

| File | Component | What it is |
|---|---|---|
| `launcher.slint` | `LauncherWindow` (+ the `Launcher` global) | the application launcher |
| `popups.slint` | `PreviewPopup` | task hover previews |
| `osd.slint` | `OsdWindow` | the volume/brightness on-screen display (replacement mode) |
| `banner.slint` | `BannerWindow` | notification banners |

To change one's look:
1. Copy it to `%APPDATA%\ferroshell\library\` (yours) or to `<theme>\library\` (part of a
   theme).
2. Change the relative imports at the top to `@ferroshell/…`, e.g.
   `@ferroshell/api.slint`.
3. Keep the exported component, its properties and its callbacks. The shell sets
   `value`/`muted`/`kind` on the OSD, for example, and listens for the banner's
   `clicked`/`closed`.

A copy that fails to compile is ignored: the built-in one is used instead and the error
is logged. Safe mode always uses the built-ins.

## Restyling applet popups

An applet's popup (the calendar, volume mixer, network list and so on) is the
`popup.slint` in its widget package. To restyle one, override the whole widget: copy
`builtin\widgets\<id>\` to `%APPDATA%\ferroshell\widgets\<id>\` or `<theme>\widgets\<id>\`,
then edit `popup.slint`, `ui.slint` or both.

The shell draws each popup's window:
- a rounded rectangle in `popup-background` with a `panel-border` hairline;
- the theme's `backdrop` behind it;
- keyboard focus, Escape to close, and placement next to the applet.

Popups have the same `Theme` tokens and system services as panel widgets (see
[widget-api.md](widget-api.md#system-services)). `@ferroshell/controls.slint` provides
themed sliders, switches, list rows and vector icons to build them from.