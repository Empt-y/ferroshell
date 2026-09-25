# Themes

A theme is a folder with a `theme.toml`, in `%APPDATA%\ferroshell\themes\<name>\`
(yours) or `%LOCALAPPDATA%\ferroshell\builtin\themes\<name>\` (built-in). Select it with
`theme = "<name>"` in `config.toml`, or in `fsh-settings` (which can preview before
applying). Edits apply as soon as you save. `fsh-ctl shell preview_theme '{"name":"nord"}'`
tries one without saving (`'{}'` goes back).

## Built-in themes

| Theme | Character |
|---|---|
| `breeze-dark`, `breeze-light` | KDE Plasma's defaults: line indicators, slide-in popups |
| `fluent-dark`, `fluent-light` | Windows 11: mica, your accent colour, pill indicators |
| `nord` | arctic blues, dot indicators |
| `dracula` | purple and pink, everything glows, popups zoom in |
| `catppuccin-mocha`, `catppuccin-latte` | soft pastels, big rounded corners, icons lift on hover |
| `gruvbox` | warm and retro, square corners, underline hover, no transparency |
| `tokyo-night` | neon-lit night blues, dots, lifting icons |
| `rose-pine` | muted rose and pine, pills and a soft glow |
| `solarized-light` | Solarized's light palette, calm fades |
| `synthwave` | neon pink and cyan on midnight purple, lots of glow |
| `frosted-glass` | barely-there tinted glass, huge corners, zoom-in popups |
| `high-contrast` | black, white and yellow, thick borders, gentle motion |
| `classic` | grey, square and instant, like it's 1998 |

Copy any of them into your own themes folder and change what you like.

A theme only needs the values it changes; everything else keeps the default.

```toml
name = "Midnight"
backdrop = "acrylic"   # "acrylic", "mica" or "none" (Windows 11 materials behind the panel)
font = ""              # font family; empty = system UI font

[colors]               # "#rrggbb" or "#rrggbbaa"
panel-background = "#10131acc"   # alpha lets the backdrop show through
accent = "system"                # follow the Windows accent colour

[metrics]              # logical pixels (animation-speed is a multiplier)
radius = 6
icon-size = 26
animation-speed = 1.5            # slower, dreamier motion; 0 turns it off

[style]
indicator-style = "pill"         # how running/active tasks are marked
hover-effect = "lift"            # what hovering a panel button does
open-animation = "zoom"          # how popups, the launcher, banners and the OSD appear
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
| `accent-foreground` | `#ffffff` | text and icons on the accent colour |
| `indicator` | `#3daee9` | running/active task indicator (follows Windows with `accent = "system"`) |
| `popup-border` | `#ffffff1a` | border of popups, the launcher, banners and the OSD |
| `shadow` | `#00000066` | drop shadows (slider knobs, toggles) |
| `glow` | `#3daee980` | the `glow` hover effect and indicator style |

| Metric | Default |
|---|---|
| `radius` | 4 |
| `panel-radius` | 8 |
| `floating-margin` | 6 |
| `spacing` | 2 |
| `padding` | 3 |
| `font-size` | 13 |
| `icon-size` | 24 |
| `popup-radius` | 10 |
| `border-width` | 1 |
| `shadow-blur` | 16 |
| `indicator-size` | 2 |
| `animation-speed` | 1 (a multiplier: 0 = no animation, 2 = half speed) |

| Style | Choices (first is the default) |
|---|---|
| `indicator-style` | `line`, `dot`, `pill`, `glow`, `none` |
| `hover-effect` | `fill`, `lift` (icons grow), `glow`, `underline` |
| `open-animation` | `slide`, `fade`, `zoom`, `none` |

### Motion

Everything that moves follows `animation-speed`: hovering and pressing buttons, task
indicators growing, the pulsing "wants attention" bar, tasks and desktop icons fading
in, toggles, sliders, list highlights, and popups, the launcher, banners and the OSD
opening. When Windows' **Animation effects** is off (Settings > Accessibility > Visual
effects), Ferroshell turns animation off too, whatever the theme says.

In your own widgets, use `Theme.fast`, `Theme.normal` and `Theme.slow` for durations
(they already include `animation-speed`), and wrap a window's content in `Reveal`
(from `@ferroshell/controls.slint`) to give it the theme's open animation.

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