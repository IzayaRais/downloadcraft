// Isolated-world relay for page-hook.js (which runs in the page and cannot
// reach extension APIs). Loaded at document_start so early API answers are
// not missed before content.js starts at document_idle.
(() => {
  if (window.__dcPageRelay) return;
  window.__dcPageRelay = true;
  window.addEventListener("message", (ev) => {
    if (ev.source !== window || !ev.data || ev.data.__dc !== "page-media") return;
    const items = Array.isArray(ev.data.items) ? ev.data.items.slice(0, 30) : [];
    if (!items.length) return;
    try {
      chrome.runtime.sendMessage({ type: "pageMedia", items, pageUrl: location.href }, () => {
        void chrome.runtime.lastError; // extension reloaded: nothing to do
      });
    } catch (e) { /* context invalidated after an extension reload */ }
  });
})();
