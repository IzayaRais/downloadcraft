# Browser extension — "Send to Download Craft" (zero-config)

Every download is captured automatically — no pasting IDs,
no per-file clicks.

## 1. Load it once (Chrome / Edge)
1. Open `chrome://extensions`, enable **Developer mode**.
2. **Load unpacked** → select this `browser-extension/` folder.
3. Done. The extension carries a pinned key, so its ID is always
   `pgdhmfgefjnfijmlheboncdmfhcnoikk` — nothing to copy.

(If you loaded an older version before the pinned key, remove it and load
again so the stable ID applies.)

## 2. Connect it from the app (once)
1. Run Download Craft → **Settings** → **Browser integration**.
2. Click **Install Chrome manifest** (or Edge / Firefox).
3. That's it — no IDs, no packets to send by hand.

## 3. Use it
- Download anything normally in the browser — the extension cancels the
  browser's own download and Download Craft takes over automatically.
- If the app isn't reachable, the file falls back to a normal browser
  download so nothing is ever lost (badge shows `!`).
- Right-click any link / video / audio → **Download with Download Craft**.
- Toolbar icon: toggle automatic capture, paste a URL, or pick from the
  files and videos detected on the current page.
- Firefox: load `manifest-firefox.json` instead (rename it to
  `manifest.json` inside a copy of this folder).

## YouTube videos (in-page quality panel)

On `youtube.com/watch`, `youtu.be/*`, and Shorts pages the extension adds a
compact circular button at the player's top-right. Clicking it opens a
message-bubble panel next to it with:

- **Send video link to app** — sends the page URL itself (the app resolves
  it), for when no direct stream URL is available.
- **Video Files** — the direct qualities found on the page (`MP4 · 1080p`,
  `WEBM · 1440p`, …) with sizes, the highest one pre-selected. Picking a
  row and pressing **Download** sends that stream; when the source is
  video-only the best audio track travels with it, so the app merges both.
- **Other Files** — subtitle tracks (VTT, one chip per language) and
  audio-only tracks (e.g. `♪ M4A 128k`).
- A theme toggle (☀ / ☾) in the send row, plus **Download / Cancel / ✕**.

The same button appears on **Facebook** reels (`facebook.com/reel/…`), where
the panel lists the reel's HD/SD files with its caption, duration and any
audio-only stream captured from the player.

While a video plays, streams the player fetches are merged into the panel:
a stale quality row flips from "via app" to a real download as soon as its
deciphered URL shows up.

The same qualities also appear in the toolbar popup under the video title.

Why the extension and not pasting links? The page runs in your real
browser session (real cookies/TLS), so YouTube serves it direct stream
URLs that server-side fetching from a flagged network cannot get.

## After updating the app or extension

1. In Download Craft → **Settings** → **Browser integration**, reinstall
   the host for your browser (this also cleans up legacy registrations).
2. In `chrome://extensions`, hit **reload** on the unpacked extension
   (or remove + load unpacked again for Firefox).
3. **Reload your open YouTube tabs too** (Ctrl+R). This step is mandatory:
   Chrome keeps the old page script running after an extension reload, and
   the old script can no longer talk to the app. If you ever see
   "Extension context invalidated" or a version-mismatch toast, it means
   this step was skipped — the current script detects it and, in most
   cases, refreshes the tab's script itself when you click again.
