const $ = (id) => document.getElementById(id);

chrome.runtime.sendMessage({ type: "getState" }, (res) => {
  if (chrome.runtime.lastError || !res) {
    $("status").textContent = "Extension background unreachable — reload the extension.";
    return;
  }
  $("auto").checked = !!res.autoCapture;
  if (res.minSizeKB !== undefined) $("minSize").value = String(res.minSizeKB);
  if (typeof res.blockHosts === "string") $("block").value = res.blockHosts;
  if (!res.hostOk) {
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
    $("status").textContent = res && res.ok ? "Filters saved ✓" : "Could not save filters.";
  });
};

$("send").onclick = () => {
  const url = $("url").value.trim();
  if (!url) return;
  chrome.runtime.sendMessage({ type: "download", url }, (res) => {
    const ok = res && res.ok;
    $("status").textContent = ok ? "Sent to Download Craft ✓" : "App not reachable — open Download Craft once.";
  });
};

chrome.storage.local.get(["downloadCraftVideos", "downloadCraftLinks", "downloadCraftNet", "downloadCraftPage"], ({ downloadCraftVideos, downloadCraftLinks, downloadCraftNet, downloadCraftPage }) => {
  const box = $("videos");
  (downloadCraftVideos || []).forEach((v) => {
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
      const url = v.pageUrl || location.href;
      chrome.runtime.sendMessage({ type: "download", url, filename: fname }, (res) => {
        $("status").textContent = res && res.ok ? "Sent ✓ — confirm in Download Craft" : "App not reachable.";
      });
    };
    wrap.appendChild(linkBtn);
    const fmtSize = (n) => {
      if (!n || n <= 0) return "";
      const u = ["B", "KB", "MB", "GB"];
      let v = n, i = 0;
      while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
      return i === 0 ? `${n} ${u[i]}` : `${v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
    };
    const dispOf = (st) => st.disp || String(st.mime || "").replace("video/", "").split(";")[0];
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
        chrome.runtime.sendMessage({ type: "download", url: st.url, filename: fname, size: st._size || null, pairUrl: (pair && pair.url) || null, pairMime: (pair && pair.mime) || null }, (res) => {
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
        chrome.runtime.sendMessage({ type: "getSizes", urls }, (res) => {
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
  const list = $("list");
  const net = (downloadCraftNet || []).filter((n) =>
    !((downloadCraftLinks || []).some((l) => l.url === n.url)));
  net.forEach((n) => {
    const div = document.createElement("div");
    div.className = "item";
    const a = document.createElement("a");
    a.href = n.url; a.textContent = `Stream (${n.mime || "media"})`; a.target = "_blank";
    const btn = document.createElement("button");
    btn.textContent = /m3u8|mpd|mpegurl|dash/i.test(n.url)
      ? "Send stream (needs app resolve)"
      : "Download stream";
    btn.onclick = () => {
      chrome.runtime.sendMessage({ type: "download", url: n.url, pageUrl: downloadCraftPage || undefined }, (res) => {
        $("status").textContent = res && res.ok ? "Sent ✓" : "App not reachable.";
      });
    };
    div.append(a, document.createElement("br"), btn);
    list.append(div);
  });
  (downloadCraftLinks || []).forEach((l) => {
    const div = document.createElement("div");
    div.className = "item";
    const a = document.createElement("a");
    a.href = l.url; a.textContent = l.text || l.url; a.target = "_blank";
    const btn = document.createElement("button");
    btn.textContent = "Download with Download Craft";
    btn.onclick = () => {
      chrome.runtime.sendMessage({ type: "download", url: l.url }, (res) => {
        $("status").textContent = res && res.ok ? "Sent ✓" : "App not reachable.";
      });
    };
    div.append(a, document.createElement("br"), btn);
    list.append(div);
  });
});
