# Engine Roadmap — make Download Craft stronger than any downloader

Date: 2026-10-15 · App v0.7.0 · Extension v0.5.8
Reader: future AI agents. All phases P0–P5 are done. New work goes under
a new phase section below (keep the same checkbox + test discipline).
Rules: clean-room only (no IDM code/binaries, no DRM bypass — see
`docs/IDM_PARITY_ROADMAP.md` legal notice). Every phase: unit + local-HTTP
fixture tests, `cargo test` green, no new deps unless phase allows.

## Session history (why the code looks like this)

- FB slice URLs: extension captured `scontent…mp4?bytestart=967&byteend=…`
  (1.1 MB preview) while total was 9.4 MB → endless
  `connection ended early (got 1192564 of 9495549)`. Fix:
  `normalize_media_url()` strips `bytestart`/`byteend` on social CDNs
  (`src/engine.rs:download()` + `download_adaptive()`), same strip in
  `browser-extension/background.js` + `content.js`.
- Extension thumbnails (v0.5.7/0.5.8): `ytThumb()` (player JSON → `hqdefault`
  fallback), FB `og:image`, generic `<video poster>`; popup + in-page menus.
- YouTube adaptive on this network: SABR-gated/`gir=yes` serves ~2 MB preview
  then persistent 403; ciphered VEVO pages expose zero direct URLs. Fix:
  one fresh re-resolve with auto-downgrade to 720p (`download()` adaptive
  branch), refusal backoff 4→2 (fail fast, no 0 B/s hang), ciphered clicks
  prefer sniffed fresh URLs by itag.
- Duplicate-queue collision (`already downloading elsewhere`): `find_dest_clash()`
  — a fresh capture of the same file refreshes the entry (new URL, progress
  reset, `drop_resume_state()`) in `model::append_to_queue`,
  `app::queue_url`, `merge_queue_from_disk`, `cli::Add`.
- Phantom progress (bar 31% → 20%): failed adaptive legs deleted work files
  but kept the byte counter. Fix: reset `shared.downloaded` on leg/mux
  failure; `poll_tasks` logs `Retrying <file> with a fresh link` on
  total-change restarts.

## Key repo contracts (do not break)

- Engine entry: `src/engine.rs:download(url,dest,conns,opts,shared)`.
  Transfer fns: `single_stream` (plain resume), `chunked_stream` (1 MiB
  bounded ranges for fragile `fbcdn/scontent/googlevideo` CDNs),
  `segmented`/`download_range` (N-way), `download_hls` (`hlsmap` sidecar),
  `download_dash`, `download_adaptive` (ffmpeg mux, unique `dcwork` dir).
- Resume files next to dest: `<name>.dcpart` (single/chunked),
  `<name>.dcpart{i}` (segmented parts), `<name>.dcmeta` (P0 binding:
  `{url,total,conns}` JSON), `<name>.hlsmap` (HLS). `drop_resume_state()`
  in `src/model.rs` deletes all of them — use it on every URL replacement.
- Queue: `%APPDATA%/download_craft/queue.json` (`DownloadItem`), inbox
  `native_inbox.json`; locks via `model::fs_lock(name)`; same-dest guard
  `claim_dest_job()` in engine.
- Extension protocol: native `{"type":"download",url,filename,pageUrl,referer,
  cookie,mime,size,pairUrl,pairMime,quality}`; `EXT_VER` must match in
  `content.js` + `background.js` + both manifests.
- Test matrix: `cargo test` (unit + local-HTTP fixtures, no network);
  resume-sensitive: `local_http_matrix`, `facebook_cdn_matrix`,
  `strict_edge_matrix`; plus `resume_meta_binding`, `fb_slice_normalize`,
  `dest_clash_refresh`, `dispositions`, `url_checks`, `mime_naming_matrix`,
  `halve_schedule`, `dash_timeline`, `ftp_full_and_resume`,
  `ftp_login_refused`, `p5_overrides_and_compat`, `update_asset_pick`.

## P0 — Resume integrity (DONE 2026-10-15)

Goal: a `.dcpart` prefix is resumable ONLY for the exact bytes that wrote it.
- [x] `<name>.dcmeta` sidecar `{url,total,conns}` (`ResumeMeta`,
  `resume_offset()`, `meta_matches()` in `src/engine.rs`).
- [x] Enforced in `single_stream_once`, `chunked_stream` (+416 total-refresh
  rebind), `segmented` (set-level, drops `dcpart{i}` on split change).
- [x] Legacy sidecar-less partials resume once by length, then bind
  (one-time hole, self-heals; keeps old fixtures green).
- [x] `drop_resume_state()` + FB-normalize + fresh-link retry paths all drop
  `.dcmeta` too. Unit test `resume_meta_binding`; matrix green.

## P1 — Filename capture (DONE 2026-10-15)

Chain (documented at the `download()` rename site, highest wins): server
Content-Disposition > resolved media title > probe disposition > queue /
extension name > URL fallback; MIME type repairs the extension when the URL
yields nothing usable.
- [x] `disposition_filename()` quote-aware (`"a;b.mp4"` survives),
  case-insensitive params, RFC 5987 `filename*` priority (`src/engine.rs`,
  `dispositions` matrix).
