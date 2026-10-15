const $ = (id) => document.getElementById(id);

chrome.runtime.sendMessage({ type: "getState" }, (res) => {
  if (chrome.runtime.lastError || !res) {
    $("status").textContent = "Extension background unreachable — reload the extension.";
    return;
  }
  $("auto").checked = !!res.autoCapture;
  if (res.minSizeKB !== undefined) $("minSize").value = String(res.minSizeKB);
  if (typeof res.blockHosts === "string") $("block").value = res.blockHosts;
  if (res.fallbackError) {
    $("status").textContent = "Browser re-download failed: " + res.fallbackError;
  } else if (!res.hostOk) {
    $("status").textContent = "App not connected — open Download Craft once (Install Chrome host)." +
      (res.hostError ? " [" + res.hostError + "]" : "");
  }
});

$("auto").onchange = () => {
  chrome.runtime.sendMessage({ type: "setAuto", value: $("auto").checked });
};

$("saveFilters").onclick = () => {
  chrome.runtime.sendMessage({
    type: "setFilters",
    minSizeKB: $("minSize").value.trim(),
    blockHosts: $("block").value.trim(),
  }, (res) => {
    if (chrome.runtime.lastError || !res) {
      $("status").textContent = "Extension background unreachable.";
    } else {
      $("status").textContent = res.ok ? "Filters saved ✓" : ("Could not save filters. " + (res.error || ""));
    }
  });
};

$("send").onclick = () => {
  const url = $("url").value.trim();
  if (!url) return;
  if (!/^https?:\/\//i.test(url) || url.length > 4000) {
    $("status").textContent = "Only http(s) links up to 4000 chars can be sent.";
    return;
  }
  chrome.runtime.sendMessage({ type: "download", url }, (res) => {
    if (chrome.runtime.lastError || !res) {
      $("status").textContent = "App not reachable — open Download Craft once.";
    } else {
      $("status").textContent = res.ok ? "Sent to Download Craft ✓" : ("App not reachable. " + (res.error || ""));
    }
  });
};

