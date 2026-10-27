# Download Craft 0.9.10 — archives, Google Drive, and a smarter video button

## What's new

**Downloads**
- **Google Drive and Google Docs links**: paste a Drive share link and the file itself downloads, with its
  real name and resume, including big files behind Drive's "can't scan for viruses" page. Docs, Sheets and
  Slides links download as Word, Excel and PowerPoint files.
- **Extract archives automatically**: Settings → On completion → *Extract archives when they finish* unpacks
  ZIP, RAR (multi-part sets too) and 7z, into a new folder or next to the archive, optionally deleting the
  archive afterwards. Right-click a finished archive → **Extract** works any time. Nothing is written outside
  the folder, existing files are kept, and password-protected archives are left alone.
- **Audio only**: the video menu's ♪ MP3 choice saves just the sound, as MP3.

**Browser extension**
- **Instagram, TikTok, X and Reddit**: the video button finds the post the video belongs to (even in a feed)
  and offers Best / 1080p / 720p / 480p / 360p or MP3; the app fetches it with its sound.
- **One click in your quality**: the button remembers the quality you picked last ("Download 720p"); its ▾
  opens the full menu. Turn it off in the popup's Settings.
- **Right-click → Download video with Download Craft** on the video under the pointer.
- **YouTube Shorts**: a small round button in the Short's own control row; it hides while you scroll and comes
  back for the next Short, and it can be dragged anywhere.
- **Download in browser** now writes a normal, indexed MP4: the sound no longer drops out partway through or
  after skipping ahead.
- The popup lists videos again (a translation step had emptied it), scrolls in one place, and its badge
  counts the same videos the list shows.
- Smaller things: the button is never cut off, waits for the player while a page loads, lights up as a whole
  on hover; the menu has a sun / moon theme switch and one Cancel.
- **Self-check**: when a site changes its layout so the button cannot find what it needs, a line appears in
  the app's log.

**App**
- **Site Grabber redesign**: clear sections, file-type chips, a levels stepper, and a results list.
- List headers with icons (Size, Status, Speed, Time left, Date), centred with their values; a selected row is
  a soft tint without an outline; Add URL looks like the other toolbar buttons.
- The toolbar fits narrow windows (icons only, then fewer buttons) and never covers Settings; Settings opens
  even while a dialog is open.
- Double-click a running download, or right-click → **Show download window**, to open its progress window.

## Updating from 0.9.9

Use **Update now** in the app: it downloads this release, checks it and restarts. Then reload the browser
extension (chrome://extensions → reload) with the files from this release.

## Install

**Windows 10/11.** Extract `DownloadCraft-0.9.10-windows-x64-portable.zip` and run `download_craft.exe` —
nothing to install. Your queue and settings in `%APPDATA%\download_craft` carry over.

**macOS 12+.** Unzip `DownloadCraft-0.9.10-macos-Universal.zip` and move `Download Craft.app` into
Applications. This development build is not notarized: right-click → Open the first time.

**Linux (x86-64, glibc 2.28+).**
- Ubuntu, Debian, Mint, Pop!_OS: `sudo apt install ./downloadcraft_0.9.10_amd64.deb`
- Fedora: `sudo dnf install ./downloadcraft-0.9.10-1.x86_64.rpm`
- Other distributions: `DownloadCraft-0.9.10-x86_64.AppImage` (mark it executable; without FUSE run it
  with `--appimage-extract-and-run`).

**Browser extension (0.9.10).**
- Chrome / Edge / Brave: extract `DownloadCraft-Chrome-0.9.10.zip`, open `chrome://extensions`, enable
  Developer mode, **Load unpacked** → the `browser-extension` folder. Already installed? Press reload on
  the extension and refresh open tabs.
- Firefox: `about:debugging#/runtime/this-firefox` → **Load Temporary Add-on…** →
  `DownloadCraft-Firefox-0.9.10.xpi` (unsigned).
- Then in the app: Settings → Browser Integration → **Install** for your browser.

## Media helpers

Settings → Plugins downloads and updates **yt-dlp**, **FFmpeg + ffprobe** and a portable
**JavaScript runtime** (Deno) into the app's own folder — no installers, no PATH changes, no sudo.
System copies on PATH or a custom file can be chosen instead.

RAR archives are read with the UnRAR source code (freeware licence, extraction only); see `NOTICE.md`.
