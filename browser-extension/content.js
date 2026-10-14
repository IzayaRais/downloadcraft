// Content script: collects downloadable links AND page media
// (<video>/<audio>/<source>) so the popup can one-click them.
// On YouTube watch pages it additionally extracts direct progressive
// streams from the page player (real browser session: real cookies/TLS,
// so no bot-wall) and offers an in-page download button with qualities.
(() => {
  // Must match manifest version + background.js EXT_VER. The background
  // rejects messages from any other version with a "reload this tab"
  // error, so a stale script can never fail mysteriously.
  const EXT_VER = "0.3.5";
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
    const direct = (arr) => (Array.isArray(arr) ? arr : [])
      .filter((f) => f && typeof f.url === "string" && f.url.startsWith("http"));
    // Progressive = video+audio in one file (ready to play).
    const streams = direct(pr.streamingData.formats).map((f) => ({
      url: f.url,
      label: f.qualityLabel || f.quality || "unknown",
      mime: String(f.mimeType || "").split(";")[0],
      kind: "av",
    }));
    // Adaptive MP4 video-only (1080p/4K…) + separate audio tracks.
    const videoOnly = direct(pr.streamingData.adaptiveFormats)
      .filter((f) => String(f.mimeType || "").startsWith("video/mp4"))
      .map((f) => ({
        url: f.url,
        label: f.qualityLabel || f.quality || "unknown",
        mime: "video/mp4",
        kind: "v",
      }))
      .slice(0, 12);
    const audio = direct(pr.streamingData.adaptiveFormats)
      .filter((f) => String(f.mimeType || "").startsWith("audio/"))
      .map((f) => ({
        url: f.url,
        label: (f.audioQuality || "").replace("AUDIO_QUALITY_", "") || (f.bitrate ? Math.round(f.bitrate / 1000) + "k" : "audio"),
        mime: String(f.mimeType || "").split(";")[0],
        kind: "a",
      }))
      .slice(0, 4);
    return { videoId: id, title, streams, videoOnly, audio };
  }

  function sendDownload(url, filename) {
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
    try {
      chrome.runtime.sendMessage({ type: "download", url, filename, ver: EXT_VER }, (res) => {
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
    // id. A same-video refresh can carry a corrected title (transitional
    // document.title) or newly arrived direct streams; without this the
    // menu keeps showing whatever the first pass found.
    const sig = [
      info.videoId, info.title,
      (info.streams || []).length,
      (info.videoOnly || []).length,
      (info.audio || []).length,
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
    ].join("|");
    btn.textContent = "⬇ Download Craft";
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
    };
    // The app resolves the video itself (player API) — always offered,
    // even when the page exposes no direct streams (ciphered videos).
    mkItem("🎬 Send video link to app", "Download Craft finds the best stream itself", () => {
      sendDownload(location.href, fname);
    });
    const section = (head, list, nameFn) => {
      if (!list.length) return;
      const h = document.createElement("div");
      h.textContent = head;
      h.style.cssText = "padding:8px 8px 2px;font-size:11px;opacity:.6;text-transform:uppercase;letter-spacing:.06em;";
      menu.appendChild(h);
      list.forEach((st) => {
        mkItem(
          `⬇ ${st.label} · ${st.mime.replace("video/", "")}`,
          st.url.slice(0, 120),
          () => sendDownload(st.url, nameFn(st)),
        );
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

  // Initial pass + SPA navigation (YouTube rarely reloads the page).
  // The player response can arrive after the content script runs, so
  // retry a few times before giving up.
  collectLinks();
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
      refreshYouTube();
    }
  }, 2000);
})();
