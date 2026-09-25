# Ferroshell as your login shell

By default Ferroshell runs alongside Explorer. These scripts make Windows start Ferroshell
**instead of** Explorer when you sign in. They affect only your user account, need no
admin rights, and are undone with one script.

## Setting it up

From a normal (non-admin) PowerShell in the repository:

```powershell
cargo build --release
packaging\shell\install.ps1               # copies the build to %LOCALAPPDATA%\Programs\Ferroshell
packaging\shell\enable-login-shell.ps1    # Ferroshell from your next sign-in
```

Then sign out and back in.

- **The installed copy.** The login shell runs from `%LOCALAPPDATA%\Programs\Ferroshell`,
  never from `target\release`, so a broken or cleaned build can't break sign-in. Run
  `install.ps1` again to update it. The previous version is kept in `previous\`.
- **Notifications.** If you set up notifications
  ([package identity](../identity/README.md)), point the identity at the installed copy.
  As administrator, run:

  ```powershell
  packaging\identity\install-identity.ps1 -ExternalLocation "$env:LOCALAPPDATA\Programs\Ferroshell"
  ```

## Going back to Explorer

```powershell
packaging\shell\disable-login-shell.ps1
```

Explorer is your shell again from your next sign-in. Ferroshell still runs alongside it
if you start `fsh-session.exe` yourself.

## If something goes wrong

- **Ferroshell can't start or keeps crashing.** `fsh-session` starts Explorer itself:
  - straight away if `fsh-shell.exe` is missing;
  - otherwise after repeated crashes, first trying safe mode.
- **The supervisor dies.** The shell starts a new one. After three relaunches in a minute,
  it starts Explorer instead.
- **Black screen.**
  1. Press **Ctrl+Alt+Del**, then choose **Task Manager**, then **Run new task**, and
     run `explorer.exe`.
  2. Run `disable-login-shell.ps1`.
- **The emergency hotkey** (Ctrl+Alt+Shift+E, or whichever `fsh-ctl status` reports)
  stops and restarts Ferroshell's panels.

## What Ferroshell takes over from Explorer

| Explorer's job | As the login shell |
|---|---|
| Telling Windows the desktop is ready | Signals `ShellDesktopSwitchEvent` once the shell answers, so the "Welcome" screen clears |
| Startup apps | Starts Run/RunOnce entries, both Startup folders and Store apps' startup tasks, once per sign-in. Honours what you turned off in Task Manager's Startup apps and `[session]` in config.toml |
| The desktop | A plain wallpaper desktop, registered as the shell window. Right-click it for Personalise, Display settings and more. Ctrl+Esc opens the launcher |
| Taskbar, tray, volume/media keys, OSD, notification banners | Ferroshell's panels and applets (the tray and media keys switch on automatically without Explorer's taskbar) |
| Win+E, R, D, M, Shift+M, I, S, Q, N, X, Shift+S, 1–9 | The same shortcuts. Win+X opens a quick links menu with admin tools, Terminal and power options |
| Passing environment changes (e.g. a new `PATH`) to new apps | Rebuilt from the registry on every change |

To see what would start at sign-in, and why the rest wouldn't, without starting anything:

```powershell
& "$env:LOCALAPPDATA\Programs\Ferroshell\fsh-session.exe" --list-startup | Out-String
```

`fsh-ctl status` shows what happened at the last sign-in: shell-ready, startup apps and
any failures. `fsh-ctl shell state` shows the desktop and shortcuts.

## Not covered (yet)

Some things need Explorer itself and are missing while Ferroshell is the shell:

- Task View, Win+Tab, virtual desktops and snap layouts.
- Desktop icons (the desktop is wallpaper only). Wallpaper slideshows and Spotlight on the
  desktop don't advance either, because Explorer drives them.
- Some apps that talk to a running Explorer through COM (`Shell.Application`), e.g. a
  few installers' "launch as normal user" step.
- Store apps' startup tasks start as normal launches, so a few may open a window instead
  of starting minimised. Add them to `[session] startup-exclude` if that's annoying.
- The lock and sign-in screens stay Windows' own (they run on the secure desktop).
  `[lock-screen] sync-image` can keep the lock screen picture the same as your
  wallpaper.
