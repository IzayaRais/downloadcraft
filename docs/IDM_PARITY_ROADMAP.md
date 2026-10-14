# IDM Parity Roadmap — what IDM has that Download Craft doesn't (yet)

Date: 2026-10-14 · App v0.6.0 · Extension v0.4.0
Owner intent: reach IDM-style UX with clean-room code. Other AI agents:
implement phases below in order; each phase lists files + acceptance criteria.

> LEGAL CLEAN-ROOM NOTICE (read first):
> - IDM (Internet Download Manager) is commercial proprietary software.
>   Observed install: `C:\Program Files (x86)\Internet Download Manager\`
>   (`IDMan.exe`, `IDMGrHlp.exe`, `idmmkb.dll`, `IDMNetMon*.dll`,
>   `idmwfp*.sys`, `idmtdi*.sys`, `IDMGCExt.crx`, `IDMEdgeExt.crx`,
>   `idmmzcc*.xpi`, `IDMMsgHost.exe`, etc.).
> - NOTHING was copied into this repo: no exe/dll/sys/crx/xpi, no
>   decompiled output, no IDM JS. All work below is black-box behavioral
>   re-implementation (observable browser/OS behavior only).
> - `https://github.com/morluto/rea` was evaluated and NOT used: it is a
>   generic RE-agent MCP framework, not an IDM/YouTube library. No install needed.
> - NEVER bypass DRM (Widevine/FairPlay/PlayReady). NEVER decompile IDM to
>   extract cipher/HLS logic. YouTube cipher work must target YouTube's own
>   public player JS (like yt-dlp does), never IDM binaries.

## 1. How the RE was done (so you can reproduce without IDM sources)

Method: black-box only.
1. Listed IDM install dir (names/sizes, see above) — confirms architecture:
   browser extensions (CRX/XPI) + native msg host (`IDMMsgHost.exe`) +
   network monitors (drivers + `IDMNetMon`) + shell integration.
2. Observed IDM UX from public knowledge + local install presence:
   one-time extension enable, floating "Download this video" panel on every
   HTML5 video, auto-takeover of downloads, per-quality list, Referer/cookie
   forwarding, scheduler/queue/categories, scheduler + shutdown actions.
3. Compared against this repo (`README.md`, `src/*.rs`, `browser-extension/*`).
   Result is the parity matrix in §2 and the phase plan in §4.

Key repo contracts (do not break):
- Native protocol (stdin 4-byte LE + JSON):
  `{"type":"ping"}` → `{"type":"pong","version":1}`
  `{"type":"download","url":"https://…","filename":null|"x.mp4","pageUrl":"https://page…","referer":"…","cookie":"…","mime":"video/mp4"}` → `{"type":"ok","queued":true}`.
  Code: `src/native_host.rs` (`Incoming`, `InboxItem`, `inbox_path()` =
  `%APPDATA%/download_craft/native_inbox.json`).
- GUI inbox import: `src/app.rs:poll_inbox()` prefills Add dialog
  (`add_url/add_file/add_referer/add_cookie`) and opens it IDM-style.
  Host auto-repairs every launch: `ensure_native_host()` for
  chrome/edge/firefox (HKCU `…\NativeMessagingHosts\com.download_craft`).
- Extension messages (`browser-extension/background.js`, `content.js`
  `EXT_VER=0.4.0`): `{type:"download",url,filename,ver,pageUrl,referer,mime}`,
  `{type:"getMedia"}` → `{items:[{url,mime,size}]}`, `{type:"getState"}`.
- Engine entry: `src/engine.rs:download(url,dest,connections,opts,shared)`.
  Page→media: `resolve_media()` (YouTube player-API + `ytInitialPlayerResponse`
  + `resolve_generic()` for og:video/ld+json VideoObject/bare links).
  HLS VOD: `download_hls()` (plain only; `EXT-X-KEY`/live/oversize rejected).
  Helpers: `is_playlist_url()`, `has_direct_ext()`, `hls_pick_variant()`,
  `hls_segments()`, `hls_abs()`.

## 2. Parity matrix (IDM vs Download Craft today)

