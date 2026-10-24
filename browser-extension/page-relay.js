// Isolated-world relay for page-hook.js (which runs in the page and cannot
// reach extension APIs). Loaded at document_start so early API answers are
// not missed before content.js starts at document_idle.
(() => {
  // Re-injected after an extension update, the old relay's context is dead
  // (its sendMessage throws): replace it rather than keeping it.
  if (window.__dcPageRelay) window.removeEventListener("message", window.__dcPageRelay);
  const onMessage = (ev) => {
    if (ev.source !== window || !ev.data || ev.data.__dc !== "page-media") return;
    const items = Array.isArray(ev.data.items) ? ev.data.items.slice(0, 30) : [];
    if (!items.length) return;
    try {
      chrome.runtime.sendMessage({ type: "pageMedia", items, pageUrl: location.href }, () => {
        void chrome.runtime.lastError; // extension reloaded: nothing to do
      });
    } catch (e) { /* context invalidated after an extension reload */ }
  };
  window.__dcPageRelay = onMessage;
  window.addEventListener("message", onMessage);
})();
