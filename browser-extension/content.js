// Content script: collects downloadable links AND page media
// (<video>/<audio>/<source>) so the popup can one-click them.
// On YouTube watch pages it additionally extracts direct progressive
// streams from the page player (real browser session: real cookies/TLS,
// so no bot-wall) and offers an in-page download button with qualities.
(() => {
  // Must match manifest version + background.js EXT_VER. The background
  // rejects messages from any other version with a "reload this tab"
  // error, so a stale script can never fail mysteriously.
  const EXT_VER = "0.9.7";
  // Promise-style calls (the `await api.*` one below) go through this handle
  // so one file serves both engines: Firefox exposes the promise-returning
  // `browser` namespace, Chrome only has `chrome` (promise-capable under MV3).
  // Callback-style calls keep using `chrome`, which Firefox also provides.
  const api = (typeof browser !== "undefined" && browser.runtime) ? browser : chrome;
  // Generation guard: when the worker re-injects this script (after an
  // update), older live instances must go quiet instead of fighting over
  // the button. The newest instance always wins.
  const GEN = (window.__dcGen = (window.__dcGen || 0) + 1);
  const alive = () => window.__dcGen === GEN;
  // "Extension context invalidated" happens when Chrome reloads/updates the
  // extension while this page is open: every chrome.* call in the OLD script
  // then throws. Detect it once, shut down timers/observers (no spam), and
  // tell the user the one-click fix (reload the tab).
  let __dcDead = false;
  const __dcTimers = [];
  const __dcObservers = [];
  function ctxInvalidated(e) {
    const m = String((e && e.message) || e || "");
    return /extension context invalidated/i.test(m);
  }
  function markDead() {
    if (__dcDead) return;
    __dcDead = true;
    try { __dcTimers.forEach(clearInterval); } catch (e) {}
    try { __dcTimers.forEach(clearTimeout); } catch (e) {}
    try { __dcObservers.forEach((o) => { try { o.disconnect(); } catch (x) {} }); } catch (e) {}
    toast("Download Craft was updated — reload this tab (Ctrl+R), then click again");
  }
  const DL = /\.(zip|rar|7z|exe|msi|pdf|mp4|mkv|webm|mov|mp3|flac|wav|ogg|m4a|iso|tar|gz|dmg)$/i;

  function collectLinks() {
    try {
      const links = [...document.querySelectorAll("a[href]")]
        .map((a) => ({ url: a.href, text: (a.textContent || "").trim().slice(0, 80) }))
        .filter((l) => /^https?:\/\//i.test(l.url) && DL.test(l.url.split("?")[0]))
        .slice(0, 30);
      const media = [];
      const posterOf = (el) => {
        try {
          const v = el && el.tagName && el.tagName.toLowerCase() === "video"
            ? el
            : (el && el.closest && el.closest("video")) || null;
          const p = v && v.getAttribute && v.getAttribute("poster");
          return /^https?:\/\//i.test(p || "") ? p : "";
        } catch (e) { return ""; }
      };
      document.querySelectorAll("video[src], audio[src]").forEach((el) => {
        if (/^https?:\/\//i.test(el.currentSrc || el.src)) {
          media.push({ url: el.currentSrc || el.src, text: `Page media (${el.tagName.toLowerCase()})`, thumb: posterOf(el) });
        }
      });
      document.querySelectorAll("video source[src], audio source[src]").forEach((el) => {
        if (/^https?:\/\//i.test(el.src)) {
          media.push({ url: el.src, text: "Page media (source)", thumb: posterOf(el) });
        }
      });
      chrome.storage.local.set({
        downloadCraftLinks: [...media, ...links].slice(0, 50),
        downloadCraftPage: location.href,
      });
    } catch (e) {
      if (ctxInvalidated(e)) { markDead(); return; }
      /* page CSP may block storage; popup manual URL still works */
    }
  }

  // ---------- YouTube support ----------
  function ytVideoId() {
    try {
      const u = new URL(location.href);
      if (u.hostname.endsWith("youtube.com")) {
        if (u.pathname === "/watch") return u.searchParams.get("v");
        const m = u.pathname.match(/^\/(shorts|embed|live|v)\/([^/?#]+)/);
        if (m) return m[2];
      }
      if (u.hostname === "youtu.be") {
        const id = u.pathname.split("/").filter(Boolean)[0];
        if (id) return id;
      }
    } catch (e) {
      /* ignore */
    }
    return null;
  }

  // Balanced-brace scan: finds EVERY `var ytInitialPlayerResponse = {...};`
  // block without choking on braces inside strings. Returns all parsed
  // responses — YouTube's SPA keeps stale script tags from previous
  // videos, so callers must match by video id (never trust order).
  function playerResponses() {
    const out = [];
    try {
      for (const s of document.scripts) {
        const t = s.textContent || "";
        const marker = "ytInitialPlayerResponse = ";
        let p = t.indexOf(marker);
        while (p >= 0) {
          try {
            let i = p + marker.length;
            while (i < t.length && t[i] !== "{") i++;
            const start = i;
            let depth = 0;
            let inStr = false;
            let esc = false;
            let end = -1;
            for (; i < t.length; i++) {
              const c = t[i];
              if (inStr) {
                if (esc) esc = false;
                else if (c === "\\") esc = true;
                else if (c === '"') inStr = false;
              } else if (c === '"') {
                inStr = true;
              } else if (c === "{") {
                depth++;
              } else if (c === "}") {
                depth--;
                if (depth === 0) { end = i; break; }
              }
            }
            if (end > start) out.push(JSON.parse(t.slice(start, end + 1)));
          } catch (e) {
            /* malformed block: try the next one */
          }
          p = t.indexOf(marker, p + 1);
        }
      }
    } catch (e) {
      /* unparseable page variant */
    }
    return out;
  }

  function mimeExt(mime) {
    const m = String(mime || "").split(";")[0].trim().toLowerCase();
    if (m === "audio/mp4" || m === "audio/m4a" || m === "audio/x-m4a") return ".m4a";
    if (m === "audio/webm") return ".webm";
    if (m === "video/webm") return ".webm";
    return ".mp4";
  }

  // Short container tag for quality rows (IDM shows "MKV file / MP4 file").
  function contOf(mime) {
    const m = String(mime || "").split(";")[0].trim().toLowerCase();
    if (m === "audio/mp4" || m === "audio/m4a" || m === "audio/x-m4a") return "m4a";
    if (m === "audio/webm") return "weba";
    if (m === "video/webm") return "webm";
    if (m === "video/mp4") return "mp4";
    return (m.split("/")[1] || "mp4").slice(0, 5);
  }

  function ytThumb(pr, id) {
    try {
      const thumbs = pr && pr.videoDetails && pr.videoDetails.thumbnail && pr.videoDetails.thumbnail.thumbnails;
      if (Array.isArray(thumbs) && thumbs.length) {
        const valid = thumbs.filter((t) => t && /^https?:\/\//i.test(t.url || ""));
        if (valid.length) {
          valid.sort((a, b) => (b.width || 0) - (a.width || 0));
          return valid[0].url;
        }
      }
    } catch (e) { /* ignore */ }
    return id ? `https://i.ytimg.com/vi/${id}/hqdefault.jpg` : "";
  }

  function ytStreams() {
    const id = ytVideoId();
    if (!id) return null;
    // Strict id match: stale SPA script tags must never win.
    const prs = playerResponses();
    const pr = prs.find((p) => p && p.videoDetails && p.videoDetails.videoId === id) || null;
    const docTitle = document.title.replace(/ - YouTube$/, "");
    if (!pr || !pr.streamingData) {
      return { videoId: id, title: docTitle || "youtube_video", thumb: ytThumb(pr, id), streams: [], videoOnly: [], audio: [], subs: [] };
    }
    const title = (pr.videoDetails && pr.videoDetails.title) || docTitle || "youtube_video";
    // Stream URLs carry `expire=<unix>` and are session-locked: rows older
    // than ~2 min 403 in the app (IDM re-grabs instead of offering dead
    // links), so drop them here and let the sniffer/menu refresh refill.
    const nowS = Date.now() / 1000;
    const expOf = (u) => {
      try {
        const m = String(u || "").match(/[?&]expire=(\d+)/);
        return m ? parseInt(m[1], 10) : 0;
      } catch (e) { return 0; }
    };
    const fresh = (u) => {
      const e = expOf(u);
      return e === 0 || e > nowS + 120;
    };
    const direct = (arr) => (Array.isArray(arr) ? arr : [])
      .filter((f) => f && typeof f.url === "string" && f.url.startsWith("http") && fresh(f.url));
    // Sizes straight from the page JSON (contentLength per format): exact,
    // instant, no HEAD request — googlevideo HEADs 403 off-session, which is
    // why rows historically showed no size. Duration likewise.
    const sizeOf = (f) => {
      try {
        const n = parseInt(String(f.contentLength || "0").replace(/\D/g, ""), 10);
        return n > 0 ? n : 0;
      } catch (e) { return 0; }
    };
    const durOf = (f) => {
      try {
        const ms = parseInt(String(f.approxDurationMs || "0").replace(/\D/g, ""), 10);
        return ms > 0 && ms < 86400000 * 3 ? Math.round(ms / 1000) : 0;
      } catch (e) { return 0; }
    };
    const qlabel = (f) => {
      // qualityLabel is "1080p" but often lacks fps; append it like IDM
      // ("1080p60") when the stream is high-frame-rate.
      const q = f.qualityLabel || f.quality || "unknown";
      if (/^\d+p$/.test(q) && (f.fps || 0) >= 50) return `${q}${f.fps}`;
      return q;
    };
    // Ciphered entries carry signatureCipher instead of a direct URL (the
    // page hides URLs until playback). Offered as "(via app)" rows: the
    // click sends the page URL + quality hint and the desktop app deciphers
    // that exact quality server-side — this is what fills the menu on
    // VEVO/music videos that embed zero direct URLs.
    const ciphered = (arr, wantVideo) => (Array.isArray(arr) ? arr : [])
      .filter((f) => f && typeof f.signatureCipher === "string" && !f.url
        && (wantVideo ? String(f.mimeType || "").startsWith("video/")
                      : String(f.mimeType || "").startsWith("audio/")));
    const cipherRows = (arr, wantVideo, kind) => ciphered(arr, wantVideo).map((f) => {
      const mime = String(f.mimeType || "").split(";")[0];
      return { url: null, label: qlabel(f), mime, disp: contOf(mime), kind, itag: f.itag || 0, ciphered: true, _size: sizeOf(f), _dur: durOf(f), w: f.width || 0, h: f.height || 0 };
    });
    // Progressive = video+audio in one file (ready to play).
    const streams = direct(pr.streamingData.formats).map((f) => {
      const mime = String(f.mimeType || "").split(";")[0];
      return { url: f.url, label: qlabel(f), mime, disp: contOf(mime), kind: "av", itag: f.itag || 0, _size: sizeOf(f), _dur: durOf(f), w: f.width || 0, h: f.height || 0 };
    });
    cipherRows(pr.streamingData.formats, true, "av").forEach((s) => {
      if (!streams.some((x) => x.label === s.label && x.disp === s.disp)) streams.push(s);
    });
    // Adaptive video-only incl. WebM VP9/AV1 — this is where 1080p+/4K live
    // (IDM's "2160p 4K / 1440p HD" rows). Honestly video-only: the app labels
    // them "(no audio)" until the P1.3 ffmpeg merge lands.
    const seenQ = new Set();
    const videoOnly = direct(pr.streamingData.adaptiveFormats)
      .filter((f) => String(f.mimeType || "").startsWith("video/"))
      .map((f) => {
        const mime = String(f.mimeType || "").split(";")[0];
        return { url: f.url, label: qlabel(f), mime, disp: contOf(mime), kind: "v", itag: f.itag || 0, _size: sizeOf(f), _dur: durOf(f), w: f.width || 0, h: f.height || 0 };
      })
      .filter((s) => {
        const k = `${s.label}|${s.disp}`;
        if (seenQ.has(k)) return false;
        seenQ.add(k);
        return true;
      })
      .slice(0, 16);
    cipherRows(pr.streamingData.adaptiveFormats, true, "v").forEach((s) => {
      if (!videoOnly.some((x) => x.label === s.label && x.disp === s.disp)) videoOnly.push(s);
    });
    // Size/duration map over EVERY video format (even ones with no URL at
    // all): powers the generic ladder + popup rows with exact page-JSON
    // sizes, no network measuring needed.
    const sizes = {};
    for (const f of [...(pr.streamingData.formats || []), ...(pr.streamingData.adaptiveFormats || [])]) {
      if (!String(f.mimeType || "").startsWith("video/")) continue;
      const q = qlabel(f);
      const sz = sizeOf(f);
      if (sz > 0 && !sizes[q]) sizes[q] = { size: sz, dur: durOf(f) };
    }
    const audio = direct(pr.streamingData.adaptiveFormats)
      .filter((f) => String(f.mimeType || "").startsWith("audio/"))
      .map((f) => {
        const mime = String(f.mimeType || "").split(";")[0];
        return {
          url: f.url,
          label: (f.audioQuality || "").replace("AUDIO_QUALITY_", "") || (f.bitrate ? Math.round(f.bitrate / 1000) + "k" : "audio"),
          mime,
          disp: contOf(mime),
          kind: "a",
          itag: f.itag || 0,
          br: f.bitrate || 0,
          _size: sizeOf(f),
          _dur: durOf(f),
        };
      })
      .slice(0, 6);
    // Subtitle tracks (VTT): shown as "Other Files" chips in the panel and
    // sent to the app as plain file downloads — the timedtext URL is a
    // normal https GET, and `fmt=vtt` yields a straight .vtt file.
    const subs = [];
    try {
      const list = pr.captions && pr.captions.playerCaptionsTracklistRenderer
        && pr.captions.playerCaptionsTracklistRenderer.captionTracks;
      for (const c of (list || []).slice(0, 8)) {
        const u = String((c && c.baseUrl) || "");
        if (!/^https?:\/\//i.test(u)) continue;
        const nm = (c.name && (c.name.simpleText
          || (c.name.runs && c.name.runs[0] && c.name.runs[0].text))) || c.languageCode || "sub";
        // YouTube already says "(auto-generated)" — don't stack "(auto)" on
        // top of it, just normalize the label to a short one.
        const short = String(nm).replace(/\s*\([^)]*\bauto[^)]*\)\s*/i, "").trim() || c.languageCode || "sub";
        subs.push({
          url: /[?&]fmt=/i.test(u) ? u : u + "&fmt=vtt",
          label: `${short}${c.kind === "asr" ? " (auto)" : ""}`,
          lang: c.languageCode || "",
        });
      }
    } catch (e) { /* captions are optional */ }
    return { videoId: id, title, thumb: ytThumb(pr, id), streams, videoOnly, audio, sizes, subs };
  }

  function stripFbSlice(url) {
    try {
      const u = new URL(url);
      if (!u.searchParams.has("bytestart") && !u.searchParams.has("byteend")) return url;
      u.searchParams.delete("bytestart");
      u.searchParams.delete("byteend");
      return u.toString();
    } catch (e) { return url; }
  }

  function playerHasAudio(v) {
    return !!(v && (v.mozHasAudio === true || Number(v.webkitAudioDecodedByteCount) > 0
      || (v.audioTracks && v.audioTracks.length > 0)));
  }

  function sendDownload(url, filename, mime, size, pair, quality, dur) {
    // Use only the matching video or an unambiguous sole video. Subtitle,
    // audio-only and ordinary file links do not borrow video expectations.
    const evidence = typeof DCRank !== "undefined"
      ? DCRank.playerEvidence(document.querySelectorAll("video"), url, mime, location.href, dur)
      : {dur: Number.isFinite(Number(dur)) && Number(dur) > 0 ? Math.round(Number(dur)) : null, audioExpected:false};
    dur = evidence.dur;
    const audioExpected = evidence.audioExpected;
    // A tab running a stale script (extension reloaded without a tab
    // reload) has no extension APIs left — fail loudly, not mysteriously.
    try {
      if (!chrome || !chrome.runtime || !chrome.runtime.sendMessage) {
        throw new Error("page runs an older script — reload this tab (Ctrl+R), then click again");
      }
      void chrome.runtime.id; // throws when the context was invalidated
    } catch (e) {
      if (ctxInvalidated(e)) markDead();
      toast("Download Craft: " + (e && e.message ? e.message : e));
      return;
    }
    // blob: / mediasource streams cannot be fetched by the desktop app —
    // fall back to the page URL so the engine can resolve it server-side.
    // The extension keeps audio extensions for audio fallbacks.
    let sendUrl = stripFbSlice(url);
    if (/^blob:/i.test(url)) {
      sendUrl = stripFbSlice(location.href);
      if (!filename) {
        const base = String(document.title || "media").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim().replace(/\.+$/, "") || "media";
        filename = base + mimeExt(mime);
      }
    }
    // A player-wrapper page ("/jwplayer/?source=<encoded media>") is HTML;
    // the real stream is in the query. Unwrap it so we send the media, not
    // the shell, and rename from the real URL (the release folder for HLS).
    const inner = unwrapMediaUrl(sendUrl);
    if (inner) {
      sendUrl = stripFbSlice(inner);
      const base = mediaBase(sendUrl);
      const ext = mimeExt(mime) || outExtFor(sendUrl);
      filename = base.toLowerCase().endsWith(ext.toLowerCase()) ? base : base + ext;
    }
    try {
      chrome.runtime.sendMessage({ type: "download", url: sendUrl, filename, ver: EXT_VER, pageUrl: location.href, referer: location.href, mime: mime || null, size: size || null, pairUrl: (pair && pair.url) || null, pairMime: (pair && pair.mime) || null, quality: quality || null, dur: dur || null, audioExpected }, (res) => {
      if (chrome.runtime.lastError) {
        if (ctxInvalidated(chrome.runtime.lastError)) markDead();
        toast("Download Craft: " + chrome.runtime.lastError.message);
        return;
      }
      if (!res || !res.ok) {
        toast("Download Craft: " + ((res && res.error) || "app not reachable"));
      }
      });
    } catch (e) {
      if (ctxInvalidated(e)) markDead();
      toast("Download Craft: " + (e && e.message ? e.message : e));
    }
  }

  // ---------- naming ----------
  // A direct media URL often carries the release name
  // ("...Pooja.Meri.Jaan.2026.1080p.x264.AAC.Hin.BTs.VTL.mp4") while the page
  // title is just "Video", "Watch online" or the site name; on other sites
  // the reverse is true (hashed CDN names). Pick whichever one describes the
  // file, never a bare "video.mp4".
  const URL_DECODE = (s) => { try { return decodeURIComponent(s); } catch (e) { return s; } };
  function urlBasename(u) {
    try {
      const p = new URL(u, location.href).pathname;
      const last = p.split("/").filter(Boolean).pop() || "";
      return URL_DECODE(last);
    } catch (e) { return ""; }
  }
  function pageTitle() {
    const clean = (t) =>
      String(t || "")
        .replace(/\s+[-–—|]\s+[^-–—|]{1,40}$/, "")
        .trim();
    // The player shell is often a same-origin iframe inside the real page
    // (dooplay embeds /jwplayer/): prefer the top document's og:title so the
    // download is named for the movie/release, not the player.
    try {
      if (window.top && window.top !== window && window.top.document) {
        const td = window.top.document;
        const og = td.querySelector('meta[property="og:title"]');
        const t = clean((og && og.getAttribute("content")) || td.title);
        if (t) return t;
      }
    } catch (e) { /* cross-origin parent: fall through */ }
    let t = "";
    try {
      const og = document.querySelector('meta[property="og:title"]');
      t = (og && og.getAttribute("content")) || "";
    } catch (e) {}
    if (!t) t = String(document.title || "");
    t = t.trim();
    // "Title - Site" / "Title | Site": the suffix is chrome, not the name.
    t = clean(t) || String(document.title || "").trim();
    return t;
  }
  const GENERIC_TITLE = /^(video|videos|media|stream|player|watch|download|index|untitled|movie|online\s+video|full\s+video)$/i;
  const BORING_STEM = /^(videoplayback|index|file|media|stream|download|video|play|master|playlist|manifest|chunk|segment|init|frag\d*)$/i;
  // Extensions the desktop side supplies from the MIME type: a title that
  // already ends in one must not produce "name.mp4.mp4".
  const MEDIA_EXT_RE = /\.(mp4|mkv|webm|mov|avi|m4v|mp3|m4a|aac|ogg|wav|flac|m3u8|mpd|ts|m4s)$/i;
  // URL -> name stem. When the last segment is a generic playlist/segment
  // name (index.m3u8, master.mpd, seg1.ts) the release name is the folder
  // above it, which is what the site (and IDM) use.
  function urlStem(u) {
    let segs = [];
    try {
      segs = new URL(u, location.href).pathname.split("/").filter(Boolean);
    } catch (e) { return { stem: "", hasExt: false, fromFolder: false }; }
    if (!segs.length) return { stem: "", hasExt: false, fromFolder: false };
    const last = URL_DECODE(segs[segs.length - 1]);
    const m = last.match(/\.([a-z0-9]{2,5})$/i);
    const stem = m ? last.slice(0, -m[0].length) : last;
    if (BORING_STEM.test(stem) && segs.length >= 2) {
      return { stem: URL_DECODE(segs[segs.length - 2]), hasExt: false, fromFolder: true };
    }
    return { stem, hasExt: !!m, fromFolder: false };
  }
  function mediaBase(mediaUrl) {
    const title = pageTitle();
    const info = urlStem(mediaUrl || "");
    const stem = info.stem;
    const generic = !title || GENERIC_TITLE.test(title) || title.length < 4;
    // A descriptive URL name wins over a generic title, and over a title
    // that is clearly shorter than the name itself. Folder names behind a
    // playlist qualify too (the release name, not a hashed CDN segment).
    const descriptive =
      stem.length >= 8 && !BORING_STEM.test(stem) && (info.hasExt || info.fromFolder);
    if (descriptive && (generic || stem.length > title.length + 8)) {
      return stem.slice(0, 80);
    }
    const base = (title || stem || "video")
      .replace(MEDIA_EXT_RE, "")
      .replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_")
      .trim()
      .replace(/\.+$/, "");
    return (base || "video").slice(0, 80);
  }
  // Media extension for a URL, mapped to what the engine will actually write
  // (an HLS/DASH playlist assembles to MP4).
  function outExtFor(u) {
    const m = /\.([a-z0-9]{2,5})(?:\?|#|$)/i.exec(String(u || "").split("#")[0]);
    const e = m ? "." + m[1].toLowerCase() : "";
    if (e === ".m3u8" || e === ".mpd") return ".mp4";
    return e;
  }
  // Player wrapper pages carry the real stream in a query parameter
  // ("/jwplayer/?source=https%3A%2F%2F..."). Decode it so the download is the
  // media, not the HTML shell. Also used by the desktop engine.
  const UNWRAP_KEYS = ["source", "src", "file", "url", "video", "stream", "media", "link", "path"];
  const MEDIA_HINT_RE = /\.(m3u8|mpd|mp4|webm|mkv|mov|m4v|ts|m4s|mp3|m4a|aac|flac|ogg|wav|avi)(\?|#|$)/i;
  const PLAYER_HINT_RE = /(player|jwplayer|embed|iframe|watch|playlist)/i;
  function unwrapMediaUrl(raw) {
    try {
      let cur = String(raw || "");
      if (!cur) return "";
      let changed = false;
      for (let depth = 0; depth < 3; depth++) {
        // Already a media file/playlist: nothing to unwrap.
        if (MEDIA_HINT_RE.test(cur.split("#")[0])) break;
        const u = new URL(cur, location.href);
        let next = "";
        for (const k of UNWRAP_KEYS) {
          let v = u.searchParams.get(k);
          if (!v) continue;
          if (!/^https?:\/\//i.test(v)) v = URL_DECODE(v);
          if (!/^https?:\/\//i.test(v) || v === cur) continue;
          if (MEDIA_HINT_RE.test(v.split("#")[0]) || PLAYER_HINT_RE.test(cur)) {
            next = v;
            break;
          }
        }
        if (!next) break;
        cur = next;
        changed = true;
      }
      return changed ? cur : "";
    } catch (e) { return ""; }
  }

  // ---------- Facebook reels support (IDM-style quality rows) ----------
  // Facebook is an SPA: switching reels swaps /reel/<id> with no reload and
  // the player holds a blob: URL, so the real file URL only exists in
  // network traffic + embedded page JSON. IDM shows per-quality rows with
  // title/duration/quality/size — this rebuilds the same from:
  //   title    <- og:description / og:title (the reel caption)
  //   HD/SD    <- playable_url_quality_hd / playable_url (/hd_src/sd_src,
  //               browser_native_hd/sd_url) in the page JSON
  //   live set <- background-sniffed fbcdn/mp4 traffic for this tab
  //   duration <- duration_in_ms-style keys in the page JSON
  function fbUnescape(s) {
    try {
      return String(s || "")
        .replace(/\\\//g, "/")
        .replace(/\\u([\da-fA-F]{4})/g, (_, h) => String.fromCharCode(parseInt(h, 16)))
        .replace(/&amp;/g, "&");
    } catch (e) { return String(s || ""); }
  }
  function fbMeta(prop) {
    try {
      const el = document.querySelector(`meta[property="${prop}"]`);
      const c = el && el.getAttribute("content");
      return c ? fbUnescape(c) : "";
    } catch (e) { return ""; }
  }
  // Thumbnails: a reel page often has no og:image, and after an SPA
  // navigation the embedded JSON still describes the page that loaded first —
  // that left the sender panel on the gradient placeholder. The live player
  // is the reliable source, so candidates are collected with a source weight
  // and ranked by fbPickThumb (pure, unit-tested).
  function fbJsonUrlNear(html, key) {
    const p = String(html || "").indexOf('"' + key + '"');
    if (p < 0) return "";
    const seg = String(html).slice(p, p + 500);
    const m = seg.match(/https?:\\?\/\\?\/[^"\\\s]+/);
    return m ? fbUnescape(m[0]) : "";
  }
  // Facebook's own UI sprites are never a video thumbnail, and neither are
  // emoji images or SVGs.
  function fbThumbOk(u) {
    if (!/^https?:\/\//i.test(u || "")) return false;
    return !/rsrc\.php|static\.xx\.fbcdn\.net|\/emoji\/|\.svg(\?|$)/i.test(u);
  }
  // c = {url, kind, dist, w, h}: kind is the source weight (poster 1 ...
  // embedded JSON 0.4), dist the pixel distance from the player.
  function fbThumbScore(c) {
    if (!c || !fbThumbOk(c.url)) return -1e9;
    let s = (Number(c.kind) || 0) * 100;
    if (/fbcdn\.net|fbsbx\.com|cdninstagram\.com/i.test(c.url)) s += 25;
    if (/\/(t15|t39|t45)[./]/i.test(c.url)) s += 18; // FB video thumbnails
    const area = (Number(c.w) || 0) * (Number(c.h) || 0);
    if (area > 0) s += Math.min(20, Math.log10(area) * 4);
    if (c.dist != null) s -= Math.min(30, Number(c.dist) * 0.15);
    return s;
  }
  function fbPickThumb(list) {
    let best = "";
    let bs = 0;
    for (const c of (list || [])) {
      const s = fbThumbScore(c);
      if (s > bs) { bs = s; best = c.url; }
    }
    return best;
  }
  function fbRectDist(r, video) {
    try {
      if (!r || !video || !video.getBoundingClientRect) return null;
      const v = video.getBoundingClientRect();
      const dx = Math.max(0, v.left - r.right, r.left - v.right);
      const dy = Math.max(0, v.top - r.bottom, r.top - v.bottom);
      return Math.round(Math.hypot(dx, dy));
    } catch (e) { return null; }
  }
  // Walk up from the player to the smallest box that still holds it, so the
  // backdrop scan stays cheap (never the whole document).
  function fbPlayerHull(video) {
    try {
      let el = video;
      for (let i = 0; i < 8 && el && el.parentElement; i++) {
        el = el.parentElement;
        const r = el.getBoundingClientRect();
        if (r.width > window.innerWidth * 0.92 || r.height > window.innerHeight * 0.98) break;
      }
      return el || document.body;
    } catch (e) { return document.body; }
  }
  function fbThumbCandidates(html, video, current) {
    const out = [];
    const seen = new Set();
    const add = (url, kind, dist, w, h) => {
      if (!fbThumbOk(url) || seen.has(url)) return;
      seen.add(url);
      out.push({ url, kind, dist: dist == null ? null : dist, w: w || 0, h: h || 0 });
    };
    // 1. The player's own poster.
    try {
      for (const v of document.querySelectorAll("video")) {
        const p = v.getAttribute("poster") || v.poster || "";
        add(p, 1, v === video ? 0 : fbRectDist(v.getBoundingClientRect(), video), 0, 0);
      }
    } catch (e) { /* ignore */ }
    // 2. Images and CSS backdrops beside the player (Facebook paints the reel
    //    poster as a background-image on a div when there is no <img>).
    try {
      const hull = fbPlayerHull(video);
      const els = hull.querySelectorAll("img[src], img[data-src], div, span");
      const max = Math.min(els.length, 400);
      for (let i = 0; i < max; i++) {
        const el = els[i];
        const r = el.getBoundingClientRect ? el.getBoundingClientRect() : null;
        if (!r || r.width < 80 || r.height < 60) continue;
        const dist = fbRectDist(r, video);
        if (el.tagName === "IMG") {
          const u = el.currentSrc || el.src || el.getAttribute("data-src") || "";
          add(u, 0.8, dist, el.naturalWidth || r.width, el.naturalHeight || r.height);
          continue;
        }
        const cs = window.getComputedStyle ? window.getComputedStyle(el) : null;
        const m = String((cs && cs.backgroundImage) || (el.getAttribute("style") || ""))
          .match(/url\(["']?(https?:\/\/[^"')]+)/i);
        if (m) add(m[1], 0.8, dist, r.width, r.height);
      }
    } catch (e) { /* ignore */ }
    // 3. Page metas, then 4. whatever the embedded JSON happens to hold. Both
    //    describe the page that loaded first, so only while it is still this
    //    page: after a reel swap they are the previous reel's poster.
    if (!current) return out;
    add(fbMeta("og:image:secure_url"), 0.6, null, 0, 0);
    add(fbMeta("og:image"), 0.6, null, 0, 0);
    add(fbMeta("twitter:image"), 0.5, null, 0, 0);
    for (const k of ["preferred_thumbnail", "thumbnailImage", "thumbnail_src", "thumbnail", "image"]) {
      add(fbJsonUrlNear(html, k), 0.4, null, 0, 0);
    }
    return out;
  }
  function fbJsonUrls(html) {
    const out = {};
    const grab = (key) => {
      // "key":"https:\/\/..." or "key": "https://..."
      const re = new RegExp(`"${key}"\\s*:\\s*"((?:\\\\.|[^"\\\\])*)"`);
      const m = html.match(re);
      const u = m && fbUnescape(m[1]);
      return u && /^https?:\/\//i.test(u) ? u : "";
    };
    out.hd = grab("playable_url_quality_hd") || grab("browser_native_hd_url") || grab("hd_src");
    out.sd = grab("playable_url") || grab("browser_native_sd_url") || grab("sd_src");
    return out;
  }
  function fbJsonNum(html, keys) {
    for (const k of keys) {
      const m = html.match(new RegExp(`"${k}"\\s*:\\s*"?([\\d.]+)`));
      if (m) {
        const n = parseFloat(m[1]);
        if (n > 0 && n < 10000000) return n;
      }
    }
    return 0;
  }
  // Captions: IDM names the download after the reel's description, so the
  // caption matters as much as the file URL. Same staleness problem as the
  // thumbnail — prefer the live DOM and rank by source weight, so a meta left
  // over from the first page load never beats what is on screen now.
  function fbCleanCaption(s) {
    let t = String(s == null ? "" : s);
    try { t = t.normalize("NFC"); } catch (e) { /* ignore */ }
    t = t.replace(/\r?\n/g, " ").replace(/\s+/g, " ").trim();
    t = t.replace(/^\(\d+\)\s*/, "");
    t = t.replace(/\s*[|·]\s*Facebook\s*$/i, "").replace(/\s+on Facebook\b.*$/i, "").trim();
    return t;
  }
  // Site chrome, audio lines and counters are not a caption.
  function fbCaptionUsable(t) {
    if (!t || t.length < 12) return false;
    const low = t.toLowerCase();
    if (/^(facebook|watch|reels|video|log ?in|sign ?up|see more|more)$/.test(low)) return false;
    if (/^(see posts, photos and more|log in to facebook|content not available|this content isn)/.test(low)) return false;
    if (/^♪/.test(t) || /original audio$/i.test(t)) return false;
    if (/(^|\s)[·•]\s*follow(ed|ing)?$/i.test(t)) return false;
    if (/^\d[\d.,\s]*[km]?$/i.test(t)) return false;
    return true;
  }
  // c = {text, kind}: the strongest source wins, then the longer text.
  function fbPickCaption(list) {
    let best = "";
    let bs = 0;
    for (const c of (list || [])) {
      const t = fbCleanCaption(c && c.text);
      if (!fbCaptionUsable(t)) continue;
      const s = (Number(c && c.kind) || 0) * 100 + Math.min(t.length, 200);
      if (s > bs) { bs = s; best = t; }
    }
    return best.slice(0, 200);
  }
  // Every "message":{"text":"..."} value: the story message is the caption on
  // a fresh load, but comments match too, so keep several and let the ranker
  // choose.
  function fbJsonTexts(html, max) {
    const out = [];
    const s = String(html || "");
    const ws = (k) => { while (k < s.length && /\s/.test(s[k])) k++; return k; };
    let i = 0;
    while (out.length < (max || 6)) {
      const p = s.indexOf('"message"', i);
      if (p < 0) break;
      let j = ws(p + 9);
      if (s[j] === ":") j = ws(j + 1);
      if (s[j] === "{" && s.startsWith('"text"', j + 1)) {
        j = ws(j + 7);
        if (s[j] === ":") j = ws(j + 1);
        if (s[j] === '"') {
          j++;
          let raw = "";
          while (j < s.length) {
            const ch = s[j];
            if (ch === "\\") { raw += ch + (s[j + 1] || ""); j += 2; continue; }
            if (ch === '"') break;
            raw += ch;
            j++;
          }
          out.push(fbUnescape(raw));
          i = j + 1;
          continue;
        }
      }
      i = p + 9;
    }
    return out;
  }
  // The caption Facebook is rendering right now: the longest real sentence
  // near the player (author line, audio line and counters lose).
  function fbDomCaption(video) {
    try {
      const sel = 'div[dir="auto"], span[dir="auto"], h1, h2, h3';
      const nodes = document.querySelectorAll(sel);
      const v = video && video.getBoundingClientRect ? video.getBoundingClientRect() : null;
      let best = "";
      let bs = 0;
      for (const n of nodes) {
        if (n.querySelector && n.querySelector(sel)) continue; // leaf texts only
        const t = fbCleanCaption(n.textContent);
        if (!fbCaptionUsable(t)) continue;
        const r = n.getBoundingClientRect ? n.getBoundingClientRect() : null;
        if (!r || r.width < 60 || r.height < 10) continue;
        let s = Math.min(t.length, 180);
        if (v) {
          const d = fbRectDist(r, video);
          if (d != null) s -= Math.min(60, d * 0.2);
        }
        if (s > bs) { bs = s; best = t; }
      }
      return best;
    } catch (e) { return ""; }
  }
  function fbCaptionCandidates(html, video, current) {
    const out = [];
    const add = (text, kind) => { if (text) out.push({ text, kind }); };
    add(fbDomCaption(video), 1); // what the user is looking at
    // The metas/title/JSON come from the page that loaded: after a reel swap
    // they would name the new file after the reel the user left.
    if (current) {
      add(fbMeta("og:description"), 0.9);
      add(fbMeta("og:title"), 0.7);
      for (const t of fbJsonTexts(html, 6)) add(t, 0.55);
      add(document.title, 0.3);
    }
    return out;
  }
  // The reel on screen changes with no page load: /reel/<id> swaps through the
  // history API while Facebook keeps the same <video> node, and the page's
  // embedded JSON and the tab's sniffed requests both still describe the reel
  // the user scrolled past. Everything below is therefore keyed on the current
  // reel: a key from the URL (or from the player, for feeds whose URL has no
  // id), a generation counter, and the moment the current reel started. That is
  // what stops the previous reel's playable_url - and its sniffed video/audio
  // legs - from being sent as "the" download.
  let fbReelKey = "";
  let fbReelGen = 0;
  let fbReelAt = 0;
  // The page's own JSON/meta describe the page that loaded FIRST, so they are
  // only trustworthy while the document itself has not navigated since.
  let fbPageUrl = (typeof location !== "undefined" && location && location.href) || "";

  function fbReelKeyOf(href) {
    const s = String(href || "");
    const m = s.match(/\/reel\/(\d+)/i)
      || s.match(/\/videos\/(?:[^/?#]+\/)?(\d+)/i)
      || s.match(/[?&]v=(\d+)/i);
    return m ? m[1] : "";
  }

  // Which <video> is the user actually looking at? Facebook keeps the previous
  // and next reels mounted and preloaded, so document.querySelector("video") is
  // usually NOT the one on screen - that is how a scrolled-past reel's caption,
  // poster and dimensions ended up in the panel.
  function fbPickVideo(list, vw, vh) {
    let best = null;
    let bs = -1;
    const width = Number(vw) > 0 ? Number(vw) : 0;
    const height = Number(vh) > 0 ? Number(vh) : 0;
    for (const v of Array.from(list || [])) {
      let r = null;
      try { r = v && v.getBoundingClientRect ? v.getBoundingClientRect() : null; } catch (e) { r = null; }
      if (!r) continue;
      const visW = Math.min(r.right, width) - Math.max(r.left, 0);
      const visH = Math.min(r.bottom, height) - Math.max(r.top, 0);
      if (!(visW > 40 && visH > 40)) continue;
      let s = visW * visH;
      try { if (v.paused === false) s += 1e7; } catch (e) { /* ignore */ }
      try { if (v.videoWidth > 0) s += 1e5; } catch (e) { /* ignore */ }
      if (s > bs) { bs = s; best = v; }
    }
    return best;
  }

  function fbActiveVideo() {
    try {
      return fbPickVideo(
        document.querySelectorAll("video"),
        (window && window.innerWidth) || 0,
        (window && window.innerHeight) || 0
      );
    } catch (e) { return null; }
  }

  // Preloaded neighbours differ in poster/blob src, so this changes when the
  // reel does even on pages whose URL does not.
  function fbVideoKey(v) {
    try {
      if (!v) return "";
      const p = v.getAttribute("poster") || v.poster || "";
      const s = v.currentSrc || v.src || "";
      return `${p}|${String(s).slice(0, 120)}`;
    } catch (e) { return ""; }
  }

  // Called before anything is read so the generation is current. A no-op when
  // the reel has not changed.
  function fbSyncReel() {
    try {
      const key = fbReelKeyOf(location.href) || fbVideoKey(fbActiveVideo());
      if (key && key !== fbReelKey) {
        fbReelKey = key;
        fbReelGen++;
        fbReelAt = Date.now();
      }
      return fbReelKey;
    } catch (e) { return fbReelKey; }
  }

  // The embedded JSON and the metas describe the first load: once the reel
  // changes they name the reel the user came from.
  function fbJsonIsCurrent() {
    try { return !!fbPageUrl && location.href === fbPageUrl; } catch (e) { return false; }
  }

  // A DASH audio leg arrives as a plain .mp4, so the sniffer's content type is
  // the only reliable signal for it. Getting this wrong offered an audio-only
  // stream as a video row, which saved "with audio and no video".
  function fbAudioItem(m) {
    const mime = String((m && m.mime) || "").toLowerCase();
    if (mime.indexOf("audio/") === 0) return true;
    if (mime.indexOf("video/") === 0) return false;
    return /\.(m4a|mp3|aac|ogg|opus|weba)(\?|#|$)/i.test(String((m && m.url) || ""));
  }

  // What the player fetched for the reel on screen, newest first. The tab's
  // list keeps every reel of the session, so recency is what keeps the current
  // reel ahead of the one before it; `since` additionally drops traffic that
  // predates the current reel when that moment is known. Never returns empty
  // because of `since`: an empty list would send the page URL instead of the
  // reel's own media.
  function fbReelMedia(items, since) {
    const list = (Array.isArray(items) ? items : [])
      .filter((m) => m && /^https?:\/\//i.test(m.url || ""));
    const fresh = Number(since) > 0
      ? list.filter((m) => !m.seenAt || Number(m.seenAt) >= Number(since))
      : [];
    const use = fresh.length ? fresh : list;
    return use.slice().sort((a, b) => (Number(b.seenAt) || 0) - (Number(a.seenAt) || 0));
  }

  function fbInfo() {
    fbSyncReel();
    let html = "";
    try { html = document.documentElement.innerHTML.slice(0, 1500000); } catch (e) {}
    // The reel on screen, not the first <video> the document happens to hold:
    // Facebook keeps the neighbours mounted and preloaded.
    const video = fbActiveVideo() || document.querySelector("video");
    // Only while the page still describes itself (see fbJsonIsCurrent).
    const current = fbJsonIsCurrent();
    const urls = current ? fbJsonUrls(html) : { hd: "", sd: "" };
    // Duration: the live player describes the reel on screen; the embedded
    // JSON is the fallback for a page that has not navigated.
    let durSec = 0;
    try {
      const d = video && video.duration;
      if (Number.isFinite(d) && d > 0 && d < 36000) durSec = Math.round(d);
    } catch (e) { durSec = 0; }
    if (!durSec && current) {
      const durMs = fbJsonNum(html, ["playable_duration_in_ms", "duration_in_ms", "length_in_ms"]);
      durSec = durMs > 0 ? Math.round(durMs / 1000)
        : Math.round(fbJsonNum(html, ["videoDuration", "video_length", "duration", "length_in_seconds"]) || 0);
      if (!(durSec > 0 && durSec < 36000)) durSec = 0;
    }
    // Dimensions: the live player first (it is the reel on screen, and it is
    // what labels the quality), then the metas/JSON while they are still about
    // this page.
    let w = 0;
    let h = 0;
    try { w = Math.round(video && video.videoWidth) || 0; h = Math.round(video && video.videoHeight) || 0; } catch (e) { w = 0; h = 0; }
    if (current) {
      if (!(w > 0)) w = parseInt((document.querySelector('meta[property="og:video:width"]') || {}).content, 10) || 0;
      if (!(h > 0)) h = parseInt((document.querySelector('meta[property="og:video:height"]') || {}).content, 10) || 0;
      if (!(w > 0)) w = Math.round(fbJsonNum(html, ["original_width", "video_width", "width"]) || 0);
      if (!(h > 0)) h = Math.round(fbJsonNum(html, ["original_height", "video_height", "height"]) || 0);
    }
    // Title: the reel description — that is what names the saved file.
    const title = fbPickCaption(fbCaptionCandidates(html, video, current)) || "facebook_video";
    return {
      hd: urls.hd,
      sd: urls.sd,
      durSec,
      w,
      h,
      title,
      current,
      since: fbReelAt,
      key: fbReelKey,
      thumb: fbPickThumb(fbThumbCandidates(html, video, current)),
    };
  }
  function fbQuality(w, h, isHd) {
    // The label counts lines, so a portrait reel (1080x1920) and a landscape
    // one (1920x1080) are both 1080p — take the shorter side.
    const q = w > 0 && h > 0 ? Math.min(w, h) : Math.max(w || 0, h || 0);
    if (q >= 1080) return "1080p HD";
    if (q >= 720) return "720p HD";
    if (q >= 480) return "480p";
    if (q > 0) return `${q}p`;
    return isHd ? "HD" : "SD";
  }
  // The numeric cap the desktop app needs from a panel label: "720p HD" →
  // "720p" (yt-dlp then picks the best stream at or below it). "HD"/"SD"
  // carry no number, so the app falls back to best rather than guessing.
  function fbQualityCap(q) {
    const m = String(q || "").match(/(\d{3,4})p/);
    return m ? `${m[1]}p` : "";
  }
  function fbFilename(title) {
    const s = String(title || "facebook_video").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim();
    const base = (s.replace(/\.+$/, "") || "facebook_video").slice(0, 80);
    return base.endsWith(".mp4") ? base : base + ".mp4";
  }
  // The rows the reels panel offers. The page's own HD/SD encodes lead while
  // they still describe this page; after a scroll they are dropped and the band
  // captured legs of the reel on screen lead instead. Audio legs become chips
  // and one of them pairs a video-only leg.
  function fbMenuRows(info, items) {
    const seen = new Set();
    const rows = [];
    const audio = [];
    const fname = fbFilename(info.title);
    const dur = info.durSec > 0 ? `, ${info.durSec} sec` : "";
    const push = (url, quality, fromJson, size, thumb) => {
      if (!url || !/^https?:\/\//i.test(url) || seen.has(url)) return;
      seen.add(url);
      rows.push({
        url,
        fname,
        q: fbQualityCap(quality),
        label: `⬇ ${info.title.slice(0, 40)}, MP4 file${dur}, quality ${quality}`,
        _size: size || 0,
        dur: info.durSec || null,
        thumb: thumb || info.thumb || "",
        fromJson: !!fromJson,
      });
    };
    // P4: when a playlist is on the tab, loose segments are its children
    // (ad slivers included) — offer the playlist, never the parts.
    const R = (typeof DCRank !== "undefined") ? DCRank : null;
    const isPlaylist = (u) => R ? R.kindOf({ url: u }) === "playlist" : /m3u8|mpd|mpegurl|dash/i.test(String(u));
    const isSeg = (u) => R ? R.kindOf({ url: u }) === "segment" : /(\.ts|\.m4s|\.cmfv|\.cmfa)(\?|#|$)/i.test(String(u).split("?")[0]);
    const hasPlaylist = (Array.isArray(items) ? items : []).some((m) => m && isPlaylist(m.url));
    const live = fbReelMedia(items, info.since).filter((m) => !isPlaylist(m.url));
    const liveVideo = live.filter((m) => !fbAudioItem(m) && !(hasPlaylist && isSeg(m.url)));
    const liveAudio = live.filter((m) => fbAudioItem(m) && !isSeg(m.url));
    const liveQ = fbQuality(info.w, info.h, true);
    // The metas/JSON always describe the reel that was on screen when the page
    // loaded, so after a scroll they name the reel the user came from and must
    // not be offered at all. While the page still describes itself they are the
    // page's own HD/SD encodes, which stay ahead of the captured legs.
    if (info.current) {
      // The HD and SD URL often resolve to the same "1080p HD" text — label the
      // lower encode plainly instead of showing the same resolution twice.
      const hdQ = fbQuality(info.w, info.h, true);
      let sdQ = fbQuality(info.w, info.h, info.hd === info.sd);
      if (info.hd && info.sd && info.hd !== info.sd && sdQ === hdQ) sdQ = "SD";
      if (info.hd && info.hd !== info.sd) push(info.hd, hdQ, true);
      if (info.sd) push(info.sd, sdQ, true);
    }
    // This used to be pushed *after* the page JSON, so a scrolled-past reel's
    // URL took the first row - the "it downloads the previous video" report.
    for (const m of liveVideo.slice(0, 4)) push(m.url, liveQ, false, m.size, m.thumb);
    // Audio-only traffic from the same playback session becomes an "Other
    // Files" chip (same idea as YouTube's audio-only tracks).
    for (const m of liveAudio.slice(0, 3)) {
      audio.push({ url: m.url, mime: m.mime || "audio/mp4", size: m.size || 0 });
    }
    // Facebook serves its higher qualities as a video-only DASH leg plus a
    // separate audio leg: a captured video row would otherwise save silently.
    // Hand the newest audio leg of the same reel along so the app merges them
    // (it already merges the page URL it keeps alongside the row).
    const pair = liveVideo.length && liveAudio.length
      ? { url: liveAudio[0].url, mime: liveAudio[0].mime || "audio/mp4" }
      : null;
    return { rows, audio, pair };
  }

  // ---------- end Facebook reels support ----------
  // Pick the best directly-downloadable candidate the background sniffer
  // caught for this page: prefer real video/mp4 over playlists, biggest
  // known size first. Facebook reels render as blob: players, so this is
  // what makes the pill work without a page reload.
  function bestSniffed(items) {
    const list = (items || []).filter((m) => m && /^https?:\/\//i.test(m.url));
    if (!list.length) return null;
    const R = (typeof DCRank !== "undefined") ? DCRank : null;
    // P4: when the tab already holds a playlist, loose segments are its
    // children (the engine assembles them) — never pick a part. Without
    // rank.js, fall back to the legacy inline score below.
    const hasPlaylist = R
      ? list.some((m) => R.kindOf(m) === "playlist")
      : list.some((m) => /m3u8|mpd|mpegurl|dash/i.test(String(m.url || "")));
    const pool = R
      ? list.filter((m) => R.scoreMedia(m, { hasPlaylist }) >= 0)
      : list.slice();
    if (!pool.length) return null;
    const score = (m) => {
      if (R) return R.scoreMedia(m, { hasPlaylist });
      const u = String(m.url || "");
      const mime = String(m.mime || "").toLowerCase();
      if (/m3u8|mpd|mpegurl|dash/i.test(u)) return -1; // engine can't take playlists directly
      let s = 0;
      if (mime.startsWith("video/")) s += 100;
      if (/mp4/i.test(mime) || /\.mp4/i.test(u)) s += 50;
      if (/fbcdn|lookaside|facebook|instagram|cdninstagram/i.test(u)) s += 20;
      s += Math.min(30, Math.log10((m.size || 0) + 1) * 5);
      return s;
    };
    return pool.slice().sort((a, b) => score(b) - score(a))[0];
  }

  // Small on-page error toast (failures are otherwise silent).
  function toast(text) {
    let el = document.getElementById("dc-yt-toast");
    if (!el) {
      el = document.createElement("div");
      el.id = "dc-yt-toast";
      el.style.cssText = [
        "position:fixed", "left:18px", "bottom:18px", "z-index:2147483647",
        "max-width:min(480px,90vw)", "padding:10px 14px", "border-radius:10px",
        "background:rgba(60,16,16,.94)", "border:1px solid rgba(255,120,120,.4)",
        "color:#ffd9d9", "font:13px system-ui,sans-serif",
        "box-shadow:0 8px 28px rgba(0,0,0,.5)",
      ].join(";");
      document.documentElement.appendChild(el);
    }
    el.textContent = text;
    el.style.display = "block";
    clearTimeout(el.dataset.timer);
    el.dataset.timer = setTimeout(() => { el.style.display = "none"; }, 9000);
  }

  function safeFilename(title) {
    const s = String(title || "youtube_video").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim();
    const base = s.replace(/\.+$/, "") || "youtube_video";
    return base.endsWith(".mp4") ? base : base + ".mp4";
  }

  // A browser-session preference, shared through the worker (not website storage).
  let barPosition = null;
  let positionRevision = 0;
  let activeBarDrag = null;
  function validBarPosition(p) {
    return p && Number.isFinite(p.x) && Number.isFinite(p.y) && p.x >= 0 && p.x <= 1 && p.y >= 0 && p.y <= 1;
  }
  function applySavedBarPosition(host) {
    if (!barPosition) return false;
    const fullscreen=document.fullscreenElement;
    const parent=fullscreen || document.documentElement;
    if(host.parentNode!==parent)parent.appendChild(host);
    const width=Math.max(0,Math.min(240,window.innerWidth-16));
    host.style.position="fixed";host.style.width=width+"px";
    host.style.right="auto";host.style.bottom="auto";
    host.style.left=(8+barPosition.x*Math.max(0,window.innerWidth-width-16))+"px";
    host.style.top=(8+barPosition.y*Math.max(0,window.innerHeight-44))+"px";
    return true;
  }
  function positionDraggedPanel(host,panel) {
    if(!panel || !barPosition)return;
    const parent=document.fullscreenElement || document.documentElement;
    if(panel.parentNode!==parent)parent.appendChild(panel);
    const r=host.getBoundingClientRect();
    const width=Math.min(300,Math.max(0,window.innerWidth-16));
    panel.style.position="fixed";panel.style.width=width+"px";
    panel.style.left=Math.max(8,Math.min(window.innerWidth-width-8,r.right-width))+"px";
    panel.style.right="auto";panel.style.bottom="auto";
    const above=Math.max(0,r.top-14),below=Math.max(0,window.innerHeight-r.bottom-14);
    const naturalHeight=panel.scrollHeight || panel.offsetHeight || 300;
    const up=naturalHeight>below && above>below;
    const available=up?above:below;
    panel.style.boxSizing="border-box";panel.style.maxHeight=available+"px";
    panel.style.overflowY="auto";panel.style.zIndex="2147483646";
    const height=Math.min(naturalHeight,available);
    panel.style.top=(up?Math.max(8,r.top-height-6):r.bottom+6)+"px";
    const arrow=panel.querySelector('[data-dc-p="arrow"]');if(arrow)arrow.style.display="none";
  }
  function rememberBarPosition() {
    try { chrome.runtime.sendMessage({type:"setVideoBarPosition",position:barPosition},()=>{void chrome.runtime.lastError;}); } catch(e) { /* keep page-local drag usable */ }
  }
  function loadBarPosition() {
    const revision=positionRevision;
    try { chrome.runtime.sendMessage({type:"getVideoBarPosition"},res=>{
      if(chrome.runtime.lastError || !alive() || revision!==positionRevision)return;
      barPosition=validBarPosition(res && res.position) ? res.position : null;
      syncDismissControls();
    }); } catch(e) { /* default placement remains available */ }
  }
  function installBarDrag(host,main,grip) {
    let drag=null,suppressClick=false;
    host.addEventListener("click",ev=>{
      if(suppressClick){suppressClick=false;ev.preventDefault();ev.stopImmediatePropagation();}
    },true);
    const update=(left,top)=>{
      const width=Math.min(240,window.innerWidth-16);
      barPosition={x:Math.max(0,Math.min(1,(left-8)/Math.max(1,window.innerWidth-width-16))),
        y:Math.max(0,Math.min(1,(top-8)/Math.max(1,window.innerHeight-44)))};
      positionRevision++;applySavedBarPosition(host);
    };
    for(const control of [main,grip]) {
      control.style.touchAction="none";
      control.addEventListener("pointerdown",ev=>{
        if(ev.button!==0 || ev.isPrimary===false)return;
        const r=host.getBoundingClientRect();
        drag={id:ev.pointerId,x:ev.clientX,y:ev.clientY,left:r.left,top:r.top,moved:false,previous:barPosition};
        suppressClick=false;activeBarDrag=host;
        // Move before capture; moving a captured node can lose pointer capture.
        const parent=document.fullscreenElement || document.documentElement;
        if(host.parentNode!==parent)parent.appendChild(host);
        host.style.position="fixed";host.style.left=r.left+"px";host.style.top=r.top+"px";
        host.style.right="auto";host.style.bottom="auto";
        control.setPointerCapture(ev.pointerId);ev.stopPropagation();
      });
      control.addEventListener("pointermove",ev=>{
        if(!drag || ev.pointerId!==drag.id)return;
        const dx=ev.clientX-drag.x,dy=ev.clientY-drag.y;
        if(!drag.moved && Math.hypot(dx,dy)<5)return;
        if(!drag.moved)hidePanels();
        drag.moved=true;ev.preventDefault();ev.stopPropagation();update(drag.left+dx,drag.top+dy);
      });
      const finish=(ev,cancel)=>{
        if(!drag || ev.pointerId!==drag.id)return;
        const moved=drag.moved,previous=drag.previous;
        drag=null;activeBarDrag=null;
        if(control.hasPointerCapture(ev.pointerId))control.releasePointerCapture(ev.pointerId);
        if(cancel){barPosition=previous;positionRevision++;syncDismissControls();}
        else if(moved){suppressClick=true;setTimeout(()=>{suppressClick=false;},0);rememberBarPosition();ev.preventDefault();ev.stopPropagation();}
      };
      control.addEventListener("pointerup",ev=>finish(ev,false));
      control.addEventListener("pointercancel",ev=>finish(ev,true));
      control.addEventListener("lostpointercapture",ev=>finish(ev,true));
    }
    grip.addEventListener("click",ev=>{ev.preventDefault();ev.stopPropagation();});
    grip.addEventListener("keydown",ev=>{
      const delta={ArrowLeft:[-1,0],ArrowRight:[1,0],ArrowUp:[0,-1],ArrowDown:[0,1]}[ev.key];
      if(!delta)return;
      ev.preventDefault();ev.stopPropagation();hidePanels();
      const r=host.getBoundingClientRect(),step=ev.shiftKey?24:8;
      update(r.left+delta[0]*step,r.top+delta[1]*step);rememberBarPosition();
    });
  }

  // IDM-style: the circle lives ON the video player (top-right corner),
  // falling back to a fixed viewport corner when no player is found. The
  // bubble's arrow always points back at the circle (up when the panel sits
  // below it, down when it sits above it).
  function placeInPlayer(btn, panel) {
    if (!btn) return false;
    const setArrow = (side) => {
      if (!panel) return;
      const a = panel.querySelector('[data-dc-p="arrow"]');
      if (!a) return;
      const T = panel._dcT || dcPanelTokens();
      const top = side === "top";
      a.setAttribute("data-dc-arrow", top ? "top" : "bottom");
      a.style.background = T.bg;
      a.style.right = "11px";
      a.style.left = "auto";
      a.style.top = top ? "-7px" : "auto";
      a.style.bottom = top ? "auto" : "-7px";
      a.style.borderLeftColor = top ? T.border : "transparent";
      a.style.borderTopColor = top ? T.border : "transparent";
      a.style.borderRightColor = top ? "transparent" : T.border;
      a.style.borderBottomColor = top ? "transparent" : T.border;
    };
    if(applySavedBarPosition(btn)){positionDraggedPanel(btn,panel);return true;}
    const player = document.getElementById("movie_player");
    if (player) {
      btn.style.position = "absolute";
      btn.style.top = "12px";
      btn.style.right = "12px";
      btn.style.bottom = "auto";
      if (panel) {
        panel.style.position = "absolute";
        panel.style.top = "56px";
        panel.style.right = "12px";
        panel.style.bottom = "auto";
        if (panel.parentElement !== player) player.appendChild(panel);
      }
      setArrow("top");
      if (btn.parentElement !== player) player.appendChild(btn);
      return true;
    }
    btn.style.position = "fixed";
    btn.style.right = "18px";
    btn.style.bottom = "18px";
    btn.style.top = "auto";
    if (panel) {
      panel.style.position = "fixed";
      panel.style.right = "18px";
      panel.style.bottom = "62px";
      panel.style.top = "auto";
      if (panel.parentElement !== document.documentElement) document.documentElement.appendChild(panel);
      setArrow("bottom");
    }
    if (btn.parentElement !== document.documentElement) document.documentElement.appendChild(btn);
    return false;
  }

  // The Facebook pill lives inside the player box, which is only as wide as
  // the video: a 300px panel anchored in there gets clipped on the left once
  // the reel column is narrower than the panel (narrow/vertical reels). Anchor
  // the panel to the page instead, clamped into the viewport, and keep the
  // arrow pointing at the pill.
  function placeFbPanel(pill, panel) {
    if (!pill || !panel) return;
    const host = document.body || document.documentElement;
    if (panel.parentElement !== host) host.appendChild(panel);
    let pr;
    try { pr = pill.getBoundingClientRect(); } catch (e) { pr = null; }
    if (!pr) return;
    const de = document.documentElement;
    const vw = window.innerWidth || de.clientWidth || 360;
    const vh = window.innerHeight || de.clientHeight || 640;
    const ox = window.pageXOffset || de.scrollLeft || 0;
    const oy = window.pageYOffset || de.scrollTop || 0;
    // 300px is the design width; shrink on very narrow viewports so both
    // edges stay on screen.
    const w = Math.max(240, Math.min(300, vw - 16));
    let left = Math.round(pr.right - w); // right edges aligned with the pill
    left = Math.min(Math.max(8, left), Math.max(8, vw - w - 8));
    // ~300px tall; if the pill sits low, lift the panel so it still fits.
    const est = 300;
    let top = Math.round(pr.bottom + 6);
    if (top + est > vh - 8) top = Math.max(8, vh - 8 - est);
    panel.style.position = "absolute";
    panel.style.width = w + "px";
    panel.style.left = left + ox + "px";
    panel.style.top = top + oy + "px";
    panel.style.right = "auto";
    panel.style.bottom = "auto";
    panel.style.zIndex = "2147483647";
    const arrow = panel.querySelector('[data-dc-p="arrow"]');
    if (arrow) {
      // Centre the arrow over the pill, then keep it inside the panel's radius.
      const at = pr.left + pr.width / 2 - left - 7;
      arrow.style.left = Math.round(Math.min(Math.max(12, at), w - 26)) + "px";
      arrow.style.right = "auto";
    }
  }

  // Presence pre-check: qualities already in the app queue get a "✓ Queued"
  // tag and a toast instead of re-sending into an "already in queue" error.
  function markQueuedRows(pairs) {
    try {
      const urls = pairs.map((p) => p.st.url).filter(Boolean).slice(0, 25);
      if (!urls.length) return;
      chrome.runtime.sendMessage({ type: "isQueued", urls }, (res) => {
        if (chrome.runtime.lastError) return;
        if (!alive()) return;
        const hit = new Set((res && res.queued) || []);
        if (!hit.size) return;
        for (const p of pairs) {
          if (!hit.has(p.st.url) || !p.item || !p.item.el.isConnected) continue;
          if (p.item.el.getAttribute("data-dc-queued")) continue;
          p.item.el.setAttribute("data-dc-queued", "1");
          p.item.fmtEl.textContent = "✓ Queued";
          p.item.el.title = "Already in the download queue";
          p.item.el.onclick = (ev) => {
            ev.stopPropagation();
            toast("Download Craft: already in queue");
          };
        }
      });
    } catch (e) { /* best-effort */ }
  }

  function fmtSize(n) {    if (!n || n <= 0) return "";
    const u = ["B", "KB", "MB", "GB"];
    let v = n, i = 0;
    while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
    return i === 0 ? `${n} ${u[i]}` : `${v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
  }

  // Seconds → "3:26" for rows with page-JSON durations (shared rank.js
  // helper when loaded, inline fallback otherwise).
  function fmtDurSec(sec) {
    if (typeof DCRank !== "undefined" && DCRank.fmtDur) return DCRank.fmtDur(sec);
    const n = Math.round(Number(sec) || 0);
    if (!(n > 0) || n >= 86400) return "";
    return `${Math.floor(n / 60)}:${String(n % 60).padStart(2, "0")}`;
  }

  // ---------- In-page floating panel (extension_popup.html design) ----------
  // The trigger is a compact download bar; the panel is a message bubble with an
  // arrow, a "send link" row, video info, a 2-column quality grid, "Other
  // Files" chips and a Download / Cancel / X action row. Every rule is set
  // inline (a page's own CSS can never leak in), and each element carries a
  // `data-dc-p` role so the light/dark toggle can repaint it without
  // touching layout.
  const SVG_NS = "http://www.w3.org/2000/svg";
  const ICON_DOWN = "M12 4V16M6 10L12 16L18 10";
  const ICON_SEND = "M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z";
  const ICON_CLOSE = "M6 6l12 12M18 6L6 18";
  const ICON_PLAY = "M8 5.5v13l11-6.5-11-6.5z";

  // Transparent-background inline SVG (no web_accessible_resources, crisp at
  // every UI scale). `currentColor` lets the caller theme it with `color`.
  function svgIcon(px, d, w) {
    const svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("width", String(px));
    svg.setAttribute("height", String(px));
    svg.setAttribute("aria-hidden", "true");
    svg.setAttribute("focusable", "false");
    svg.style.cssText = "display:block;flex:0 0 auto;pointer-events:none;";
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", d);
    if (w === 0) {
      path.setAttribute("fill", "currentColor");
      path.setAttribute("stroke", "none");
    } else {
      path.setAttribute("fill", "none");
      path.setAttribute("stroke", "currentColor");
      path.setAttribute("stroke-width", String(w || 2));
      path.setAttribute("stroke-linecap", "round");
      path.setAttribute("stroke-linejoin", "round");
    }
    svg.appendChild(path);
    return svg;
  }

  // One isolated toolbar: the primary action and dismiss button never overlap.
  // Shadow DOM keeps player/site button and SVG rules out of these controls.
  function videoDownloadBar(title) {
    const host = document.createElement("div");
    host.title = title;
    host.style.cssText = "all:initial;display:block;width:240px;height:28px;margin:0;padding:0;border:0;box-sizing:border-box;z-index:2147483647;";
    const root = host.attachShadow({mode:"open"});
    const style = document.createElement("style");
    style.textContent = `
      :host { color-scheme:light; }
      * { box-sizing:border-box; }
      .bar { display:flex;width:100%;height:28px;overflow:hidden;border:1px solid #aab7c4;
        border-radius:5px;background:#f8fafc;box-shadow:0 1px 5px #0003;font:12px/1.2 system-ui,sans-serif; }
      button { all:unset;box-sizing:border-box;display:flex;align-items:center;justify-content:center;cursor:pointer;
        color:#243444;background:#f8fafc;font:600 12px/1.2 system-ui,sans-serif; }
      button:hover { background:#e7eff7; }
      button:focus-visible { outline:2px solid #2563eb;outline-offset:-3px; }
      .grip { flex:0 0 18px;width:18px;cursor:grab;color:#7b8794;touch-action:none; }
      .grip:active { cursor:grabbing; }
      .download { touch-action:none;flex:1;min-width:0;gap:6px;padding:0 8px; }
      .label { white-space:nowrap;overflow:hidden;text-overflow:ellipsis; }
      .close { flex:0 0 25px;width:25px;border-left:1px solid #d5dee7; }
      .close:hover { color:#a91d2b;background:#ffe9ec; }
      [hidden] { display:none !important; }
      svg { display:block;flex:none;width:14px;height:14px;pointer-events:none; }
    `;
    const bar = document.createElement("div");bar.className="bar";
    const main = document.createElement("button");main.type="button";main.className="download";
    main.title=title;main.setAttribute("aria-label","Download this video");
    const arrow=svgIcon(14,ICON_DOWN,2.5);arrow.style.color="#168047";
    main.appendChild(arrow);
    const label=document.createElement("span");label.className="label";label.textContent="Download this video";
    main.appendChild(label);
    const close=document.createElement("button");close.type="button";close.className="close";close.hidden=true;
    close.setAttribute("data-dc-dismiss","1");
    close.setAttribute("aria-label","Hide download bar for this video");
    close.title="Hide for this video — restore from the extension toolbar";
    close.appendChild(svgIcon(12,ICON_CLOSE,2));
    const grip=document.createElement("button");grip.type="button";grip.className="grip";
    grip.title="Drag to move · arrow keys move when focused";
    grip.setAttribute("aria-label","Move video download bar");
    grip.appendChild(svgIcon(12,"M8 5h.01M8 12h.01M8 19h.01M16 5h.01M16 12h.01M16 19h.01",3));
    bar.append(grip,main,close);root.append(style,bar);
    installBarDrag(host,main,grip);
    host.__dcMain=main;host.__dcClose=close;
    return host;
  }

  // Theme: tokens + painter live in rank.js (node-tested); these wrappers
  // fall back to the dark mock when rank.js failed to load.
  const DCUI = (typeof DCRank !== "undefined" && DCRank.paintDcPanel) ? DCRank : null;
  const DC_FALLBACK = {
    mode: "dark", bg: "#0f172a", card: "#1e293b", hover: "#334155", border: "#334155",
    text: "#f1f5f9", dim: "#94a3b8", muted: "#64748b", accent: "#3b82f6",
    accentLight: "rgba(59,130,246,.16)", shadow: "0 8px 32px rgba(0,0,0,.4)",
    thumbBg: "linear-gradient(135deg,#667eea,#764ba2)",
    toggleBorder: "rgba(241,245,249,.25)", toggle: "☀",
  };
  function dcThemeMode() { return DCUI ? DCUI.dcThemeMode() : "dark"; }
  function dcSetTheme(mode) { if (DCUI) DCUI.dcSetTheme(mode); }
  function dcPanelTokens() { return (DCUI && DCUI.panelTokens(dcThemeMode())) || DC_FALLBACK; }
  function paintDcPanel(root) { if (root && DCUI) DCUI.paintDcPanel(root); }
  function allPanels() { try { return [...document.querySelectorAll("[data-dc-panel]")]; } catch (e) { return []; } }
  function hidePanels() { allPanels().forEach((p) => { p.style.display = "none"; }); }
  // Click-away / Escape closes any open panel. One listener per page (a
  // per-rebuild listener leaked and stale closures fought the new panel);
  // panels stop their own clicks from reaching the document.
  function installDcCloser() {
    if (window.__dcDocClick) return;
    window.__dcDocClick = true;
    document.addEventListener("click", () => { hidePanels(); });
    document.addEventListener("keydown", (e) => {
      if (e && (e.key === "Escape" || e.keyCode === 27)) hidePanels();
    });
  }

  // A plain styled div (role = data-dc-p paint role, may be null).
  function dcText(text, role, css) {
    const el = document.createElement("div");
    if (role) el.setAttribute("data-dc-p", role);
    if (css) el.style.cssText = css;
    el.textContent = text == null ? "" : String(text);
    return el;
  }

  // Light/dark toggle for a panel (same idea as rank.js's menu toggle, but
  // it repaints the panel design and any other open panel).
  function panelThemeToggle(panel) {
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("data-dc-p", "toggle");
    b.title = "Toggle light / dark";
    b.style.cssText = "flex:0 0 auto;width:22px;height:22px;padding:0;margin:0;border-radius:999px;"
      + "border:1px solid;background:transparent;cursor:pointer;font:11px/1 system-ui,sans-serif;";
    b.onclick = (ev) => {
      if (ev && ev.stopPropagation) ev.stopPropagation();
      dcSetTheme(dcThemeMode() === "light" ? "dark" : "light");
      try { allPanels().forEach(paintDcPanel); } catch (e) { /* theme is best-effort */ }
    };
    return b;
  }

  // One quality cell: container + resolution on the left, size on the right.
  function dcQualityItem(o) {
    const T = dcPanelTokens();
    const el = document.createElement("div");
    el.setAttribute("data-dc-p", "item");
    el.style.cssText = "display:flex;align-items:center;justify-content:space-between;gap:6px;box-sizing:border-box;"
      + "border:1px solid " + T.border + ";border-radius:6px;background:" + T.card + ";cursor:pointer;padding:6px 8px;";
    if (o.tip) el.title = o.tip;
    const col = document.createElement("div");
    col.style.cssText = "display:flex;flex-direction:column;gap:1px;min-width:0;";
    const fmt = dcText(o.format || "", "text", "font-size:10px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;");
    const res = dcText(o.res || "", "muted", "font-size:9px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;");
    col.appendChild(fmt);
    col.appendChild(res);
    const size = dcText(o.size || "—", "dim", "flex:0 0 auto;font-size:10px;text-align:right;white-space:nowrap;");
    el.appendChild(col);
    el.appendChild(size);
    el.onmouseenter = () => { if (!el.getAttribute("data-dc-sel")) el.style.background = dcPanelTokens().hover; };
    el.onmouseleave = () => { if (!el.getAttribute("data-dc-sel")) el.style.background = dcPanelTokens().card; };
    return { el, fmtEl: fmt, resEl: res, sizeEl: size, setSize: (t) => { size.textContent = t; } };
  }

  // "Other Files" chip (subtitles, audio-only tracks).
  function dcFileChip(f) {
    const T = dcPanelTokens();
    const el = document.createElement("div");
    el.setAttribute("data-dc-p", "chip");
    el.style.cssText = "display:inline-flex;align-items:center;gap:5px;box-sizing:border-box;padding:4px 8px;"
      + "border:1px solid " + T.border + ";border-radius:5px;background:" + T.card + ";cursor:pointer;"
      + "font-size:10px;font-weight:500;";
    if (f.tip) el.title = f.tip;
    el.appendChild(dcText(f.label || "file", null, "white-space:nowrap;"));
    el.onclick = (ev) => {
      if (ev && ev.stopPropagation) ev.stopPropagation();
      hidePanels();
      if (f.send) f.send();
    };
    el.onmouseenter = () => { el.style.borderColor = dcPanelTokens().accent; };
    el.onmouseleave = () => { el.style.borderColor = dcPanelTokens().border; };
    return { el };
  }

  // Build the bubble panel. Spec:
  //   {title, thumb, dur, res, size, format,          // video info block
  //    hint, sendLabel, onSendLink,                   // "send link" row
  //    primaryLabel, onPrimary, onClose}              // action row
  // plus api.addQuality({format,res,size,tip,send,onPick}) / api.addChip(...).
  function dcPanel(spec) {
    const s = spec || {};
    const T = dcPanelTokens();
    const items = [];
    const chips = [];
    let selected = null;
    const panel = document.createElement("div");
    panel.setAttribute("data-dc-panel", "1");
    panel.style.cssText = [
      "display:none", "position:relative", "width:300px", "max-width:96vw",
      "box-sizing:border-box", "padding:0", "margin:0", "text-align:left",
      "border:1px solid " + T.border, "border-radius:12px",
      "background:" + T.bg, "color:" + T.text, "box-shadow:" + T.shadow,
      "font:12px/1.35 system-ui,'Segoe UI',sans-serif", "z-index:2147483647",
    ].join(";");
    // Clicks inside the panel never reach the page (YouTube's own handlers
    // would pause/close the player, and the document closer would fire).
    panel.onclick = (ev) => { if (ev && ev.stopPropagation) ev.stopPropagation(); };

    const arrow = document.createElement("div");
    arrow.setAttribute("data-dc-p", "arrow");
    arrow.setAttribute("data-dc-arrow", "top");
    arrow.style.cssText = "position:absolute;top:-7px;right:11px;left:auto;width:14px;height:14px;"
      + "background:" + T.bg + ";border:1px solid transparent;border-left-color:" + T.border
      + ";border-top-color:" + T.border + ";border-radius:2px 0 0 0;transform:rotate(45deg);pointer-events:none;";
    panel.appendChild(arrow);

    const inner = document.createElement("div");
    inner.style.cssText = "display:flex;flex-direction:column;border-radius:11px;overflow:hidden;";

    // --- "Send video link to app" row
    const sendRow = document.createElement("div");
    sendRow.setAttribute("data-dc-p", "sendrow");
    sendRow.style.cssText = "display:flex;align-items:center;gap:8px;box-sizing:border-box;margin:4px 8px 0;padding:10px 12px;border-radius:6px;";
    const sendIcon = svgIcon(14, ICON_SEND, 2);
    sendIcon.setAttribute("data-dc-p", "icon");
    const sendTxt = document.createElement("div");
    sendTxt.style.cssText = "flex:1;min-width:0;";
    sendTxt.appendChild(dcText(s.sendLabel || "Send video link to app", "text", "font-size:11px;"));
    sendTxt.appendChild(dcText(s.hint || "", "muted", "font-size:9px;margin-top:1px;line-height:1.35;"));
    sendRow.appendChild(sendIcon);
    sendRow.appendChild(sendTxt);
    sendRow.appendChild(panelThemeToggle(panel));
    sendRow.onmouseenter = () => { sendRow.style.background = (panel._dcT || dcPanelTokens()).hover; };
    sendRow.onmouseleave = () => { sendRow.style.background = "transparent"; };
    if (s.onSendLink) {
      sendRow.style.cursor = "pointer";
      sendRow.onclick = (ev) => {
        if (ev && ev.stopPropagation) ev.stopPropagation();
        hidePanels();
        s.onSendLink();
      };
    }
    inner.appendChild(sendRow);

    // --- video info (thumb + title + resolution / size / container)
    const info = document.createElement("div");
    info.style.cssText = "display:flex;gap:10px;align-items:center;box-sizing:border-box;padding:10px 12px;";
    const thumb = document.createElement("div");
    thumb.setAttribute("data-dc-p", "thumb");
    thumb.style.cssText = "position:relative;flex:0 0 80px;width:80px;height:48px;border-radius:6px;overflow:hidden;"
      + "display:flex;align-items:center;justify-content:center;background:" + T.thumbBg + ";color:#fff;";
    const play = svgIcon(14, ICON_PLAY, 0);
    play.style.opacity = ".85";
    if (s.thumb && /^https?:\/\//i.test(s.thumb)) {
      const im = document.createElement("img");
      im.src = s.thumb;
      im.loading = "lazy";
      im.style.cssText = "width:100%;height:100%;object-fit:cover;display:block;";
      im.onload = () => { play.style.display = "none"; };
      im.onerror = () => { im.remove(); };
      thumb.appendChild(im);
    }
    thumb.appendChild(play);
    if (s.dur) {
      thumb.appendChild(dcText(s.dur, null,
        "position:absolute;bottom:2px;right:4px;font-size:7px;color:#fff;background:rgba(0,0,0,.7);padding:1px 3px;border-radius:2px;line-height:1;"));
    }
    const det = document.createElement("div");
    det.style.cssText = "flex:1;min-width:0;";
    const titleEl = dcText(s.title, "text", "font-size:12px;font-weight:600;margin-bottom:4px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;");
    titleEl.title = s.title || "";
    const meta = document.createElement("div");
    meta.style.cssText = "display:flex;align-items:center;gap:8px;flex-wrap:wrap;";
    const resEl = dcText(s.res || "", "accent", "font-size:10px;font-weight:500;");
    const sizeEl = dcText(s.size ? "• " + s.size : "", "muted", "font-size:10px;");
    const fmtEl = dcText(s.format ? String(s.format).toUpperCase() : "", "chipmini",
      "font-size:9px;padding:1px 5px;border-radius:3px;");
    meta.appendChild(resEl);
    meta.appendChild(sizeEl);
    meta.appendChild(fmtEl);
    det.appendChild(titleEl);
    det.appendChild(meta);
    info.appendChild(thumb);
    info.appendChild(det);
    inner.appendChild(info);

    // --- scrollable middle: quality grid + other files
    const scroller = document.createElement("div");
    scroller.style.cssText = "max-height:min(58vh,420px);overflow-y:auto;overflow-x:hidden;";
    const mkDivider = () => {
      const d = document.createElement("div");
      d.setAttribute("data-dc-p", "divider");
      d.style.cssText = "height:1px;margin:0 12px;background:" + T.border + ";";
      return d;
    };
    const gridDivider = mkDivider();
    const gridSec = document.createElement("div");
    gridSec.style.cssText = "padding:8px 12px;";
    const gridHead = document.createElement("div");
    gridHead.style.cssText = "display:flex;align-items:center;justify-content:space-between;margin-bottom:6px;";
    const badge = dcText("", "badge", "font-size:9px;padding:2px 6px;border-radius:8px;");
    gridHead.appendChild(dcText("Video Files", "muted", "font-size:10px;font-weight:600;letter-spacing:.5px;text-transform:uppercase;"));
    gridHead.appendChild(badge);
    const grid = document.createElement("div");
    grid.style.cssText = "display:grid;grid-template-columns:1fr 1fr;gap:5px;";
    gridSec.appendChild(gridHead);
    gridSec.appendChild(grid);
    scroller.appendChild(gridDivider);
    scroller.appendChild(gridSec);

    const chipsDivider = mkDivider();
    const chipsSec = document.createElement("div");
    chipsSec.style.cssText = "padding:8px 12px;";
    const chipsHead = document.createElement("div");
    chipsHead.style.cssText = "display:flex;align-items:center;justify-content:space-between;margin-bottom:6px;";
    chipsHead.appendChild(dcText("Other Files", "muted", "font-size:10px;font-weight:600;letter-spacing:.5px;text-transform:uppercase;"));
    const chipsWrap = document.createElement("div");
    chipsWrap.style.cssText = "display:flex;gap:6px;flex-wrap:wrap;";
    chipsSec.appendChild(chipsHead);
    chipsSec.appendChild(chipsWrap);
    scroller.appendChild(chipsDivider);
    scroller.appendChild(chipsSec);
    inner.appendChild(scroller);

    // --- action row: Download / Cancel / X
    const actions = document.createElement("div");
    actions.setAttribute("data-dc-p", "borders");
    actions.style.cssText = "display:flex;gap:6px;box-sizing:border-box;padding:8px 12px;border-top:1px solid " + T.border + ";";
    const mkBtn = (label, role, on, icon) => {
      const b = document.createElement("button");
      b.type = "button";
      b.setAttribute("data-dc-p", role);
      b.style.cssText = "display:flex;align-items:center;justify-content:center;gap:4px;flex:1;box-sizing:border-box;"
        + "padding:7px 12px;margin:0;border-radius:5px;border:1px solid;cursor:pointer;"
        + "font:500 11px/1.2 system-ui,'Segoe UI',sans-serif;";
      if (label) b.appendChild(dcText(label, null, "white-space:nowrap;"));
      if (icon) b.appendChild(svgIcon(10, icon, 2.5));
      b.onclick = (ev) => {
        if (ev && ev.stopPropagation) ev.stopPropagation();
        if (on) on();
      };
      return b;
    };
    const primary = mkBtn(s.primaryLabel || "Download", "btnp", () => {
      hidePanels();
      if (s.onPrimary) s.onPrimary();
    }, ICON_DOWN);
    const cancel = mkBtn("Cancel", "btn", () => {
      hidePanels();
      if (s.onClose) s.onClose();
    }, null);
    const close = mkBtn("", "btn", () => {
      hidePanels();
      if (s.onClose) s.onClose();
    }, ICON_CLOSE);
    close.style.flex = "0 0 36px";
    close.title = "Close";
    close.setAttribute("aria-label", "Close");
    actions.appendChild(primary);
    actions.appendChild(cancel);
    actions.appendChild(close);
    inner.appendChild(actions);

    panel.appendChild(inner);

    function select(it) {
      if (selected && selected.el) selected.el.removeAttribute("data-dc-sel");
      selected = it || null;
      if (selected && selected.el) selected.el.setAttribute("data-dc-sel", "1");
      paintDcPanel(panel);
    }
    function sync() {
      badge.textContent = items.length === 1 ? "1 quality" : `${items.length} qualities`;
      gridDivider.style.display = items.length ? "block" : "none";
      gridSec.style.display = items.length ? "block" : "none";
      chipsDivider.style.display = chips.length ? "block" : "none";
      chipsSec.style.display = chips.length ? "block" : "none";
      scroller.style.display = (items.length || chips.length) ? "block" : "none";
    }
    const api = {
      panel,
      grid,
      items,
      select,
      selected: () => selected,
      addQuality: (q) => {
        const it = dcQualityItem(q);
        it.q = q;
        it.el.onclick = (ev) => {
          if (ev && ev.stopPropagation) ev.stopPropagation();
          select(it);
          if (q.onPick) q.onPick();
        };
        grid.appendChild(it.el);
        items.push(it);
        // The highest quality is selected up front (like the mockup), so the
        // primary button always has something to download.
        if (!selected) {
          select(it);
          if (q.onPick) q.onPick();
        }
        sync();
        return it;
      },
      addChip: (c) => {
        const chip = dcFileChip(c);
        chipsWrap.appendChild(chip.el);
        chips.push(chip);
        sync();
        return chip;
      },
      setInfo: (o) => {
        if (o.res != null) resEl.textContent = o.res;
        if (o.size != null) sizeEl.textContent = o.size ? "• " + o.size : "";
        if (o.format != null) fmtEl.textContent = String(o.format).toUpperCase();
      },
      setPrimaryLabel: (t) => { primary.firstChild.textContent = t; },
      setHint: (t) => { sendTxt.lastChild.textContent = t; },
      sync,
      show: () => { panel.style.display = "block"; paintDcPanel(panel); },
      hide: () => { panel.style.display = "none"; },
    };
    paintDcPanel(panel);
    sync();
    return api;
  }

  // Size enrichment for grid cells whose page JSON carried no contentLength:
  // ask the background for a HEAD/Range measurement. Never for rows with an
  // exact page-JSON size (they already print it) and never for "via app"
  // rows (a watch-page byte count would be a bogus size).
  function enrichSizes(pairs) {
    try {
      const need = (pairs || []).filter((p) => p && p.sizeEl && p.st
        && !(p.st._size > 0) && !p.st.ciphered && /^https?:\/\//i.test(p.st.url || ""));
      const urls = [...new Set(need.map((p) => p.st.url))].slice(0, 12);
      if (!urls.length) return;
      chrome.runtime.sendMessage({ type: "getSizes", urls, referer: location.href }, (res) => {
        if (chrome.runtime.lastError) return;
        if (!alive()) return;
        const sizes = (res && res.sizes) || {};
        for (const p of need) {
          const n = sizes[p.st.url] || 0;
          if (n > 0 && p.sizeEl.isConnected) {
            p.st._size = n;
            p.sizeEl.textContent = fmtSize(n);
          }
        }
      });
    } catch (e) { /* sizes are best-effort */ }
  }

  // itag → quality/container for labeling sniffed live streams (page JSON
  // hides adaptive URLs until playback, but the player's traffic carries
  // itags — this rebuilds IDM-style "1080p · mp4" rows from them).
  const ITAG_Q = {17:"144p",18:"360p",22:"720p",36:"240p",43:"360p",133:"240p",134:"360p",135:"480p",136:"720p",137:"1080p",160:"144p",242:"240p",243:"360p",244:"480p",247:"720p",248:"1080p",271:"1440p",272:"2160p",278:"144p",298:"720p60",299:"1080p60",300:"1440p60",301:"2160p60",302:"720p60",303:"1080p60",308:"1440p60",313:"2160p",315:"2160p60",394:"144p",395:"240p",396:"360p",397:"480p",398:"720p",399:"1080p"};
  const ITAG_A = {139:"48k",140:"128k",141:"256k",249:"50k",250:"70k",251:"160k",599:"32k",600:"64k",601:"128k"};
  const PROG_ITAG = new Set([17,18,22,36,43]);
  function sniffItag(m) {
    try {
      const im = String(m.url || "").match(/[?&]itag=(\d+)/);
      return im ? im[1] : "";
    } catch (e) { return ""; }
  }
  // Frame size for the panel's resolution line ("1080p" → "1920×1080").
  const RES_WH = {"4320p":"7680×4320","2160p":"3840×2160","1440p":"2560×1440","1080p":"1920×1080","720p":"1280×720","480p":"854×480","360p":"640×360","240p":"426×240","144p":"256×144"};
  function resWH(label, w, h) {
    if (w > 0 && h > 0) return `${w}×${h}`;
    const m = String(label || "").match(/^(\d+)p/);
    return (m && RES_WH[m[1] + "p"]) || String(label || "video");
  }

  // Sniffed live stream → panel cell parts (container + quality). Audio-only
  // traffic becomes an "Other Files" chip instead of a video cell.
  function sniffParts(m) {
    const itag = parseInt(sniffItag(m), 10) || 0;
    const cont = (contOf(m.mime) || "mp4").toUpperCase();
    if (itag && ITAG_Q[itag]) {
      return { video: true, format: cont + (PROG_ITAG.has(itag) ? "" : " + audio"), res: resWH(ITAG_Q[itag], 0, 0) };
    }
    if (itag && ITAG_A[itag]) return { video: false, format: cont, res: ITAG_A[itag] };
    return { video: /video/i.test(m.mime || ""), format: cont, res: "stream" };
  }

  // IDM-style network catch: merge background-sniffed playing streams
  // (googlevideo range requests from the running player) into the panel.
  // Needed because YouTube often embeds ZERO direct URLs for music/VEVO
  // videos — the page JSON has formats but no `url` fields; only the live
  // traffic carries the deciphered stream URLs. Sniffed URLs carry the
  // player's deciphered `n` parameter, so cells whose itag matches a sniffed
  // stream get their URL SWAPPED to the fresh one (stale page URLs 403).
  function appendSniffed(api, known, pairs, bestAudio, baseName) {
    try {
      const clean = String(baseName || "video").replace(/\.mp4$/i, "");
      chrome.runtime.sendMessage({ type: "getMedia", pageUrl: location.href }, (res) => {
        if (chrome.runtime.lastError) return;
        if (!alive() || !api || !api.panel.isConnected) return;
        const all = (res && res.items) || [];
        const items = all.filter((m) => m && /^https?:\/\//i.test(m.url) && !known.has(m.url));
        // Video-only traffic gets an audio track paired, so the app can merge
        // it: the page's best audio when it has a direct URL, else the audio
        // the sniffer just saw in the same playback session.
        const sniffAudio = all.find((x) => x && /^audio\//i.test(x.mime || "") && /^https?:\/\//i.test(x.url || ""));
        const pickAudio = (bestAudio && bestAudio.url)
          ? { url: bestAudio.url, mime: bestAudio.mime }
          : (sniffAudio ? { url: sniffAudio.url, mime: sniffAudio.mime } : null);
        const pairOf = (m) => (pickAudio && pickAudio.url && pickAudio.url !== m.url) ? pickAudio : null;
        const byItag = {};
        for (const m of all) {
          try {
            const im = String(m.url || "").match(/[?&]itag=(\d+)/);
            if (im && !byItag[im[1]]) byItag[im[1]] = m;
          } catch (e) {}
        }
        // Swap stale page URLs for fresh player-deciphered ones (same itag):
        // the row stops being a "via app" hint and becomes a real download.
        const swapped = [];
        const swappedItags = new Set();
        for (const p of pairs || []) {
          const tag = p.st && p.st.itag ? String(p.st.itag) : null;
          const fresh = tag && byItag[tag];
          if (fresh && fresh.url !== p.st.url && p.item && p.item.el.isConnected) {
            p.st.url = fresh.url;
            p.st.ciphered = false;
            const cont = (contOf(fresh.mime) || p.st.disp || "mp4").toUpperCase();
            const fmt = cont + (p.st.kind === "v" ? " + audio" : "");
            if (p.item.fmtEl) p.item.fmtEl.textContent = fmt;
            if (p.item.q) p.item.q.format = fmt;
            p.item.el.title = `${p.st.label} · ${cont} · fresh URL from the player`;
            known.add(fresh.url);
            swappedItags.add(tag);
            swapped.push(p);
          }
        }
        if (swapped.length) enrichSizes(swapped);
        if (!items.length) return;
        // Late playback can add streams after the first merge: append the
        // new ones as extra grid cells (same look as the page-JSON rows),
        // never duplicating a row that is already listed — neither a cell
        // that was just refreshed in place, nor one with the same label.
        items.slice(0, 8).forEach((m) => {
          known.add(m.url);
          const part = sniffParts(m);
          const tag = sniffItag(m);
          if (tag && swappedItags.has(tag)) return;
          if (!part.video) {
            api.addChip({
              label: `♪ ${part.format} ${part.res}`,
              tip: m.url.slice(0, 120),
              send: () => sendDownload(m.url, `${clean} [audio]${mimeExt(m.mime)}`, m.mime, m.size || null),
            });
            return;
          }
          if (api.items.some((x) => x.q && x.q.format === part.format && x.q.res === part.res)) return;
          if (tag && api.items.some((x) => x.q && x.q.st && String(x.q.st.itag) === tag)) return;
          const qtag = ITAG_Q[parseInt(tag, 10)] || part.res;
          const it = api.addQuality({
            format: part.format,
            res: part.res,
            size: m.size > 0 ? fmtSize(m.size) : "",
            tip: `${m.url.slice(0, 120)} — detected from the player`,
            send: () => sendDownload(m.url, `${clean} [${qtag}].mp4`, m.mime, m.size || null, pairOf(m)),
          });
          enrichSizes([{ item: it, sizeEl: it.sizeEl, st: { url: m.url, _size: m.size || 0 } }]);
        });
        api.sync();
        try { paintDcPanel(api.panel); } catch (e) { /* theme is best-effort */ }
      });
    } catch (e) { /* ignore */ }
  }

  // Dismiss just this video, surviving periodic refreshes and quality menu
  // rebuilds. A different video gets its own control; page reload resets it.
  const dismissedVideos = new Set();
  function dismissalKey(video, id) {
    return DCRank.videoDismissKey(location.href, video && (video.currentSrc || video.src), id);
  }
  function syncDismissControls() {
    // Remove the separate X buttons left by an older injected extension.
    document.querySelectorAll("button[data-dc-dismiss]").forEach(el=>el.remove());
    document.querySelectorAll("[data-dc-pill]").forEach(host=>{
      const video=host.__dcVideo;
      if (!video || !video.isConnected) { host.remove(); return; }
      const fullscreen=document.fullscreenElement;
      const parent=fullscreen && fullscreen.contains(video) ? fullscreen : document.documentElement;
      if(host.parentNode!==parent)parent.appendChild(host);
      const r=video.getBoundingClientRect();
      const vw=window.innerWidth, vh=window.innerHeight;
      const width=Math.min(240,r.width-16,vw-16);
      host.style.display=r.width>=120 && r.height>=70 && r.bottom>0 && r.top<vh && r.right>0 && r.left<vw ? "block" : "none";
      if(activeBarDrag===host || applySavedBarPosition(host))return;
      host.style.position="fixed";host.style.right="auto";host.style.bottom="auto";
      host.style.width=Math.max(0,width)+"px";
      host.style.left=Math.max(8,Math.min(vw-width-8,r.right-width-8))+"px";
      host.style.top=Math.max(8,Math.min(vh-36,r.top+8))+"px";
    });
    const yt=document.getElementById && document.getElementById("dc-yt-btn");
    if(yt)placeInPlayer(yt,document.getElementById("dc-yt-panel"));
  }
  function addDismissControl(button, key, panel) {
    const close=button.__dcClose;
    if(!close)return;
    close.hidden=false;
    close.onclick = ev => {
      ev.preventDefault();ev.stopPropagation();
      if(dismissedVideos.size>=100)dismissedVideos.clear();
      dismissedVideos.add(typeof key==="function" ? key() : key);
      button.remove();
      if(panel)panel.remove();
      if(button.__dcFbPanel)button.__dcFbPanel.remove();
    };
  }
  document.addEventListener("scroll",syncDismissControls,true);
  window.addEventListener("resize",syncDismissControls);
  document.addEventListener("fullscreenchange",syncDismissControls);

  function ensureButton(info) {
    const key = dismissalKey(null,info.videoId);
    if (dismissedVideos.has(key)) {
      const old=document.getElementById("dc-yt-btn");
      if(old){if(old.__dcClose)old.__dcClose.remove();old.remove();}
      const panel=document.getElementById("dc-yt-panel");if(panel)panel.remove();
      return;
    }
    syncDismissControls();
    const oldBtn = document.getElementById("dc-yt-btn");
    const oldPanel = document.getElementById("dc-yt-panel");
    if (oldBtn && oldBtn.dataset.gen !== String(GEN)) {
      // Adopted DOM from an older live instance (e.g. re-injected after
      // an update): always rebuild so the handlers belong to this
      // (current) generation.
      oldBtn.remove();
      if (oldPanel) oldPanel.remove();
    }
    // Rebuild whenever the content signature changes — not just the video
    // id. Expiries AND url sets are included so refreshed (re-issued) URLs
    // rebuild the menu instead of serving the stale 403-prone rows forever.
    const expSig = (l) => (l || []).map((s) => {
      try {
        const m = String(s.url || "").match(/[?&]expire=(\d+)/);
        return m ? m[1] : "0";
      } catch (e) { return "0"; }
    }).join(",");
    const urlSig = (l) => {
      // Cheap rotation fingerprint: itags + url-length checksum. Same-second
      // re-issues with identical expiries still rebuild.
      let h = 0;
      for (const s of (l || [])) {
        const u = String(s.url || "");
        h = (h * 31 + (s.itag || 0)) >>> 0;
        h = (h * 31 + u.length) >>> 0;
        for (let i = 0; i < Math.min(u.length, 48); i++) h = (h * 31 + u.charCodeAt(i)) >>> 0;
      }
      return String(h);
    };
    const sigParts = [
      info.videoId, info.title,
      (info.streams || []).length,
      (info.videoOnly || []).length,
      (info.audio || []).length,
      expSig(info.streams), expSig(info.videoOnly), expSig(info.audio),
      urlSig(info.streams), urlSig(info.videoOnly), urlSig(info.audio),
    ];
    const sig = sigParts.join("|");
    // NOTE: the old button/panel were already dropped above when they
    // belonged to an older generation, so reaching here means same-generation
    // DOM.
    if (oldBtn && oldBtn.isConnected) {
      if (oldBtn.dataset.sig === sig) {
        // Same content: keep everything, just re-anchor (the player
        // element can be recreated by SPA navigation).
        placeInPlayer(oldBtn, oldPanel);
        addDismissControl(oldBtn,key,oldPanel);
        return;
      }
      oldBtn.remove();
      if (oldPanel) oldPanel.remove();
    }
    const btn = videoDownloadBar("Choose video quality in Download Craft");
    btn.id = "dc-yt-btn";
    btn.dataset.vid = info.videoId;
    btn.dataset.gen = String(GEN);
    btn.dataset.sig = sigParts.join("|");
    const fname = safeFilename(info.title);
    const tagged = (base, st) => {
      const clean = base.replace(/\.mp4$/i, "");
      if (st.kind === "v") return `${clean} [${st.label}].mp4`;
      if (st.kind === "a") return `${clean} [audio]${mimeExt(st.mime)}`;
      return base;
    };
    // Video-only cells bundle the best audio track so the app merges them to
    // a single file (IDM's "MKV file" behavior).
    const bestAudio = (info.audio || []).slice().sort((a, b) => (b.br || 0) - (a.br || 0))[0] || null;
    const pairFor = (st) => (st.kind === "v" && bestAudio && bestAudio.url !== st.url)
      ? { url: bestAudio.url, mime: bestAudio.mime }
      : null;
    const sendStream = (st, name) => {
      // Ciphered rows have no URL at build time: hand the page + quality hint
      // over and the app deciphers that exact quality itself.
      if (st.ciphered && !st.url) {
        sendDownload(location.href, name, st.mime, st._size || null, pairFor(st), st.label, st._dur || null);
      } else {
        // A direct row already carries the exact stream, but the quality hint
        // travels with it too: if the app ever has to re-resolve the page (a
        // stale signed URL), it must not silently fall back to "best".
        sendDownload(st.url || location.href, name, st.mime, st._size || null, pairFor(st), st.label, st._dur || null);
      }
    };
    // One cell per quality. The same height + container twice (a progressive
    // row and its video-only twin) keeps the progressive one: it plays as-is.
    const seenQ = new Set();
    const entries = [];
    const addEntry = (st, kind, viaApp) => {
      const hh = parseInt((String(st.label || "").match(/^(\d+)/) || [0, 0])[1], 10) || 0;
      const key = `${hh}|${String(st.disp || "").toLowerCase()}`;
      if (!hh || seenQ.has(key)) return;
      seenQ.add(key);
      const cont = (st.disp || contOf(st.mime) || "mp4").toUpperCase();
      const fps = (String(st.label || "").match(/^(\d+)p(\d+)$/) || [])[2];
      // A video-only cell is downloaded together with the paired audio leg, so
      // its shown size has to include that leg or the row's number never
      // matches the merged file the app reports (the complaint that started
      // this: the panel's size and the progress window's size disagreed).
      const audioSz = (kind === "v" && !viaApp && bestAudio && bestAudio._size > 0)
        ? bestAudio._size
        : 0;
      entries.push({
        h: hh,
        size: (st._size || 0) + audioSz,
        dur: st._dur || 0,
        st,
        // Ciphered/"via app" rows have no container pairing of their own —
        // the app decides, so no "+ audio" marker there.
        format: cont + (viaApp ? " via app" : (kind === "v" ? " + audio" : "")),
        res: resWH(st.label, st.w, st.h),
        tip: `${st.label}${fps ? ` (${fps} fps)` : ""} · ${cont} · ${info.title}`
          + (kind === "v" ? " — merged with the audio track when the download finishes" : "")
          + (viaApp ? " — the app resolves this quality itself" : ""),
        send: () => sendStream(st, tagged(fname, st)),
      });
    };
    for (const st of (info.streams || [])) addEntry(st, "av", !!st.ciphered);
    for (const st of (info.videoOnly || [])) addEntry(st, "v", !!st.ciphered);
    // Full ladder fallback: when the page barely exposes direct rows
    // (ciphered embeds, music videos), still offer every standard quality like
    // IDM does — the app resolves each to the closest stream at or below it.
    // Sizes come from the page-JSON map (contentLength per quality).
    if (entries.length < 6) {
      const sizeByQ = {};
      for (const [q, m] of Object.entries(info.sizes || {})) {
        if (!sizeByQ[q] && m && m.size > 0) sizeByQ[q] = m.size;
      }
      for (const s of [...(info.streams || []), ...(info.videoOnly || [])]) {
        if (s && s.label && s._size > 0 && !sizeByQ[s.label]) sizeByQ[s.label] = s._size;
      }
      const sizeForQ = (q) => {
        if (sizeByQ[q]) return sizeByQ[q];
        const k = Object.keys(sizeByQ).find((x) => x.startsWith(q));
        return k ? sizeByQ[k] : 0;
      };
      const haveH = new Set(entries.map((e) => e.h));
      for (const q of [1080, 720, 480, 360, 240, 144]) {
        if (haveH.has(q)) continue;
        const label = `${q}p`;
        const sz = sizeForQ(label);
        entries.push({
          h: q,
          size: sz,
          dur: 0,
          st: null,
          format: "MP4 via app",
          res: RES_WH[label],
          tip: `${label} · MP4 · the app resolves this quality itself`,
          send: () => sendDownload(location.href, fname, "video/mp4", sz || null, null, label, null),
        });
      }
    }
    entries.sort((a, b) => (b.h - a.h) || (b.size - a.size));
    // "Direct" means at least one row the browser already holds a real URL
    // for — ciphered pages (VEVO) only carry qualities the app must resolve,
    // so they get the mockup's "press play" hint instead.
    const hasDirect = entries.some((e) => e.st && e.st.url);
    const infoOf = (e) => ({
      res: e ? e.res : "",
      size: e && e.size > 0 ? fmtSize(e.size) : "",
      format: e ? e.format.split(" ")[0] : "MP4",
    });
    let api = null;
    api = dcPanel({
      title: info.title,
      thumb: info.thumb,
      dur: entries[0] && entries[0].dur > 0 ? fmtDurSec(entries[0].dur) : "",
      res: infoOf(entries[0]).res,
      size: infoOf(entries[0]).size,
      format: infoOf(entries[0]).format,
      hint: hasDirect
        ? "Pick a quality below, or send the link as-is."
        : "No direct links yet — press play, then reopen for fresh qualities.",
      onSendLink: () => sendDownload(location.href, fname),
      primaryLabel: entries.length ? "Download" : "Send to app",
      onPrimary: () => {
        const it = api.selected();
        if (it && it.q && it.q.send) it.q.send();
        else sendDownload(location.href, fname);
      },
    });
    api.panel.id = "dc-yt-panel";
    const sizePairs = [];
    entries.forEach((e) => {
      const it = api.addQuality({
        format: e.format,
        res: e.res,
        size: e.size > 0 ? fmtSize(e.size) : "",
        tip: e.tip,
        send: e.send,
        onPick: () => api.setInfo(infoOf(e)),
      });
      if (e.st) sizePairs.push({ item: it, sizeEl: it.sizeEl, st: e.st });
    });
    // "Other Files": subtitle tracks and audio-only streams as chips.
    for (const sb of (info.subs || [])) {
      api.addChip({
        label: `📄 VTT ${sb.label}`,
        tip: sb.url.slice(0, 120),
        send: () => sendDownload(sb.url, `${fname} [${sb.lang || sb.label}].vtt`, "text/vtt", null, null, null, null),
      });
    }
    for (const st of (info.audio || [])) {
      const kb = st.br > 0 ? Math.round(st.br / 1000) + "k" : st.label;
      api.addChip({
        label: `♪ ${(st.disp || "m4a").toUpperCase()} ${kb}`,
        tip: `${st.label || "audio"}${st._size > 0 ? " · " + fmtSize(st._size) : ""} — audio-only track`,
        send: () => sendDownload(st.url, tagged(fname, st), st.mime, st._size || null, null, null, st._dur || null),
      });
    }
    api.sync();
    enrichSizes(sizePairs);
    markQueuedRows(sizePairs);
    appendSniffed(api, new Set([
      ...info.streams.map((s) => s.url),
      ...(info.videoOnly || []).map((s) => s.url),
      ...(info.audio || []).map((s) => s.url),
    ]), sizePairs, bestAudio, fname);
    btn.onclick = (ev) => {
      ev.stopPropagation();
      if (api.panel.style.display === "none") {
        hidePanels();
        api.show();
        positionDraggedPanel(btn,api.panel);
      } else {
        api.hide();
      }
    };
    installDcCloser();
    placeInPlayer(btn, api.panel);
    addDismissControl(btn,key,api.panel);
  }

  function refreshYouTube() {
    const id = ytVideoId();
    const oldBtn = document.getElementById("dc-yt-btn");
    const oldPanel = document.getElementById("dc-yt-panel");
    if (!id) {
      if (oldBtn) oldBtn.remove();
      if (oldPanel) oldPanel.remove();
      try {
        chrome.storage.local.set({ downloadCraftVideos: [] });
      } catch (e) {
        /* ignore */
      }
      return false;
    }
    // Direct page streams when YouTube exposes them (fast path with
    // quality choice); otherwise the app resolves the video itself via
    // its player-API fallback — so the button always appears.
    // ytStreams() matches the CURRENT video id, so SPA autoplay can
    // never show the previous video's title/streams.
    const info = ytStreams() || { videoId: id, title: document.title.replace(/ - YouTube$/, ""), thumb: `https://i.ytimg.com/vi/${id}/hqdefault.jpg`, streams: [], videoOnly: [], audio: [], subs: [] };
    try {
      chrome.storage.local.set({
        downloadCraftVideos: [{
          videoId: info.videoId, title: info.title, thumb: info.thumb || "",
          streams: info.streams, videoOnly: info.videoOnly || [], audio: info.audio || [],
          sizes: info.sizes || {}, subs: info.subs || [],
          pageUrl: location.href,
        }],
        downloadCraftPage: location.href,
      });
    } catch (e) {
      /* ignore */
    }
    ensureButton(info);
    return true;
  }

  // ---------- Universal video overlay (IDM-style) ----------
  // Floating "Download this video" pill on EVERY playable <video>, not just
  // YouTube. Uses the element's currentSrc when it is http(s), otherwise
  // falls back to network-sniffed candidates from the background worker or
  // the page URL (engine resolves og:video / VideoObject server-side).
  function mediaUrlOf(el) {
    try {
      const u = el.currentSrc || el.src || "";
      if (/^https?:\/\//i.test(u)) return u;
      const s = el.querySelector && el.querySelector("source[src]");
      if (s && /^https?:\/\//i.test(s.src)) return s.src;
    } catch (e) { /* ignore */ }
    return "";
  }

  function ensureUniversalButtons() {
    if (!alive()) return;
    syncDismissControls();
    // YouTube watch/Shorts pages already have the quality menu button —
    // a second generic pill on the same player is the "double button" bug.
    try { if (ytVideoId()) return; } catch (e) { /* ignore */ }
    try {
      // Drop pills left by an older injected generation (their handlers are
      // dead after an extension reload) so only this generation's pills show.
      document.querySelectorAll("[data-dc-pill]").forEach((el) => {
        if (el.dataset.gen !== String(GEN)) el.remove();
      });
      const vids = [...document.querySelectorAll("video")].slice(0, 10);
      for (const v of vids) {
        const key = dismissalKey(v,null);
        if (dismissedVideos.has(key)) continue;
        if (v.__dcBtn) {
          if (!v.__dcBtn.isConnected) v.__dcBtn = null;
          else if (v.__dcKey === key) continue;
          else { if(v.__dcBtn.__dcClose)v.__dcBtn.__dcClose.remove(); v.__dcBtn.remove();v.__dcBtn=null; }
        }
        // Skip anything inside YouTube's player (covered by its own button).
        try { if (v.closest && v.closest("#movie_player")) continue; } catch (e) {}
        // Skip tiny preview thumbnails.
        const r = v.getBoundingClientRect ? v.getBoundingClientRect() : null;
        if (r && (r.width < 120 || r.height < 70)) continue;
        const pill = videoDownloadBar("Download this video with Download Craft");
        pill.setAttribute("data-dc-pill", "1");
        pill.dataset.gen = String(GEN);
        pill.style.position = "absolute";
        pill.style.top = "8px";
        pill.style.right = "8px";
        pill.style.zIndex = "2147483646";
        pill.onclick = (ev) => {
          ev.stopPropagation(); ev.preventDefault();
          // Facebook reels: open the IDM-style quality menu (title + HD/SD
          // rows with sizes) instead of firing one mystery download — the
          // player only holds a blob: URL and every reel needs no reload.
          if (isFacebookPage()) {
            handleFbPill(pill);
            return;
          }
          const direct = mediaUrlOf(v);
          if (direct) {
            const base = mediaBase(direct);
            const ext = /\.mp3|\.m4a|\.wav|\.ogg/i.test(direct) ? "" : ".mp4";
            let vdur = null;
            try {
              const d = Math.round(v.duration);
              if (Number.isFinite(d) && d > 0 && d < 86400) vdur = d;
            } catch (e) { /* duration unknown */ }
            sendDownload(direct, base + ext, v.getAttribute && v.getAttribute("type"), null, null, null, vdur);
          } else {
            // Ask background for sniffed media URLs for this tab first.
            // (Facebook reels render as blob: players — the real fbcdn/mp4
            // URL only exists in the sniffed network traffic.)
            try {
              chrome.runtime.sendMessage({ type: "getMedia", pageUrl: location.href }, (res) => {
                if (chrome.runtime.lastError) {
                  if (ctxInvalidated(chrome.runtime.lastError)) markDead();
                  sendDownload(location.href, undefined, null);
                  return;
                }
                const items = (res && res.items) || [];
                const best = bestSniffed(items);
                if (best) {
                  const base = mediaBase(best.url);
                  sendDownload(best.url, base + ".mp4", best.mime, best.size || null);
                } else {
                  sendDownload(location.href, undefined, null);
                }
              });
            } catch (e) {
              if (ctxInvalidated(e)) markDead();
              sendDownload(location.href, undefined, null);
            }
          }
        };
        try {
          pill.__dcVideo=v;
          document.documentElement.appendChild(pill);
          v.__dcBtn=pill;v.__dcKey=key;
          addDismissControl(pill,()=>dismissalKey(v,null),null);
          syncDismissControls();
        } catch (e) { /* overlay is best-effort */ }
      }
    } catch (e) { /* ignore */ }
  }

  function isFacebookPage() {
    try {
      const h = location.hostname.toLowerCase();
      return h === "facebook.com" || h.endsWith(".facebook.com");
    } catch (e) { return false; }
  }

  // Facebook quality panel (IDM-style): the same circle + bubble as YouTube,
  // with the reel's HD/SD rows (and any fresh sniffed stream) as quality
  // cells, so every reel offers a choice instead of one mystery "Facebook.mp4".
  function openFbMenu(pill, rows, title, thumb, audio, pair) {
    try { document.querySelectorAll("#dc-fb-panel").forEach((m) => m.remove()); } catch (e) {}
    const first = (rows && rows[0]) || {};
    const fname = first.fname || fbFilename(title);
    let api = null;
    api = dcPanel({
      title,
      thumb: thumb || first.thumb || "",
      dur: first.dur > 0 ? fmtDurSec(first.dur) : "",
      res: "",
      size: "",
      format: "MP4",
      hint: "Pick a quality below, or send the link as-is.",
      onSendLink: () => sendDownload(location.href, fname, null, null, null, null, first.dur || null),
      onPrimary: () => {
        const it = api.selected();
        if (it && it.q && it.q.send) it.q.send();
      },
    });
    api.panel.id = "dc-fb-panel";
    pill.__dcFbPanel=api.panel;
    const sizePairs = [];
    (rows || []).forEach((st) => {
      const qm = String(st.label || "").match(/quality\s+(.+?)(?:,|$)/i);
      const q = {
        res: (qm && qm[1]) || "video",
        size: st._size > 0 ? fmtSize(st._size) : "",
        format: "MP4",
        tip: `${(qm && qm[1]) || "video"} · MP4 · ${title}`,
        send: () => sendDownload(st.url, st.fname, "video/mp4", st._size || null, (pair && !st.fromJson) ? pair : null, st.q || null, st.dur || null),
      };
      const it = api.addQuality({ ...q, onPick: () => api.setInfo(q) });
      sizePairs.push({ item: it, sizeEl: it.sizeEl, st });
    });
    // "Other Files": audio-only streams the sniffer saw in this session.
    for (const a of (audio || [])) {
      const base = String(fname).replace(/\.mp4$/i, "");
      api.addChip({
        label: `♪ ${(mimeExt(a.mime) || ".m4a").replace(".", "").toUpperCase()}${a.size > 0 ? " " + fmtSize(a.size) : ""}`,
        tip: `${a.mime || "audio"} — audio-only track`,
        send: () => sendDownload(a.url, `${base} [audio]${mimeExt(a.mime) || ".m4a"}`, a.mime, a.size || null, null, null, first.dur || null),
      });
    }
    api.sync();
    enrichSizes(sizePairs);
    api.panel.style.display = "block";
    // Page-anchored, viewport-clamped: the player box clips a panel that is
    // wider than the reel column (see placeFbPanel).
    placeFbPanel(pill, api.panel);
    paintDcPanel(api.panel);
  }

  function handleFbPill(pill) {
    const info = fbInfo();
    try {
      chrome.runtime.sendMessage({ type: "getMedia", pageUrl: location.href }, (res) => {
        if (chrome.runtime.lastError) {
          if (ctxInvalidated(chrome.runtime.lastError)) markDead();
          sendDownload(location.href, fbFilename(info.title), null, null, null, null, info.durSec || null);
          return;
        }
        const items = (res && res.items) || [];
        const data = fbMenuRows(info, items);
        const rows = data.rows;
        const audio = data.audio;
        if (!rows.length) {
          sendDownload(location.href, fbFilename(info.title), null, null, null, null, info.durSec || null);
        } else if (rows.length === 1 && !audio.length) {
          const r0 = rows[0];
          sendDownload(r0.url, r0.fname, "video/mp4", r0._size || null, (data.pair && !r0.fromJson) ? data.pair : null, r0.q || null, r0.dur || info.durSec || null);
        } else {
          openFbMenu(pill, rows, info.title, info.thumb, audio, data.pair);
        }
      });
    } catch (e) {
      if (ctxInvalidated(e)) markDead();
      sendDownload(location.href, fbFilename(info.title), null, null, null, null, info.durSec || null);
    }
  }
  function pullNetworkMedia() {
    try {
      chrome.runtime.sendMessage({ type: "getMedia", pageUrl: location.href }, (res) => {
        if (chrome.runtime.lastError) return;
        if (!alive()) return;
        const items = (res && res.items) || [];
        if (!items.length) return;
        // On Facebook the tab's list spans every reel of the session, so the
        // streams offered for storage are narrowed to the reel on screen -
        // otherwise the popup keeps offering the reel the user scrolled away
        // from.
        let use = items;
        try {
          if (isFacebookPage()) {
            fbSyncReel();
            use = fbReelMedia(items, fbReelAt);
          }
        } catch (e) { use = items; }
        if (!use.length) return;
        try {
          chrome.storage.local.get(["downloadCraftLinks"], (cur) => {
            const have = new Set(((cur && cur.downloadCraftLinks) || []).map((l) => l.url));
            let pageThumb = "";
            try {
              const pv = document.querySelector("video[poster]");
              const pp = pv && pv.getAttribute("poster");
              if (/^https?:\/\//i.test(pp || "")) pageThumb = pp;
            } catch (e) { /* ignore */ }
            const add = use
              .filter((m) => !have.has(m.url))
              .map((m) => ({ url: m.url, text: `Stream (${m.mime || "media"})`, thumb: m.thumb || pageThumb }));
            if (add.length) {
              chrome.storage.local.get(["downloadCraftLinks", "downloadCraftPage"], (c2) => {
                const merged = [...(((c2 && c2.downloadCraftLinks) || [])), ...add].slice(0, 50);
                chrome.storage.local.set({ downloadCraftLinks: merged, downloadCraftPage: location.href });
              });
            }
          });
        } catch (e) { /* ignore */ }
      });
    } catch (e) { /* background unreachable */ }
  }

  // Popup peek: the popup asks the ACTIVE tab directly so it never shows
  // another tab's stale snapshot (global storage keys are fallback only).
  try {
    chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
      if (msg && msg.type === "restoreVideoButtons" && alive()) {
        dismissedVideos.clear();
        refreshYouTube();ensureUniversalButtons();
        sendResponse({ok:true});return false;
      }
      if (msg && msg.type === "mediaFrameInfo") {
        const videos = [...document.querySelectorAll("video")];
        const d = videos.length === 1 ? videos[0].duration : 0;
        sendResponse({pageUrl:location.href, videoCount:videos.length,
          audioExpected:videos.length === 1 && playerHasAudio(videos[0]),
          durationSecs:Number.isFinite(d) && d > 0 && d < 86400 ? Math.max(1, Math.round(d)) : null});
        return false;
      }
      if (msg && msg.type === "vimeoFramePage") {
        sendResponse({ pageUrl: location.href });
        return false;
      }
      if (!msg || msg.type !== "peekState") return false;
      if (!alive()) return false;
      (async () => {
        try {
          collectLinks();
          refreshYouTube();
        } catch (e) { /* best effort */ }
        let net = [];
        try {
          const r = await api.runtime.sendMessage({ type: "getMedia", pageUrl: location.href });
          net = (r && r.items) || [];
        } catch (e) { /* ignore */ }
        let cur = {};
        try {
          cur = await new Promise((resolve) => {
            chrome.storage.local.get(
              ["downloadCraftVideos", "downloadCraftLinks"],
              (c) => resolve(c || {})
            );
          });
        } catch (e) { /* ignore */ }
        sendResponse({
          videos: cur.downloadCraftVideos || [],
          links: cur.downloadCraftLinks || [],
          net,
          page: location.href,
          title: document.title || "",
        });
      })();
      return true;
    });
  } catch (e) { /* messaging unavailable */ }

  // Ordinary archive/installer links and explicit <a download> links can
  // bypass the browser downloader entirely. Preserve modified clicks and
  // fall back to the original link if the app declines capture.
  const bypassDownloadLinks = new WeakSet();
  document.addEventListener("click", ev => {
    if (!alive() || ev.button !== 0 || ev.ctrlKey || ev.metaKey || ev.shiftKey || ev.altKey) return;
    const a = ev.target && ev.target.closest && ev.target.closest("a[href]");
    if (!a) return;
    if (bypassDownloadLinks.has(a)) { bypassDownloadLinks.delete(a); return; }
    if (!DCRank.directDownloadLink(a.href,a.hasAttribute("download"))) return;
    ev.preventDefault(); ev.stopImmediatePropagation();
    const fallback = () => { bypassDownloadLinks.add(a); a.click(); };
    try {
      chrome.runtime.sendMessage({type:"captureLink",url:a.href,
        filename:a.getAttribute("download") || null,pageUrl:location.href}, res => {
        if (chrome.runtime.lastError || !res || !res.ok) fallback();
      });
    } catch (e) { fallback(); }
  },true);

  loadBarPosition();
  window.addEventListener("focus",loadBarPosition);

  // Initial pass + SPA navigation (YouTube AND Facebook rarely reload).
  // Facebook reels swap /reel/<id> via history API: no reload, same video
  // element, new blob: src. Patch pushState/replaceState + popstate so the
  // pill + sniffed candidates refresh instantly instead of on the 2 s poll.
  // The player response can arrive after the content script runs, so
  // retry a few times before giving up.
  collectLinks();
  ensureUniversalButtons();
  pullNetworkMedia();
  installDcCloser();
  function spaRefresh() {
    if (__dcDead || !alive()) return;
    // A reel swap is an SPA navigation inside the same document: mark the reel
    // as current right away so everything captured before it counts as the reel
    // the user scrolled away from.
    try { if (isFacebookPage()) fbSyncReel(); } catch (e) { /* best effort */ }
    try { collectLinks(); } catch (e) { if (ctxInvalidated(e)) return markDead(); }
    try { ensureUniversalButtons(); } catch (e) { if (ctxInvalidated(e)) return markDead(); }
    try { pullNetworkMedia(); } catch (e) { if (ctxInvalidated(e)) return markDead(); }
    try { refreshYouTube(); } catch (e) { if (ctxInvalidated(e)) return markDead(); }
  }
  try {
    const fireSpa = () => setTimeout(spaRefresh, 300);
    if (!window.__dcHistPatched) {
      window.__dcHistPatched = true;
      const ps = history.pushState;
      const rs = history.replaceState;
      history.pushState = function () { try { ps.apply(this, arguments); } finally { fireSpa(); } };
      history.replaceState = function () { try { rs.apply(this, arguments); } finally { fireSpa(); } };
      window.addEventListener("popstate", fireSpa);
      window.addEventListener("hashchange", fireSpa);
    }
  } catch (e) { /* history patch is best-effort */ }
  try {
    // Debounced + generation-gated: a superseded instance disconnects
    // instead of fighting the new one, and mutation storms (infinite
    // scroll) collapse into one pass per 400 ms.
    let moTimer = 0;
    const mo = new MutationObserver(() => {
      if (!alive()) { try { mo.disconnect(); } catch (e) {} return; }
      clearTimeout(moTimer);
      moTimer = setTimeout(() => {
        if (!alive()) return;
        collectLinks();
        ensureUniversalButtons();
      }, 400);
    });
    mo.observe(document.documentElement, { childList: true, subtree: true });
    __dcObservers.push(mo);
  } catch (e) { /* observer optional */ }
  __dcTimers.push(setInterval(() => { if (__dcDead || !alive()) return; try { pullNetworkMedia(); } catch (x) { if (ctxInvalidated(x)) markDead(); } }, 8000));
  if (!refreshYouTube()) {
    let tries = 0;
    const timer = setInterval(() => {
      tries++;
      if (__dcDead || !alive() || refreshYouTube() || tries >= 6) clearInterval(timer);
    }, 2000);
    __dcTimers.push(timer);
  }
  document.addEventListener("yt-navigate-finish", () => {
    if (!alive()) return; // superseded by a newer injected instance
    collectLinks();
    ensureUniversalButtons();
    refreshYouTube();
  });
  let lastUrl = location.href;
  let lastTitle = document.title;
  let lastReel = "";
  try { if (isFacebookPage()) lastReel = fbSyncReel(); } catch (e) { lastReel = ""; }
  const watcher = setInterval(() => {
    if (__dcDead || !alive()) { clearInterval(watcher); return; } // superseded: stop
    // The reels feed also changes the player without changing the URL (the
    // overlay keeps "/"), so the reel key is part of the change test.
    let reel = "";
    try { if (isFacebookPage()) reel = fbSyncReel(); } catch (e) { reel = ""; }
    if (location.href !== lastUrl || document.title !== lastTitle || reel !== lastReel) {
      lastUrl = location.href;
      lastTitle = document.title;
      lastReel = reel;
      // Drop pills bound to the previous reel's <video> node so the new
      // reel gets a fresh pill even when Facebook reuses the DOM node.
      try {
        document.querySelectorAll("video").forEach((v) => {
          if (v.__dcBtn && !v.__dcBtn.isConnected) v.__dcBtn = null;
        });
      } catch (e) {}
      collectLinks();
      ensureUniversalButtons();
      refreshYouTube();
    }
  }, 1000);
  __dcTimers.push(watcher);
})();
