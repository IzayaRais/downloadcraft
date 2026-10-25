# Download Craft vs IDM: the remaining gaps

What Internet Download Manager 6.43 does that Download Craft does not do yet, checked against the code
on 25 Oct 2026 (branch `feat/dynamic-engine`).

How it was built:

- **IDM's side** comes from the reverse engineering in `My Project/IDM reverse engineering/ANALYSIS.md`
  (desktop app) and `idm_ext_inspect/ANALYSIS.md` (browser extension).
- **Download Craft's side** was checked against the source.

**Priority:** 🔴 users notice it often · 🟠 power users expect it · 🟢 nice to have.
**Effort:** S = a day or less · M = a few days · L = a week or more.

---

## Already equal or ahead of IDM

Do not redo these:

- **Engine**
  - Dynamic segmentation with work stealing, splitting by speed (IDM only halves).
  - One pre-allocated file instead of joining parts.
  - Crash-safe resume journal.
  - Throttling and per-IP connection-limit detection.
  - Slow-connection restart.
  - Expired-link refresh.
  - Checks on every reply.
  - Reusable as its own crate (`crates/downloadcraft-engine`).
- **Video**
  - HLS/DASH (including AES-128) and yt-dlp for 1,000+ sites.
  - YouTube native ⇄ yt-dlp fallback.
  - Facebook/Instagram/Vimeo handling.
  - The extension downloads in the browser without the app (IDM cannot).
- **Browser integration**
  - Pause-then-hand-off.
  - POST downloads.
  - The browser's headers, IP and proxy travel with each capture.
  - A live connection to the app.
  - Capture settings and site rules pushed by the app.
  - Fetch-through-page.
  - Alt-click to keep a download in the browser.
  - Download all/selected links.
  - A context-menu entry on the browser's downloads page.
  - Welcome page and notifications.
- **App**
  - Queue, scheduler with speed windows, speed limit, per-server cap.
  - Categories and folder rules, rename template.
  - Clipboard watching, drop basket, mini window.
  - Finish actions (shutdown/sleep), antivirus hook, SHA-256 check.
  - Command-line tool, tray, themes.
  - FTP.
  - Site grabber (depth 1).
  - ZIP preview (`peek`, command line only).
- **Platforms:** Windows, macOS and Linux. IDM is Windows-only.

---

## 1. Performance and footprint 🔴

Measured on the same PC, both apps idle:

| | Download Craft | IDM |
|---|---|---|
| Program size | 31 MB | 6 MB |
| Memory | 275 MB | 35 MB |
| CPU over its run | 138 s in 22 min | 77 s in ~10 h |

| # | Gap | Priority | Effort | How |
|---|---|---|---|---|
| 1.1 | Redraws while nothing changes | 🔴 | M | Repaint only on change: ~1/s while downloads run, none while idle or hidden (`request_repaint_after`), no animations off screen |
| 1.2 | High memory | 🔴 | M | Load only the fonts in use and subset them; drop unused textures; check the font atlas size; profile with a heap tool |
| 1.3 | Large program | 🟠 | S | Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`, `panic = "abort"`; check which dependencies are heavy (`cargo bloat`) |
| 1.4 | Start-up time | 🟠 | S | Measure cold start; load the queue and fonts lazily; open the window first |
| 1.5 | Helper process starts per browser message | 🟢 | S | Mostly solved by the live connection; keep the host binary small (a separate small host executable would start faster) |

## 2. Download engine

| # | Gap | Priority | Effort | How |
|---|---|---|---|---|
| 2.1 | At most 16 connections per download (IDM: 32) | 🟢 | S | Raise the clamp to 32 in the app and the engine (`connections.clamp(1, 16)`); keep 8 as the default |
| 2.2 | No HTTP/2 | 🟢 | M | Optional per server: try HTTP/2 when a server refuses HTTP/1.1; keep HTTP/1.1 for segmented downloads (one socket per segment is the speed-up) |
| 2.3 | No mirrors / alternative URLs for one file | 🟠 | M | Engine crate: `Job.mirrors`, so workers can take segments from any mirror that has the same size and ETag (IDM `AddAlternative`) |
| 2.4 | Login types: Basic only | 🟠 | M | Digest auth (in the engine), NTLM/Negotiate on Windows (SSPI) for company servers |
| 2.5 | No saved logins per site | 🟠 | S | IDM's "Site logins": host pattern → user/password, stored with the OS keychain; applied automatically |
| 2.6 | FTP uses one connection | 🟠 | M | Segmented FTP (`REST` + several data connections), FTPS, reconnect-and-resume like HTTP |
| 2.7 | No temporary-folder setting | 🟢 | S | Setting for where `.dcdata`/`.dcjournal` live (fast disk), moved to the target folder at the end |
| 2.8 | Error recovery is generic | 🟢 | M | IDM has ~40 recovery paths ("restart types"); add the missing ones as they turn up: DNS switch, TLS fallback, redirect loops, `Content-Length` lies |
| 2.9 | No per-download connection log | 🟠 | S | IDM keeps a log per download (connects, ranges, errors). Show the engine's `Event`s in a Log tab in the progress window |
| 2.10 | "Refresh download address" for any site | 🔴 | M | IDM opens the page and waits for the user to click the link again, then continues the same file. The engine hook exists (`on_link_expired`); add the UI and match the new link by size |

## 3. Queues and scheduling

| # | Gap | Priority | Effort | How |
|---|---|---|---|---|
| 3.1 | One queue only | 🟠 | L | Several named queues, each with its own schedule, concurrency and finish action; move items between queues |
| 3.2 | No queue synchronization | 🟢 | M | IDM can check a queue's files for newer server versions on a schedule (size/date) and re-download changed ones |
| 3.3 | No download quotas | 🟢 | S | "At most N MB per X hours" (IDM, for capped connections) |
| 3.4 | No "download later" from the capture dialog | 🟠 | S | A button in the File Info dialog that adds to a queue without starting it |

## 4. Finding files

| # | Gap | Priority | Effort | How |
|---|---|---|---|---|
| 4.1 | Site grabber follows links one level deep | 🟠 | L | IDM's Site Grabber: depth N, stay-on-site/path rules, file-type and size filters, saved projects that can be run again, logins |
| 4.2 | No batch download with wildcards | 🟠 | S | `https://x/img(001-200).jpg` / `a-z` patterns, expanded into a pick list |
| 4.3 | ZIP preview only on the command line | 🟢 | S | Show `peek`'s list in the File Info dialog for `.zip` links and allow picking one file (range download of one entry) |
| 4.4 | No import of IDM's list | 🟢 | S | Import IDM's `.ef2` export (and plain text, already done), so IDM users can switch |
| 4.5 | YouTube playlists/channels | 🟠 | M | Use yt-dlp `--flat-playlist` to list the entries, then queue them as separate rows with one quality choice |

