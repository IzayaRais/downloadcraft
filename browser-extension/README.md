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

## YouTube videos (IDM-style button)

On `youtube.com/watch`, `youtu.be/*`, and Shorts pages the extension adds
a floating **⬇ Download Craft** button (bottom-right). Clicking it lists
the video's direct progressive qualities (e.g. `720p · mp4`); picking one
sends the stream URL to the app, which pops open the **Add download**
dialog prefilled with the video title — confirm and it downloads like any
other file. The same qualities also appear in the toolbar popup under the
video title.

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
