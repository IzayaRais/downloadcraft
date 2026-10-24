// DCBrowserDL: download a video entirely in the browser (no app), the way
// MAX does without its helper: a video + audio pair is fetched and joined
// (DCMp4), an HLS or DASH stream is fetched piece by piece (AES-128 HLS
// decrypted with WebCrypto) and joined. Runs where fetch, crypto.subtle and
// Blob exist: Chrome's offscreen document, Firefox's background page.
// Plain single files never come here - the browser's own downloader takes
// those (resumable, shown in the browser's download list).
(function (g) {
  "use strict";

  const PARALLEL = 6;
  const MAX_BYTES = 4 * 1024 * 1024 * 1024;

  function concat(parts) {
    const n = parts.reduce((s, p) => s + p.length, 0);
    if (n > MAX_BYTES) throw new Error("over 4 GB — too big to assemble in the browser; use the app");
    const out = new Uint8Array(n);
    let at = 0;
    for (const p of parts) { out.set(p, at); at += p.length; }
    return out;
  }

  // GET with live byte progress. `range` = {start, end} for byte-range pieces.
  async function fetchBytes(url, { signal, range, onBytes, retries = 3 } = {}) {
    let lastErr = null;
    for (let attempt = 0; attempt <= retries; attempt++) {
      let got = 0;
      try {
        const headers = range ? { Range: `bytes=${range.start}-${range.end}` } : undefined;
        const res = await fetch(url, { signal, headers, credentials: "include", cache: "no-store" });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        if (!res.body || !res.body.getReader) {
          const b = new Uint8Array(await res.arrayBuffer());
          got = b.length;
          if (onBytes) onBytes(b.length);
          return b;
        }
        const reader = res.body.getReader();
        const chunks = [];
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          chunks.push(value);
          got += value.length;
          if (onBytes) onBytes(value.length);
        }
        const want = Number(res.headers.get("content-length") || 0);
        if (want && got < want && !res.headers.get("content-encoding")) throw new Error(`connection ended early (${got} of ${want} bytes)`);
        return concat(chunks);
      } catch (e) {
        if (signal && signal.aborted) throw new Error("cancelled");
        lastErr = e;
        // This attempt's bytes are thrown away: take them off the progress.
        if (onBytes && got) onBytes(-got);
        await new Promise((r) => setTimeout(r, 600 * (attempt + 1)));
      }
    }
    throw new Error(`${String((lastErr && lastErr.message) || lastErr)} — ${url.slice(0, 80)}`);
  }

  async function fetchText(url, signal) {
    const b = await fetchBytes(url, { signal });
    return new TextDecoder().decode(b);
  }

  // Run `fn` over `items` with a fixed number in flight; results in order.
  async function pool(items, n, fn) {
    const out = new Array(items.length);
    let next = 0;
    const worker = async () => {
      while (next < items.length) {
        const i = next++;
        out[i] = await fn(items[i], i);
      }
    };
    await Promise.all(Array.from({ length: Math.min(n, items.length) }, worker));
    return out;
  }

  // Progress: bytes so far and pieces done/total, reported at most ~4x/s.
  function tracker(onProgress) {
    const st = { bytes: 0, done: 0, total: 0, phase: "Downloading" };
    let last = 0;
    const emit = (force) => {
      const now = Date.now();
      if (!force && now - last < 250) return;
      last = now;
      try { onProgress && onProgress({ ...st }); } catch (e) { /* ignore */ }
    };
    return {
      st,
      bytes: (n) => { st.bytes += n; emit(false); },
      piece: () => { st.done++; emit(false); },
      add: (n) => { st.total += n; emit(true); },
      phase: (p) => { st.phase = p; emit(true); },
    };
  }

  async function decryptor(signal) {
    const keys = new Map();
    return async (seg, data) => {
      if (!seg.key) return data;
      if (seg.key.method !== "AES-128") throw new Error("this stream is DRM-protected and cannot be downloaded");
      let k = keys.get(seg.key.url);
      if (!k) {
        const raw = await fetchBytes(seg.key.url, { signal });
        k = await crypto.subtle.importKey("raw", raw.slice(0, 16), "AES-CBC", false, ["decrypt"]);
        keys.set(seg.key.url, k);
      }
      const plain = await crypto.subtle.decrypt({ name: "AES-CBC", iv: g.DCStreams.ivFor(seg) }, k, data);
      return new Uint8Array(plain);
    };
  }

  // One HLS media playlist -> {data, mp4}: fMP4 pieces get their init
  // segment in front (an .mp4), MPEG-TS pieces are joined as they are (.ts).
  async function hlsMedia(url, signal, t) {
    const pl = g.DCStreams.parseM3u8(await fetchText(url, signal), url);
    if (pl.master) throw new Error("expected a media playlist");
    if (pl.live) throw new Error("this is a live stream — record it with the app");
    if (pl.protected) throw new Error("this stream is DRM-protected and cannot be downloaded");
    const decrypt = await decryptor(signal);
    t.add(pl.segments.length);
    const parts = await pool(pl.segments, PARALLEL, async (s) => {
      const b = await fetchBytes(s.url, { signal, range: s.range, onBytes: t.bytes });
      const plain = await decrypt(s, b);
      t.piece();
      return plain;
    });
    const map = pl.segments[0] && pl.segments[0].map;
    if (map) {
      const init = await fetchBytes(map.url, { signal, range: map.range });
      return { data: concat([init, ...parts]), mp4: true };
    }
    return { data: concat(parts), mp4: g.DCMp4.isMp4(parts[0] || new Uint8Array()) };
  }

  async function runHls(job, signal, t) {
    const top = g.DCStreams.parseM3u8(await fetchText(job.url, signal), job.url);
    if (!top.master) {
      const one = await hlsMedia(job.url, signal, t);
      return { data: one.data, ext: one.mp4 ? ".mp4" : ".ts" };
    }
    const v = g.DCStreams.pickVariant(top.variants, job.quality);
    if (!v) throw new Error("no playable quality in this stream");
    const group = top.audios.filter((a) => a.group === v.audio);
    const audio = group.find((a) => a.default) || group[0] || null;
    const [video, sound] = await Promise.all([
      hlsMedia(v.url, signal, t),
      audio && audio.url !== v.url ? hlsMedia(audio.url, signal, t) : Promise.resolve(null),
    ]);
    if (sound && video.mp4 && sound.mp4) {
      t.phase("Joining video and audio");
      return { data: g.DCMp4.merge(video.data, sound.data), ext: ".mp4" };
    }
    if (sound && !video.mp4) {
      // Separate MPEG-TS audio cannot be joined here: keep the picture.
      return { data: video.data, ext: ".ts", note: "audio track is separate in this stream — the app keeps both" };
    }
    return { data: video.data, ext: video.mp4 ? ".mp4" : ".ts" };
  }

  async function dashRep(rep, signal, t) {
    const files = (rep.init ? [rep.init] : []).concat(rep.segments);
    t.add(files.length);
    const parts = await pool(files, PARALLEL, async (u) => {
      const b = await fetchBytes(u, { signal, onBytes: t.bytes });
      t.piece();
      return b;
    });
    return concat(parts);
  }

  async function runDash(job, signal, t) {
    if (typeof DOMParser === "undefined") throw new Error("this browser cannot read DASH manifests here");
    const m = g.DCStreams.parseMpd(await fetchText(job.url, signal), job.url, new DOMParser());
    if (m.protected) throw new Error("this stream is DRM-protected and cannot be downloaded");
    const mp4 = (r) => /mp4/i.test(r.mime);
    const vids = m.video.filter(mp4).length ? m.video.filter(mp4) : m.video;
    const v = g.DCStreams.pickVariant(vids, job.quality);
    const auds = m.audio.filter(mp4).length ? m.audio.filter(mp4) : m.audio;
    const a = auds.slice().sort((x, y) => y.bw - x.bw)[0] || null;
    if (!v && !a) throw new Error("no playable track in this stream");
    const [vd, ad] = await Promise.all([v ? dashRep(v, signal, t) : null, a ? dashRep(a, signal, t) : null]);
    if (vd && ad) {
      t.phase("Joining video and audio");
      return { data: g.DCMp4.merge(vd, ad), ext: ".mp4" };
    }
    return { data: vd || ad, ext: vd ? ".mp4" : ".m4a" };
  }

  async function runPair(job, signal, t) {
    t.add(2);
    const [v, a] = await Promise.all([
      fetchBytes(job.url, { signal, onBytes: t.bytes }).then((b) => { t.piece(); return b; }),
      fetchBytes(job.pairUrl, { signal, onBytes: t.bytes }).then((b) => { t.piece(); return b; }),
    ]);
    t.phase("Joining video and audio");
    return { data: g.DCMp4.merge(v, a), ext: ".mp4" };
  }

  // job: {kind: "pair"|"hls"|"dash", url, pairUrl?, quality?}.
  // Resolves {blob, ext, note?}; rejects with a readable reason.
  async function run(job, onProgress, signal) {
    const t = tracker(onProgress);
    const out = job.kind === "pair" ? await runPair(job, signal, t)
      : job.kind === "hls" ? await runHls(job, signal, t)
      : job.kind === "dash" ? await runDash(job, signal, t)
      : null;
    if (!out) throw new Error(`unknown download kind "${job.kind}"`);
    t.phase("Saving");
    const type = out.ext === ".ts" ? "video/mp2t" : out.ext === ".m4a" ? "audio/mp4" : "video/mp4";
    return { blob: new Blob([out.data], { type }), ext: out.ext, note: out.note || "", bytes: out.data.length };
  }

  // Which runner a URL needs (the popup/background decide "file" otherwise).
  function kindFor(url, mime, pairUrl) {
    const u = String(url || "").toLowerCase().split("#")[0];
    const m = String(mime || "").toLowerCase();
    if (/\.m3u8(\?|$)/.test(u) || /mpegurl/.test(m)) return "hls";
    if (/\.mpd(\?|$)/.test(u) || /dash\+xml/.test(m)) return "dash";
    if (pairUrl) return "pair";
    return "file";
  }

  const DCBrowserDL = { run, kindFor, pool, concat };
  g.DCBrowserDL = DCBrowserDL;
  if (typeof module !== "undefined" && module.exports) module.exports = DCBrowserDL;
})(typeof globalThis !== "undefined" ? globalThis : this);
