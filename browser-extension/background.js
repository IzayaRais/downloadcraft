// Background worker: every browser download is captured
// automatically, cancelled in-browser, and handed to the native host.
// No clicks, no pasting — zero config.

const HOST = "com.download_craft";
const GRACE_MS = 15_000; // ignore our own fallback re-downloads
const seen = new Map(); // url -> timestamp (loop guard)
let hostOk = true;
let lastHostError = "";
// Must match manifest version + content.js EXT_VER. Tabs running an older
// content script are rejected with a "reload the tab" error instead of
// failing mysteriously (their extension APIs are gone after a reload).
const EXT_VER = "0.4.9";

function markSeen(url) {
  seen.set(url, Date.now());
  if (seen.size > 200) {
    const cut = Date.now() - GRACE_MS;
    for (const [u, t] of seen) if (t < cut) seen.delete(u);
  }
}
function freshSeen(url) {
  const t = seen.get(url);
  return t !== undefined && Date.now() - t < GRACE_MS;
}

async function settings() {
  const s = await chrome.storage.local.get({
    autoCapture: true,
    minSizeKB: 100,
    blockHosts: "",
  });
  return s;
}

function hostBlocked(url, blockHosts) {
  try {
    const h = new URL(url).hostname.toLowerCase();
    const blocked = String(blockHosts || "")
      .split(/[\s,;]+/)
      .map((x) => x.trim().toLowerCase())
      .filter(Boolean);
    return blocked.some((b) => h === b || h.endsWith("." + b));
  } catch (e) {
    return false;
  }
}

// Curated cookies for ONE url only (same-host). Never logged; capped.
async function cookiesFor(url) {
  try {
    if (!chrome.cookies || !chrome.cookies.getAll) return null;
    const list = await chrome.cookies.getAll({ url });
    if (!list || !list.length) return null;
    const s = list
      .filter((c) => c && c.name)
      .slice(0, 40)
      .map((c) => `${c.name}=${c.value || ""}`)
      .join("; ");
    return s.length > 6000 ? s.slice(0, 6000) : s;
  } catch (e) {
    return null;
  }
}

async function sendToHost(url, filename, extra) {
  try {
    let cookie = (extra && extra.cookie) || null;
    if (!cookie && /^https?:\/\//i.test(url)) {
      cookie = await cookiesFor(url); // best-effort, same-host only
    }
    const res = await chrome.runtime.sendNativeMessage(HOST, {
      type: "download",
      url,
      filename: filename || null,
      pageUrl: (extra && extra.pageUrl) || null,
      referer: (extra && extra.referer) || (extra && extra.pageUrl) || null,
      cookie,
      mime: (extra && extra.mime) || null,
      size: (extra && extra.size) || null,
    });
    return !!res && (res.type === "ok" || res.type === "pong");
  } catch (e) {
    lastHostError = (e && e.message) || String(e);
    return false;
  }
}

