# IDM-style video catch — integration notes (clean-room)

Date: 2026-10-14

## What was requested
- Look at local IDM install on C: drive and bring its behavior into this repo.
- Use https://github.com/morluto/rea for reverse engineering.
- Apply extension + YouTube/other-site download catch to `Download Craft`.

## What was actually done (and why)

### 1. IDM install — referenced, NOT copied
Location found: `C:\Program Files (x86)\Internet Download Manager\`
(`IDMGrHlp.exe`, `idmmkb.dll`, `IDMGCExt.crx`, `idmmzcc.xpi`, etc. present.)

Nothing was copied into this repo. Reason:
- IDM is commercial proprietary software (~6 MB exe + drivers + DLLs).
  Copying binaries/extension code into git would violate its license and
  bloat the repo.
- Only black-box observable behavior was re-implemented clean-room:
  1. Auto-capture of browser downloads (`chrome.downloads.onCreated`).
  2. In-page floating "Download this video" button (IDM's hallmark).
  3. Network sniffing of media responses (video/*, audio/*, .mp4/.m3u8/.mpd).
  4. Right-click "Download with ..." on link/video/audio.
  5. Forwarding page URL as Referer so media servers accept the request.

No IDM binary, CRX, XPI, or decompiled code is in this repo.

### 2. `morluto/rea` — not needed / not used
https://github.com/morluto/rea is a generic "Reverse Engineer Anything"
MCP agent framework (native binaries, Electron, .NET, websites). It is:
- Not IDM-specific, not a YouTube-download library.
- Not required to implement browser video capture.
- Heavyweight (needs Node 22+, Hopper/Ghidra/IDA for native work).

So it was NOT installed. The work below uses only observable browser
behavior + existing `Download Craft` engine code.

If you still want REA for other RE tasks: `npx rea-agents setup`.

### 3. Changes in this patch (v0.4.0 extension)

`browser-extension/manifest.json`, `manifest-firefox.json`:
- version `0.3.5` → `0.4.0`
- added permissions: `tabs`, `cookies`, `webRequest` (for sniffer)

`browser-extension/background.js`:
- `sendToHost()` now sends `{pageUrl, referer, mime}`.
- new per-tab `webRequest.onHeadersReceived` sniffer for
  `video/*`, `audio/*`, `.mp4/.webm/.mkv/.m3u8/.mpd`, stores last 25/tab.
- new `getMedia` message for content/popup.
- context-menu + download handlers forward `pageUrl`.

`browser-extension/content.js`:
- `EXT_VER 0.4.0`, `sendDownload()` sends pageUrl/referer/mime, blob: → page URL fallback.
- new universal overlay: pill button on every `<video>` (MutationObserver),
  IDM-style. Falls back to sniffed URL or page URL when `currentSrc` is blob:.
- `pullNetworkMedia()` merges sniffed streams into popup storage.

`browser-extension/popup.js`:
- shows `downloadCraftNet` (sniffed streams) separately, with HLS/DASH label.

`src/native_host.rs`:
- `Incoming` + `InboxItem` gain `page_url/referer/cookie/mime` (serde defaults,
  old inbox files still load). blob: rejected loudly. Referer defaults to pageUrl.

`src/app.rs` (`poll_inbox`):
- prefills Add dialog `referer` + `cookie` from inbox (media servers often 403
  without Referer).

`src/engine.rs`:
- `resolve_generic()`: + `twitter:player`, ld+json `embedUrl`, Vimeo-style
  progressive scan; skips `.m3u8/.mpd` playlists (new `is_playlist_url()`).
- `download()`: fail-fast on playlist URLs with guidance text.
- helpers: `is_playlist_url()`, `has_direct_ext()`.

### 4. What still does NOT work (honest limits)
- Encrypted/ciphered YouTube streams (signature decipher) — use extension
  direct quality buttons when offered, else 720p progressive fallback.
- HLS (.m3u8) / DASH (.mpd) segment assembly + transmux — out of scope for
  the progressive engine; extension labels these explicitly.
- DRM (Widevine/FairPlay) sites — cannot and will not bypass.
- blob: MediaSource players with no network bia — only page-URL resolve works.

### 5. How to test
1. `cargo test` — 13 passed (incl. existing YouTube/generic fixtures).
2. `node --check browser-extension/*.js` — syntax OK.
3. Load `browser-extension/` unpacked in Chrome/Edge, open
   Download Craft once → Settings → Browser integration → Install.
4. Open YouTube watch + a plain MP4 test page + Vimeo: check for
   floating pill, popup qualities, Add-dialog prefill with Referer.

### 6. Double-button + sizes fix (ext v0.4.2–0.4.5, 2026-10-14 pm)
Live DevTools proof on a watch page: old build showed BOTH
`⬇ Download this video` (universal pill) and `⬇ Download Craft` (quality
menu). Fixed: universal pill skips `ytVideoId()` pages + `#movie_player`
videos + removes stale-generation pills (`data-dc-pill` GEN cleanup), so
watch pages show exactly one button (renamed to `⬇ Download this video`,
IDM wording, v0.4.3). Quality items now enrich to IDM-style
`⬇ 360p · mp4 (8.2 MB)` via background `getSizes` (HEAD, then Range 0-0
fallback, 12-URL cap, cached). v0.4.4: full adaptive list incl. WebM
VP9/AV1 (144p–2160p) with container tags + fps-aware labels + popup sizes.
v0.4.5: `appendSniffed()` merges live network-caught googlevideo streams
into the menu — needed because music/VEVO pages embed ZERO direct URLs
(all 27 formats `hasUrl:false`; only the running player's traffic has
deciphered URLs, same source IDM reads). Add dialog `start_probe`
(`src/app.rs`) now resolves page→media before sizing (fixes 0 B).
Reload extension + tab (Ctrl+R) to pick up. Still open (P1.3): video+audio
merge via ffmpeg for 4K-with-sound single files.

### 7. IDM-style separate Download File Info window (2026-10-15)
Captures no longer open inside the main window. The native host now spawns
one detached always-on-top `Download File Info` window per download
(`--confirm-download`, new `src/confirm.rs`): URL, Category combo,
remember-path, Save in + File name, probed size, Description, and
Download Later / Start Download / Cancel — mirroring IDM's dialog.
It writes `queue.json` directly (+ `wake.json` for Start, auto-launching the
main app if closed); `claim_inbox()` stops the main window double-prompting,
`poll_inbox()` gained a 10 s age gate as backstop, and the main app merges
foreign queue entries + consumes wake (`merge_queue_from_disk`, `check_wake`,
every 2 s poll + at startup). `DownloadItem.description` added (serde-default,
old queues load fine). Smoke-tested: dialog process stays alive with no crash,
14/14 tests green.

### 8. ABDM-style dialog + centering + browser-known sizes (2026-10-15)
Dialog retitled `Add download`, centered on first frame via
`viewport().monitor_size` + `OuterPosition`, restyled to AB Download
Manager's Add dialog: tall rounded fields, URL+copy / folder+browse /
filename+re-probe+gear rows, green `✔` size badge, big 42 px
Add / accent Download / Cancel buttons, gear-toggled advanced
(referer/cookie/connections). `0 B` never shows anymore (zero = unknown,
main Add dialog too). Size pipeline end-to-end: menu/popup `getSizes`
results now travel `content.js → background → InboxItem.size →
--size → dialog` so VEVO-type videos (zero server-side URLs) still show
their browser-measured size (ext v0.4.6, reload + Ctrl+R).

### 9. Separate progress window + no-hang Start (2026-10-15)
Start Download no longer downloads "inside the main list": the main app pops
a standalone always-on-top progress window per download (`--progress <id>`,
new `src/progress.rs`) with Name/Status/Size/Downloaded/Speed/ETA/Resume +
bar + Part Info table + Pause/Resume/Cancel/Close/Open-file — live numbers
via `progress.json` (~1 s snapshots), orders back via `cmd.json`
(`write_progress_snapshot`, `check_cmd`, spawned from `check_wake` and main
Add-dialog Download). Closing it never stops the transfer. Confirm-dialog
hang fixed: tasklist/launch moved to a background thread so Start closes
instantly (slowness was synchronous process-scan + 233 MB debug cold start;
release build recommended). Live DevTools check proved the user's tab still
runs pre-0.4.4 content (renamed button but only `360p · mp4`, no sizes) —
extension reload + Ctrl+R still required to get sizes/full list/passthrough.

### 10. 403 fail-fast + expiring-link filter + row pills (2026-10-15)
Your screenshot's `403 Forbidden` on googlevideo = stale/session-locked
stream URL (they expire, ~6 h, IP/session-bound). Engine now maps 401/403
to actionable errors (`http_status_err`: YouTube → "replay the video, reopen
the menu for a fresh quality"; other hosts → login/cookie guidance) and
fails fast without burning 5 retries (`is_final_http_error` in both retry
loops; `/forbidden` fixture + assertions in `local_http_matrix`). Extension
v0.4.7 drops page URLs expiring within 2 min from the menu, folds expiries
into the rebuild signature (fresh links rebuild rows), and shows a
"press play, then reopen" hint when a page embeds zero direct links.
Main list: Downloading rows now render a PRIMARY `%` pill + bar like the
other states (removed one-off `status_text`). Full ABDM visual redesign
still open — needs your specifics (sidebar/rows/theme?).

### 11. Fresh-link auto-retry + itag URL swap (2026-10-15)
Your persisted 403 proved page URLs 403 even when fresh: YouTube's `n`
throttle param is deciphered by the running player, not embedded in page
JSON. Two lawful fixes (no cipher breaking — both reuse player-supplied
fresh URLs): engine `download()` retries ONCE on googlevideo-403 when the
watch page is known (referer), re-resolving via player API, dropping refused
partials and re-probing (`is_googlevideo_url`/`is_youtube_page` + asserts);
extension v0.4.8 captures `itag` per row and swaps stale row URLs for
player-deciphered sniffed URLs of the same itag before you click
(`appendSniffed` + `enrichSizes` re-run). Rebuild done, 14/14 green.

### 13. itag-labeled sniffed rows (2026-10-15, ext v0.4.9)
Live page parse of your exact video proved YouTube embeds ONLY itag 18
(360p) — all 18 adaptive formats (1080p→144p, mp4+webm+AV1) have NO direct
URL until playback, so no page parser can list them. v0.4.9 labels sniffed
live-traffic rows via an itag→quality table (`⬇ 1080p · mp4 (no audio)`,
audio rows, progressive rows without the tag), giving the IDM-style full
list whenever the video plays. Your tab was fingerprinted on v0.4.3
(renamed button + lone 360p row + zero sizes) — reload REQUIRED.

### 12. Unified theme system (2026-10-15)
`pub const RADIUS = 10.0` + `apply_widget_theme(ctx)` in `src/app.rs` now
own ALL geometry: rounded controls, roomy buttons, type scale, spacing —
applied at startup, on every theme switch, and reused by the Add-download
and progress windows (their duplicated style blocks deleted). All cards
(settings, toast, header bar, context menu, advanced frame), row
select/hover rects, and icon buttons unified to RADIUS (was a 6/8/10 mix).
Paint-only change: no hit-testing or behavior touched. 14/14 green.

### 13. P1 stream assembly complete (2026-10-15)
- HLS: 4–8 parallel segments, ranged-HEAD byte totals, `hlsmap` resume,
  AES-128 via vendored `src/aes128.rs` (computed S-boxes, FIPS-197 B + NIST
  SP 800-38A green, cross-checked against .NET; my first memorized vectors
  were corrupt — fixed from .NET output, not memory).
- Adaptive: `download_adaptive()` + `ffmpeg_mux()` + full pair plumbing
  (menu → inbox → confirm → queue → `start()`); ext v0.5.0 bundles best
  audio with every video-only row.
- DASH: BaseURL + number-SegmentTemplate (+init), best V+A, ffmpeg mux,
  DRM/live/Timeline refused. 19/19 tests green (HLS plain+AES, DASH
  video-only, mux-error paths, all fixtures local).

### 14. P2 YouTube decipher complete (2026-10-15)
Server-side `signatureCipher` decipher from YouTube's public `base.js`
(detect reverse/splice/swap chains, apply to `s`, assemble URL) + `n`-fix
when a second simple-shape function exists (else untouched + ratebypass) +
button-naming guidance errors. Fixture-tested parse→apply→assemble→n.
Honest note: exotic player shapes fall back safely; extension fresh links
remain the primary path. 20/20 tests green.

### 17. Pasted-spec hardening round (2026-10-15)
1. No console window in release builds (`windows_subsystem`), logs already
   file/in-app bound. 2. Truncation: part-length enforcement, unknown-total
   re-probe resume, advertised-length checks on HLS/DASH segments, and the
   phantom-Completed guard now requires unknown totals. 3. Filename
   placeholders/hashes skipped in URL guessing. 4. Progress window: compact
   tabs (Info/Speed/On Completion), mini part bars + chunk map, live global
   speed cap + finish-action commands, aligned buttons. 5. Settings two-pane
   sidebar (Appearance/Download Engine/Categories & Rules/Browser/Updates)
   with theme buttons; Material Icons registered in every window and
   unicode glyphs replaced (no more tofu). 6. Extension `query` presence
   pre-check marks ✓ Queued rows; sniffer covers m4s/ts; ghost-lock risk
   contained by stale-stealing locks. 24/24 tests green.

### 15. P3 convenience parity complete (2026-10-15)
- Grabber (`src/grabber.rs` + toolbar button + dialog): presets, custom
  exts, same-site depth-1 crawl, preview, queue-all deduped.
- Rules + templates: ordered ext→folder rules (max 8) + `{name}{ext}`
  `{date}` `{host}` templates with preview, everywhere including confirm.
- Basket (`--basket`: paste + drop .txt/.url) and Mini (`--mini`: live
  counts/speed/bar, pause-all/resume) standalone windows, both smoke-tested.
- Speed windows (overnight-capable, live atomic cap, no restart needed) +
  opt-in GitHub update check. 24/24 tests green, roadmap all-DONE except
  forever-out-of-scope (DRM, IDM code).

### 16. Full bug audit fixes (2026-10-15)
Three-layer audit (engine / app+IPC / extension), everything fixed:
- IPC integrity: `model::fs_lock` guards every queue/inbox/wake/cmd
  read-modify-write; `save_queue` merges foreign ids via `known_ids`
  (own deletions stay deleted); `alloc_id` refreshes under lock; corrupt
  queue files quarantine instead of nuking; inbox capped at 200 with
  metadata refresh on re-capture; host survives malformed frames.
- Engine: real `$RepresentationID$`, HLS byte rollback, resume-map file
  checks, saturating counters/arithmetics, poison-proof mutexes, per-job
  locks + unique work dirs, decrypt abort, cleanup-on-error, typed final
  HTTP errors, rep-level startNumber, char-safe template expansion.
- App: concurrency-capped start_queue, phantom-completion guard, immediate
  cancel/pause state, finish-action ignores paused/failed with a log line,
  heartbeat liveness instead of process counting, safe argv parsing,
  xdg-open fallback.
- Extension v0.5.1: erase-after-ack capture, per-tab popup peek, debounced
  gated observer, same-video itag swaps, re-merging sniff section, single
  delegated click closer, validated filters/URLs, referrer on sizing,
  Firefox ESR floor declared. 24/24 tests green.