- [x] `filename_from_url()`: `reel`/`reels`/`embed`/`post`/`posts`/`share`
  placeholders + all-digit ID skip (`/reel/113054…` → fallback, never the
  ID as a name); fbcdn tails still win (`url_checks`).
- [x] `ext_for_mime()` + `is_fallback_filename()` +
  `filename_from_url_with_mime()` (`src/model.rs`): `download_123.bin` +
  video/mp4 → `.mp4`; bare `song` + audio/mpeg → `song.mp3`; suffixed
  names untouched; unknown types keep `.bin` (`mime_naming_matrix`,
  10-URL acceptance).
- [x] Engine post-probe rename for fallback/extensionless names from probed
  MIME (non-HTML only); confirm dialog probe thread same (probed MIME,
  else extension `--mime`, newly plumbed extension → `native_host
  spawn_confirm` → `ConfirmApp.page_mime`).
- [x] Extension audit: every `{type:"download"}` message carries
  filename/size/mime keys (null where unknowable for page URLs); media
  rows always send all three. No JS change needed.
- [x] Full suite 32/32 green (incl. all HTTP matrices).

## P2 — Throughput (DONE 2026-10-15)

- [x] Dynamic connections: `halve_conns()` 16→8→4→2 in `download()` —
  throttled segmented failures (416 / cut-short parts / `Range` / 429 via
  `segmented_throttled()`) halve the split and retry before collapsing to
  one sequential stream; hard refusals (401/403/410) still go straight
  single. Stale part sets are dropped by the P0 resume binding (conns are
  part of the sidecar); the progress bar holds its max across re-splits.
  Unit test `halve_schedule`.
- [x] Parallel DASH segments: `dash_fetch_blob()` extracted as a standalone
  task-safe fetch; `dash_rep_download()` fetches media segments in bounded
  parallel (`connections.clamp(2, 8)`, semaphore, indexed `dashpart{i}`
  files) with strictly ordered concat; init segment stays sequential;
  failures clean up parts. New `/dash/tpl.mpd` fixture (init marker +
  3 ordered thirds) in `local_http_matrix`. `download_dash()` now honors
  `connections` instead of ignoring it.
- [x] Connection reuse: pooled keepalive (`pool_idle_timeout` 30 s,
  `pool_max_idle_per_host` 16) + `tcp_keepalive` 60 s in
  `EngineOpts::build_client()`. HTTP/1.1-only + IPv4 pin KEPT: media edges
  demonstrably RST H2 and serve bot-walls/cuts over broken IPv6 (see code
  comments); flipping either needs a live CDN proof run first.
- [x] Full suite 33/33 green (incl. all HTTP matrices + new DASH case).
- [x] P2 follow-up — YouTube speed (2026-10-15): `google_parts()` sizes
  googlevideo splits to the strict ~2 MiB window without exceeding what
  was asked; `SegFallback` decision table (Halve → Chunked → Single,
  unit-tested) now routes googlevideo segmented failures through one
  chunked round before the single stream; new `adaptive_leg()` helper
  gives adaptive video/audio legs the same segmented-first policy
  (previously forced sequential chunked — the actual slow-vs-IDM gap),
  with halving + chunked fallback and no progress double-count.
  Tests `google_split_sizing`, `seg_fallback_table`; full suite green.

## P3 — Protocols (DONE 2026-10-15, except SFTP/BitTorrent — see bottom)

- [x] DASH SegmentTimeline: `timeline_count()` counts uniform contiguous
  timelines (`r` repeats, `r="-1"` with known period); varying/gapped
  timelines stay loud errors. Wired per-rep (rep wins) + set-level
  (rep-blocks excluded) into `parse_mpd()` → `DashRep.timeline_count`,
  preferred over duration math in `download_dash()`. Unit test
  `dash_timeline` + `/dash/tl.mpd` e2e (no MPD duration, count purely
  from timeline).
- [x] HLS key rotation: `hls_key_schedule()` per-segment schedule (NONE
  clears to plain; SAMPLE-AES/unknown still loud); distinct key URIs
  (cap 8) fetched once into a map; per-segment IV explicit-or-sequence.
  `hls_map_init()`: EXT-X-MAP init fetched up front (BYTERANGE refused
  loudly) and prepended at merge, counted in progress. Fixtures
  `/hls-rot/` (k1/k2, mixed IVs) + `/hls-map/` (init + 2 segs), e2e exact.
- [x] Plain FTP backend (`src/ftp.rs`, zero new deps): PASV binary RETR +
  REST resume on the shared `.dcpart`/`.dcmeta` contract, SIZE totals,
  login errors naming username/password, FTPS loudly refused (no silent
  plaintext downgrade). Routed in `download()` with conflict/overwrite
  handling; `is_download_url()` admits `ftp(s)://`. Fixture FTP server +
  `ftp_full_and_resume` / `ftp_login_refused` tests.
