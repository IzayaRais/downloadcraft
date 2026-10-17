// Content script: collects downloadable links AND page media
// (<video>/<audio>/<source>) so the popup can one-click them.
// On YouTube watch pages it additionally extracts direct progressive
// streams from the page player (real browser session: real cookies/TLS,
// so no bot-wall) and offers an in-page download button with qualities.
(() => {
  // Must match manifest version + background.js EXT_VER. The background
  // rejects messages from any other version with a "reload this tab"
  // error, so a stale script can never fail mysteriously.
  const EXT_VER = "0.9.1";
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
      return { videoId: id, title: docTitle || "youtube_video", thumb: ytThumb(pr, id), streams: [], videoOnly: [], audio: [] };
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
      return { url: null, label: qlabel(f), mime, disp: contOf(mime), kind, itag: f.itag || 0, ciphered: true, _size: sizeOf(f), _dur: durOf(f) };
    });
    // Progressive = video+audio in one file (ready to play).
    const streams = direct(pr.streamingData.formats).map((f) => {
      const mime = String(f.mimeType || "").split(";")[0];
      return { url: f.url, label: qlabel(f), mime, disp: contOf(mime), kind: "av", itag: f.itag || 0, _size: sizeOf(f), _dur: durOf(f) };
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
        return { url: f.url, label: qlabel(f), mime, disp: contOf(mime), kind: "v", itag: f.itag || 0, _size: sizeOf(f), _dur: durOf(f) };
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
    return { videoId: id, title, thumb: ytThumb(pr, id), streams, videoOnly, audio, sizes };
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

  function sendDownload(url, filename, mime, size, pair, quality, dur) {
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
      sendUrl = location.href;
      if (!filename) {
        const base = String(document.title || "media").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim().replace(/\.+$/, "") || "media";
        filename = base + mimeExt(mime);
      }
    }
    try {
      chrome.runtime.sendMessage({ type: "download", url: sendUrl, filename, ver: EXT_VER, pageUrl: location.href, referer: location.href, mime: mime || null, size: size || null, pairUrl: (pair && pair.url) || null, pairMime: (pair && pair.mime) || null, quality: quality || null, dur: dur || null }, (res) => {
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
  function fbInfo() {
    let html = "";
    try { html = document.documentElement.innerHTML.slice(0, 1500000); } catch (e) {}
    const urls = fbJsonUrls(html);
    // Duration: ms-style keys dominate FB JSON; fall back to plain seconds.
    let durMs = fbJsonNum(html, ["playable_duration_in_ms", "duration_in_ms", "length_in_ms"]);
    let durSec = durMs > 0 ? Math.round(durMs / 1000)
      : Math.round(fbJsonNum(html, ["videoDuration", "video_length", "duration", "length_in_seconds"]) || 0);
    if (!(durSec > 0 && durSec < 36000)) durSec = 0;
    // Dimensions: meta first (current reel), JSON fallbacks.
    let w = parseInt((document.querySelector('meta[property="og:video:width"]') || {}).content, 10) || 0;
    let h = parseInt((document.querySelector('meta[property="og:video:height"]') || {}).content, 10) || 0;
    if (!(w > 0)) w = Math.round(fbJsonNum(html, ["original_width", "video_width", "width"]) || 0);
    if (!(h > 0)) h = Math.round(fbJsonNum(html, ["original_height", "video_height", "height"]) || 0);
    // Title: caption first, then page title. Never "Facebook".
    let title = fbMeta("og:description") || fbMeta("og:title") || "";
    title = title.replace(/\s*\|\s*Facebook\s*$/i, "").replace(/\s+on Facebook.*$/i, "").trim();
    if (!title || /^facebook$/i.test(title)) {
      title = String(document.title || "").replace(/\s*\|\s*Facebook\s*$/i, "").trim() || "facebook_video";
    }
    if (/^facebook$/i.test(title)) title = "facebook_video";
    return { hd: urls.hd, sd: urls.sd, durSec, w, h, title: title.slice(0, 100), thumb: fbMeta("og:image") };
  }
  function fbQuality(w, h, isHd) {
    const q = Math.max(w || 0, h || 0);
    if (q >= 1080) return "1080p HD";
    if (q >= 720) return "720p HD";
    if (q >= 480) return "480p";
    if (q > 0) return `${q}p`;
    return isHd ? "HD" : "SD";
  }
  function fbFilename(title) {
    const s = String(title || "facebook_video").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim();
    const base = (s.replace(/\.+$/, "") || "facebook_video").slice(0, 80);
    return base.endsWith(".mp4") ? base : base + ".mp4";
  }
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

  // IDM-style: the button lives ON the video player (top-right corner),
  // falling back to a fixed viewport corner when no player is found.
  function placeInPlayer(btn, menu) {
    if (!btn) return false;
    const player = document.getElementById("movie_player");
    if (player) {
      btn.style.position = "absolute";
      btn.style.top = "12px";
      btn.style.right = "12px";
      btn.style.bottom = "auto";
      if (menu) {
        menu.style.position = "absolute";
        menu.style.top = "52px";
        menu.style.right = "12px";
        menu.style.bottom = "auto";
        if (menu.parentElement !== player) player.appendChild(menu);
      }
      if (btn.parentElement !== player) player.appendChild(btn);
      return true;
    }
    btn.style.position = "fixed";
    btn.style.right = "18px";
    btn.style.bottom = "18px";
    btn.style.top = "auto";
    if (menu) {
      menu.style.position = "fixed";
      menu.style.right = "18px";
      menu.style.bottom = "62px";
      menu.style.top = "auto";
      if (menu.parentElement !== document.documentElement) document.documentElement.appendChild(menu);
    }
    if (btn.parentElement !== document.documentElement) document.documentElement.appendChild(btn);
    return false;
  }

  // Presence pre-check: rows already in the app queue get a "✓ Queued"
  // tag and toast instead of re-sending into an "already in queue" error.
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
          if (!hit.has(p.st.url) || !p.btn.isConnected) continue;
          if (!p.btn.textContent.includes("✓ Queued")) {
            p.btn.textContent = `${p.btn.textContent} ✓ Queued`;
          }
          p.btn.onclick = (ev) => {
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

  // Brand mark for the floating "Download this video" buttons: the outlined
  // accent circle with a download arrow — the same mark the app draws in its
  // own title bar (and the icon shipped in assets/icon.png). Inline SVG keeps
  // it crisp at every UI scale and needs no web_accessible_resources entry.
  function brandIcon(px) {
    const NS = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(NS, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("width", String(px));
    svg.setAttribute("height", String(px));
    svg.setAttribute("aria-hidden", "true");
    svg.setAttribute("focusable", "false");
    svg.style.cssText = "flex:0 0 auto;display:block;pointer-events:none;";
    const add = (tag, attrs) => {
      const el = document.createElementNS(NS, tag);
      for (const k in attrs) el.setAttribute(k, attrs[k]);
      svg.appendChild(el);
    };
    add("circle", { cx: "12", cy: "12", r: "9", fill: "none", stroke: "#3b82f6", "stroke-width": "2" });
    add("path", {
      d: "M12 8v7M8.5 11.5 12 15l3.5-3.5",
      fill: "none",
      stroke: "#3b82f6",
      "stroke-width": "2",
      "stroke-linecap": "round",
      "stroke-linejoin": "round",
    });
    return svg;
  }

  // "[icon] Download this video": the label lives in its own span so a state
  // update that rewrites a button's text can never drop the mark.
  function labelButton(b, text, iconPx) {
    b.appendChild(brandIcon(iconPx));
    const span = document.createElement("span");
    span.setAttribute("data-dc", "label");
    span.textContent = text;
    b.appendChild(span);
    return span;
  }

  // In-page menu theming (dark + light like the "extension ui.png" mockup).
  // Real implementation lives in rank.js (node-tested); these wrappers
  // degrade to the historic dark look when rank.js failed to load.
  const DCUI = (typeof DCRank !== "undefined" && DCRank.paintDcMenu) ? DCRank : null;
  function dcThemeMode() { return DCUI ? DCUI.dcThemeMode() : "dark"; }
  function dcTokens() {
    if (DCUI) return DCUI.dcTokens();
    return { mode: "dark", bg: "rgba(16,17,23,.92)", border: "1px solid rgba(255,255,255,.1)", radius: "12px", shadow: "0 8px 28px rgba(0,0,0,.5)", text: "#e6e6f0", dim: "rgba(230,230,240,.6)", itemBg: "transparent", itemBorder: "1px solid transparent", itemHover: "#35363e", toggleBorder: "rgba(255,255,255,.25)", toggle: "☀" };
  }
  function paintDcMenu(menu) { if (DCUI) DCUI.paintDcMenu(menu); }
  function dcThemeToggle(menu) { return DCUI ? DCUI.dcThemeToggle(menu, document) : null; }

  function enrichSizes(pairs) {
    // pairs: [{btn, st, base}] — fill in "(12.4 MB)" once HEAD/Range answers.
    // Rows carrying a page-JSON size (_size) already print it; never
    // re-fetch those (googlevideo HEADs 403 and we'd just flicker).
    try {
      const need = pairs.filter((p) => !(p.st._size > 0));
      const urls = need.map((p) => p.st.url).filter(Boolean).slice(0, 12);
      if (!urls.length) return;
      chrome.runtime.sendMessage({ type: "getSizes", urls, referer: location.href }, (res) => {
        if (chrome.runtime.lastError) return;
        if (!alive()) return;
        const sizes = (res && res.sizes) || {};
        for (const p of need) {
          if (!p.btn.isConnected) continue;
          const n = sizes[p.st.url] || 0;
          const s = fmtSize(n);
          if (n > 0) p.st._size = n;
          if (s) p.btn.textContent = `${p.base} (${s})`;
        }
      });
    } catch (e) { /* sizes are best-effort */ }
  }

  // itag → quality/container for labeling sniffed live streams (page JSON
  // hides adaptive URLs until playback, but the player's traffic carries
  // itags — this rebuilds IDM-style "1080p · mp4" rows from them).
  const ITAG_Q = {17:"144p",18:"360p",22:"720p",36:"240p",43:"360p",133:"240p",134:"360p",135:"480p",136:"720p",137:"1080p",160:"144p",242:"240p",243:"360p",244:"480p",247:"720p",248:"1080p",271:"1440p",272:"2160p",278:"144p",302:"720p60",303:"1080p60",308:"1440p60",313:"2160p",315:"2160p60",394:"144p",395:"240p",396:"360p",397:"480p",398:"720p",399:"1080p"};
  const ITAG_A = {139:"low",140:"medium",141:"high",249:"low",250:"medium",251:"high",599:"low",600:"medium"};
  const PROG_ITAG = new Set([17,18,22,36,43]);
  function sniffLabel(m) {
    let itag = 0;
    try {
      const im = String(m.url || "").match(/[?&]itag=(\d+)/);
      itag = im ? parseInt(im[1], 10) : 0;
    } catch (e) {}
    const cont = contOf(m.mime);
    if (itag && ITAG_Q[itag]) {
      const av = PROG_ITAG.has(itag) ? "" : " (no audio)";
      return { text: `⬇ ${ITAG_Q[itag]} · ${cont}${av}`, itag };
    }
    if (itag && ITAG_A[itag]) {
      return { text: `⬇ ${ITAG_A[itag]} · ${cont} (audio)`, itag };
    }
    const tag = /video/i.test(m.mime || "") ? "video" : /audio/i.test(m.mime || "") ? "audio" : "stream";
    return { text: `⬇ playing ${tag} · ${cont}`, itag: 0 };
  }

  // IDM-style network catch: merge background-sniffed playing streams
  // (googlevideo range requests from the running player) into the YouTube
  // menu. Needed because YouTube often embeds ZERO direct URLs for music/
  // VEVO videos — the page JSON has formats but no `url` fields; only the
  // live network traffic carries the deciphered stream URLs.
  // Sniffed URLs carry the player's deciphered `n` parameter, so rows whose
  // itag matches a sniffed stream get their URL SWAPPED to the fresh one
  // (stale page URLs 403 in the app).
  function appendSniffed(menu, known, pairs) {
    try {
      chrome.runtime.sendMessage({ type: "getMedia", pageUrl: location.href }, (res) => {
        if (chrome.runtime.lastError) return;
        if (!alive() || !menu.isConnected) return;
        const items = ((res && res.items) || [])
          .filter((m) => m && /^https?:\/\//i.test(m.url) && !known.has(m.url));
        const byItag = {};
        for (const m of ((res && res.items) || [])) {
          try {
            const im = String(m.url || "").match(/[?&]itag=(\d+)/);
            if (im && !byItag[im[1]]) byItag[im[1]] = m;
          } catch (e) {}
        }
        // Swap stale page URLs for fresh player-deciphered ones (same itag).
        const swapped = [];
        for (const p of pairs || []) {
          const tag = p.st && p.st.itag ? String(p.st.itag) : null;
          const fresh = tag && byItag[tag];
          if (fresh && fresh.url !== p.st.url && p.btn.isConnected) {
            p.st.url = fresh.url;
            p.btn.title = fresh.url.slice(0, 120);
            known.add(fresh.url);
            swapped.push({ btn: p.btn, st: p.st, base: p.base });
          }
        }
        if (swapped.length) enrichSizes(swapped);
        if (!items.length) return;
        // Rebuild (never duplicate): late playback can add streams after the
        // first merge, so refresh the section instead of skipping it.
        menu.querySelectorAll("[data-dc-sniff]").forEach((el) => el.remove());
        const h = document.createElement("div");
        h.setAttribute("data-dc-sniff", "1");
        h.setAttribute("data-dc", "sec");
        h.textContent = "Detected playing streams";
        h.style.cssText = "padding:8px 8px 2px;font-size:11px;opacity:.6;text-transform:uppercase;letter-spacing:.06em;";
        menu.appendChild(h);
        const pairs = [];
        items.slice(0, 8).forEach((m) => {
          known.add(m.url);
          const lab = sniffLabel(m);
          const base = lab.text;
          const b = document.createElement("button");
          b.setAttribute("data-dc", "item");
          b.textContent = m.size > 0 ? `${base} (${fmtSize(m.size)})` : base;
          b.title = m.url.slice(0, 120);
          b.style.cssText = [
            "display:block", "width:100%", "text-align:left", "margin:2px 0",
            "padding:7px 10px", "border-radius:6px", "border:1px solid transparent",
            "background:transparent", "color:#e6e6f0", "cursor:pointer", "font:13px system-ui,sans-serif",
          ].join(";");
          b.onmouseenter = () => { b.style.background = (menu._dcT || dcTokens()).itemHover; };
          b.onmouseleave = () => { b.style.background = (menu._dcT || dcTokens()).itemBg; };
          b.onclick = (ev) => {
            ev.stopPropagation();
            menu.style.display = "none";
            sendDownload(m.url, undefined, m.mime, m.size || null);
          };
          menu.appendChild(b);
          pairs.push({ btn: b, st: { url: m.url }, base });
        });
        enrichSizes(pairs);
        try { paintDcMenu(menu); } catch (e) { /* theme is best-effort */ }
      });
    } catch (e) { /* ignore */ }
  }

  function ensureButton(info) {
    const oldBtn = document.getElementById("dc-yt-btn");
    const oldMenu = document.getElementById("dc-yt-menu");
    if (oldBtn && oldBtn.dataset.gen !== String(GEN)) {
      // Adopted DOM from an older live instance (e.g. re-injected after
      // an update): always rebuild so the handlers belong to this
      // (current) generation.
      oldBtn.remove();
      if (oldMenu) oldMenu.remove();
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
    // NOTE: oldBtn/oldMenu were already dropped above when they belonged
    // to an older generation, so reaching here means same-generation DOM.
    if (oldBtn && oldBtn.isConnected) {
      if (oldBtn.dataset.sig === sig) {
        // Same content: keep everything, just re-anchor (the player
        // element can be recreated by SPA navigation).
        placeInPlayer(oldBtn, oldMenu);
        return;
      }
      oldBtn.remove();
      if (oldMenu) oldMenu.remove();
    }
    const btn = document.createElement("button");
    btn.id = "dc-yt-btn";
    btn.dataset.vid = info.videoId;
    btn.dataset.gen = String(GEN);
    btn.dataset.sig = sigParts.join("|");
    labelButton(btn, "Download this video", 15);
    btn.title = "Send video to Download Craft";
    btn.style.cssText = [
      "display:inline-flex", "align-items:center", "gap:7px", "white-space:nowrap",
      "padding:6px 13px", "border-radius:999px", "border:1px solid rgba(255,255,255,.13)",
      "background:rgba(12,12,16,.68)", "backdrop-filter:blur(10px)",
      "color:#f2f2f7", "font:500 12px/1.4 system-ui,sans-serif", "letter-spacing:.02em",
      "cursor:pointer", "box-shadow:0 2px 12px rgba(0,0,0,.35)", "z-index:2147483647",
      "transition:background .15s ease,transform .15s ease",
    ].join(";");
    const menu = document.createElement("div");
    menu.id = "dc-yt-menu";
    menu.style.cssText = [
      "display:none", "min-width:230px", "max-width:320px",
      "background:rgba(16,17,23,.92)", "backdrop-filter:blur(12px)",
      "border:1px solid rgba(255,255,255,.1)", "border-radius:12px",
      "padding:6px", "box-shadow:0 8px 28px rgba(0,0,0,.5)",
      "font:13px system-ui,sans-serif", "color:#e6e6f0", "z-index:2147483647",
    ].join(";");
    const fname = safeFilename(info.title);
    menu.innerHTML = "";
    const head = document.createElement("div");
    head.style.cssText = "display:flex;align-items:center;gap:8px;padding:4px 8px 8px;font-weight:600;";
    if (info.thumb && /^https?:\/\//i.test(info.thumb)) {
      const im = document.createElement("img");
      im.src = info.thumb;
      im.loading = "lazy";
      im.style.cssText = "width:96px;height:54px;object-fit:cover;border-radius:6px;flex-shrink:0;";
      im.onerror = () => { im.style.display = "none"; };
      head.appendChild(im);
    }
    const ht = document.createElement("span");
    ht.setAttribute("data-dc", "head");
    ht.style.cssText = "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;";
    ht.textContent = info.title;
    ht.title = info.title;
    head.appendChild(ht);
    { const __t = dcThemeToggle(menu); if (__t) head.appendChild(__t); }
    menu.appendChild(head);
    const mkItem = (label, tip, onClick) => {
      const b = document.createElement("button");
      b.setAttribute("data-dc", "item");
      b.textContent = label;
      if (tip) b.title = tip;
      b.style.cssText = [
        "display:block", "width:100%", "text-align:left", "margin:2px 0",
        "padding:7px 10px", "border-radius:6px", "border:1px solid transparent",
        "background:transparent", "color:#e6e6f0", "cursor:pointer", "font:13px system-ui,sans-serif",
      ].join(";");
      b.onmouseenter = () => { b.style.background = (menu._dcT || dcTokens()).itemHover; };
      b.onmouseleave = () => { b.style.background = (menu._dcT || dcTokens()).itemBg; };
      b.onclick = (ev) => {
        ev.stopPropagation();
        menu.style.display = "none";
        onClick();
      };
      menu.appendChild(b);
      return b;
    };
    // The app resolves the video itself (player API) — always offered,
    // even when the page exposes no direct streams (ciphered videos).
    mkItem("🎬 Send video link to app", "Download Craft finds the best stream itself", () => {
      sendDownload(location.href, fname);
    });
    if (!info.streams.length && !(info.videoOnly || []).length && !(info.audio || []).length) {
      // VEVO-type pages embed zero direct URLs: tell the user the fix is
      // playback (the sniffer then merges live stream rows below).
      const h = document.createElement("div");
      h.setAttribute("data-dc", "hint");
      h.textContent = "No direct links yet — press play, then reopen this menu for fresh qualities.";
      h.style.cssText = "padding:8px;font-size:12px;opacity:.75;line-height:1.4;";
      menu.appendChild(h);
    }
    const sizePairs = [];
    // Video-only rows bundle the best audio track so the app can merge
    // them to a single file (IDM's "MKV file" behavior).
    const bestAudio = (info.audio || []).slice().sort((a, b) => (b.br || 0) - (a.br || 0))[0] || null;
    const pairFor = (st) => (st.kind === "v" && bestAudio && bestAudio.url !== st.url)
      ? { url: bestAudio.url, mime: bestAudio.mime }
      : null;
    const section = (head, list, nameFn) => {
      if (!list.length) return;
      const h = document.createElement("div");
      h.setAttribute("data-dc", "sec");
      h.textContent = head;
      h.style.cssText = "padding:8px 8px 2px;font-size:11px;opacity:.6;text-transform:uppercase;letter-spacing:.06em;";
      menu.appendChild(h);
      list.forEach((st) => {
        // IDM-style row: "<title>, MP4 file, quality 1080p HD 60 fps".
        const m = String(st.label || "").match(/^(\d+)p(\d+)?$/);
        const hh = m ? parseInt(m[1], 10) : 0;
        const fps = m && m[2] ? parseInt(m[2], 10) : 0;
        const hd = hh >= 720 ? " HD" : "";
        const fr = fps >= 50 ? ` ${fps} fps` : "";
        const qtext = hh > 0 ? `${hh}p${hd}${fr}` : st.label;
        const cont = (st.disp || contOf(st.mime) || "mp4").toUpperCase();
        const via = st.ciphered ? " (via app)" : "";
        const shortT = info.title.length > 42 ? info.title.slice(0, 42) + "…" : info.title;
        // Page-JSON sizes print immediately (no network round-trip), and
        // page-JSON durations ride along to the Add dialog.
        const presize = st._size > 0 ? ` (${fmtSize(st._size)})` : "";
        const predur = st._dur > 0 ? `, ${fmtDurSec(st._dur)}` : "";
        const base = `⬇ ${shortT}, ${cont} file, quality ${qtext}${via}${presize}${predur}`;
        const b = mkItem(
          base,
          st.ciphered ? "The app deciphers this quality itself" : (st.url || "").slice(0, 120),
          // Ciphered rows have no direct URL at build time: the sniffer
          // swaps in the player's fresh deciphered URL by itag once you
          // press play (see appendSniffed) — prefer it when present, else
          // send the page + quality hint for server-side resolve.
          // st._size rides along so the Add dialog shows it upfront.
          st.ciphered
            ? () => (st.url
              ? sendDownload(st.url, nameFn(st), st.mime, st._size || null, pairFor(st), null, st._dur || null)
              : sendDownload(location.href, nameFn(st), st.mime, st._size || null, pairFor(st), st.label, st._dur || null))
            : () => sendDownload(st.url, nameFn(st), st.mime, st._size || null, pairFor(st), null, st._dur || null),
        );
        sizePairs.push({ btn: b, st, base });
      });
    };
    const tagged = (base, st) => {
      const clean = base.replace(/\.mp4$/i, "");
      if (st.kind === "v") return `${clean} [${st.label}].mp4`;
      if (st.kind === "a") return `${clean} [audio]${mimeExt(st.mime)}`;
      return fname;
    };
    section("Video + audio", info.streams, () => fname);
    section("Video only (no audio)", info.videoOnly || [], (st) => tagged(fname, st));
    section("Audio only", info.audio || [], (st) => tagged(fname, st));
    // Full ladder fallback: when the page exposes almost nothing directly
    // (ciphered embeds, music videos), still offer every standard quality
    // like IDM does — the engine resolves each to the closest stream at or
    // below it, so a click always downloads something sensible. Sizes come
    // from the page JSON map (contentLength per quality), no network needed.
    {
      const haveQ = new Set([
        ...(info.streams || []).map((s) => s.label),
        ...((info.videoOnly || []).map((s) => s.label)),
      ]);
      const sizeByQ = {};
      const durByQ = {};
      for (const s of [...(info.streams || []), ...(info.videoOnly || [])]) {
        if (s && s.label && s._size > 0 && !sizeByQ[s.label]) sizeByQ[s.label] = s._size;
        if (s && s.label && s._dur > 0 && !durByQ[s.label]) durByQ[s.label] = s._dur;
      }
      // Plus every page-JSON size (formats with no URL at all still count).
      for (const [q, m] of Object.entries(info.sizes || {})) {
        if (!sizeByQ[q] && m && m.size > 0) sizeByQ[q] = m.size;
        if (!durByQ[q] && m && m.dur > 0) durByQ[q] = m.dur;
      }
      // Ladder asks plain "1080p" while the map may hold "1080p60".
      const sizeForQ = (q) => {
        if (sizeByQ[q]) return sizeByQ[q];
        const k = Object.keys(sizeByQ).find((k) => k.startsWith(q));
        return k ? sizeByQ[k] : 0;
      };
      const durForQ = (q) => {
        if (durByQ[q]) return durByQ[q];
        const k = Object.keys(durByQ).find((k) => k.startsWith(q));
        return k ? durByQ[k] : 0;
      };
      const ladder = ["1080p", "720p", "480p", "360p", "240p", "144p"].filter((q) => !haveQ.has(q) && ![...haveQ].some((h) => String(h).startsWith(q)));
      if (ladder.length && (info.streams || []).length + (info.videoOnly || []).length < 6) {
        const h = document.createElement("div");
        h.setAttribute("data-dc", "sec");
        h.textContent = "More qualities (via app)";
        h.style.cssText = "padding:8px 8px 2px;font-size:11px;opacity:.6;text-transform:uppercase;letter-spacing:.06em;";
        menu.appendChild(h);
        const shortT = info.title.length > 42 ? info.title.slice(0, 42) + "…" : info.title;
        ladder.forEach((q) => {
          const hh = parseInt(q, 10);
          const sz = sizeForQ(q);
          const dz = durForQ(q);
          const base = `⬇ ${shortT}, MP4 file, quality ${q}${hh >= 720 ? " HD" : ""} (via app)${sz > 0 ? ` (${fmtSize(sz)})` : ""}${dz > 0 ? `, ${fmtDurSec(dz)}` : ""}`;
          mkItem(base, "The app resolves this quality itself", () => {
            sendDownload(location.href, fname, "video/mp4", sz || null, pairFor({ kind: "av" }), q, dz || null);
          });
          // Deliberately NOT in sizePairs: sizing a watch page would print
          // the HTML byte count as a bogus "(2.1 MB)" on the row.
        });
      }
    }
    enrichSizes(sizePairs);
    markQueuedRows(sizePairs);
    appendSniffed(menu, new Set([
      ...info.streams.map((s) => s.url),
      ...(info.videoOnly || []).map((s) => s.url),
      ...(info.audio || []).map((s) => s.url),
    ]), sizePairs);
    btn.onclick = (ev) => {
      ev.stopPropagation();
      const show = menu.style.display === "none";
      if (show) paintDcMenu(menu);
      menu.style.display = show ? "block" : "none";
    };
    btn.onmouseenter = () => { btn.style.background = "rgba(40,41,55,.85)"; };
    btn.onmouseleave = () => { btn.style.background = "rgba(12,12,16,.68)"; };
    // One delegated closer per generation (a per-rebuild listener leaked
    // and stale closures fought the new menu).
    if (!window.__dcDocClick) {
      window.__dcDocClick = true;
      document.addEventListener("click", () => {
        document.querySelectorAll("#dc-yt-menu").forEach((m) => { m.style.display = "none"; });
      });
    }
    placeInPlayer(btn, menu);
    paintDcMenu(menu);
  }

  function refreshYouTube() {
    const id = ytVideoId();
    const oldBtn = document.getElementById("dc-yt-btn");
    const oldMenu = document.getElementById("dc-yt-menu");
    if (!id) {
      if (oldBtn) oldBtn.remove();
      if (oldMenu) oldMenu.remove();
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
    const info = ytStreams() || { videoId: id, title: document.title.replace(/ - YouTube$/, ""), thumb: `https://i.ytimg.com/vi/${id}/hqdefault.jpg`, streams: [], videoOnly: [], audio: [] };
    try {
      chrome.storage.local.set({
        downloadCraftVideos: [{
          videoId: info.videoId, title: info.title, thumb: info.thumb || "",
          streams: info.streams, videoOnly: info.videoOnly || [], audio: info.audio || [],
          sizes: info.sizes || {},
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
    // YouTube watch/Shorts pages already have the quality menu button —
    // a second generic pill on the same player is the "double button" bug.
    try { if (ytVideoId()) return; } catch (e) { /* ignore */ }
    try {
      // Drop pills left by an older injected generation (their handlers are
      // dead after an extension reload) so only this generation's pills show.
      document.querySelectorAll("button[data-dc-pill]").forEach((el) => {
        if (el.dataset.gen !== String(GEN)) el.remove();
      });
      const vids = [...document.querySelectorAll("video")].slice(0, 10);
      for (const v of vids) {
        if (v.__dcBtn) {
          if (!v.__dcBtn.isConnected) v.__dcBtn = null;
          else continue;
        }
        // Skip anything inside YouTube's player (covered by its own button).
        try { if (v.closest && v.closest("#movie_player")) continue; } catch (e) {}
        // Skip tiny preview thumbnails.
        const r = v.getBoundingClientRect ? v.getBoundingClientRect() : null;
        if (r && (r.width < 120 || r.height < 70)) continue;
        const pill = document.createElement("button");
        labelButton(pill, "Download this video", 13);
        pill.title = "Send to Download Craft";
        pill.setAttribute("data-dc-pill", "1");
        pill.dataset.gen = String(GEN);
        pill.style.cssText = [
          "display:inline-flex", "align-items:center", "gap:6px", "white-space:nowrap",
          "position:absolute", "top:8px", "right:8px", "z-index:2147483646",
          "padding:4px 10px", "border-radius:999px",
          "border:1px solid rgba(255,255,255,.15)",
          "background:rgba(10,10,14,.78)", "color:#fff",
          "font:500 11px/1.4 system-ui,sans-serif", "cursor:pointer",
          "box-shadow:0 2px 10px rgba(0,0,0,.4)",
        ].join(";");
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
            const base = (document.title || "video").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").slice(0, 80);
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
                  const base = (document.title || "video").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").slice(0, 80);
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
          const cs = getComputedStyle(v.parentElement || v);
          if (v.parentElement && cs.position === "static") {
            v.parentElement.style.position = "relative";
          }
          (v.parentElement || document.documentElement).appendChild(pill);
          // Anchor inside offset parent.
          if (v.parentElement) {
            pill.style.position = "absolute";
          }
          v.__dcBtn = pill;
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

  // Facebook quality menu (IDM-style): title header + HD/SD rows with
  // duration and measured sizes, so every reel offers a choice instead of
  // one mystery "Facebook.mp4".
  function openFbMenu(pill, rows, title, thumb) {
    try { document.querySelectorAll("#dc-fb-menu").forEach((m) => m.remove()); } catch (e) {}
    const menu = document.createElement("div");
    menu.id = "dc-fb-menu";
    menu.style.cssText = [
      "position:absolute", "top:40px", "right:8px", "z-index:2147483647",
      "min-width:250px", "max-width:340px",
      "background:rgba(16,17,23,.94)", "backdrop-filter:blur(12px)",
      "border:1px solid rgba(255,255,255,.1)", "border-radius:12px",
      "padding:6px", "box-shadow:0 8px 28px rgba(0,0,0,.5)",
      "font:13px system-ui,sans-serif", "color:#e6e6f0",
    ].join(";");
    const head = document.createElement("div");
    head.style.cssText = "display:flex;align-items:center;gap:8px;padding:4px 8px 8px;font-weight:600;";
    const headThumb = thumb || ((rows && rows[0] && rows[0].thumb) || "");
    if (headThumb && /^https?:\/\//i.test(headThumb)) {
      const im = document.createElement("img");
      im.src = headThumb;
      im.loading = "lazy";
      im.style.cssText = "width:96px;height:54px;object-fit:cover;border-radius:6px;flex-shrink:0;";
      im.onerror = () => { im.style.display = "none"; };
      head.appendChild(im);
    }
    const ht = document.createElement("span");
    ht.setAttribute("data-dc", "head");
    ht.style.cssText = "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;";
    ht.textContent = title;
    ht.title = title;
    head.appendChild(ht);
    { const __t = dcThemeToggle(menu); if (__t) head.appendChild(__t); }
    menu.appendChild(head);
    const sizePairs = [];
    rows.forEach((st) => {
      const b = document.createElement("button");
      b.setAttribute("data-dc", "item");
      b.textContent = st.label;
      b.title = (st.url || "").slice(0, 120);
      b.style.cssText = [
        "display:block", "width:100%", "text-align:left", "margin:2px 0",
        "padding:7px 10px", "border-radius:6px", "border:1px solid transparent",
        "background:transparent", "color:#e6e6f0", "cursor:pointer", "font:13px system-ui,sans-serif",
      ].join(";");
      b.onmouseenter = () => { b.style.background = (menu._dcT || dcTokens()).itemHover; };
      b.onmouseleave = () => { b.style.background = (menu._dcT || dcTokens()).itemBg; };
      b.onclick = (ev) => {
        ev.stopPropagation();
        menu.remove();
        sendDownload(st.url, st.fname, "video/mp4", st._size || null, null, null, st.dur || null);
      };
      menu.appendChild(b);
      sizePairs.push({ btn: b, st, base: st.label });
    });
    enrichSizes(sizePairs);
    try {
      (pill.parentElement || document.documentElement).appendChild(menu);
    } catch (e) { /* ignore */ }
    paintDcMenu(menu);
    setTimeout(() => {
      const closer = (ev) => {
        if (menu.isConnected && !menu.contains(ev.target)) {
          menu.remove();
          document.removeEventListener("click", closer);
        }
      };
      document.addEventListener("click", closer);
    }, 0);
  }

  function fbMenuRows(info, items) {
    // Dedupe page-JSON URLs against sniffed traffic, then rank HD first.
    const seen = new Set();
    const rows = [];
    const fname = fbFilename(info.title);
    const dur = info.durSec > 0 ? `, ${info.durSec} sec` : "";
    const push = (url, quality) => {
      if (!url || !/^https?:\/\//i.test(url) || seen.has(url)) return;
      seen.add(url);
      rows.push({ url, fname, label: `⬇ ${info.title.slice(0, 40)}, MP4 file${dur}, quality ${quality}`, _size: 0, dur: info.durSec || null, thumb: info.thumb || "" });
    };
    if (info.hd && info.hd !== info.sd) push(info.hd, fbQuality(info.w, info.h, true));
    if (info.sd) push(info.sd, fbQuality(info.w, info.h, info.hd === info.sd));
    // Anything else the player actually fetched (fresh session URLs).
    // P4: when a playlist is on the tab, loose segments are its children
    // (ad slivers included) — offer the playlist, never the parts.
    const R = (typeof DCRank !== "undefined") ? DCRank : null;
    const hasPlaylist = (items || []).some((m) => m && (
      (R ? R.kindOf(m) === "playlist" : /m3u8|mpd|mpegurl|dash/i.test(String(m.url || "")))
    ));
    const isSeg = (u) => R ? R.kindOf({ url: u }) === "segment" : /(\.ts|\.m4s|\.cmfv|\.cmfa)(\?|#|$)/i.test(String(u).split("?")[0]);
    const rest = (items || []).filter((m) => m && /^https?:\/\//i.test(m.url) && !seen.has(m.url)
      && !/m3u8|mpd|mpegurl|dash/i.test(m.url)
      && !(hasPlaylist && isSeg(m.url)));
    rest.slice(0, 6).forEach((m) => {
      seen.add(m.url);
      rows.push({ url: m.url, fname, label: `⬇ ${info.title.slice(0, 40)}, MP4 file${dur}, quality ${fbQuality(0, 0, false)}`, _size: m.size || 0, thumb: m.thumb || info.thumb || "" });
    });
    return rows;
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
        const rows = fbMenuRows(info, items);
        if (!rows.length) {
          sendDownload(location.href, fbFilename(info.title), null, null, null, null, info.durSec || null);
        } else if (rows.length === 1) {
          sendDownload(rows[0].url, rows[0].fname, "video/mp4", rows[0]._size || null, null, null, rows[0].dur || info.durSec || null);
        } else {
          openFbMenu(pill, rows, info.title, info.thumb);
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
        try {
          chrome.storage.local.get(["downloadCraftLinks"], (cur) => {
            const have = new Set(((cur && cur.downloadCraftLinks) || []).map((l) => l.url));
            let pageThumb = "";
            try {
              const pv = document.querySelector("video[poster]");
              const pp = pv && pv.getAttribute("poster");
              if (/^https?:\/\//i.test(pp || "")) pageThumb = pp;
            } catch (e) { /* ignore */ }
            const add = items
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
      if (!msg || msg.type !== "peekState") return false;
      if (!alive()) return false;
      (async () => {
        try {
          collectLinks();
          refreshYouTube();
        } catch (e) { /* best effort */ }
        let net = [];
        try {
          const r = await chrome.runtime.sendMessage({ type: "getMedia", pageUrl: location.href });
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

  // Initial pass + SPA navigation (YouTube AND Facebook rarely reload).
  // Facebook reels swap /reel/<id> via history API: no reload, same video
  // element, new blob: src. Patch pushState/replaceState + popstate so the
  // pill + sniffed candidates refresh instantly instead of on the 2 s poll.
  // The player response can arrive after the content script runs, so
  // retry a few times before giving up.
  collectLinks();
  ensureUniversalButtons();
  pullNetworkMedia();
  function spaRefresh() {
    if (__dcDead || !alive()) return;
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
  const watcher = setInterval(() => {
    if (__dcDead || !alive()) { clearInterval(watcher); return; } // superseded: stop
    if (location.href !== lastUrl || document.title !== lastTitle) {
      lastUrl = location.href;
      lastTitle = document.title;
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
