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
const EXT_VER = "0.3.5";

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
  return chrome.storage.local.get({ autoCapture: true });
}

async function sendToHost(url, filename) {
  try {
    const res = await chrome.runtime.sendNativeMessage(HOST, {
      type: "download",
      url,
      filename: filename || null,
    });
    return !!res && (res.type === "ok" || res.type === "pong");
  } catch (e) {
    lastHostError = (e && e.message) || String(e);
    return false;
  }
}

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

chrome.contextMenus.onClicked.addListener((info) => {
  const url = info.linkUrl || info.srcUrl;
  if (url) {
    markSeen(url);
    sendToHost(url).then((ok) => {
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
  const { autoCapture } = await settings();
  if (!autoCapture) return;
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
        // Needs the "scripting" permission + sender tab id.
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
        const ok = await sendToHost(msg.url, msg.filename);
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
    if (msg.type === "getState") {
      // Fresh status on every popup open (the worker only pings at
      // startup, which can predate the app's first run).
      await sendToHostPing();
      const { autoCapture } = await settings();
      return { autoCapture, hostOk, hostError: lastHostError || null };
    }
    if (msg.type === "setAuto" && typeof msg.value === "boolean") {
      await chrome.storage.local.set({ autoCapture: msg.value });
      return { ok: true };
    }
    return { ok: false };
  })().then(sendResponse);
  return true; // async response
});
