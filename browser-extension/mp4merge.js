// DCMp4: join a video-only MP4 and an audio-only MP4 into one file, in the
// browser, without re-encoding - what "Download in browser" needs for the
// qualities sites only serve as separate video and audio (Facebook 720p+,
// YouTube HD). Both inputs are DASH representations: either fragmented
// (moov + mvex, then moof/mdat pairs - YouTube, Facebook) or plain (one
// mdat with stco/co64 chunk offsets). Output: one ftyp, one moov with both
// tracks (ids 1 and 2), then the media - fragments interleaved by time, or
// the two mdats back to back with chunk offsets moved.
// Pure: no DOM or chrome.* APIs, so it runs under `node --test` too.
(function (g) {
  "use strict";

  const CONTAINERS = new Set(["moov", "trak", "mdia", "minf", "stbl", "mvex", "moof", "traf", "edts", "dinf"]);

  const u32 = (b, i) => ((b[i] << 24) >>> 0) + (b[i + 1] << 16) + (b[i + 2] << 8) + b[i + 3];
  const put32 = (b, i, v) => { b[i] = (v >>> 24) & 255; b[i + 1] = (v >>> 16) & 255; b[i + 2] = (v >>> 8) & 255; b[i + 3] = v & 255; };
  const u64 = (b, i) => u32(b, i) * 4294967296 + u32(b, i + 4);
  const put64 = (b, i, v) => { put32(b, i, Math.floor(v / 4294967296)); put32(b, i + 4, v % 4294967296); };
  const typeAt = (b, i) => String.fromCharCode(b[i], b[i + 1], b[i + 2], b[i + 3]);

  // Child boxes of [start, end): {type, start, end, body} (body = payload start).
  function boxes(b, start, end) {
    const out = [];
    let i = start;
    while (i + 8 <= end) {
      let size = u32(b, i);
      const type = typeAt(b, i + 4);
      let body = i + 8;
      if (size === 1) {
        if (i + 16 > end) break;
        size = u64(b, i + 8);
        body = i + 16;
      } else if (size === 0) {
        size = end - i;
      }
      if (size < body - i || i + size > end) throw new Error(`mp4: bad ${type} box at ${i}`);
      out.push({ type, start: i, end: i + size, body });
      i += size;
    }
    return out;
  }
  const child = (b, box, type) => boxes(b, box.body, box.end).find((x) => x.type === type) || null;
  const path = (b, box, types) => types.reduce((cur, t) => (cur ? child(b, cur, t) : null), box);

  function parse(buf) {
    const b = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
    const top = boxes(b, 0, b.length);
    const ftyp = top.find((x) => x.type === "ftyp");
    const moov = top.find((x) => x.type === "moov");
    if (!moov) throw new Error("mp4: no moov box");
    const traks = boxes(b, moov.body, moov.end).filter((x) => x.type === "trak");
    if (!traks.length) throw new Error("mp4: no track");
    const trak = traks[0];
    const mdhd = path(b, trak, ["mdia", "mdhd"]);
    const timescale = mdhd ? (b[mdhd.body] === 1 ? u32(b, mdhd.body + 20) : u32(b, mdhd.body + 12)) : 0;
    const hdlr = path(b, trak, ["mdia", "hdlr"]);
    const handler = hdlr ? typeAt(b, hdlr.body + 8) : "";
    const mvex = child(b, moov, "mvex");
    // Fragments: each moof plus the mdat(s) after it, up to the next moof.
    const frags = [];
    for (let k = 0; k < top.length; k++) {
      if (top[k].type !== "moof") continue;
      let end = top[k].end;
      for (let j = k + 1; j < top.length && top[j].type !== "moof"; j++) {
        if (top[j].type === "mdat") end = top[j].end;
      }
      const tfdt = path(b, top[k], ["traf", "tfdt"]);
      const time = tfdt ? (b[tfdt.body] === 1 ? u64(b, tfdt.body + 4) : u32(b, tfdt.body + 4)) : 0;
      frags.push({ start: top[k].start, end, moof: top[k], time });
    }
    return { b, ftyp, moov, trak, timescale, handler, mvex, frags, mdats: top.filter((x) => x.type === "mdat"),
      mvhd: child(b, moov, "mvhd") };
  }

  const copy = (b, box) => b.slice(box.start, box.end);

  function setTrackId(box, b, id) {
    // tkhd / trex / tfhd carry the id right after their version/flags (tkhd
    // after its times).
    if (box.type === "tkhd") put32(b, box.body + (b[box.body] === 1 ? 20 : 12), id);
    else put32(b, box.body + 4, id);
  }

  // A copy of `src` (a box) with every tkhd/trex/tfhd inside set to `id`.
  function withTrackId(src, id) {
    const b = src.slice();
    const walk = (s, e) => {
      for (const x of boxes(b, s, e)) {
        if (x.type === "tkhd" || x.type === "trex" || x.type === "tfhd") setTrackId(x, b, id);
        if (CONTAINERS.has(x.type)) walk(x.body, x.end);
      }
    };
    walk(0, b.length);
    return b;
  }

  function box(type, ...parts) {
    const size = 8 + parts.reduce((n, p) => n + p.length, 0);
    const out = new Uint8Array(size);
    put32(out, 0, size);
    for (let i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i);
    let at = 8;
    for (const p of parts) { out.set(p, at); at += p.length; }
    return out;
  }

  function concat(parts) {
    const n = parts.reduce((s, p) => s + p.length, 0);
    const out = new Uint8Array(n);
    let at = 0;
    for (const p of parts) { out.set(p, at); at += p.length; }
    return out;
  }

  // mvhd of the video file, with the longer duration and next track id 3.
  function mergedMvhd(v, a) {
    const b = copy(v.b, v.mvhd);
    const off = 8;
    const v1 = b[off] === 1;
    const tsAt = off + (v1 ? 20 : 12);
    const ts = u32(b, tsAt);
    const durOf = (p) => {
      const m = p.mvhd;
      if (!m) return 0;
      const one = p.b[m.body] === 1;
      const pts = u32(p.b, m.body + (one ? 20 : 12));
      const d = one ? u64(p.b, m.body + 24) : u32(p.b, m.body + 16);
      return pts ? d / pts : 0;
    };
    const dur = Math.round(Math.max(durOf(v), durOf(a)) * ts);
    if (v1) put64(b, tsAt + 4, dur); else put32(b, tsAt + 4, Math.min(dur, 0xffffffff));
    put32(b, b.length - 4, 3);
    return b;
  }

  function mergeFragmented(v, a) {
    const tracks = [[v, 1], [a, 2]];
    const trex = tracks.map(([p, id]) => {
      const t = p.mvex && boxes(p.b, p.mvex.body, p.mvex.end).find((x) => x.type === "trex");
      return t ? withTrackId(copy(p.b, t), id) : null;
    }).filter(Boolean);
    const moov = box("moov", mergedMvhd(v, a),
      withTrackId(copy(v.b, v.trak), 1), withTrackId(copy(a.b, a.trak), 2),
      box("mvex", ...trex));
    // Fragments of both, in time order (seconds), each moof given its new
    // track id and sequence number. An explicit base-data-offset is absolute,
    // so it moves with the fragment.
    const list = [];
    for (const [p, id] of tracks) {
      for (const f of p.frags) list.push({ p, id, f, t: p.timescale ? f.time / p.timescale : 0 });
    }
    list.sort((x, y) => x.t - y.t || x.id - y.id);
    const head = concat([v.ftyp ? copy(v.b, v.ftyp) : box("ftyp"), moov]);
    const parts = [head];
    let pos = head.length;
    let seq = 1;
    for (const { p, id, f } of list) {
      const piece = withTrackId(p.b.slice(f.start, f.end), id);
      const moof = boxes(piece, 0, piece.length)[0];
      const mfhd = child(piece, moof, "mfhd");
      if (mfhd) put32(piece, mfhd.body + 4, seq++);
      for (const traf of boxes(piece, moof.body, moof.end).filter((x) => x.type === "traf")) {
        const tfhd = child(piece, traf, "tfhd");
        if (tfhd && (u32(piece, tfhd.body) & 0x1)) {
          const at = tfhd.body + 8;
          put64(piece, at, u64(piece, at) + (pos - f.start));
        }
      }
      parts.push(piece);
      pos += piece.length;
    }
    return concat(parts);
  }

  function mergePlain(v, a) {
    // Layout: ftyp, moov, then each file's mdats in order. Chunk offsets
    // (stco/co64) are absolute, so each is moved by where its mdat lands.
    const traks = [[v, withTrackId(copy(v.b, v.trak), 1)], [a, withTrackId(copy(a.b, a.trak), 2)]];
    const mvhd = mergedMvhd(v, a);
    const ftyp = v.ftyp ? copy(v.b, v.ftyp) : box("ftyp");
    const moovSize = 8 + mvhd.length + traks.reduce((n, [, t]) => n + t.length, 0);
    let pos = ftyp.length + moovSize;
    const media = [];
    for (const [p, t] of traks) {
      const moves = p.mdats.map((m) => {
        const r = { from: m.start, to: m.end, delta: pos - m.start };
        pos += m.end - m.start;
        media.push(p.b.slice(m.start, m.end));
        return r;
      });
      const shift = (off) => {
        const m = moves.find((x) => off >= x.from && off < x.to);
        return m ? off + m.delta : off;
      };
      const walk = (s, e) => {
        for (const x of boxes(t, s, e)) {
          if (x.type === "stco" || x.type === "co64") {
            const n = u32(t, x.body + 4);
            for (let k = 0; k < n; k++) {
              if (x.type === "stco") {
                const at = x.body + 8 + k * 4;
                const nv = shift(u32(t, at));
                if (nv > 0xffffffff) throw new Error("mp4: file too large to merge here");
                put32(t, at, nv);
              } else {
                const at = x.body + 8 + k * 8;
                put64(t, at, shift(u64(t, at)));
              }
            }
          }
          if (CONTAINERS.has(x.type)) walk(x.body, x.end);
        }
      };
      walk(0, t.length);
    }
    return concat([ftyp, box("moov", mvhd, ...traks.map(([, t]) => t)), ...media]);
  }

  // Join `video` and `audio` (ArrayBuffer/Uint8Array) into one MP4.
  function merge(video, audio) {
    let v = parse(video);
    let a = parse(audio);
    // Accept the two in either order.
    if (v.handler === "soun" && a.handler === "vide") [v, a] = [a, v];
    const fragmented = v.frags.length > 0 && a.frags.length > 0;
    if (fragmented) return mergeFragmented(v, a);
    if (!v.frags.length && !a.frags.length) return mergePlain(v, a);
    throw new Error("mp4: one input is fragmented and the other is not");
  }

  // Whether bytes look like an MP4 (ftyp/styp/moov/moof first box).
  function isMp4(buf) {
    const b = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
    return b.length >= 8 && ["ftyp", "styp", "moov", "moof", "sidx"].includes(typeAt(b, 4));
  }

  const DCMp4 = { merge, parse, isMp4 };
  g.DCMp4 = DCMp4;
  if (typeof module !== "undefined" && module.exports) module.exports = DCMp4;
})(typeof globalThis !== "undefined" ? globalThis : this);