| # | IDM capability | Download Craft status | Gap |
|---|---|---|---|
| B1 | Chrome/Edge/Firefox integration, one-time setup, auto-repair | DONE — unpacked load once + `ensure_native_host()` auto-repairs on launch (`src/app.rs:1058,2199`) | Only UX text; needs Opera/Brave/Vivaldi + store packaging |
| B2 | Auto-takeover of downloads (`downloads.onCreated` cancel+requeue, fallback to browser if host down) | DONE (`background.js`) | Needs per-site / per-type filters |
| B3 | Floating "Download this video" on EVERY video + quality menu | PARTIAL — YouTube button + universal `<video>` pill + `getMedia` sniffing (`content.js`, `background.js` webRequest) | Needs in-browser progress panel, audio-only pill, iframe support |
| B4 | Network monitor for media (headers sniffing) | PARTIAL — `webRequest.onHeadersReceived` for video/audio/m3u8/mpd, 25/tab | Needs size/duration heuristics, duplicate-ranking, `declarativeNetRequest` MV3-proofing |
| B5 | Referer/cookie/auth forwarding | PARTIAL — pageUrl→referer prefill (`native_host.rs`, `app.rs:poll_inbox`); engine has referer/cookie/basic-auth/proxy opts | Needs cookie-jar export, auth-header passthrough |
| Y1 | YouTube progressive direct links | DONE via extension (browser-deciphered) + server player-API fallback (`engine.rs`) | Ciphered-only videos need page/extension path; no server-side signature decipher |
| Y2 | YouTube adaptive 1080p+ (video-only + audio merge) | MISSING — extension lists v-only/a-only but engine can't mux | Needs ffmpeg merge step |
| Y3 | YouTube signature-cipher (`sig`/`n` decipher from player JS) | MISSING server-side (extension path covers most) | Needs clean-room player-JS mini-interpreter or yt-dlp delegation |
| H1 | HLS plain VOD assembly | DONE (`download_hls()`: master→best variant→concat, pause/cancel/limit) | Needs byte-progress via segment HEAD, parallel segments |
| H2 | HLS AES-128 (`EXT-X-KEY`) | MISSING (rejected loudly) | Needs `aes` crate + key fetch with referer |
| H3 | DASH (`.mpd`) video+audio mux | MISSING (rejected loudly) | Needs MPD parse + ffmpeg mux |
| S1 | Site grabber / batch / spider | MISSING | New feature: crawl page links by filter |
| S2 | Scheduler + queue + concurrency + retry-all + shutdown/sleep/hibernate | DONE (`app.rs`, `model.rs`) | Needs per-download schedule, speed-scheduler |
| S3 | Categories + file-type routing + auto-folders | PARTIAL (`category_of()`, `category_folders`) | Needs customizable type→folder rules UI |
| S4 | Speed limiter (global token-bucket) | DONE (`RateLimiter`) | Needs per-download cap + scheduler |
| S5 | Segmented 1–16 + resume via Range | DONE (`segmented()`, `single_stream()` resume) | Needs dynamic re-split of slow segments |
| S6 | Proxy (incl. PAC), NTLM/Kerberos | PARTIAL (static proxy URL + basic auth) | Needs PAC + system-proxy + NTLM |
| S7 | FTP support | MISSING (HTTP/S only) | New protocol backend |
| S8 | Antivirus hook, finish actions, notifications, tray, clipboard watch | DONE | Needs per-category AV + post-scripts |
| S9 | Drag-drop basket / mini progress window | MISSING | New UI |
| S10 | Multilingual UI, update checker, portable updater | MISSING | New |
| S11 | Opera/Brave/Vivaldi + Web Store signed builds | MISSING (unpacked + pinned key only) | Needs store publish + `allowed_origins` per ID |

Out of scope FOREVER: DRM removal, IDM binary/code copying, license-key workarounds.

## 3. Roadmap phases (implement in order)

### P0 — Browser-catch hardening (no new deps) — DONE 2026-10-14 (ext v0.4.1)
- [x] P0.1 Per-site + per-type auto-capture filters (`blockHosts`, `minSizeKB` in
      `background.js:settings()`, `setFilters` message, popup UI). Skips blocked
      hosts in `downloads.onCreated` + sniffer; playlists always pass size gate.
- [x] P0.2 In-browser progress + iframe videos (`all_frames:true` in both
      manifests; per-tab badge count in `noteMedia()`, cleared on tab close).
