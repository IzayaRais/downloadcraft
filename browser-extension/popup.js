const $ = (id) => document.getElementById(id);

// Promise-style calls (`await api.*` below) work in both engines: Firefox
// exposes the promise-returning `browser` namespace, Chrome only `chrome`
// (promise-capable under MV3). Callback-style calls keep using `chrome`.
const api = (typeof browser !== "undefined" && browser.runtime) ? browser : chrome;
const R = (typeof DCRank !== "undefined") ? DCRank : null;

// ---------- theme: the app's Light / Dark / System choice ----------
function applyTheme(appTheme) {
  let mode = appTheme;
  if (mode !== "light" && mode !== "dark") {
    mode = window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  const T = R ? R.panelTokens(mode) : null;
  if (!T) return;
  const css = document.documentElement.style;
  css.setProperty("--bg", T.bg);
  css.setProperty("--card", T.card);
  css.setProperty("--hover", T.hover);
  css.setProperty("--line", T.border);
  css.setProperty("--text", T.text);
  css.setProperty("--dim", T.dim);
  css.setProperty("--muted", T.muted);
  css.setProperty("--accent", T.accent);
  css.setProperty("--accent-soft", T.accentLight);
  css.setProperty("--thumb", T.thumbBg);
  css.setProperty("--ok", mode === "light" ? "#15803d" : "#22c55e");
  css.setProperty("--bad", mode === "light" ? "#b91c1c" : "#ef4444");
  document.documentElement.style.colorScheme = mode;
}
chrome.storage.local.get({ appTheme: "system" }, (r) => applyTheme(r && r.appTheme));
chrome.storage.onChanged.addListener((ch, area) => {
  if (area === "local" && ch.appTheme) applyTheme(ch.appTheme.newValue);
});

function status(text, kind) {
  const el = $("status");
  el.textContent = text || "";
  el.className = kind || "";
}

// ---------- tabs ----------
function showTab(which) {
  const videos = which === "videos";
  $("tabVideos").classList.toggle("on", videos);
  $("tabSettings").classList.toggle("on", !videos);
  $("viewVideos").hidden = !videos;
  $("viewSettings").hidden = videos;
}
$("tabVideos").onclick = () => showTab("videos");
$("tabSettings").onclick = () => showTab("settings");

// ---------- settings ----------
chrome.runtime.sendMessage({ type: "getState" }, (res) => {
  if (chrome.runtime.lastError || !res) {
    $("conn").textContent = "Extension error";
    $("conn").className = "conn bad";
    status("Extension background unreachable — reload the extension.", "bad");
    return;
  }
  $("conn").textContent = res.hostOk ? "● Connected" : "● App offline";
  $("conn").className = res.hostOk ? "conn ok" : "conn bad";
  $("conn").title = res.hostOk ? "Download Craft is ready" : (res.hostError || "Open Download Craft once to connect it.");
  $("auto").checked = !!res.autoCapture;
  $("deep").checked = res.deepDetect !== false;
  $("hideUI").checked = !!res.hideBrowserUI;
  $("hideUI").disabled = !(res.downloadUI && res.downloadUI.supported);
  if (res.minSizeKB !== undefined) $("minSize").value = String(res.minSizeKB);
  if (typeof res.blockHosts === "string") $("block").value = res.blockHosts;
  if (res.fallbackError) status("Browser re-download failed: " + res.fallbackError, "bad");
  else if (!res.hostOk) status("Open Download Craft once so the browser can reach it.", "bad");
});

$("auto").onchange = () => chrome.runtime.sendMessage({ type: "setAuto", value: $("auto").checked });
$("deep").onchange = () => chrome.runtime.sendMessage({ type: "setDeepDetect", value: $("deep").checked });
$("hideUI").onchange = () => {
  chrome.runtime.sendMessage({ type: "setBrowserUI", value: $("hideUI").checked }, (res) => {
    status(res && res.ok ? "Browser download UI preference saved" : "This browser could not change its download UI.", res && res.ok ? "ok" : "bad");
  });
};
$("saveFilters").onclick = () => {
  chrome.runtime.sendMessage({ type: "setFilters", minSizeKB: $("minSize").value.trim(), blockHosts: $("block").value.trim() }, (res) => {
    if (chrome.runtime.lastError || !res) status("Extension background unreachable.", "bad");
    else status(res.ok ? "Filters saved ✓" : ("Could not save filters. " + (res.error || "")), res.ok ? "ok" : "bad");
  });
};
$("restoreVideoButtons").onclick = async () => {
  try {
    const [tab] = await api.tabs.query({ active: true, currentWindow: true });
    if (!tab || !tab.id) throw new Error("No active page");
    // Omit frameId so embedded Vimeo and other players receive it too.
    await api.tabs.sendMessage(tab.id, { type: "restoreVideoButtons" });
    status("Video download buttons restored.", "ok");
  } catch (e) { status("Reload this page, then try again.", "bad"); }
};

// ---------- sending ----------
function send(msg) {
  chrome.runtime.sendMessage(msg, (res) => {
    if (chrome.runtime.lastError || !res) status("App not reachable — open Download Craft once.", "bad");
    else status(res.ok ? "Sent to Download Craft ✓" : ("Not sent. " + (res.error || "")), res.ok ? "ok" : "bad");
  });
}
$("send").onclick = () => {
  const url = $("url").value.trim();
  if (!url) return;
  if (!/^https?:\/\//i.test(url) || url.length > 4000) {
    status("Only http(s) links up to 4000 chars can be sent.", "bad");
    return;
  }
  send({ type: "download", url });
};

// ---------- building cards ----------
function fmtSize(n) {
  if (!n || n <= 0) return "";
  const u = ["B", "KB", "MB", "GB"];
  let v = n, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return i === 0 ? `${n} ${u[i]}` : `${v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
}
const fmtDur = (s) => (R ? R.fmtDur(s) : "");
const safeName = (s, ext = ".mp4") => {
  const base = String(s || "video").replace(/[<>:"/\\|?*]/g, "_").trim().slice(0, 120) || "video";
  return base.toLowerCase().endsWith(ext) ? base : base + ext;
};
const contOf = (mime) => String(mime || "").split(";")[0].split("/")[1] || "mp4";
const tagOf = (url, mime) => {
  const m = String(mime || "").toLowerCase();
  if (/dash/.test(m) || /\.mpd(\?|$)/i.test(url)) return "DASH";
  if (/mpegurl/.test(m) || /\.m3u8(\?|$)/i.test(url)) return "HLS";
  if (m.startsWith("audio/")) return "AUDIO";
  const ext = (R ? R.extOf(url) : "") || contOf(m);
  return (ext || "file").toUpperCase().slice(0, 5);
};

const SVG = {
  down: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v11M7 10l5 5 5-5M5 20h14"/></svg>',
  dots: '<svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><circle cx="12" cy="5" r="1.8"/><circle cx="12" cy="12" r="1.8"/><circle cx="12" cy="19" r="1.8"/></svg>',
  play: '<svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>',
};

let openMenu = null;
document.addEventListener("click", () => { if (openMenu) { openMenu.remove(); openMenu = null; } });

// item: { title, thumb, dur, tag, link, options: [{ label, size, approx, url, msg }] }
function card(item) {
  const el = document.createElement("div");
  el.className = "card";
  const th = document.createElement("div");
  th.className = "thumb";
  if (item.thumb && /^https?:\/\//i.test(item.thumb)) {
    const im = document.createElement("img");
    im.src = item.thumb; im.loading = "lazy"; im.alt = "";
    im.onerror = () => { im.remove(); };
    th.appendChild(im);
  } else {
    const ph = document.createElement("div");
    ph.className = "ph"; ph.innerHTML = SVG.play;
    th.appendChild(ph);
  }
  const d = fmtDur(item.dur);
  if (d) { const s = document.createElement("span"); s.className = "dur"; s.textContent = d; th.appendChild(s); }

  const body = document.createElement("div");
  body.className = "body";
  const title = document.createElement("div");
  title.className = "title"; title.textContent = item.title || "Video"; title.title = item.title || "";
  const meta = document.createElement("div");
  meta.className = "meta";
  const tag = document.createElement("span");
  tag.className = "tag"; tag.textContent = item.tag || "VIDEO";
  const size = document.createElement("span");
  const queued = document.createElement("span");
  queued.className = "queued";
  meta.append(tag, size, queued);

  const row = document.createElement("div");
  row.className = "row";
  const sel = document.createElement("select");
  item.options.forEach((o, i) => {
    const op = document.createElement("option");
    op.value = String(i);
    op.textContent = o.label;
    sel.appendChild(op);
  });
  if (item.options.length < 2) sel.disabled = item.options.length === 0;
  const showSize = () => {
    const o = item.options[Number(sel.value)] || {};
    size.textContent = o.size > 0 ? (o.approx ? "~" : "") + fmtSize(o.size) : "";
    queued.textContent = o.queued ? "✓ Queued" : "";
  };
  sel.onchange = showSize;
  item.refresh = showSize;
  const dl = document.createElement("button");
  dl.className = "dl"; dl.innerHTML = SVG.down + "<span>Download</span>";
  dl.title = "Download with Download Craft";
  dl.onclick = () => { const o = item.options[Number(sel.value)]; if (o) send(o.msg); };
  const more = document.createElement("button");
  more.className = "more"; more.innerHTML = SVG.dots; more.title = "More";
  more.onclick = (ev) => {
    ev.stopPropagation();
    if (openMenu) { openMenu.remove(); openMenu = null; return; }
    const o = item.options[Number(sel.value)] || {};
    const link = o.url || item.link;
    const m = document.createElement("div");
    m.className = "menu";
    const add = (label, fn) => { const b = document.createElement("button"); b.textContent = label; b.onclick = (e) => { e.stopPropagation(); fn(); m.remove(); openMenu = null; }; m.appendChild(b); };
    if (link) add("Copy link", async () => { try { await navigator.clipboard.writeText(link); status("Link copied", "ok"); } catch (e) { status("Could not copy", "bad"); } });
    if (link) add("Open link in new tab", () => api.tabs.create({ url: link }));
    if (item.page) add("Send page link instead", () => send({ type: "download", url: item.page, pageUrl: item.page }));
    more.appendChild(m);
    openMenu = m;
  };
  row.append(sel, dl, more);
  body.append(title, meta, row);
  el.append(th, body);
  showSize();
  return el;
}

// A YouTube video from the page's player data: every quality in one select.
function youtubeItem(v, pageUrl) {
  const page = v.pageUrl || pageUrl;
  const stem = safeName(v.title || "youtube_video").replace(/\.mp4$/i, "");
  const best = (arr) => (arr || []).slice().sort((a, b) => {
    // By bitrate (`br`): YouTube's audio labels are "MEDIUM"/"LOW".
    const bn = (x) => Number(x.br) > 0 ? Number(x.br) : (parseInt((String(x.label || "").match(/(\d+)k/) || [0, 0])[1], 10) * 1000 || 0);
    return bn(b) - bn(a);
  })[0] || null;
  const bestA = best(v.audio);
  const opts = [];
  const height = (l) => parseInt(String(l || "").match(/^(\d+)/) ? String(l).match(/^(\d+)/)[1] : "0", 10) || 0;
  const pushStream = (st, kind) => {
    const pair = kind === "v" && bestA && bestA.url && bestA.url !== st.url ? { url: bestA.url, mime: bestA.mime } : null;
    const ext = /webm/i.test(st.mime || "") ? ".webm" : (kind === "a" ? ".m4a" : ".mp4");
    const name = kind === "a" ? `${stem} [audio]${ext}` : kind === "v" || st.ciphered ? `${stem} [${st.label}]${ext}` : `${stem}${ext}`;
    const label = kind === "a"
      ? `Audio · ${contOf(st.mime)}${st.br ? ` · ${Math.round(st.br / 1000)}k` : ""}`
      : `${st.label} · ${(st.disp || contOf(st.mime)).toUpperCase()}${kind === "v" ? " + audio" : ""}`;
    const exact = st._size > 0 ? st._size + (pair && bestA._size > 0 ? bestA._size : 0) : 0;
    opts.push({
      h: kind === "a" ? -1 : height(st.label),
      label,
      size: exact || st._est || 0,
      approx: !exact && st._est > 0,
      url: st.url || null,
      msg: st.ciphered && !st.url
        ? { type: "download", url: page, filename: name, pageUrl: page, quality: st.label, size: st._size || null, dur: st._dur || null, pairUrl: pair && pair.url, pairMime: pair && pair.mime }
        : { type: "download", url: st.url, filename: name, pageUrl: page, quality: st.label, size: st._size || null, dur: st._dur || null, pairUrl: pair && pair.url, pairMime: pair && pair.mime },
    });
  };
  (v.streams || []).forEach((st) => pushStream(st, "av"));
  (v.videoOnly || []).forEach((st) => pushStream(st, "v"));
  // The app resolves every standard quality itself (sizes from page JSON).
  const have = new Set(opts.map((o) => o.h));
  const sizes = v.sizes || {};
  const dur = Math.max(0, ...Object.values(sizes).map((m) => (m && m.dur) || 0));
  for (const q of [2160, 1440, 1080, 720, 480, 360, 240, 144]) {
    if (have.has(q)) continue;
    const key = Object.keys(sizes).find((k) => k.startsWith(`${q}p`));
    if (!key && q > 1080) continue;
    const sz = key ? sizes[key].size || 0 : 0;
    opts.push({ h: q, label: `${q}p · MP4`, size: sz, url: null,
      msg: { type: "download", url: page, filename: `${stem} [${q}p].mp4`, pageUrl: page, quality: `${q}p`, size: sz || null, dur: dur || null } });
  }
  (v.audio || []).forEach((st) => pushStream(st, "a"));
  opts.sort((a, b) => b.h - a.h);
  const firstDur = (v.streams || []).concat(v.videoOnly || []).map((s) => s._dur).find((x) => x > 0) || dur;
  return { title: v.title || v.videoId, thumb: v.thumb, dur: firstDur, tag: "YOUTUBE", link: page, page, options: opts };
}

function render(peek, pageUrl) {
  const box = $("videos");
  box.textContent = "";
  const sections = { streams: [], videos: [], other: [] };
  for (const v of (peek.videos || [])) sections.videos.push(youtubeItem(v, pageUrl));

  const net = peek.net || [];
  const have = new Set((peek.links || []).map((l) => l.url));
  const hasPlaylist = net.some((n) => n && R && R.kindOf(n) === "playlist");
  const seen = new Set();
  const pageTitle = peek.title || "";
  const netItems = net
    .filter((n) => n && /^https?:\/\//i.test(n.url || "") && !have.has(n.url))
    .filter((n) => { const k = R ? R.canonicalUrl(n.url) : n.url; if (seen.has(k)) return false; seen.add(k); return true; })
    .map((n) => ({ ...n, _score: R ? R.scoreMedia(n, { hasPlaylist }) : 0 }))
    .filter((n) => n._score >= 0)
    .sort((a, b) => b._score - a._score);
  for (const n of netItems) {
    const kind = R ? R.kindOf(n) : "file";
    const isStream = kind === "playlist";
    const isSub = kind === "subtitle";
    const tag = isSub ? "SUBS" : kind === "segment" ? "PART" : tagOf(n.url, n.mime);
    const name = isStream || isSub ? undefined : safeName(pageTitle || "video", /^audio\//i.test(n.mime || "") ? ".m4a" : ".mp4");
    const item = {
      title: pageTitle || "Video",
      thumb: n.thumb, dur: n.dur, tag, link: n.url, page: pageUrl,
      options: [{
        label: isStream ? (n.master ? "Best quality" : "Stream") : isSub ? "Subtitles" : `${tag}${n.pairUrl ? " + audio" : ""}`,
        size: n.size || 0, url: n.url,
        msg: { type: "download", url: n.url, filename: name, pageUrl: pageUrl || undefined, pairUrl: n.pairUrl || undefined, pairMime: n.pairMime || undefined, dur: n.dur || undefined },
      }],
    };
    (isStream ? sections.streams : isSub || kind === "segment" ? sections.other : sections.videos).push(item);
  }
  for (const l of (peek.links || [])) {
    if (!/^https?:\/\//i.test(l.url)) continue; // defense-in-depth: never offer odd schemes
    sections.other.push({
      title: l.text || l.url, thumb: l.thumb, tag: tagOf(l.url, ""), link: l.url, page: pageUrl,
      options: [{ label: "Download with Download Craft", url: l.url, msg: { type: "download", url: l.url, pageUrl: pageUrl || undefined } }],
    });
  }

  const total = sections.streams.length + sections.videos.length + sections.other.length;
  $("count").textContent = String(sections.streams.length + sections.videos.length);
  if (!total) {
    const e = document.createElement("div");
    e.className = "empty";
    e.innerHTML = "<b>No videos found yet</b>Play the video on this page, then open this again.";
    box.appendChild(e);
    return;
  }
  const all = [];
  for (const [key, label] of [["videos", "Videos"], ["streams", "Streams (HLS / DASH)"], ["other", "Other files"]]) {
    if (!sections[key].length) continue;
    const h = document.createElement("div");
    h.className = "sec";
    h.innerHTML = `<span>${label}</span><span class="n">${sections[key].length}</span>`;
    box.appendChild(h);
    for (const it of sections[key]) { box.appendChild(card(it)); all.push(it); }
  }
  enrich(all, pageUrl);
}

// Sizes for rows that have none (measured in the browser session) and
// "✓ Queued" for rows already in the app.
function enrich(items, pageUrl) {
  const opts = items.flatMap((it) => it.options.map((o) => ({ it, o })));
  const urls = [...new Set(opts.map(({ o }) => o.url).filter(Boolean))];
  if (urls.length) {
    chrome.runtime.sendMessage({ type: "isQueued", urls: urls.slice(0, 25) }, (res) => {
      if (chrome.runtime.lastError) return;
      const hit = new Set((res && res.queued) || []);
      for (const { it, o } of opts) if (o.url && hit.has(o.url)) { o.queued = true; it.refresh && it.refresh(); }
    });
  }
  const need = [...new Set(opts.filter(({ o }) => o.url && !(o.size > 0)).map(({ o }) => o.url))].slice(0, 12);
  if (need.length) {
    chrome.runtime.sendMessage({ type: "getSizes", urls: need, referer: pageUrl }, (res) => {
      if (chrome.runtime.lastError) return;
      const sizes = (res && res.sizes) || {};
      for (const { it, o } of opts) {
        if (o.url && sizes[o.url] > 0 && !(o.size > 0)) { o.size = sizes[o.url]; o.approx = false; it.refresh && it.refresh(); }
      }
    });
  }
}

// Per-tab state: ask the ACTIVE tab's content script first (always fresh,
// never another tab's leftovers); fall back to the stored snapshot.
async function activeTab() {
  try {
    const tabs = await api.tabs.query({ active: true, currentWindow: true });
    return (tabs && tabs[0]) || null;
  } catch (e) {
    return null;
  }
}

(async () => {
  const tab = await activeTab();
  const tabUrl = (tab && tab.url && /^https?:\/\//i.test(tab.url)) ? tab.url : null;
  if (tab && tab.id !== undefined) {
    try {
      // The top frame only: every iframe also answers (ad/account frames
      // replied first with an empty or foreign video list).
      const peek = await api.tabs.sendMessage(tab.id, { type: "peekState" }, { frameId: 0 });
      if (peek && (peek.videos || peek.links || peek.net)) {
        render(peek, peek.page || tabUrl);
        return;
      }
    } catch (e) {
      /* content unreachable (no script on this page): fall through */
    }
  }
  // Fallback: last stored snapshot, filtered to this tab's page when known.
  chrome.storage.local.get(
    ["downloadCraftVideos", "downloadCraftLinks", "downloadCraftNet", "downloadCraftPage"],
    ({ downloadCraftVideos, downloadCraftLinks, downloadCraftNet, downloadCraftPage }) => {
      const samePage = (p) => !tabUrl || !p || p.split("#")[0] === tabUrl.split("#")[0];
      const ok = samePage(downloadCraftPage);
      render({
        videos: (downloadCraftVideos || []).filter((v) => samePage(v.pageUrl || downloadCraftPage)),
        links: ok ? (downloadCraftLinks || []) : [],
        net: ok ? (downloadCraftNet || []) : [],
        title: tab && tab.title,
      }, downloadCraftPage || tabUrl);
    }
  );
})();
