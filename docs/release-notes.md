Luneterm {version}, a glass terminal for Windows, macOS and Linux.

## What's new

**Bell and program notifications.** A bell in a background tab puts an amber dot on that tab. A
program that prints an OSC 9 message or an `OSC 777;notify` gets a small in-app
toast with Go to tab and Dismiss, and every bell and message is kept in a bell button at the right of
the tab strip, with an unread badge and a list you can clear. Nothing is sent to the operating system
except the Dock or taskbar attention request a bell already made. Turn the messages off in Settings,
Terminal, Behavior, "Notifications from programs" (on by default).

## These builds are unsigned

There is no Apple Developer ID, no notarization and no Windows Authenticode signature, so each
system warns on first launch.

**macOS** (Apple silicon only; `luneterm-{version}-macos-arm64.zip`). Unzip and move `Luneterm.app`
to Applications. On first launch macOS refuses to open it; go to System Settings > Privacy &
Security and choose Open Anyway. Or remove the quarantine flag in a terminal:

    xattr -dr com.apple.quarantine /Applications/Luneterm.app

**Windows** (x64; `luneterm-{version}-windows-x64.zip`). Unzip and run `luneterm.exe`. If SmartScreen
appears, choose More info, then Run anyway.

**Linux** (x86_64; `luneterm-{version}-linux-x64.tar.gz`). Unpack and run `./luneterm`. It needs
glibc {glibc} or newer, an X11 or Wayland session, a Vulkan or OpenGL ES driver, `libxkbcommon` and, on X11,
`libxkbcommon-x11` (Debian: `libxkbcommon-x11-0`).
Linux has no glass effect: it uses an opaque dark theme.

## Checking a download

Each archive has a `.sha256` file beside it. Put both in one directory, then:

    shasum -a 256 -c <archive>.sha256                 # macOS
    sha256sum -c <archive>.sha256                     # Linux
    Get-FileHash <archive> -Algorithm SHA256          # Windows: compare with the .sha256 file

Third-party licences travel with each build: `licenses/` beside the binary, and
`Luneterm.app/Contents/Resources/licenses` on macOS.