## 5. Video

| # | Gap | Priority | Effort | How |
|---|---|---|---|---|
| 5.1 | No converter | 🟢 | M | IDM ships a converter (`idmvconv`). Add "Convert to MP3 / MP4 (H.264) / smaller" after download through FFmpeg, as a finish option and a right-click action |
| 5.2 | Native overlay panel | 🟢 | L | IDM draws its own window over the player. Download Craft keeps the in-page bar on purpose (it works on all systems); only revisit if sites keep breaking the bar |
| 5.3 | Subtitles as files | 🟢 | S | The extension finds subtitle tracks; let the app save them next to the video (`--write-subs` for yt-dlp, the VTT/SRT link otherwise) |

## 6. Browser integration (what is left)

| # | Gap | Priority | Effort | How |
|---|---|---|---|---|
| 6.1 | Chrome PAC/system proxy | 🟢 | M | Only fixed proxies are passed on today. For PAC, ask the app to evaluate the PAC script (or fall back to the system proxy) |
| 6.2 | Live browser test of the new hand-off | 🔴 | S | Pause/resume, POST, Alt-click, page-fetch and notifications are covered by tests but not yet used day to day: try each on real sites and fix what turns up |
| 6.3 | Store listing | 🟠 | M | Chrome Web Store / Edge / Firefox Add-ons: privacy text, permission reasons (proxy, notifications, webRequest), screenshots; the welcome page already covers Firefox's consent |
| 6.4 | Opera/Yandex specifics | 🟢 | S | IDM special-cases them; check the extension in Opera (sidebar, downloads page) |

## 7. App features and polish

| # | Gap | Priority | Effort | How |
|---|---|---|---|---|
| 7.1 | No sounds | 🟢 | S | Optional sounds for "download complete", "failed", "queue done" (IDM has these) |
| 7.2 | English only | 🟠 | L | Translatable strings (Fluent or a JSON per language) for the app and the extension (`_locales`); IDM ships ~20 languages |
| 7.3 | No speed graph per download | 🟢 | S | A small speed history graph in the progress window |
| 7.4 | No statistics | 🟢 | S | Total downloaded today/this month, average speeds |
| 7.5 | Drop target | 🟢 | S | The basket covers it; add dropping links straight onto the main window and the tray icon |
| 7.6 | Keyboard shortcuts | 🟢 | S | Delete, Ctrl+A, Enter = open, Space = pause/resume, as in IDM's list |

---

## Suggested order

1. **6.2** live browser testing, then **1.1–1.3** footprint. Users see these first.
2. **2.10** refresh download address, **2.9** per-download log, **3.4** download later.
3. **3.1** named queues, **4.5** YouTube playlists, **4.2** batch wildcards.
4. **2.3** mirrors, **2.4/2.5** logins, **2.6** segmented FTP.
5. **4.1** full site grabber, **7.2** languages.
6. The rest as time allows.
