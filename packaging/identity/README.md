# Package identity for fsh-shell

Windows only lets apps with a **package identity** read other apps' notifications
(`UserNotificationListener`), which Ferroshell's notifications applet needs. Ferroshell is a
plain Win32 app, so it gets identity the way Microsoft recommends for such apps: a
**sparse package** (a "package with external location") that contains only a manifest and
logos and points at the folder holding `fsh-shell.exe`.

Everything else in Ferroshell works without this. You only need it for notifications.

## Files

| File | What it is |
|---|---|
| `AppxManifest.xml` | The package: identity `Ferroshell.Shell`, publisher `CN=Ferroshell Local Identity`, the `userNotificationListener` capability. Write virtualization is disabled so Ferroshell (and apps it launches) keep using the real `%APPDATA%` and registry. |
| `fsh-shell.manifest` | Embedded into `fsh-shell.exe` by `apps/fsh-shell/build.rs`; its `<msix>` element links the exe to the package. |
| `Assets/`, `make-logos.ps1` | The package's logos and the script that draws them. |
| `build-package.ps1` | Builds and signs `out/Ferroshell.Identity.msix`. **Normal user.** |
| `install-identity.ps1` | Trusts the certificate and registers the package. **Administrator.** |
| `uninstall-identity.ps1` | Undoes the install. **Administrator.** |

## Setting it up

1. Build Ferroshell's release binaries (quit the shell first so the exe isn't locked):
   ```
   fsh-ctl quit
   cargo build --release
   ```
2. As your normal user, build the package. The first run creates a self-signed
   certificate in your personal store (`Cert:\CurrentUser\My`); nothing trusts it yet.
   ```
   powershell -ExecutionPolicy Bypass -File packaging\identity\build-package.ps1
   ```
3. **As administrator**, install it. This is the step that changes system trust: it adds
   the certificate to *Local Machine > Trusted People* and registers the package for your
   user with `target\release` as its external location. It then checks that a fresh
   `fsh-shell.exe` reports its identity.
   ```
   powershell -ExecutionPolicy Bypass -File packaging\identity\install-identity.ps1
   ```
4. Start Ferroshell again (`fsh-session.exe`), or `fsh-ctl restart` if it's running.
   `fsh-ctl shell status` now shows an `identity` like
   `Ferroshell.Shell_0.1.0.0_x64__…`.

Rebuilding `fsh-shell.exe` in place keeps the identity: the package vouches for the
folder and the embedded manifest, not the exe's bytes. Moving Ferroshell elsewhere needs
the install step again with `-ExternalLocation <new folder>`.

## Checking nothing else changed

- `fsh-shell --identity` prints the identity and exits (exit code 0 with it, 2 without).
- Your settings still come from `%APPDATA%\ferroshell\config.toml`: change something in
  fsh-settings and watch the panel update.
- Apps started from the launcher behave as before (their settings are where they were).

## Undoing it

```
powershell -ExecutionPolicy Bypass -File packaging\identity\uninstall-identity.ps1 [-RemoveSigningCertificate]
```

This unregisters the package and removes the certificate from Trusted People
(`-RemoveSigningCertificate` also deletes the one in your personal store). Restart the
shell afterwards.
