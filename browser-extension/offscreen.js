// Runs "Download in browser" jobs for the background (Chrome's offscreen
// document). Each finished file becomes a Blob URL that the background hands
// to chrome.downloads, then asks this page to revoke.
(() => {
  "use strict";
  const running = new Map(); // job id -> AbortController
  const urls = new Map(); // job id -> blob URL

  const tell = (msg) => {
    try { chrome.runtime.sendMessage({ ...msg, from: "offscreen" }); } catch (e) { /* background asleep */ }
  };

  chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (!msg || msg.target !== "offscreen") return false;
    // Only the extension's own pages may drive this document.
    if (sender && sender.id !== chrome.runtime.id) return false;
    if (msg.type === "bdl-start" && msg.job && !running.has(msg.job.id)) {
      const ctl = new AbortController();
      running.set(msg.job.id, ctl);
      DCBrowserDL.run(msg.job, (p) => tell({ type: "bdl-progress", id: msg.job.id, progress: p }), ctl.signal)
        .then((r) => {
          const url = URL.createObjectURL(r.blob);
          urls.set(msg.job.id, url);
          tell({ type: "bdl-ready", id: msg.job.id, blobUrl: url, ext: r.ext, note: r.note, bytes: r.bytes });
        })
        .catch((e) => tell({ type: "bdl-failed", id: msg.job.id, error: ctl.signal.aborted ? "cancelled" : String((e && e.message) || e) }))
        .finally(() => running.delete(msg.job.id));
      sendResponse({ ok: true });
    } else if (msg.type === "bdl-cancel") {
      const ctl = running.get(msg.id);
      if (ctl) ctl.abort();
      sendResponse({ ok: true });
    } else if (msg.type === "bdl-revoke") {
      const url = urls.get(msg.id);
      if (url) { URL.revokeObjectURL(url); urls.delete(msg.id); }
      sendResponse({ ok: true, idle: running.size === 0 && urls.size === 0 });
    } else {
      return false;
    }
    return false;
  });
})();
