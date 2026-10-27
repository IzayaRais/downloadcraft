# Download Craft 0.9.9 — a faster engine, a new look, one-click updates

## What's new

**Download engine**
- **Dynamic segmentation**: idle connections take over the slowest part's remaining bytes, so a download
  finishes together instead of waiting on one slow part. Up to 32 connections, mirrors (`url | mirror`
  in Add URL), a temporary folder setting, and a log per download (Log tab in the progress window).
- **More protocols and logins**: segmented FTP with resume, FTPS (`ftpes://` explicit, `ftps://`
  implicit), HTTP Digest, saved site logins (password encrypted for your Windows account), and
  NTLM / Kerberos logins for company servers through Windows' own HTTP stack.
- **Recovers from more**: refresh an expired link by clicking the download again in the browser,
  HTTP/2 when a server refuses HTTP/1.1, a retry that allows IPv6 when a host cannot be reached, plain
  words for redirect loops and bad certificates, and Windows proxy scripts (PAC / auto-detect).

**App**
- **Redesigned main window**: toolbar and list in one rounded card, a filled Add URL button, outline
  icons for every category and state, statuses as a dot or a percentage over a thin bar, slim scroll
  bars, Windows 11 rounded corners, and a search box that opens when you point at it.
- **IDM-style columns**: dragging a divider changes only that column and moves the others along.
- **Compact right-click menu** with icons, shortcuts, Convert (MP3, MP4, smaller 720p) and Move to queue
  submenus, plus ZIP: pick files (download single files out of a remote ZIP).
- **Progress window**: a livelier progress bar, cleaner tabs, and at most six connection rows before the
  list scrolls.
- **Queues and finding files**: named queues with their own schedules and limits, queue sync, download
  quota, YouTube playlists and channels, batch patterns like `img(001-200).jpg`, a Site Grabber that
  follows links several levels deep and saves projects, and import of IDM `.ef2` lists.
- **One-click update** (portable Windows): Update now downloads the new version, checks it against the
  release's checksum, replaces this copy's files and restarts. If anything fails, nothing is changed.
- **Start with Windows** (on by default): opens in the tray at sign-in, from wherever the portable
  folder is now. Turn it off in Settings → Appearance → Window.
- Bengali translation (Settings → Appearance → Language), statistics, sounds, keyboard shortcuts in
  the list (Ctrl+A, Delete, Space, Enter), and subtitles saved next to videos.

**Browser extension**
- **New video button**: a glass pill that stays see-through over the video and shows fully for a few
  seconds when it appears; its quality menu opens on the page, so the player never cuts it off.
- **Facebook feed**: the button downloads the video it sits on, not a neighbouring one.
- The popup, the in-page panels and the right-click menu follow the browser's language (English,
  Bengali).

## Updating from 0.9.8

0.9.8 does not have one-click updates yet: download this release and extract it over your old folder
once (your list and settings in `%APPDATA%\download_craft` carry over). From 0.9.9 on, the app updates
itself.

## Install

**Windows 10/11.** Extract `DownloadCraft-0.9.9-windows-x64-portable.zip` and run `download_craft.exe` —
nothing to install. Your queue and settings in `%APPDATA%\download_craft` carry over.

**macOS 12+.** Unzip `DownloadCraft-0.9.9-macos-Universal.zip` and move `Download Craft.app` into
Applications. This development build is not notarized: right-click → Open the first time.

**Linux (x86-64, glibc 2.28+).**
- Ubuntu, Debian, Mint, Pop!_OS: `sudo apt install ./downloadcraft_0.9.9_amd64.deb`
- Fedora: `sudo dnf install ./downloadcraft-0.9.9-1.x86_64.rpm`
- Other distributions: `DownloadCraft-0.9.9-x86_64.AppImage` (mark it executable; without FUSE run it
  with `--appimage-extract-and-run`).

**Browser extension (0.9.9).**
- Chrome / Edge / Brave: extract `DownloadCraft-Chrome-0.9.9.zip`, open `chrome://extensions`, enable
  Developer mode, **Load unpacked** → the `browser-extension` folder. Already installed? Press reload on
  the extension and refresh open tabs.
- Firefox: `about:debugging#/runtime/this-firefox` → **Load Temporary Add-on…** →
  `DownloadCraft-Firefox-0.9.9.xpi` (unsigned).
- Then in the app: Settings → Browser Integration → **Install** for your browser.

## Media helpers

Settings → Plugins downloads and updates **yt-dlp**, **FFmpeg + ffprobe** and a portable
**JavaScript runtime** (Deno) into the app's own folder — no installers, no PATH changes, no sudo.
System copies on PATH or a custom file can be chosen instead.