- [x] P0.3 Cookie-jar + auth passthrough (`cookiesFor()` same-host, ≤40 cookies,
      ≤6 KB, never logged; `native_host.rs` already accepted `cookie`).
- [x] P0.4 Opera/Brave/Vivaldi support (`native_host_key()`/`legacy_host_key()`,
      auto-repair loop covers 6 browsers, Settings Brave/Opera buttons,
      `ButtonId::SBrave/SOpera`).

### P1 — Stream assembly (light deps allowed)
- [ ] P1.1 HLS hardening: parallel segments (4–8), byte progress via ranged HEAD,
      resume map. Files: `engine.rs:download_hls()`. Accept: 200 MB VOD completes,
      pause/resume keeps segments, `cargo test` green.
- [ ] P1.2 HLS AES-128: `EXT-X-KEY` fetch (referer-scoped), `aes-128-cbc` decrypt,
      IV from playlist or sequence. New dep: `aes` + `cbc` (or `openssl` if present).
      Accept: SAMPLE-AES rejected still; AES-128 VOD decrypts; key URL never logged.
- [ ] P1.3 Adaptive merge via ffmpeg (optional external): when extension sends
      `v-only + audio` pair or engine picks DASH/HLS variants, call `ffmpeg -i v -i a -c copy`.
      Files: `engine.rs` (ffmpeg probe `which ffmpeg`, fallback error), Settings UI path.
      Accept: 1080p v-only + m4a → single `.mp4` when ffmpeg present; clear error otherwise.
- [ ] P1.4 DASH `.mpd` (non-DRM): parse Representation set, download best V+A,
      ffmpeg mux. Accept: test vector MPD → mp4; DRM manifests rejected with message.

### P2 — YouTube robustness (no IDM code, target YouTube's public player only)
- [ ] P2.1 Server-side `signatureCipher` support: fetch `base.js` player, extract
      decipher operations (reverse/splice/swap), apply to `s` param — clean-room,
      tested against fixtures (NEVER copy yt-dlp verbatim; re-implement + cite).
      Accept: pasted watch URL with ciphered-only formats resolves without extension.
- [ ] P2.2 `n`-param throttle bypass same way. Accept: googlevideo 403-throttle gone
      on test vectors; no hardcoded player version.
- [ ] P2.3 Extension-first UX: when server resolve fails with bot-wall, Add dialog
      deep-links "open in browser + click pill". Files: `app.rs` error text.
      Accept: wall error shows one-click guidance.

### P3 — IDM-convenience parity
- [ ] P3.1 Site grabber: crawl same-origin links by extension filter, queue batch.
      Files: new `src/grabber.rs` + GUI dialog. Accept: gallery page → N queued, deduped.
- [ ] P3.2 Type→folder rules + rename templates (`{title} [{q}].{ext}`).
      Files: `model.rs`, Settings UI. Accept: mp4→Video dir, rule reorder persists.
- [ ] P3.3 Mini progress window / drag-drop basket. Files: `app.rs` (second viewport).
      Accept: drop URL onto basket queues it.
- [ ] P3.4 Scheduler v2 (per-item time + speed windows) + update checker.
      Accept: nightly 2–6 AM full-speed window works; update check is opt-in.

## 4. Test matrix (every phase must keep green)
- `cargo test` (unit + local HTTP matrix, no network) — currently 14 passed.
- `node --check browser-extension/*.js`.
- Manual: YouTube watch + Shorts, plain MP4 page, Vimeo page, HLS VOD URL
  (`test/test.m3u8`-style local fixture preferred), 403-referer page.
- Regression: unpacked reload → tab reload (Ctrl+R) → pill + popup qualities →
  Add dialog prefill (referer) → segmented + HLS + checksum paths.

## 5. RE evidence log (append here, keep clean-room)
- 2026-10-14: listed `C:\Program Files (x86)\Internet Download Manager\`
  (exe/dll/sys/crx/xpi inventory in `docs/IDM_INTEGRATION_NOTES.md`).
  Implemented v0.4.0 extension (sniffer + universal pill + pageUrl/referer) and
  `download_hls()` plain VOD. No IDM code copied. `cargo test` 14/14.
- Next RE entries: record URL + behavior + fixture hash only. No binaries in git.
