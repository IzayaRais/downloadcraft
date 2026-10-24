// DCStreams: read HLS playlists and DASH manifests into plain lists of
// files to fetch, for "Download in browser" (no app). Pure: no DOM, fetch or
// chrome.* (the MPD reader takes any DOMParser-like parser, so it also runs
// under node with a tiny XML reader in the tests).
(function (g) {
  "use strict";

  const abs = (u, base) => { try { return new URL(u, base).href; } catch (e) { return ""; } };

  // `KEY=VALUE,KEY="quoted, value"` attribute lists of HLS tags.
  function attrs(s) {
    const out = {};
    const re = /([A-Z0-9-]+)=("[^"]*"|[^,]*)/g;
    for (let m; (m = re.exec(s));) out[m[1]] = m[2].replace(/^"|"$/g, "");
    return out;
  }

  // HLS playlist -> master (variants + audio renditions) or media (segments).
  function parseM3u8(text, baseUrl) {
    const lines = String(text || "").split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
    if (lines[0] !== "#EXTM3U") throw new Error("not an HLS playlist");
    const variants = [];
    const audios = [];
    const segments = [];
    let key = null;
    let map = null;
    let seq = 0;
    let pendingVariant = null;
    let dur = 0;
    let segDur = 0;
    let range = null;
    let lastRangeEnd = 0;
    let live = true;
    let encrypted = false;
    for (const l of lines) {
      if (l.startsWith("#EXT-X-STREAM-INF:")) {
        const a = attrs(l.slice(18));
        const [w, h] = (a.RESOLUTION || "").split("x").map(Number);
        pendingVariant = { bw: Number(a.BANDWIDTH) || 0, w: w || 0, h: h || 0, audio: a.AUDIO || "", codecs: a.CODECS || "" };
      } else if (l.startsWith("#EXT-X-MEDIA:")) {
        const a = attrs(l.slice(13));
        if (a.TYPE === "AUDIO" && a.URI) {
          audios.push({ group: a["GROUP-ID"] || "", url: abs(a.URI, baseUrl), lang: a.LANGUAGE || "", name: a.NAME || "", default: a.DEFAULT === "YES" });
        }
      } else if (l.startsWith("#EXT-X-MEDIA-SEQUENCE:")) {
        seq = parseInt(l.slice(22), 10) || 0;
      } else if (l.startsWith("#EXT-X-KEY:")) {
        const a = attrs(l.slice(11));
        if (!a.METHOD || a.METHOD === "NONE") key = null;
        else {
          encrypted = true;
          key = { method: a.METHOD, url: a.URI ? abs(a.URI, baseUrl) : "", iv: a.IV || "", format: a.KEYFORMAT || "identity" };
        }
      } else if (l.startsWith("#EXT-X-MAP:")) {
        const a = attrs(l.slice(11));
        map = a.URI ? { url: abs(a.URI, baseUrl), range: a.BYTERANGE ? byteRange(a.BYTERANGE, 0) : null } : null;
      } else if (l.startsWith("#EXTINF:")) {
        segDur = parseFloat(l.slice(8)) || 0;
      } else if (l.startsWith("#EXT-X-BYTERANGE:")) {
        range = byteRange(l.slice(17), lastRangeEnd);
      } else if (l === "#EXT-X-ENDLIST") {
        live = false;
      } else if (!l.startsWith("#")) {
        const url = abs(l, baseUrl);
        if (!url) continue;
        if (pendingVariant) {
          variants.push({ ...pendingVariant, url });
          pendingVariant = null;
        } else {
          segments.push({ url, seq, key, map, range, dur: segDur });
          if (range) lastRangeEnd = range.end + 1;
          dur += segDur;
          seq++;
          range = null;
        }
      }
    }
    if (variants.length) return { master: true, variants, audios };
    return { master: false, segments, live, duration: dur, encrypted,
      // SAMPLE-AES and key systems other than plain AES-128 are DRM.
      protected: segments.some((s) => s.key && (s.key.method !== "AES-128" || s.key.format !== "identity")) };
  }

  function byteRange(s, prevEnd) {
    const [len, off] = String(s).split("@").map(Number);
    const start = Number.isFinite(off) ? off : prevEnd;
    return { start, end: start + len - 1 };
  }

  // Best variant at or under a "720p"-style cap (highest overall without one).
  function pickVariant(variants, quality) {
    const m = String(quality || "").match(/(\d{3,4})/);
    const cap = m ? Number(m[1]) : 0;
    const h = (v) => Math.min(v.w || 0, v.h || 0) || v.h || 0;
    const sorted = (variants || []).slice().sort((a, b) => (h(b) - h(a)) || (b.bw - a.bw));
    return (cap && sorted.find((v) => h(v) && h(v) <= cap)) || sorted[0] || null;
  }

  // ISO 8601 duration ("PT1H2M3.5S") to seconds.
  function isoDur(s) {
    const m = String(s || "").match(/P(?:(\d+)D)?T?(?:(\d+)H)?(?:(\d+)M)?(?:([\d.]+)S)?/);
    if (!m) return 0;
    return (Number(m[1]) || 0) * 86400 + (Number(m[2]) || 0) * 3600 + (Number(m[3]) || 0) * 60 + (Number(m[4]) || 0);
  }

  // DASH MPD -> {duration, video:[rep], audio:[rep], protected}. A rep lists
  // the files to fetch in order: {init, segments:[url]} or one whole file.
  function parseMpd(text, baseUrl, parser) {
    const doc = parser.parseFromString(String(text || ""), "application/xml");
    const mpd = doc.getElementsByTagName("MPD")[0];
    if (!mpd) throw new Error("not a DASH manifest");
    if (mpd.getAttribute("type") === "dynamic") throw new Error("live DASH streams are not supported in the browser");
    const duration = isoDur(mpd.getAttribute("mediaPresentationDuration"));
    const kids = (el, tag) => Array.from(el.childNodes || []).filter((n) => n.nodeName === tag || n.localName === tag);
    const baseOf = (el, base) => {
      const b = kids(el, "BaseURL")[0];
      return b && b.textContent.trim() ? abs(b.textContent.trim(), base) : base;
    };
    const period = kids(mpd, "Period")[0];
    if (!period) throw new Error("DASH manifest has no period");
    let base = baseOf(period, baseOf(mpd, baseUrl));
    const out = { duration, video: [], audio: [], protected: false };
    for (const set of kids(period, "AdaptationSet")) {
      const setBase = baseOf(set, base);
      const setMime = set.getAttribute("mimeType") || set.getAttribute("contentType") || "";
      const setTemplate = kids(set, "SegmentTemplate")[0] || null;
      const setProtected = kids(set, "ContentProtection").length > 0;
      for (const rep of kids(set, "Representation")) {
        const mime = rep.getAttribute("mimeType") || setMime;
        const isAudio = /audio/i.test(mime) || /audio/i.test(set.getAttribute("contentType") || "");
        const isVideo = /video/i.test(mime) || /video/i.test(set.getAttribute("contentType") || "");
        if (!isAudio && !isVideo) continue;
        const r = {
          id: rep.getAttribute("id") || "",
          bw: Number(rep.getAttribute("bandwidth")) || 0,
          w: Number(rep.getAttribute("width") || set.getAttribute("width")) || 0,
          h: Number(rep.getAttribute("height") || set.getAttribute("height")) || 0,
          mime: mime || (isAudio ? "audio/mp4" : "video/mp4"),
          codecs: rep.getAttribute("codecs") || set.getAttribute("codecs") || "",
          protected: setProtected || kids(rep, "ContentProtection").length > 0,
          init: "",
          segments: [],
        };
        if (r.protected) out.protected = true;
        const repBase = baseOf(rep, setBase);
        const tpl = kids(rep, "SegmentTemplate")[0] || setTemplate;
        const list = kids(rep, "SegmentList")[0] || kids(set, "SegmentList")[0];
        const fill = (s) => String(s || "")
          .replace(/\$RepresentationID\$/g, r.id)
          .replace(/\$Bandwidth\$/g, String(r.bw));
        if (tpl) {
          const media = tpl.getAttribute("media") || "";
          const timescale = Number(tpl.getAttribute("timescale")) || 1;
          const start = Number(tpl.getAttribute("startNumber") || "1");
          if (tpl.getAttribute("initialization")) r.init = abs(fill(tpl.getAttribute("initialization")), repBase);
          const num = (s, n, t) => s
            .replace(/\$Number(?:%0(\d+)d)?\$/g, (_, w) => (w ? String(n).padStart(Number(w), "0") : String(n)))
            .replace(/\$Time(?:%0(\d+)d)?\$/g, (_, w) => (w ? String(t).padStart(Number(w), "0") : String(t)));
          const timeline = kids(tpl, "SegmentTimeline")[0];
          if (timeline) {
            let t = 0;
            let n = start;
            for (const s of kids(timeline, "S")) {
              if (s.getAttribute("t") != null) t = Number(s.getAttribute("t"));
              const d = Number(s.getAttribute("d")) || 0;
              const repeat = Number(s.getAttribute("r") || "0");
              for (let k = 0; k <= (repeat < 0 ? 0 : repeat); k++) {
                r.segments.push(abs(num(fill(media), n, t), repBase));
                t += d;
                n++;
              }
            }
          } else {
            const d = Number(tpl.getAttribute("duration")) || 0;
            const count = d > 0 && duration > 0 ? Math.ceil((duration * timescale) / d) : 0;
            for (let k = 0; k < count; k++) r.segments.push(abs(num(fill(media), start + k, k * d), repBase));
          }
        } else if (list) {
          const ini = kids(list, "Initialization")[0];
          if (ini && ini.getAttribute("sourceURL")) r.init = abs(ini.getAttribute("sourceURL"), repBase);
          for (const s of kids(list, "SegmentURL")) {
            if (s.getAttribute("media")) r.segments.push(abs(s.getAttribute("media"), repBase));
          }
        } else {
          // SegmentBase / bare BaseURL: the representation is one file.
          r.segments.push(repBase);
        }
        (isAudio ? out.audio : out.video).push(r);
      }
    }
    return out;
  }

  // AES-128 IV for a segment: the tag's IV, else the media sequence number.
  function ivFor(seg) {
    const iv = new Uint8Array(16);
    const hex = String((seg.key && seg.key.iv) || "").replace(/^0x/i, "");
    if (hex) {
      const h = hex.padStart(32, "0");
      for (let i = 0; i < 16; i++) iv[i] = parseInt(h.substr(i * 2, 2), 16);
    } else {
      let n = seg.seq;
      for (let i = 15; i >= 12 && n > 0; i--) { iv[i] = n & 255; n = Math.floor(n / 256); }
    }
    return iv;
  }

  const DCStreams = { parseM3u8, pickVariant, parseMpd, isoDur, ivFor };
  g.DCStreams = DCStreams;
  if (typeof module !== "undefined" && module.exports) module.exports = DCStreams;
})(typeof globalThis !== "undefined" ? globalThis : this);
