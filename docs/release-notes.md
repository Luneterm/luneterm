Luneterm {version}, a glass terminal for Windows, macOS and Linux.

## Installing

Download the setup file for your system from the assets below. Luneterm installs for your user
(no administrator rights) and then updates itself: Settings > About checks for updates at launch
and once a day (switch it off there) and has a "Check for updates" button. An update is downloaded
when you choose Update, and "Restart to update" closes all shells.

## These builds are unsigned

There is no Apple Developer ID, no notarization and no Windows Authenticode signature, so each
system warns on first launch.

**macOS** (Apple silicon only; `luneterm-{version}-macos-arm64-setup.dmg`). Open the disk image and
run Install Luneterm. The `.dmg` is attached a few minutes after the release is published; if it is
not on the page yet, wait, or use the zip below. On first launch macOS may refuse to open the installer
or the app; go to System Settings > Privacy & Security and choose Open Anyway.

Alternative: `luneterm-{version}-macos-arm64-setup.zip`. Unzip it and move `Luneterm.app` to
`/Applications` or `~/Applications` (updates work from those two places). Or remove the quarantine
flag in a terminal:

    xattr -dr com.apple.quarantine /Applications/Luneterm.app

**Windows** (x64; `luneterm-{version}-windows-x64-setup.exe`). Run it. If SmartScreen appears,
choose More info, then Run anyway. It installs to `%LOCALAPPDATA%\Programs\Luneterm` and adds a
Start menu entry.

**Linux** (x86_64; `luneterm-{version}-linux-x64-setup`). Make it executable (`chmod +x`) and run it;
it installs under `~/.local/share` with a `luneterm` command in `~/.local/bin` and a desktop entry.
It needs glibc {glibc} or newer, an X11 or Wayland session, a Vulkan or OpenGL ES driver, `libxkbcommon` and, on X11,
`libxkbcommon-x11` (Debian: `libxkbcommon-x11-0`).
Linux has no glass effect: it uses an opaque dark theme.

## Checking a download

Each setup file has a `.sha256` file beside it. Put both in one directory, then:

    shasum -a 256 -c <file>.sha256                    # macOS
    sha256sum -c <file>.sha256                        # Linux
    Get-FileHash <file> -Algorithm SHA256             # Windows: compare with the .sha256 file

Third-party licences travel with each build: `licenses/` beside the binary, and
`Luneterm.app/Contents/Resources/licenses` on macOS; the installer and updater components carry
theirs in `lunepack-NOTICES.txt` (macOS: `Contents/Resources/`).
