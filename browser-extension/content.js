// Content script: collects downloadable links AND page media
// (<video>/<audio>/<source>) so the popup can one-click them.
// On YouTube watch pages it additionally extracts direct progressive
// streams from the page player (real browser session: real cookies/TLS,
// so no bot-wall) and offers an in-page download button with qualities.
(() => {
  // Must match manifest version + background.js EXT_VER. The background
  // rejects messages from any other version with a "reload this tab"
  // error, so a stale script can never fail mysteriously.
  const EXT_VER = "0.5.0";
  // Generation guard: when the worker re-injects this script (after an
  // update), older live instances must go quiet instead of fighting over
  // the button. The newest instance always wins.
  const GEN = (window.__dcGen = (window.__dcGen || 0) + 1);
  const alive = () => window.__dcGen === GEN;
  const DL = /\.(zip|rar|7z|exe|msi|pdf|mp4|mkv|webm|mov|mp3|flac|wav|ogg|m4a|iso|tar|gz|dmg)$/i;

  function collectLinks() {
    try {
      const links = [...document.querySelectorAll("a[href]")]
        .map((a) => ({ url: a.href, text: (a.textContent || "").trim().slice(0, 80) }))
        .filter((l) => /^https?:\/\//i.test(l.url) && DL.test(l.url.split("?")[0]))
        .slice(0, 30);
      const media = [];
      document.querySelectorAll("video[src], audio[src]").forEach((el) => {
        if (/^https?:\/\//i.test(el.currentSrc || el.src)) {
          media.push({ url: el.currentSrc || el.src, text: `Page media (${el.tagName.toLowerCase()})` });
        }
      });
      document.querySelectorAll("video source[src], audio source[src]").forEach((el) => {
        if (/^https?:\/\//i.test(el.src)) {
          media.push({ url: el.src, text: "Page media (source)" });
        }
      });
      chrome.storage.local.set({
        downloadCraftLinks: [...media, ...links].slice(0, 50),
        downloadCraftPage: location.href,
      });
    } catch (e) {
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

  function ytStreams() {
    const id = ytVideoId();
    if (!id) return null;
    // Strict id match: stale SPA script tags must never win.
    const prs = playerResponses();
    const pr = prs.find((p) => p && p.videoDetails && p.videoDetails.videoId === id) || null;
    const docTitle = document.title.replace(/ - YouTube$/, "");
    if (!pr || !pr.streamingData) {
      return { videoId: id, title: docTitle || "youtube_video", streams: [], videoOnly: [], audio: [] };
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
    const qlabel = (f) => {
      // qualityLabel is "1080p" but often lacks fps; append it like IDM
      // ("1080p60") when the stream is high-frame-rate.
      const q = f.qualityLabel || f.quality || "unknown";
      if (/^\d+p$/.test(q) && (f.fps || 0) >= 50) return `${q}${f.fps}`;
      return q;
    };
    // Progressive = video+audio in one file (ready to play).
    const streams = direct(pr.streamingData.formats).map((f) => {
      const mime = String(f.mimeType || "").split(";")[0];
      return { url: f.url, label: qlabel(f), mime, disp: contOf(mime), kind: "av", itag: f.itag || 0 };
    });
    // Adaptive video-only incl. WebM VP9/AV1 — this is where 1080p+/4K live
    // (IDM's "2160p 4K / 1440p HD" rows). Honestly video-only: the app labels
    // them "(no audio)" until the P1.3 ffmpeg merge lands.
    const seenQ = new Set();
    const videoOnly = direct(pr.streamingData.adaptiveFormats)
      .filter((f) => String(f.mimeType || "").startsWith("video/"))
      .map((f) => {
        const mime = String(f.mimeType || "").split(";")[0];
        return { url: f.url, label: qlabel(f), mime, disp: contOf(mime), kind: "v", itag: f.itag || 0 };
      })
      .filter((s) => {
        const k = `${s.label}|${s.disp}`;
        if (seenQ.has(k)) return false;
        seenQ.add(k);
        return true;
      })
      .slice(0, 16);
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
        };
      })
      .slice(0, 6);
    return { videoId: id, title, streams, videoOnly, audio };
  }

  function sendDownload(url, filename, mime, size, pair) {
    // A tab running a stale script (extension reloaded without a tab
    // reload) has no extension APIs left — fail loudly, not mysteriously.
    try {
      if (!chrome || !chrome.runtime || !chrome.runtime.sendMessage) {
        throw new Error("page runs an older script — reload this tab (Ctrl+R), then click again");
      }
    } catch (e) {
      toast("Download Craft: " + (e && e.message ? e.message : e));
      return;
    }
    // blob: / mediasource streams cannot be fetched by the desktop app —
    // fall back to the page URL so the engine can resolve it server-side.
    let sendUrl = url;
    if (/^blob:/i.test(url)) {
      sendUrl = location.href;
      if (!filename) filename = safeFilename(document.title) ;
    }
    try {
      chrome.runtime.sendMessage({ type: "download", url: sendUrl, filename, ver: EXT_VER, pageUrl: location.href, referer: location.href, mime: mime || null, size: size || null, pairUrl: (pair && pair.url) || null, pairMime: (pair && pair.mime) || null }, (res) => {
      if (chrome.runtime.lastError) {
        toast("Download Craft: " + chrome.runtime.lastError.message);
        return;
      }
      if (!res || !res.ok) {
        toast("Download Craft: " + ((res && res.error) || "app not reachable"));
      }
      });
    } catch (e) {
      toast("Download Craft: " + (e && e.message ? e.message : e));
    }
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

  function fmtSize(n) {
    if (!n || n <= 0) return "";
    const u = ["B", "KB", "MB", "GB"];
    let v = n, i = 0;
    while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
    return i === 0 ? `${n} ${u[i]}` : `${v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
  }

  function enrichSizes(pairs) {
    // pairs: [{btn, st, base}] — fill in "(12.4 MB)" once HEAD/Range answers.
    try {
      const urls = pairs.map((p) => p.st.url).filter(Boolean).slice(0, 12);
      if (!urls.length) return;
      chrome.runtime.sendMessage({ type: "getSizes", urls }, (res) => {
        if (!alive()) return;
        const sizes = (res && res.sizes) || {};
        for (const p of pairs) {
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
      chrome.runtime.sendMessage({ type: "getMedia" }, (res) => {
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
        if (menu.querySelector("[data-dc-sniff]")) return; // already merged
        const h = document.createElement("div");
        h.setAttribute("data-dc-sniff", "1");
        h.textContent = "Detected playing streams";
        h.style.cssText = "padding:8px 8px 2px;font-size:11px;opacity:.6;text-transform:uppercase;letter-spacing:.06em;";
        menu.appendChild(h);
        const pairs = [];
        items.slice(0, 8).forEach((m) => {
          known.add(m.url);
          const lab = sniffLabel(m);
          const base = lab.text;
          const b = document.createElement("button");
          b.textContent = m.size > 0 ? `${base} (${fmtSize(m.size)})` : base;
          b.title = m.url.slice(0, 120);
          b.style.cssText = [
            "display:block", "width:100%", "text-align:left", "margin:2px 0",
            "padding:7px 10px", "border-radius:6px", "border:1px solid transparent",
            "background:transparent", "color:#e6e6f0", "cursor:pointer", "font:13px system-ui,sans-serif",
          ].join(";");
          b.onmouseenter = () => { b.style.background = "#35363e"; };
          b.onmouseleave = () => { b.style.background = "transparent"; };
          b.onclick = (ev) => {
            ev.stopPropagation();
            menu.style.display = "none";
            sendDownload(m.url, undefined, m.mime, m.size || null);
          };
          menu.appendChild(b);
          pairs.push({ btn: b, st: { url: m.url }, base });
        });
        enrichSizes(pairs);
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
    // id. Expiries are included so refreshed (re-issued) URLs rebuild the
    // menu instead of serving the stale 403-prone rows forever.
    const expSig = (l) => (l || []).map((s) => {
      try {
        const m = String(s.url || "").match(/[?&]expire=(\d+)/);
        return m ? m[1] : "0";
      } catch (e) { return "0"; }
    }).join(",");
    const sig = [
      info.videoId, info.title,
      (info.streams || []).length,
      (info.videoOnly || []).length,
      (info.audio || []).length,
      expSig(info.streams), expSig(info.videoOnly), expSig(info.audio),
    ].join("|");
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
    btn.dataset.sig = [
      info.videoId, info.title,
      (info.streams || []).length,
      (info.videoOnly || []).length,
      (info.audio || []).length,
      expSig(info.streams), expSig(info.videoOnly), expSig(info.audio),
    ].join("|");
    btn.textContent = "⬇ Download this video";
    btn.title = "Send video to Download Craft";
    btn.style.cssText = [
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
    head.style.cssText = "padding:4px 8px 8px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;";
    head.textContent = info.title;
    head.title = info.title;
    menu.appendChild(head);
    const mkItem = (label, tip, onClick) => {
      const b = document.createElement("button");
      b.textContent = label;
      if (tip) b.title = tip;
      b.style.cssText = [
        "display:block", "width:100%", "text-align:left", "margin:2px 0",
        "padding:7px 10px", "border-radius:6px", "border:1px solid transparent",
        "background:transparent", "color:#e6e6f0", "cursor:pointer", "font:13px system-ui,sans-serif",
      ].join(";");
      b.onmouseenter = () => { b.style.background = "#35363e"; };
      b.onmouseleave = () => { b.style.background = "transparent"; };
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
      h.textContent = head;
      h.style.cssText = "padding:8px 8px 2px;font-size:11px;opacity:.6;text-transform:uppercase;letter-spacing:.06em;";
      menu.appendChild(h);
      list.forEach((st) => {
        const base = `⬇ ${st.label} · ${st.disp || contOf(st.mime)}`;
        const b = mkItem(
          base,
          st.url.slice(0, 120),
          () => sendDownload(st.url, nameFn(st), st.mime, st._size || null, pairFor(st)),
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
    enrichSizes(sizePairs);
    appendSniffed(menu, new Set([
      ...info.streams.map((s) => s.url),
      ...(info.videoOnly || []).map((s) => s.url),
      ...(info.audio || []).map((s) => s.url),
    ]), sizePairs);
    btn.onclick = (ev) => {
      ev.stopPropagation();
      menu.style.display = menu.style.display === "none" ? "block" : "none";
    };
    btn.onmouseenter = () => { btn.style.background = "rgba(40,41,55,.85)"; };
    btn.onmouseleave = () => { btn.style.background = "rgba(12,12,16,.68)"; };
    document.addEventListener("click", () => { menu.style.display = "none"; });
    placeInPlayer(btn, menu);
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
    const info = ytStreams() || { videoId: id, title: document.title.replace(/ - YouTube$/, ""), streams: [], videoOnly: [], audio: [] };
    try {
      chrome.storage.local.set({
        downloadCraftVideos: [{
          videoId: info.videoId, title: info.title,
          streams: info.streams, videoOnly: info.videoOnly || [], audio: info.audio || [],
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
        pill.textContent = "⬇ Download this video";
        pill.title = "Send to Download Craft";
        pill.setAttribute("data-dc-pill", "1");
        pill.dataset.gen = String(GEN);
        pill.style.cssText = [
          "position:absolute", "top:8px", "right:8px", "z-index:2147483646",
          "padding:4px 10px", "border-radius:999px",
          "border:1px solid rgba(255,255,255,.15)",
          "background:rgba(10,10,14,.78)", "color:#fff",
          "font:500 11px/1.4 system-ui,sans-serif", "cursor:pointer",
          "box-shadow:0 2px 10px rgba(0,0,0,.4)",
        ].join(";");
        pill.onclick = (ev) => {
          ev.stopPropagation(); ev.preventDefault();
          const direct = mediaUrlOf(v);
          if (direct) {
            const base = (document.title || "video").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").slice(0, 80);
            const ext = /\.mp3|\.m4a|\.wav|\.ogg/i.test(direct) ? "" : ".mp4";
            sendDownload(direct, base + ext, v.getAttribute && v.getAttribute("type"));
          } else {
            // Ask background for sniffed media URLs for this tab first.
            try {
              chrome.runtime.sendMessage({ type: "getMedia" }, (res) => {
                const items = (res && res.items) || [];
                if (items.length) {
                  const best = items[items.length - 1];
                  sendDownload(best.url, undefined, best.mime);
                } else {
                  sendDownload(location.href, undefined, null);
                }
              });
            } catch (e) {
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

  // Merge background-sniffed network media into popup storage so qualities
  // show even for blob: players.
  function pullNetworkMedia() {
    try {
      chrome.runtime.sendMessage({ type: "getMedia" }, (res) => {
        if (!alive()) return;
        const items = (res && res.items) || [];
        if (!items.length) return;
        try {
          chrome.storage.local.get(["downloadCraftLinks"], (cur) => {
            const have = new Set(((cur && cur.downloadCraftLinks) || []).map((l) => l.url));
            const add = items
              .filter((m) => !have.has(m.url))
              .map((m) => ({ url: m.url, text: `Stream (${m.mime || "media"})` }));
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

  // Initial pass + SPA navigation (YouTube rarely reloads the page).
  // The player response can arrive after the content script runs, so
  // retry a few times before giving up.
  collectLinks();
  ensureUniversalButtons();
  pullNetworkMedia();
  try {
    const mo = new MutationObserver(() => { collectLinks(); ensureUniversalButtons(); });
    mo.observe(document.documentElement, { childList: true, subtree: true });
  } catch (e) { /* observer optional */ }
  setInterval(() => { if (alive()) pullNetworkMedia(); }, 8000);
  if (!refreshYouTube()) {
    let tries = 0;
    const timer = setInterval(() => {
      tries++;
      if (refreshYouTube() || tries >= 6) clearInterval(timer);
    }, 2000);
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
    if (!alive()) { clearInterval(watcher); return; } // superseded: stop
    if (location.href !== lastUrl || document.title !== lastTitle) {
      lastUrl = location.href;
      lastTitle = document.title;
      collectLinks();
      ensureUniversalButtons();
      refreshYouTube();
    }
  }, 2000);
})();
