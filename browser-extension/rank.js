// DCRank: pure media-candidate ranking shared by background/content/popup.
// No chrome.* APIs here so it runs under `node --test` too (see rank.test.js).
// Loaded as a classic script everywhere (manifest content_scripts order,
// popup.html script tags, background importScripts) and as a module in tests.
(function (g) {
  "use strict";

  // Playlist manifests the engine assembles itself (offer these, not parts).
  function isPlaylistUrl(u) {
    return /(\.m3u8|\.mpd)(\?|#|$)/i.test(String(u || "").split("?")[0])
      || /m3u8|mpegurl|dash/i.test(String(u || ""));
  }

  // Stream parts that only make sense inside a playlist assembly.
  function isSegmentUrl(u) {
    const base = String(u || "").split("?")[0];
    return /(\.ts|\.m4s|\.cmfv|\.cmfa|\.mpd\.seg)(\?|#|$)/i.test(base)
      || /\/chunklist|\/seg-|sequence/i.test(base);
  }

  // Canonical identity for duplicate collapse: slice selectors
  // (bytestart/byteend) address the same file, so they collapse.
  function canonicalUrl(u) {
    try {
      const x = new URL(String(u || ""));
      x.searchParams.delete("bytestart");
      x.searchParams.delete("byteend");
      x.hash = "";
      return x.toString();
    } catch (e) {
      return String(u || "");
    }
  }

  // 'playlist' | 'segment' | 'file'
  function kindOf(entry) {
    const u = (entry && entry.url) || "";
    if (isPlaylistUrl(u)) return "playlist";
    if (isSegmentUrl(u)) return "segment";
    return "file";
  }

  // Rank score (higher = offer first). A playlist on the tab demotes loose
  // segments: they are children the engine fetches itself, and ad-segments
  // hide among them — never offer parts when the whole is available.
  function scoreMedia(entry, ctx) {
    const e = entry || {};
    const u = String(e.url || "");
    const mime = String(e.mime || "").toLowerCase();
    const hasPlaylist = !!(ctx && ctx.hasPlaylist);
    const kind = kindOf(e);
    if (kind === "segment" && hasPlaylist) return -1;
    let s = kind === "file" ? 300 : kind === "playlist" ? 200 : 50;
    if (mime.startsWith("video/")) s += 100;
    else if (mime.startsWith("audio/")) s += 60;
    if (/mp4/i.test(mime) || /\.mp4/i.test(u)) s += 50;
    if (/fbcdn|lookaside|facebook|instagram|cdninstagram/i.test(u)) s += 20;
    // Exact page-JSON sizes print instantly; measured sizes rank bigger
    // files first (skips ad slivers the size gate let through).
    const size = e.size || e._size || 0;
    if (size > 0) s += Math.min(30, Math.log10(size + 1) * 5);
    return s;
  }

  // Collapse same-file duplicates (slice variants, re-sniffs): keep the
  // biggest-known entry, count distinct tabs for the "×N tabs" hint.
  function collapseDupes(items) {
    const byCanon = new Map();
    for (const m of (items || [])) {
      if (!m || !/^https?:\/\//i.test(m.url || "")) continue;
      const k = canonicalUrl(m.url);
      const size = m.size || m._size || 0;
      const hit = byCanon.get(k);
      if (!hit) {
        byCanon.set(k, { item: m, size, tabs: new Set(m.tabId !== undefined ? [m.tabId] : []) });
      } else {
        if (size > hit.size) { hit.item = m; hit.size = size; }
        if (m.tabId !== undefined) hit.tabs.add(m.tabId);
      }
    }
    return [...byCanon.values()].map((h) => ({ ...h.item, tabs: h.tabs.size || undefined }));
  }

  // Seconds → "3:26" for quality rows with known durations.
  function fmtDur(sec) {
    const n = Math.round(Number(sec) || 0);
    if (!(n > 0) || n >= 86400) return "";
    const m = Math.floor(n / 60);
    const s = String(n % 60).padStart(2, "0");
    return `${m}:${s}`;
  }

  // Floating panel tokens: the "extension_popup.html" design — a circle
  // trigger opening a message-bubble panel with a 2-column quality grid,
  // other-file chips and a Download / Cancel / X action row. Same keys in
  // both modes so the paint code never branches on missing fields.
  function panelTokens(mode) {
    if (mode === "light") {
      return {
        mode: "light",
        bg: "#ffffff",
        card: "#f8fafc",
        hover: "#eef2f7",
        border: "#e2e8f0",
        text: "#0f172a",
        dim: "#64748b",
        muted: "#94a3b8",
        accent: "#2563eb",
        accentLight: "rgba(37,99,235,.10)",
        shadow: "0 12px 32px rgba(15,23,42,.18)",
        thumbBg: "linear-gradient(135deg,#c7d2fe,#a5b4fc)",
        toggleBorder: "rgba(15,23,42,.2)",
        toggle: "☾",
      };
    }
    return {
      mode: "dark",
      bg: "#0f172a",
      card: "#1e293b",
      hover: "#334155",
      border: "#334155",
      text: "#f1f5f9",
      dim: "#94a3b8",
      muted: "#64748b",
      accent: "#3b82f6",
      accentLight: "rgba(59,130,246,.16)",
      shadow: "0 8px 32px rgba(0,0,0,.4)",
      thumbBg: "linear-gradient(135deg,#667eea,#764ba2)",
      toggleBorder: "rgba(241,245,249,.25)",
      toggle: "☀",
    };
  }

  // Repaint a built panel in the current theme. Role tags live in
  // `data-dc-p` (layout is inline-styled at build time and never touched
  // here), and the bubble arrow reads `data-dc-arrow` for its side.
  function paintDcPanel(root) {
    if (!root) return;
    const T = panelTokens(dcThemeMode());
    root._dcT = T;
    root.style.background = T.bg;
    root.style.borderColor = T.border;
    root.style.color = T.text;
    root.style.boxShadow = T.shadow;
    root.querySelectorAll("[data-dc-p]").forEach((el) => {
      const k = el.getAttribute("data-dc-p");
      if (k === "text") el.style.color = T.text;
      else if (k === "dim") el.style.color = T.dim;
      else if (k === "muted") el.style.color = T.muted;
      else if (k === "accent") el.style.color = T.accent;
      else if (k === "icon") el.style.color = T.accent;
      else if (k === "thumb") el.style.background = T.thumbBg;
      else if (k === "divider") el.style.background = T.border;
      else if (k === "borders") el.style.borderColor = T.border;
      else if (k === "card") { el.style.background = T.card; el.style.borderColor = T.border; }
      else if (k === "chip") { el.style.background = T.card; el.style.borderColor = T.border; el.style.color = T.text; }
      else if (k === "chipmini") { el.style.background = T.hover; el.style.color = T.dim; }
      else if (k === "btn") { el.style.background = T.card; el.style.borderColor = T.border; el.style.color = T.text; }
      else if (k === "btnp") { el.style.background = T.accent; el.style.borderColor = T.accent; el.style.color = "#fff"; }
      else if (k === "badge") { el.style.background = T.accentLight; el.style.color = T.accent; }
      else if (k === "item") {
        const sel = !!el.getAttribute("data-dc-sel");
        el.style.background = sel ? T.accentLight : T.card;
        el.style.borderColor = sel ? T.accent : T.border;
        el.style.color = T.text;
      } else if (k === "arrow") {
        const side = el.getAttribute("data-dc-arrow") === "bottom" ? "bottom" : "top";
        el.style.background = T.bg;
        el.style.borderLeftColor = side === "top" ? T.border : "transparent";
        el.style.borderTopColor = side === "top" ? T.border : "transparent";
        el.style.borderRightColor = side === "top" ? "transparent" : T.border;
        el.style.borderBottomColor = side === "top" ? "transparent" : T.border;
      } else if (k === "toggle") {
        el.style.color = T.text;
        el.style.borderColor = T.toggleBorder;
        el.textContent = T.toggle;
      }
    });
  }

  // In-page theme: auto from prefers-color-scheme, overridable per page via
  // the panel's toggle. Runs against the real DOM in content scripts AND
  // under node with stubbed globals (see rank.test.js "panel paint").
  function dcThemeMode() {
    try {
      const g = (typeof globalThis !== "undefined") ? globalThis : {};
      if (g.__dcTheme === "light" || g.__dcTheme === "dark") return g.__dcTheme;
      if (typeof window !== "undefined" && (window.__dcTheme === "light" || window.__dcTheme === "dark")) {
        return window.__dcTheme;
      }
      const mm = (typeof window !== "undefined" && window.matchMedia) || g.matchMedia;
      if (mm && mm("(prefers-color-scheme: light)").matches) return "light";
    } catch (e) {}
    return "dark";
  }
  function dcSetTheme(mode) {
    try {
      const g = (typeof globalThis !== "undefined") ? globalThis : {};
      if (mode === "light" || mode === "dark") {
        g.__dcTheme = mode;
        if (typeof window !== "undefined") window.__dcTheme = mode;
      }
    } catch (e) {}
  }

  const DCRank = { isPlaylistUrl, isSegmentUrl, canonicalUrl, kindOf, scoreMedia, collapseDupes, fmtDur, panelTokens, dcThemeMode, dcSetTheme, paintDcPanel };
  g.DCRank = DCRank;
  if (typeof module !== "undefined" && module.exports) module.exports = DCRank;
})(typeof globalThis !== "undefined" ? globalThis : this);