- [ ] SFTP + BitTorrent/magnet: need SSH/torrent crates (crates.io was
  unreachable when P1.2 vendored AES). Do NOT hand-roll SSH or BitTorrent
  wire protocols — revisit with network access.
- [x] Full suite 36/36 green (incl. all HTTP matrices + new cases).

## P4 — Capture intelligence (DONE 2026-10-15)

- [x] Shared ranking (`browser-extension/rank.js`, no chrome APIs, UMD for
  node): playlist/segment/file kinds, score (file > playlist > segment,
  segments demoted to -1 under a known playlist = ad/part skip), canonical
  slice-collapsing identity, cross-tab collapse (biggest wins, tab count),
  `m:ss` durations. `rank.test.js`: 7/7 via `node --test`.
- [x] Sniffer (`background.js`): per-tab playlist memory demoting loose
  `.ts`/`.m4s`/`.cmfv` children (menus offer the whole, badge counts
  downloadables only); `seenTabs` canonical map annotating `getMedia`
  items with `tabs` ("×N tabs" hint); `dur` threaded content →
  background → native host. Extension v0.5.9 (rank.js wired in both
  manifests + popup; EXT_VER matched).
- [x] Menus (`content.js`/`popup.js`): `bestSniffed` + FB rows + popup list
  rank/collapse via DCRank (legacy inline scoring kept as fallback);
  durations on YT rows/ladder (`_dur`), FB rows (`durSec`), generic pill
  (`<video>.duration`); `dur` sent on every quality click.
- [x] App (`src/`): `dur` plumbed native InboxItem → `--dur` →
  `ConfirmApp.duration_secs` badge (`3:26 • 12.4 MB`) → stored
  `DownloadItem.duration_secs` (serde-default, display/metadata only);
  `queue_grab_results()` routed through `queue_url()` (now bool) so
  gallery Download-all dedupes + refreshes + autostarts like every capture.
- [x] Full suite 36/36 Rust + 7/7 node green.
- Known limitation (documented, not a bug): response headers carry no
  duration — durations come from page JSON/DOM only; tiny-but-plausible
  ad segments below the size gate were already skipped by `minSizeKB`.
- Extension UI themes (v0.6.0): in-page download menus (YouTube + Facebook)
  follow `prefers-color-scheme` with a per-page ☀/☾ toggle; light card
  follows `extension ui.png` (white, gray sections, bordered pills).
  Theme tokens + paint code live in `rank.js` (node-tested with stubbed
  DOM: `menu paint` suite); `content.js` delegates with dark fallback.
  Popup follows the OS via CSS media query.
- Known hard limit (verified live 2026-10-15): label-gated music streams
  (e.g. Diya Jahan VEVO-type) 403 EVERY non-browser client — the browser's
  own fresh itag-18 URL 206s in-page yet 403s via this engine (rustls),
  curl/Schannel over H1 and H2 alike. Cause is server-side session
  attestation (`siu`/`spc` params), not URL freshness or headers — no
  downloader change can pass it; the app correctly fails fast with the
  fresh-row guidance instead of hanging.
- YouTube speed + IDM gap, diagnosed live 2026-10-15 (see P2 follow-up):
  (a) today's `base.js` (embed `0229cd64`, and by shape the watch player)
  contains ZERO classic-shape decipher functions — P2.1 sig/`n` parsing is
  a proven-harmless no-op on current players (API-issued `n` passes
  through untouched); wrong-`n` throttling is therefore NOT our slow
  cause; (b) real slow cause was routing: googlevideo adaptive legs were
  forced sequential 1 MiB chunked while IDM runs 8–16 parallel ranges;
  (c) baseline: 10 MB parallel download runs at line rate (~7 MB/s peak,
  = curl single-stream on the same link — link-limited, healthy).

## P5 — Reliability UX (DONE 2026-10-15)

- [x] Per-download retry/timeout: `DownloadItem.max_retries/timeout_secs`
  (`None` = global; serde-default so old queues parse), `eff_retries()` /
  `eff_timeout()` helpers, Details → Settings sliders (0 = global),
  plumbed through GUI `start()` + CLI `opts_for()`. Test
  `p5_overrides_and_compat` (incl. old-JSON compat).
- [x] ETA smoothing + speed graph: EWMA (α=0.4) readout in `poll_tasks`
  (feeds existing `format_eta`), raw half-second samples in capped
  per-download history (60, pruned), mini area chart with peak in the
  Details Info tab.
- [x] Checksum auto-repair: first `checksum mismatch` failure per digest
  (`checksum_repaired` latch, reset on checksum change) drops resume
  state and redownloads from scratch once, then fails loudly. Covers all
  backends uniformly at the app layer.
- [x] Portable updater: `pick_windows_asset()` (portable → x64 → windows →
  any zip, fixture-tested `update_asset_pick`); About modal gained
  Check-for-updates + Download-update (new `UpdDownload` button) which
  queues the asset as a normal download. Never self-replaces (Windows
  locks the running exe) — unzip guidance in the dialog + log.
- [x] Full suite 38/38 green (incl. all HTTP matrices).
