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

  // In-page menu theme tokens (P4 polish): the dark card is the historic
  // look; the light card follows the "extension ui.png" mockup (white
  // card, gray section labels, bordered pills). Same keys both modes so
  // paint code never branches on missing fields.
  function menuTokens(mode) {
    if (mode === "light") {
      return {
        mode: "light",
        bg: "#ffffff",
        border: "1px solid rgba(0,0,0,.08)",
        radius: "16px",
        shadow: "0 12px 32px rgba(0,0,0,.18)",
        text: "#202124",
        dim: "#9aa0a6",
        itemBg: "#ffffff",
        itemBorder: "1px solid #e1e3e6",
        itemHover: "#f1f3f4",
        toggleBorder: "rgba(0,0,0,.2)",
        toggle: "☾",
      };
    }
    return {
      mode: "dark",
      bg: "rgba(16,17,23,.92)",
      border: "1px solid rgba(255,255,255,.1)",
      radius: "12px",
      shadow: "0 8px 28px rgba(0,0,0,.5)",
      text: "#e6e6f0",
      dim: "rgba(230,230,240,.6)",
      itemBg: "transparent",
      itemBorder: "1px solid transparent",
      itemHover: "#35363e",
      toggleBorder: "rgba(255,255,255,.25)",
      toggle: "☀",
    };
  }

  // In-page menu theming: historic dark card + a light card following the
  // "extension ui.png" mockup (white card, gray sections, bordered pills).
  // Auto from prefers-color-scheme, overridable per page via the toggle.
  // These run against the real DOM in content scripts AND under node with
  // stubbed globals (see rank.test.js "menu paint").
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
  function dcTokens() {
    return menuTokens(dcThemeMode());
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
  // Repaint a built menu in the current theme without touching layout
  // (position/display belong to the open/close toggles).
  function paintDcMenu(menu) {
    if (!menu) return;
    const T = dcTokens();
    menu._dcT = T;
    menu.style.background = T.bg;
    menu.style.border = T.border;
    menu.style.borderRadius = T.radius;
    menu.style.boxShadow = T.shadow;
    menu.style.color = T.text;
    menu.querySelectorAll('[data-dc="sec"],[data-dc="hint"]').forEach((el) => { el.style.color = T.dim; });
    menu.querySelectorAll('[data-dc="head"]').forEach((el) => { el.style.color = T.text; });
    menu.querySelectorAll('[data-dc="item"]').forEach((el) => {
      el.style.background = T.itemBg;
      el.style.border = T.itemBorder;
      el.style.color = T.text;
    });
    menu.querySelectorAll('[data-dc="toggle"]').forEach((el) => {
      el.style.color = T.text;
      el.style.borderColor = T.toggleBorder;
      el.textContent = T.toggle;
    });
  }
  function dcThemeToggle(menu, doc) {
    const d = doc || ((typeof document !== "undefined") ? document : null);
    if (!d) return null;
    const t = d.createElement("button");
    t.setAttribute("data-dc", "toggle");
    t.title = "Toggle light / dark";
    t.style.cssText = "margin-left:auto;width:26px;height:26px;border-radius:999px;border:1px solid;cursor:pointer;background:transparent;font-size:13px;line-height:1;flex-shrink:0;";
    t.onclick = (ev) => {
      if (ev && ev.stopPropagation) ev.stopPropagation();
      dcSetTheme(dcThemeMode() === "light" ? "dark" : "light");
      paintDcMenu(menu);
    };
    return t;
  }

  const DCRank = { isPlaylistUrl, isSegmentUrl, canonicalUrl, kindOf, scoreMedia, collapseDupes, fmtDur, menuTokens, dcThemeMode, dcTokens, dcSetTheme, paintDcMenu, dcThemeToggle };
  g.DCRank = DCRank;
  if (typeof module !== "undefined" && module.exports) module.exports = DCRank;
})(typeof globalThis !== "undefined" ? globalThis : this);
