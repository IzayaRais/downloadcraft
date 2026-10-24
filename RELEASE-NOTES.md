# Download Craft 0.9.8 — download in the browser, Download all links, a new popup

## What's new

**Browser extension**
- **Download in the browser — no app needed.** Every video card has a second home under
  *Download in browser*. Plain files use the browser's own downloader; a separate video + audio pair
  (Facebook 720p+, many players) is fetched and joined into one MP4 without re-encoding; HLS and DASH
  streams are fetched piece by piece (standard AES-128 HLS decrypted) and joined. DRM and live streams
  are named as such instead of failing. Saved files go to the app's category folders inside the
  browser's Downloads folder, under their real names, and appear in the app's list when it runs.
- **New popup**: toolbar (type filter, title search, auto-send, detection on/off, pause all app
  downloads, rescan, clear), *Download with Download Craft* and *Download in browser* sections, and a
  **Downloads** tab with browser jobs (progress, cancel, history) and the app's queue (pause, resume,
  cancel).
- **Download all links**: right-click a page → *Download all links with Download Craft*.
- **Facebook reels**: one card per reel from Facebook's own data — every quality with its sound track,
  duration, thumbnail and size, H.264 preferred so files play everywhere.
- The video bar waits for the page ("Resolving…") when clicked early, shows once per Facebook reel,
  and stays at the player's corner in fullscreen.
- YouTube refuses downloads made by the browser itself (its links only work inside YouTube's player),
  so its cards hand the video to the app.

**App**
- **Download all links window**: types, sizes, type chips, search, sortable and resizable columns,
  Shift-click ranges, category folders or one folder.
- **Streams show honest progress**: HLS/DASH downloads through yt-dlp count fragments instead of a size
  estimate that keeps changing; pausing and resuming no longer inflates the downloaded size; temporary
  pieces stay in a hidden `.download_craft_parts` folder.
- **One-click JavaScript runtime** (portable Deno) for yt-dlp in Settings → Plugins.
- Resizable list columns with visible dividers; the Download File Info dialog keeps the browser's file
  name; long names in Bengali and other scripts keep their quality tag; a finished video + audio merge is
  never reported as "connection ended early".

## Install

**Windows 10/11.** Extract `DownloadCraft-0.9.8-windows-x64-portable.zip` and run `download_craft.exe` —
nothing to install. Your queue and settings in `%APPDATA%\download_craft` carry over.

**macOS 12+.** Unzip `DownloadCraft-0.9.8-macos-Universal.zip` and move `Download Craft.app` into
Applications. This development build is not notarized: right-click → Open the first time.

**Linux (x86-64, glibc 2.28+).**
- Ubuntu, Debian, Mint, Pop!_OS: `sudo apt install ./downloadcraft_0.9.8_amd64.deb`
- Fedora: `sudo dnf install ./downloadcraft-0.9.8-1.x86_64.rpm`
- Other distributions: `DownloadCraft-0.9.8-x86_64.AppImage` (mark it executable; without FUSE run it
  with `--appimage-extract-and-run`).

**Browser extension (0.9.8).**
- Chrome / Edge / Brave: extract `DownloadCraft-Chrome-0.9.8.zip`, open `chrome://extensions`, enable
  Developer mode, **Load unpacked** → the `browser-extension` folder. Already installed? Press reload on
  the extension and refresh open tabs.
- Firefox: `about:debugging#/runtime/this-firefox` → **Load Temporary Add-on…** →
  `DownloadCraft-Firefox-0.9.8.xpi` (unsigned).
- Then in the app: Settings → Browser Integration → **Install** for your browser.

## Media helpers

Settings → Plugins downloads and updates **yt-dlp**, **FFmpeg + ffprobe** and now a portable
**JavaScript runtime** (Deno) into the app's own folder — no installers, no PATH changes, no sudo.
System copies on PATH or a custom file can be chosen instead.
