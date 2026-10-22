# Download Craft 0.9.7 development build

## Choose a Linux installer

- Fedora Workstation: `downloadcraft-0.9.7-1.x86_64.rpm`; install with your software center or `sudo dnf install ./downloadcraft-0.9.7-1.x86_64.rpm`.
- Ubuntu, Debian, Linux Mint and Pop!_OS: `downloadcraft_0.9.7_amd64.deb`; install with your software center or `sudo apt install ./downloadcraft_0.9.7_amd64.deb`.
- Other desktop distributions: `DownloadCraft-0.9.7-x86_64.AppImage`; mark it executable and open it. If FUSE is unavailable, run it with `--appimage-extract-and-run`.

All three are x86-64 builds targeting glibc 2.28 or newer. They require a desktop
with X11 or Wayland and OpenGL. The AppImage is portable across compatible Linux
desktops; a different executable is not required for each distribution, while
DEB and RPM provide each distribution's normal installer integration.
The app appears in the launcher as Download Craft.

AppImage browser registration uses the permanent AppImage path, including when
launched in extract-and-run mode. Keep the image at a permanent location.

## Media helpers on Mac and Linux

Open Settings → Plugins:

1. Press **Download yt-dlp** (or **Update yt-dlp**) to fetch the correct native
   helper. You can also select a system or custom executable; press Save to apply.
2. Expand FFmpeg and press **Download tools**. This installs FFmpeg and ffprobe
   together into your own application data folder, without sudo or Homebrew.
   Both downloads are checked against pinned SHA-256 hashes and must pass their
   version checks before replacing the previous pair.
3. Prefer a system FFmpeg? Use **Choose FFmpeg…**, then Save and Check again.
   Homebrew, `/usr/local/bin`, the system directories and `~/.local/bin` are
   searched even when launched from the Dock or desktop menu.

The optional managed FFmpeg binaries are from Descript's FFmpeg/ffprobe builds
(tag b7.1.0-rc.1); download URLs and hashes are recorded in
`assets/helper-downloads.json`. The upstream build sources/license are at
https://github.com/descriptinc/ffmpeg-ffprobe-static . They are downloaded on
request and are not bundled into these installers.

YouTube also needs **Node.js 22+ or Deno 2.3+**. The Plugins pane checks the
version, links to the runtime installation guide and passes the detected full
path to yt-dlp. Unsupported old Node installations are no longer reported as
ready. System alternatives for FFmpeg are `brew install ffmpeg` on Mac,
`sudo apt install ffmpeg` on Debian/Ubuntu, or `sudo dnf install ffmpeg-free`
on Fedora. The managed tools option avoids differences in distribution codecs.

## Browser setup

Settings → Browser Integration has buttons to open stable extension folders.
These copies remain available after an AppImage unmounts or a Mac app moves to
Applications. Chrome/Brave use `browser-extension`; Firefox uses
`browser-extension-firefox`. Both have automatic takeover and the video-dismiss
X, including fullscreen. Refresh tabs after updating the extension.

The Firefox development extension uses temporary installation through
`about:debugging`; permanent installation requires Mozilla signing. Sandboxed
Snap/Flatpak browsers may need their distribution's native messaging portal or
a normally installed browser. The application does not change browser sandbox
permissions.

## macOS

The Mac app includes Apple Silicon and Intel code. Move the `.app` into
Applications, then launch it before connecting the browser extension. Both
architectures require macOS 12 or newer so the managed helper workflow has
a consistent minimum (the Apple Silicon FFmpeg build requires macOS 12). Development binaries are not notarized;
macOS may require approval in Privacy & Security. The app includes its extension
files inside Resources as well as in the ZIP.

Mac/Linux use a normal Dock/taskbar window; closing the main window quits, so
minimize it while downloads run. Mac UI omits the unsupported hibernate action.

## What Changed

This release incorporates PR #51 from [@dazzlejacket](https://github.com/dazzlejacket),
including browser video takeover and draggable video controls, improved download
handling, and macOS/Linux support. The integration also preserves Download Craft
0.9.7 and includes the conflict resolutions reviewed for this release.

The browser extension compatibility fix is version 0.9.7.1; the desktop app remains
version 0.9.7. Chrome now skips Firefox-only blocking webRequest registration.

## Fixes in this release

- Preserve helper/custom paths when the main window saves its state.
- Apply settings changes before subsequent main-window saves.
- Match helper status to the actual downloader's selection and check executable permissions.
- Keep working managed tools when a download or version check fails.
- Find Homebrew tools and pass full JavaScript-runtime paths from Dock launches.
- Replace Windows-only helper labels, paths, executable filters and instructions on Mac/Linux.
- Use a persistent AppImage path for the browser host and child windows.
- Supply DEB, RPM, AppImage and a universal Mac app, with reproducible packaging scripts.

See VALIDATION.txt in the source/release archive for the exact checks and
remaining runtime limits. PR #51 is integrated; other product proposals remain
unimplemented.

HTTPS downloads use the operating system's trusted certificate store, including
locally trusted certificates on managed networks. Certificate verification stays
enabled.

The first Settings save in a fresh profile is now detected correctly, including
changes deferred while the Add Download dialog is open.