function fmtSize(n) {
  if (!n || n <= 0) return "";
  const u = ["B", "KB", "MB", "GB"];
  let v = n, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return i === 0 ? `${n} ${u[i]}` : `${v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
}
const dispOf = (st) => st.disp || String(st.mime || "").replace("video/", "").split(";")[0];

function renderVideos(box, videos, pageUrl) {
  (videos || []).forEach((v) => {
    const wrap = document.createElement("div");
    wrap.className = "item";
    const t = document.createElement("div");
    t.style.fontWeight = "600";
    t.style.marginBottom = "4px";
    t.textContent = "🎬 " + (v.title || v.videoId);
    wrap.appendChild(t);
    const linkBtn = document.createElement("button");
    linkBtn.style.marginRight = "4px";
    linkBtn.style.marginBottom = "4px";
    linkBtn.textContent = "Send video link to app";
    linkBtn.title = "Download Craft finds the best stream itself";
    linkBtn.onclick = () => {
      const name = String(v.title || "youtube_video").replace(/[<>:"/\\|?*]/g, "_");
      const fname = name.endsWith(".mp4") ? name : name + ".mp4";
      const url = v.pageUrl || pageUrl;
      if (!url || !/^https?:\/\//i.test(url)) {
        $("status").textContent = "No page URL for this video.";
        return;
      }
      chrome.runtime.sendMessage({ type: "download", url, filename: fname, pageUrl: url }, (res) => {
        $("status").textContent = res && res.ok ? "Sent ✓ — confirm in Download Craft" : "App not reachable.";
      });
    };
    wrap.appendChild(linkBtn);
    const sizeJobs = [];
    const bestAudio = (arr) => (arr || []).slice().sort((a, b) => {
      const bn = (x) => { const m = String(x.label || "").match(/(\d+)k/); return m ? parseInt(m[1], 10) : 0; };
      return bn(b) - bn(a);
    })[0] || null;
    const bestA = bestAudio(v.audio);
    const qbtn = (st, fname) => {
      const btn = document.createElement("button");
      btn.style.marginRight = "4px";
      btn.style.marginBottom = "4px";
      const base = `${st.label} · ${dispOf(st)}${st.kind === "v" ? " (no audio)" : st.kind === "a" ? " (audio)" : ""}`;
      btn.textContent = base;
      const pair = (st.kind === "v" && bestA && bestA.url !== st.url)
        ? { url: bestA.url, mime: bestA.mime }
        : null;
      btn.onclick = () => {
        chrome.runtime.sendMessage({ type: "download", url: st.url, filename: fname, pageUrl, size: st._size || null, pairUrl: (pair && pair.url) || null, pairMime: (pair && pair.mime) || null }, (res) => {
          $("status").textContent = res && res.ok ? "Sent ✓ — confirm in Download Craft" : "App not reachable.";
        });
      };
      wrap.appendChild(btn);
      sizeJobs.push({ btn, st, base });
    };
    const cleanTitle = String(v.title || "youtube_video").replace(/[<>:"/\\|?*]/g, "_");
    const base = cleanTitle.endsWith(".mp4") ? cleanTitle : cleanTitle + ".mp4";
    const tagged = (st) => {
      const stem = base.replace(/\.mp4$/i, "");
      const ext = String(st.mime || "").startsWith("audio/")
        ? (/webm/i.test(st.mime) ? ".webm" : ".m4a")
        : (/webm/i.test(st.mime) ? ".webm" : ".mp4");
      if (st.kind === "v") return `${stem} [${st.label}]${ext}`;
      if (st.kind === "a") return `${stem} [audio]${ext}`;
      return base;
    };
    (v.streams || []).forEach((st) => qbtn(st, tagged(st)));
    (v.videoOnly || []).forEach((st) => qbtn(st, tagged(st)));
    (v.audio || []).forEach((st) => qbtn(st, tagged(st)));
    box.appendChild(wrap);
    // IDM-style sizes (best-effort; googlevideo may 403 HEAD off-session).
    try {
      const urls = sizeJobs.map((j) => j.st.url).filter(Boolean).slice(0, 12);
      if (urls.length) {
        chrome.runtime.sendMessage({ type: "getSizes", urls, referer: pageUrl }, (res) => {
          if (chrome.runtime.lastError) return;
          const sizes = (res && res.sizes) || {};
          for (const j of sizeJobs) {
            const n = sizes[j.st.url] || 0;
            const s = fmtSize(n);
            if (n > 0) j.st._size = n;
            if (s && j.btn.isConnected) j.btn.textContent = `${j.base} (${s})`;
          }
        });
      }
    } catch (e) { /* ignore */ }
  });
}

function renderLinks(list, links, net, pageUrl) {
  const have = new Set((links || []).map((l) => l.url));
  (net || []).filter((n) => !have.has(n.url)).forEach((n) => {
    const div = document.createElement("div");
    div.className = "item";
    const a = document.createElement("a");
    a.href = n.url; a.textContent = `Stream (${n.mime || "media"})`; a.target = "_blank";
    const btn = document.createElement("button");
    btn.textContent = /m3u8|mpd|mpegurl|dash/i.test(n.url)
      ? "Send stream (needs app resolve)"
      : "Download stream";
    btn.onclick = () => {
      chrome.runtime.sendMessage({ type: "download", url: n.url, pageUrl: pageUrl || undefined }, (res) => {
        $("status").textContent = res && res.ok ? "Sent ✓" : "App not reachable.";
      });
    };
    div.append(a, document.createElement("br"), btn);
    list.append(div);
  });
  (links || []).forEach((l) => {
    if (!/^https?:\/\//i.test(l.url)) return; // defense-in-depth: never offer odd schemes
    const div = document.createElement("div");
    div.className = "item";
    const a = document.createElement("a");
    a.href = l.url; a.textContent = l.text || l.url; a.target = "_blank";
    const btn = document.createElement("button");
    btn.textContent = "Download with Download Craft";
    btn.onclick = () => {
      chrome.runtime.sendMessage({ type: "download", url: l.url, pageUrl: pageUrl || undefined }, (res) => {
        $("status").textContent = res && res.ok ? "Sent ✓" : "App not reachable.";
      });
    };
    div.append(a, document.createElement("br"), btn);
    list.append(div);
  });
}

// Per-tab state: ask the ACTIVE tab's content script first (always fresh,
// never another tab's leftovers); fall back to the stored snapshot.
async function activeTab() {
  try {
    const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
    return (tabs && tabs[0]) || null;
  } catch (e) {
    return null;
  }
}

(async () => {
  const box = $("videos");
  const list = $("list");
  const tab = await activeTab();
  const tabUrl = (tab && tab.url && /^https?:\/\//i.test(tab.url)) ? tab.url : null;
  if (tab && tab.id !== undefined) {
    try {
      const peek = await chrome.tabs.sendMessage(tab.id, { type: "peekState" });
      if (peek && (peek.videos || peek.links || peek.net)) {
        renderVideos(box, peek.videos, peek.page || tabUrl);
        renderLinks(list, peek.links, peek.net, peek.page || tabUrl);
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
      renderVideos(
        box,
        (downloadCraftVideos || []).filter((v) => samePage(v.pageUrl || downloadCraftPage)),
        downloadCraftPage || tabUrl
      );
      renderLinks(
        list,
        (downloadCraftLinks || []).filter(() => samePage(downloadCraftPage)),
        (downloadCraftNet || []).filter(() => samePage(downloadCraftPage)),
        downloadCraftPage || tabUrl
      );
    }
  );
})();
