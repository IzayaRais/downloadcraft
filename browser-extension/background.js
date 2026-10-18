// Background worker: every browser download is captured
// automatically, cancelled in-browser, and handed to the native host.
// No clicks, no pasting — zero config.

const HOST = "com.download_craft";
const GRACE_MS = 15_000; // ignore our own fallback re-downloads
const seen = new Map(); // url -> timestamp (loop guard)
let hostOk = true;
let lastHostError = "";
let lastFallbackError = "";
// Must match manifest version + content.js EXT_VER. Tabs running an older
// content script are rejected with a "reload the tab" error instead of
// failing mysteriously (their extension APIs are gone after a reload).
const EXT_VER = "0.9.3";

// Shared ranking (P4): playlist/segment/file kinds, scores, duplicate
// collapse. Service workers support importScripts; if it ever fails the
// guarded fallbacks below keep the old behavior instead of breaking.
try {
  if (typeof importScripts === "function") importScripts("rank.js");
} catch (e) { /* RANK stays null -> legacy paths */ }

function markSeen(url, filename) {
  seen.set(url + "|" + (filename || ""), Date.now());
  if (seen.size > 200) {
    const cut = Date.now() - GRACE_MS;
    for (const [u, t] of seen) if (t < cut) seen.delete(u);
  }
}
function freshSeen(url, filename) {
  const t = seen.get(url + "|" + (filename || ""));
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

// Curated cookies for ONE url only (same-host). Never logged. Whole
// name=value pairs only — a byte cut could split a pair and corrupt auth.
async function cookiesFor(url) {
  try {
    if (!chrome.cookies || !chrome.cookies.getAll) return null;
    const list = await chrome.cookies.getAll({ url });
    if (!list || !list.length) return null;
    const pairs = list
      .filter((c) => c && c.name)
      .slice(0, 40)
      .map((c) => `${c.name}=${c.value || ""}`);
    let s = "";
    for (const p of pairs) {
      const add = (s ? "; " : "") + p;
      if ((s + add).length > 6000) break;
      s += add;
    }
    return s || null;
  } catch (e) {
    return null;
  }
}

function stripFbSlice(url) {
  try {
    const u = new URL(url);
    const h = u.hostname.toLowerCase();
    const social = h.includes("fbcdn") || h.includes("scontent") || h.includes("facebook")
      || h.includes("fbsbx") || h.includes("lookaside") || h.includes("cdninstagram") || h.includes("instagram");
    if (!social) return url;
    if (!u.searchParams.has("bytestart") && !u.searchParams.has("byteend")) return url;
    u.searchParams.delete("bytestart");
    u.searchParams.delete("byteend");
    return u.toString();
  } catch (e) { return url; }
}

async function sendToHost(url, filename, extra) {
  try {
    url = stripFbSlice(url);
    let cookie = (extra && extra.cookie) || null;
    if (!cookie && /^https?:\/\//i.test(url)) {
      cookie = await cookiesFor(url); // best-effort, same-host only
      // CDN hosts (fbcdn/lookaside) carry no cookies of their own — fall
      // back to the page's session cookies so expiring signed URLs keep
      // working without a page reload.
      const pageUrl = extra && (extra.pageUrl || extra.referer);
      if (!cookie && pageUrl && /^https?:\/\//i.test(pageUrl)) {
        try {
          const uh = new URL(url).hostname.toLowerCase();
          const ph = new URL(pageUrl).hostname.toLowerCase();
          if (uh !== ph) cookie = await cookiesFor(pageUrl);
        } catch (e) { /* host parse failed: no cookies */ }
      }
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
      pairUrl: (extra && extra.pairUrl) || null,
      pairMime: (extra && extra.pairMime) || null,
      quality: (extra && extra.quality) || null,
      dur: (extra && extra.dur) || null,
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
const tabPlaylist = new Map(); // tabId -> playlist url (P4: demotes loose segments)
const seenTabs = new Map(); // canonical url -> Set(tabId), for the "×N tabs" hint
const sizeCache = new Map(); // url -> bytes (quality-menu sizes)
const MEDIA_MIME = /^(video\/|audio\/|application\/(x-mpegurl|vnd\.apple\.mpegurl|dash\+xml|mp4|octet-stream))/i;
const MEDIA_URL = /\.(mp4|webm|mkv|mov|avi|m4v|mp3|m4a|aac|flac|ogg|wav|m3u8|mpd|m4s|ts|cmfv|cmfa)(\?|#|$)/i;

function rankKind(url) {
  if (typeof DCRank !== "undefined" && DCRank.isPlaylistUrl(url)) return "playlist";
  if (typeof DCRank !== "undefined" && DCRank.isSegmentUrl(url)) return "segment";
  // Legacy fallback when rank.js failed to load (same shapes, inline).
  if (/(\.m3u8|\.mpd)(\?|#|$)/i.test(String(url).split("?")[0]) || /m3u8|mpegurl|dash/i.test(String(url))) return "playlist";
  if (/(\.ts|\.m4s|\.cmfv|\.cmfa)(\?|#|$)/i.test(String(url).split("?")[0])) return "segment";
  return "file";
}

function canonKey(url) {
  if (typeof DCRank !== "undefined") return DCRank.canonicalUrl(url);
  return String(url || "").split("#")[0];
}

function noteMedia(tabId, entry) {
  if (tabId === undefined || tabId < 0) return;
  if (!entry || !entry.url || !/^https?:\/\//i.test(entry.url)) return;
  entry.url = stripFbSlice(entry.url);
  if (entry.url.length > 4000) return;
  const kind = rankKind(entry.url);
  // P4: a playlist on the tab demotes loose segments — they are children
  // the engine fetches itself (ad slivers hide among them), so menus must
  // offer the whole, and the badge must not count parts as downloads.
  if (kind === "playlist") tabPlaylist.set(tabId, entry.url);
  if (kind === "segment" && tabPlaylist.get(tabId)) entry.child = true;
  let list = tabMedia.get(tabId);
  if (!list) { list = []; tabMedia.set(tabId, list); }
  if (list.some((e) => e.url === entry.url)) return;
  list.push({ ...entry, kind, seenAt: Date.now() });
  if (list.length > 25) {
    const dropped = list.splice(0, list.length - 25);
    for (const d of dropped) pruneSeen(d.url, tabId);
  }
  // Cross-tab duplicate collapse (P4): one canonical entry, tab count.
  const ck = canonKey(entry.url);
  let tabs = seenTabs.get(ck);
  if (!tabs) { tabs = new Set(); seenTabs.set(ck, tabs); }
  tabs.add(tabId);
  if (seenTabs.size > 500) {
    seenTabs.clear(); // bounded best-effort; rebuilt by live traffic
  }
  // Legacy global snapshot: fallback only (popup prefers per-tab peek).
  try {
    chrome.storage.local.set({ downloadCraftNet: list.slice(-25) });
  } catch (e) { /* ignore */ }
  // Badge counts downloadable candidates, never playlist children.
  const badgeN = list.filter((e) => !e.child).length;
  try {
    chrome.action.setBadgeText({ text: String(badgeN), tabId }).catch(() => {});
    chrome.action.setBadgeBackgroundColor({ color: "#1F6FEB", tabId }).catch(() => {});
  } catch (e) { /* ignore */ }
}

function pruneSeen(url, tabId) {
  try {
    const ck = canonKey(url);
    const tabs = seenTabs.get(ck);
    if (!tabs) return;
    tabs.delete(tabId);
    if (!tabs.size) seenTabs.delete(ck);
  } catch (e) { /* ignore */ }
}

function forgetTab(tabId) {
  tabMedia.delete(tabId);
  tabPlaylist.delete(tabId);
  try {
    for (const [ck, tabs] of seenTabs) {
      tabs.delete(tabId);
      if (!tabs.size) seenTabs.delete(ck);
    }
  } catch (e) { /* ignore */ }
  try { chrome.action.setBadgeText({ text: "", tabId }).catch(() => {}); } catch (e) {}
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
          let crange = "";
          for (const h of headers) {
            const n = String(h.name || "").toLowerCase();
            if (n === "content-type") ctype = String(h.value || "");
            if (n === "content-length") clen = parseInt(h.value || "0", 10) || 0;
            if (n === "content-range") crange = String(h.value || "");
          }
          // Range chunks (206) report the CHUNK size in content-length, not
          // the file size — judge the size filter by the Content-Range total
          // (`bytes 0-0/12345`), or a throttled CDN's drips get skipped and
          // Facebook reels never produce a pill without a reload.
          let totalForFilter = clen;
          const crm = crange.match(/\/(\d+)\s*$/);
          if (crm) totalForFilter = parseInt(crm[1], 10) || clen;
          const mime = ctype.split(";")[0].trim();
          const looksMedia = MEDIA_MIME.test(ctype) || MEDIA_URL.test(details.url.split("?")[0]);
          if (!looksMedia) return;
          // Skip tiny chunks unless HLS/DASH playlist (threshold configurable).
          const minB = Math.max(0, (parseInt(cfg.minSizeKB, 10) || 100)) * 1024;
          const isPlaylist = /m3u8|mpd|mpegurl|dash/i.test(details.url) || /m3u8|mpd|mpegurl|dash/i.test(ctype);
          if (!isPlaylist && totalForFilter > 0 && totalForFilter < minB) return;
          noteMedia(details.tabId, {
            url: details.url,
            mime,
            size: totalForFilter || 0,
            pageUrl: details.documentUrl || null,
          });
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
      forgetTab(tabId);
    });
  }
  // New navigation (same tab, new video): drop the previous video's URLs so
  // itag-swaps and menus can never graft stale streams. SPA history swaps
  // (Facebook reels, YouTube shorts) fire onUpdated with a new url but no
  // "loading" status — clear on any url change too, so the next reel can't
  // offer the previous reel's fbcdn link.
  if (chrome.tabs && chrome.tabs.onUpdated) {
    chrome.tabs.onUpdated.addListener((tabId, info) => {
      if ((info.status === "loading" && info.url !== undefined) || info.url !== undefined) {
        forgetTab(tabId);
      }
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
  if (url && /^https?:\/\//i.test(url)) {
    markSeen(url, null);
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
  const cfg = await settings();
  if (!cfg.autoCapture) return;
  if (hostBlocked(url, cfg.blockHosts)) return; // P0.1 site filter
  if (freshSeen(url, item.filename)) return; // our own fallback re-download
  // Honor the size filter here too (sniffer-only before): tiny files stay
  // in the browser. Unknown sizes (0/undefined) still capture.
  const minB = Math.max(0, (parseInt(cfg.minSizeKB, 10) || 0)) * 1024;
  const known = item.fileSize || item.totalBytes || 0;
  if (minB > 0 && known > 0 && known < minB) return;
  try {
    await chrome.downloads.cancel(item.id);
  } catch (e) {
    // Already finished (fast download): cancelling is impossible, and
    // re-sending would deliver a SECOND copy — leave the browser's file.
    return;
  }
  const ok = await sendToHost(url, item.filename);
  // Erase the browser entry only once the app confirmed receipt.
  if (ok) {
    try {
      await chrome.downloads.erase({ id: item.id });
    } catch (e) {
      /* already gone */
    }
  }
  hostOk = ok;
  setBadge();
  if (!ok) {
    // Native host missing: hand the file back to the browser so nothing is lost.
    markSeen(url, item.filename);
    chrome.downloads.download({ url, saveAs: false }, () => {
      if (chrome.runtime.lastError) {
        lastFallbackError = chrome.runtime.lastError.message || "browser re-download failed";
        setBadge();
      } else {
        lastFallbackError = "";
      }
    });
  }
});

// ---- popup / content-script messages ----
chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  (async () => {
    if (!msg || typeof msg !== "object") return { ok: false };
    if (msg.type === "download" && msg.url) {
      if (!/^https?:\/\//i.test(msg.url) || msg.url.length > 4000) {
        return { ok: false, error: "only http(s) links up to 4000 chars can be sent" };
      }
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
      markSeen(msg.url, msg.filename);
      try {
        const pageUrl = msg.pageUrl || (sender && sender.tab && sender.tab.url) || null;
        const ok = await sendToHost(msg.url, msg.filename, {
          pageUrl,
          referer: msg.referer || pageUrl,
          mime: msg.mime || null,
          size: msg.size || null,
          pairUrl: msg.pairUrl || null,
          pairMime: msg.pairMime || null,
          quality: msg.quality || null,
          dur: msg.dur || null,
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
    // Presence pre-check: which urls are already queued (no side effects).
    // Lets menus mark "✓ Queued" rows instead of sending dupes that bounce
    // back as "already in queue" errors.
    if (msg.type === "isQueued" && Array.isArray(msg.urls)) {
      try {
        const urls = msg.urls
          .filter((u) => typeof u === "string" && /^https?:\/\//i.test(u))
          .slice(0, 25);
        if (!urls.length) return { queued: [] };
        const res = await chrome.runtime.sendNativeMessage(HOST, { type: "query", urls });
        if (res && res.type === "queued" && Array.isArray(res.urls)) {
          return { queued: res.urls };
        }
        return { queued: [] };
      } catch (e) {
        return { queued: [] };
      }
    }
    if (msg.type === "getMedia") {      // Popup passes its tab; content scripts arrive with sender.tab.
      const tabId = (msg.tabId !== undefined)
        ? msg.tabId
        : (sender && sender.tab && sender.tab.id);
      const list = (tabId !== undefined && tabMedia.get(tabId)) || [];
      const page = msg.pageUrl || null;
      // SPA pages report a fresh pageUrl while the sniffer's documentUrl may
      // lag one navigation behind — prefer exact matches but fall back to
      // the tab's recent items instead of answering empty (empty = the pill
      // silently sends the unresolvable page URL and the user must reload).
      const items = list.slice(-25).filter((m) => !page || !m.pageUrl || m.pageUrl === page);
      const full = items.length ? items : list.slice(-25);
      // P4 cross-tab hint: how many tabs have seen the same file.
      const annotated = full.map((m) => {
        let tabs = 1;
        try {
          const hit = seenTabs.get(canonKey(m.url));
          if (hit) tabs = Math.max(1, hit.size);
        } catch (e) { /* ignore */ }
        return { ...m, tabs };
      });
      return { items: annotated };
    }
    if (msg.type === "getSizes" && Array.isArray(msg.urls)) {
      // IDM-style file sizes for the quality menu. HEAD first, then a
      // bytes=0-0 Range fallback (media servers often hide length on HEAD).
      // credentials:include sends the browser's own cookies (googlevideo
      // answers 403 to anonymous HEADs but serves logged-in sessions), with
      // the page as referrer. (fetch still cannot set Cookie manually —
      // forbidden header — so off-session hosts may stay unknown; sizes
      // stay best-effort.)
      const seenUrls = new Set();
      const urls = msg.urls
        .filter((u) => typeof u === "string" && /^https?:\/\//i.test(u) && !seenUrls.has(u) && (seenUrls.add(u), true))
        .slice(0, 12);
      const referrer = (typeof msg.referer === "string" && /^https?:\/\//i.test(msg.referer))
        ? msg.referer
        : undefined;
      const out = {};
      await Promise.all(urls.map(async (u) => {
        try {
          if (sizeCache.has(u)) { out[u] = sizeCache.get(u); return; }
          const ctl = new AbortController();
          const timer = setTimeout(() => ctl.abort(), 8000);
          let len = 0;
          try {
            const r = await fetch(u, { method: "HEAD", signal: ctl.signal, credentials: "include", redirect: "follow", ...(referrer ? { referrer, referrerPolicy: "unsafe-url" } : {}) });
            len = parseInt(r.headers.get("content-length") || "0", 10) || 0;
          } finally {
            clearTimeout(timer);
          }
          if (!len) {
            const ctl2 = new AbortController();
            const t2 = setTimeout(() => ctl2.abort(), 8000);
            try {
              const r2 = await fetch(u, { method: "GET", headers: { Range: "bytes=0-0" }, signal: ctl2.signal, credentials: "include", redirect: "follow", ...(referrer ? { referrer, referrerPolicy: "unsafe-url" } : {}) });
              const cr = r2.headers.get("content-range") || "";
              const m = cr.match(/\/(\d+)\s*$/);
              len = m ? parseInt(m[1], 10) || 0 : (parseInt(r2.headers.get("content-length") || "0", 10) || 0);
            } finally {
              clearTimeout(t2);
            }
          }
          out[u] = len;
          sizeCache.set(u, len);
          // Drop the oldest half instead of wiping the cache outright.
          if (sizeCache.size > 200) {
            let n = 0;
            for (const k of sizeCache.keys()) {
              sizeCache.delete(k);
              if (++n >= 100) break;
            }
          }
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
      return { autoCapture: !!cfg.autoCapture, minSizeKB: cfg.minSizeKB, blockHosts: cfg.blockHosts, hostOk, hostError: lastHostError || null, fallbackError: lastFallbackError || null };
    }
    if (msg.type === "setAuto" && typeof msg.value === "boolean") {
      await chrome.storage.local.set({ autoCapture: msg.value });
      return { ok: true };
    }
    if (msg.type === "setFilters") {
      const patch = {};
      let ok = false;
      if (msg.minSizeKB !== undefined) {
        const n = parseInt(msg.minSizeKB, 10);
        if (!Number.isFinite(n) || n < 0 || n > 100000) {
          return { ok: false, error: "min size must be 0–100000 KB" };
        }
        patch.minSizeKB = n;
        ok = true;
      }
      if (typeof msg.blockHosts === "string" && msg.blockHosts.length < 4000) {
        patch.blockHosts = msg.blockHosts;
        ok = true;
      }
      if (!ok) return { ok: false, error: "nothing valid to save" };
      await chrome.storage.local.set(patch);
      return { ok: true };
    }
    return { ok: false };
  })().then(sendResponse);
  return true; // async response
});
