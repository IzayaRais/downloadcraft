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

### P1 — Stream assembly (light deps allowed) — DONE 2026-10-15
- [x] P1.1 HLS hardening: 4–8 parallel segments (semaphore), byte totals via
      concurrent ranged HEADs, `hlsmap` resume sidecar (skips done segs across
      restarts), per-seg files merged in order. Fixture e2e byte-exact.
- [x] P1.2 HLS AES-128: vendored FIPS-197 `src/aes128.rs` (computed S-boxes,
      zero new deps — crates.io was unreachable), key fetch referer-scoped,
      explicit-IV or sequence IV, SAMPLE-AES/rotation refused. FIPS-197 B +
      NIST SP 800-38A vectors green (.NET cross-checked) + fixture e2e exact.
- [x] P1.3 Adaptive merge via ffmpeg: `download_adaptive()` (concurrent legs,
      aggregated progress, MP4 stays / WebM→MKV), `ffmpeg_mux()` (`-c copy`,
      no-window flag, clear install guidance), pair plumbing end-to-end
      (menu rows bundle best audio → `pairUrl` → inbox → confirm note →
      queue `pair_url` → `start()` adaptive). Fixture: audio-less exact;
      audio+no-ffmpeg → loud ffmpeg error. Real 1080p+ merge needs ffmpeg
      installed (`winget install ffmpeg`).
- [x] P1.4 DASH `.mpd` (non-DRM): hand-rolled MPD parse (BaseURL +
      number-based SegmentTemplate incl. init, `%0Nd` padding, durations),
      best video+audio, ffmpeg mux; DRM/live/Timeline loudly refused.
      Fixture e2e: video-only BaseURL exact; V+A without ffmpeg → ffmpeg error.

### P2 — YouTube robustness (no IDM code, target YouTube's public player only) — DONE 2026-10-15
- [x] P2.1 Server-side `signatureCipher`: `base.js` fetch (ops cached per
      player URL), classic-shape decipher detection (reverse/splice/swap via
      helper-object analysis, shapes detected never hardcoded), `s` transform
      + URL assembly in the page path. Fixture e2e of parse→apply→assemble.
      Debugging log: two real parser bugs found by fixtures (needle included
      `=` so body started ON `{`; method loop choked on `,MB:` separators).
- [x] P2.2 `n`-param: same-shape second function applied ONLY when confidently
      distinct from the decipher (`try_simple_n`, fixture-tested); otherwise
      the original `n` + `ratebypass=yes` is kept (never worse). No full JS
      interpreter — exotic players fall back safely by design.
- [x] P2.3 Extension-first UX: bot-wall + no-link errors now name the floating
      ⬇ button + quality menu explicitly (`src/engine.rs`).

### P3 — IDM-convenience parity — DONE 2026-10-15
- [x] P3.1 Site grabber: `src/grabber.rs` (pure tag/meta extraction, relative
      URLs, same-host depth-1 crawl capped 15 pages/200 links, dedupe) + GUI
      dialog (URL, Mixed/Video/Audio/Images/Documents/Archives preset cycle,
      custom exts, depth check, Grab → preview → Queue-all deduped vs queue).
      Fixture-tested incl. relative resolution.
- [x] P3.2 Type→folder rules + rename templates (`{name}{ext}{date}{host}`):
      ordered first-match rules (max 8, defaults = categories), template with
      ext preservation + fallback, applied at probe/inbox/import/confirm
      suggestion points (never double-applied), Settings section with preview,
      persisted in `ui.json`. `model.rs` fixture-tested.
- [x] P3.3 Mini progress window / drag-drop basket: `--basket` (paste + drop
      `.txt`/`.url` files, recent queue) + `--mini` (active/queued/speed/bar,
      Open-main, Pause-all/Resume via `cmd.json`, Close never stops). Both
      smoke-tested alive. (Standalone `--progress` windows from before cover
      the per-download half.)
- [x] P3.4 Scheduler v2 (speed windows) + update checker: `SpeedWindow`
      HH:MM ranges with overnight wrap, live `AtomicU64` cap plumbed through
      `EngineOpts.global_limit` into every limiter (running downloads obey
      without restart; `u64::MAX` = no window), Settings section with live
      status, persisted. Opt-in GitHub-releases checker (manual + once per
      session, failures are status text, no auto-download). Pure helpers
      fixture-tested; 24/24 green.

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