// ---- IDM-style network sniffer ----
// Watches response headers for media payloads (direct mp4/webm/mkv, audio,
// HLS playlists, DASH manifests). Candidates are stored per-tab so the
// popup + content overlay can offer them even when the DOM only holds a
// blob: URL (un-downloadable directly). Clean-room behavior, no IDM code.
const tabMedia = new Map(); // tabId -> [{url, mime, size, pageUrl, seenAt}]
const sizeCache = new Map(); // url -> bytes (quality-menu sizes)
const MEDIA_MIME = /^(video\/|audio\/|application\/(x-mpegurl|vnd\.apple\.mpegurl|dash\+xml|mp4|octet-stream))/i;
const MEDIA_URL = /\.(mp4|webm|mkv|mov|avi|m4v|mp3|m4a|aac|flac|ogg|wav|m3u8|mpd)(\?|#|$)/i;

function noteMedia(tabId, entry) {
  if (tabId === undefined || tabId < 0) return;
  if (!entry || !entry.url || !/^https?:\/\//i.test(entry.url)) return;
  if (entry.url.length > 4000) return;
  let list = tabMedia.get(tabId);
  if (!list) { list = []; tabMedia.set(tabId, list); }
  if (list.some((e) => e.url === entry.url)) return;
  list.push({ ...entry, seenAt: Date.now() });
  if (list.length > 25) list.splice(0, list.length - 25);
  // Persist latest for the popup (best-effort).
  try {
    chrome.storage.local.set({ downloadCraftNet: list.slice(-25) });
  } catch (e) { /* ignore */ }
  // P0.2: per-tab badge count so the user sees catches in-browser.
  try {
    chrome.action.setBadgeText({ text: String(list.length), tabId }).catch(() => {});
    chrome.action.setBadgeBackgroundColor({ color: "#1F6FEB", tabId }).catch(() => {});
  } catch (e) { /* ignore */ }
}

try {
  if (chrome.webRequest && chrome.webRequest.onHeadersReceived) {
    chrome.webRequest.onHeadersReceived.addListener(
      (details) => {
        (async () => {
        try {
          if (details.tabId === undefined || details.tabId < 0) return;
          if (details.type !== "media" && details.type !== "xmlhttprequest" && details.type !== "other") return;
          const cfg = await settings().catch(() => ({ blockHosts: "", minSizeKB: 100 }));
          if (hostBlocked(details.url, cfg.blockHosts)) return;
          const headers = details.responseHeaders || [];
          let ctype = "";
          let clen = 0;
          for (const h of headers) {
            const n = String(h.name || "").toLowerCase();
            if (n === "content-type") ctype = String(h.value || "");
            if (n === "content-length") clen = parseInt(h.value || "0", 10) || 0;
          }
          const mime = ctype.split(";")[0].trim();
          const looksMedia = MEDIA_MIME.test(ctype) || MEDIA_URL.test(details.url.split("?")[0]);
          if (!looksMedia) return;
          // Skip tiny chunks unless HLS/DASH playlist (threshold configurable).
          const minB = Math.max(0, (parseInt(cfg.minSizeKB, 10) || 100)) * 1024;
          const isPlaylist = /m3u8|mpd|mpegurl|dash/i.test(details.url) || /m3u8|mpd|mpegurl|dash/i.test(ctype);
          if (!isPlaylist && clen > 0 && clen < minB) return;
          noteMedia(details.tabId, { url: details.url, mime, size: clen || 0 });
        } catch (e) { /* never break navigation */ }
        })();
      },
      { urls: ["http://*/*", "https://*/*"] },
      ["responseHeaders"]
    );
  }
} catch (e) { /* webRequest unavailable (Firefox MV3 w/o permission) */ }

try {
  if (chrome.tabs && chrome.tabs.onRemoved) {
    chrome.tabs.onRemoved.addListener((tabId) => {
      tabMedia.delete(tabId);
      try { chrome.action.setBadgeText({ text: "", tabId }).catch(() => {}); } catch (e) {}
    });
  }
} catch (e) { /* ignore */ }

function setBadge() {
  chrome.action
    .setBadgeText({ text: hostOk ? "" : "!" })
    .catch(() => {});
  if (!hostOk) {
    chrome.action
      .setBadgeBackgroundColor({ color: "#C42B1C" })
      .catch(() => {});
  }
}

// Ping once at startup so the badge reflects reality.
sendToHostPing();
async function sendToHostPing() {
  try {
    const res = await chrome.runtime.sendNativeMessage(HOST, { type: "ping" });
    hostOk = !!res && res.type === "pong";
    if (!hostOk) lastHostError = "ping answered, but no pong (old host version?)";
  } catch (e) {
    hostOk = false;
    lastHostError = (e && e.message) || String(e);
  }
  setBadge();
}

chrome.runtime.onInstalled.addListener(() => {
  // removeAll first: re-installs otherwise fail with duplicate-id.
  chrome.contextMenus.removeAll().then(() => {
    chrome.contextMenus.create({
      id: "send-to-download-craft",
      title: "Download with Download Craft",
      contexts: ["link", "video", "audio"],
    });
  });
  sendToHostPing();
  // Push the current page script into every open tab. Content scripts in
  // already-open tabs would otherwise keep running the pre-update code
  // (and fail once their APIs are gone) until each tab is reloaded by hand.
  refreshOpenTabs();
});

async function refreshOpenTabs() {
  try {
    const tabs = await chrome.tabs.query({});
    for (const t of tabs) {
      if (t.id === undefined) continue;
      try {
        await chrome.scripting.executeScript({
          target: { tabId: t.id },
          files: ["content.js"],
        });
      } catch (e) {
        /* chrome:// pages, web store, etc. — not scriptable, skip */
      }
    }
  } catch (e) {
    /* tabs API unavailable */
  }
}

chrome.contextMenus.onClicked.addListener((info, tab) => {
  const url = info.linkUrl || info.srcUrl;
  if (url) {
    markSeen(url);
    const pageUrl = (tab && tab.url) || null;
    sendToHost(url, null, { pageUrl }).then((ok) => {
      hostOk = ok;
      setBadge();
    });
  }
});

// ---- automatic capture ----
chrome.downloads.onCreated.addListener(async (item) => {
  const url = item.finalUrl || item.url;
  if (!url || !/^https?:\/\//i.test(url)) return; // blobs/data stay in browser
  if (freshSeen(url)) return; // our own fallback re-download
  const cfg = await settings();
  if (!cfg.autoCapture) return;
  if (hostBlocked(url, cfg.blockHosts)) return; // P0.1 site filter
  try {
    await chrome.downloads.cancel(item.id);
  } catch (e) {
    /* already finished */
  }
  try {
    await chrome.downloads.erase({ id: item.id });
  } catch (e) {
    /* keep going */
  }
  const ok = await sendToHost(url, item.filename);
  hostOk = ok;
  setBadge();
  if (!ok) {
    // Native host missing: hand the file back to the browser so nothing is lost.
    markSeen(url);
    chrome.downloads.download({ url, saveAs: false }, () => {
      void chrome.runtime.lastError;
    });
  }
});

// ---- popup / content-script messages ----
chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  (async () => {
    if (!msg || typeof msg !== "object") return { ok: false };
    if (msg.type === "download" && msg.url) {
      if (msg.ver && msg.ver !== EXT_VER) {
        // Stale tab script (extension reloaded without a tab refresh):
        // push the current script into the tab so the *next* click works.
        try {
          if (sender && sender.tab && sender.tab.id !== undefined &&
              chrome.scripting && chrome.scripting.executeScript) {
            await chrome.scripting.executeScript({
              target: { tabId: sender.tab.id },
              files: ["content.js"],
            });
            return {
              ok: false,
              error: "page script refreshed itself — click once more",
            };
          }
        } catch (e) {
          /* fall through to the reload hint */
        }
        return {
          ok: false,
          error: `page runs script v${msg.ver}, app expects v${EXT_VER} — reload this tab (Ctrl+R), then click again`,
        };
      }
      markSeen(msg.url);
      try {
        const pageUrl = msg.pageUrl || (sender && sender.tab && sender.tab.url) || null;
        const ok = await sendToHost(msg.url, msg.filename, {
          pageUrl,
          referer: msg.referer || pageUrl,
          mime: msg.mime || null,
          size: msg.size || null,
        });
        hostOk = ok;
        setBadge();
        if (ok) return { ok: true };
        return {
          ok: false,
          error: lastHostError ||
            "native host unreachable — reinstall it from Download Craft → Settings → Browser integration, then restart Chrome",
        };
      } catch (e) {
        return { ok: false, error: (e && e.message) || String(e) };
      }
    }
    if (msg.type === "getMedia") {
      const tabId = sender && sender.tab && sender.tab.id;
      const list = (tabId !== undefined && tabMedia.get(tabId)) || [];
      return { items: list.slice(-25) };
    }
    if (msg.type === "getSizes" && Array.isArray(msg.urls)) {
      // IDM-style file sizes for the quality menu. HEAD first, then a
      // bytes=0-0 Range fallback (media servers often hide length on HEAD).
      // Same-origin safe: service-worker fetch under host_permissions.
      const urls = msg.urls.filter((u) => /^https?:\/\//i.test(u)).slice(0, 12);
      const out = {};
      await Promise.all(urls.map(async (u) => {
        try {
          if (sizeCache.has(u)) { out[u] = sizeCache.get(u); return; }
          const ctl = new AbortController();
          const timer = setTimeout(() => ctl.abort(), 8000);
          let len = 0;
          try {
            const r = await fetch(u, { method: "HEAD", signal: ctl.signal, credentials: "omit", redirect: "follow" });
            len = parseInt(r.headers.get("content-length") || "0", 10) || 0;
          } finally {
            clearTimeout(timer);
          }
          if (!len) {
            const ctl2 = new AbortController();
            const t2 = setTimeout(() => ctl2.abort(), 8000);
            try {
              const r2 = await fetch(u, { method: "GET", headers: { Range: "bytes=0-0" }, signal: ctl2.signal, credentials: "omit", redirect: "follow" });
              const cr = r2.headers.get("content-range") || "";
              const m = cr.match(/\/(\d+)\s*$/);
              len = m ? parseInt(m[1], 10) || 0 : (parseInt(r2.headers.get("content-length") || "0", 10) || 0);
            } finally {
              clearTimeout(t2);
            }
          }
          out[u] = len;
          sizeCache.set(u, len);
          if (sizeCache.size > 200) sizeCache.clear();
        } catch (e) {
          out[u] = 0;
        }
      }));
      return { sizes: out };
    }
    if (msg.type === "getState") {
      // Fresh status on every popup open (the worker only pings at
      // startup, which can predate the app's first run).
      await sendToHostPing();
      const cfg = await settings();
      return { autoCapture: !!cfg.autoCapture, minSizeKB: cfg.minSizeKB, blockHosts: cfg.blockHosts, hostOk, hostError: lastHostError || null };
    }
    if (msg.type === "setAuto" && typeof msg.value === "boolean") {
      await chrome.storage.local.set({ autoCapture: msg.value });
      return { ok: true };
    }
    if (msg.type === "setFilters") {
      const patch = {};
      if (msg.minSizeKB !== undefined) {
        const n = Math.max(0, Math.min(100000, parseInt(msg.minSizeKB, 10) || 0));
        patch.minSizeKB = n;
      }
      if (typeof msg.blockHosts === "string" && msg.blockHosts.length < 4000) {
        patch.blockHosts = msg.blockHosts;
      }
      await chrome.storage.local.set(patch);
      return { ok: true };
    }
    return { ok: false };
  })().then(sendResponse);
  return true; // async response
});
