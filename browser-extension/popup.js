const $ = (id) => document.getElementById(id);

chrome.runtime.sendMessage({ type: "getState" }, (res) => {
  if (chrome.runtime.lastError || !res) {
    $("status").textContent = "Extension background unreachable — reload the extension.";
    return;
  }
  $("auto").checked = !!res.autoCapture;
  if (!res.hostOk) {
    $("status").textContent = "App not connected — open Download Craft once (Install Chrome host)." +
      (res.hostError ? " [" + res.hostError + "]" : "");
  }
});

$("auto").onchange = () => {
  chrome.runtime.sendMessage({ type: "setAuto", value: $("auto").checked });
};

$("send").onclick = () => {
  const url = $("url").value.trim();
  if (!url) return;
  chrome.runtime.sendMessage({ type: "download", url }, (res) => {
    const ok = res && res.ok;
    $("status").textContent = ok ? "Sent to Download Craft ✓" : "App not reachable — open Download Craft once.";
  });
};

chrome.storage.local.get(["downloadCraftVideos", "downloadCraftLinks"], ({ downloadCraftVideos, downloadCraftLinks }) => {
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
    const qbtn = (st, fname) => {
      const btn = document.createElement("button");
      btn.style.marginRight = "4px";
      btn.style.marginBottom = "4px";
      btn.textContent = `${st.label} · ${String(st.mime || "").replace("video/", "")}${st.kind === "v" ? " (no audio)" : st.kind === "a" ? " (audio)" : ""}`;
      btn.onclick = () => {
        chrome.runtime.sendMessage({ type: "download", url: st.url, filename: fname }, (res) => {
          $("status").textContent = res && res.ok ? "Sent ✓ — confirm in Download Craft" : "App not reachable.";
        });
      };
      wrap.appendChild(btn);
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
  });
  const list = $("list");
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
