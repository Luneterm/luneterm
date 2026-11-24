Luneterm {version}, a glass terminal for Windows, macOS and Linux.

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
