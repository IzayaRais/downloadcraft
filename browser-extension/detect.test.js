// Deep detection: response classification, first-bytes sniffing, and the
// request headers replayed by the app. Run: `node --test detect.test.js`
const { describe, it } = require("node:test");
const assert = require("node:assert/strict");
const R = require("./rank.js");

const MB = 1024 * 1024;
const opts = { minBytes: 100 * 1024, deep: true };
const res = (over) => ({ url: "https://cdn.example.net/x", mime: "", type: "xmlhttprequest", method: "GET", status: 200, size: 0, ...over });

describe("classifyResponse: streams", () => {
  it("recognizes HLS and DASH by extension and by MIME", () => {
    assert.equal(R.classifyResponse(res({ url: "https://h/v/index.m3u8?t=1" }), opts).stream, "hls");
    assert.equal(R.classifyResponse(res({ url: "https://h/v/manifest.mpd" }), opts).stream, "dash");
    assert.equal(R.classifyResponse(res({ url: "https://h/play?id=9", mime: "application/x-mpegURL" }), opts).stream, "hls");
    assert.equal(R.classifyResponse(res({ url: "https://h/play?id=9", mime: "application/dash+xml" }), opts).stream, "dash");
  });
  it("uses the Content-Disposition filename for opaque download URLs", () => {
    const hit = R.classifyResponse(res({ url: "https://h/get.php?id=4", mime: "application/octet-stream", filename: "Movie.mkv", size: 50 * MB }), opts);
    assert.equal(hit.kind, "file");
    assert.equal(hit.mime, "video/x-matroska");
    assert.equal(R.classifyResponse(res({ url: "https://h/get.php?id=5", filename: "list.m3u8" }), opts).stream, "hls");
  });
});

describe("classifyResponse: files and parts", () => {
  it("keeps media files above the size gate and drops tiny ones", () => {
    assert.equal(R.classifyResponse(res({ url: "https://h/a.mp4", mime: "video/mp4", size: 5 * MB }), opts).kind, "file");
    assert.equal(R.classifyResponse(res({ url: "https://h/a.mp4", mime: "video/mp4", size: 20 * 1024 }), opts), null);
    assert.equal(R.classifyResponse(res({ url: "https://h/stream", mime: "audio/mpeg", size: 3 * MB }), opts).kind, "file");
  });
  it("still captures segments as parts (playlist resolution relies on them)", () => {
    assert.equal(R.classifyResponse(res({ url: "https://h/v/seg-12.m4s", size: 900 }), opts).kind, "segment");
    assert.equal(R.classifyResponse(res({ url: "https://h/v/clip.ts", mime: "video/mp2t" }), opts).kind, "segment");
  });
  it("keeps YouTube googlevideo range traffic as files for the itag swap", () => {
    const yt = res({ url: "https://rr1---sn-a.googlevideo.com/videoplayback?itag=137&range=0-1048575&mime=video%2Fmp4", mime: "video/mp4", size: 40 * MB });
    assert.equal(R.classifyResponse(yt, opts).kind, "file");
  });
  it("detects subtitles by extension and MIME", () => {
    assert.equal(R.classifyResponse(res({ url: "https://h/en.vtt" }), opts).kind, "subtitle");
    assert.equal(R.classifyResponse(res({ url: "https://h/sub?id=1", mime: "application/x-subrip" }), opts).kind, "subtitle");
  });
});

describe("classifyResponse: when to sniff", () => {
  it("sniffs opaque types and stream-shaped paths", () => {
    assert.ok(R.classifyResponse(res({ url: "https://h/play?id=1", mime: "text/plain" }), opts).sniff);
    assert.ok(R.classifyResponse(res({ url: "https://h/hls/abc", mime: "application/json" }), opts) === null);
    assert.ok(R.classifyResponse(res({ url: "https://h/hls/abc", mime: "text/html" }), opts).sniff);
    assert.ok(R.classifyResponse(res({ url: "https://h/v/master.txt", mime: "text/html" }), opts).sniff);
    assert.ok(R.classifyResponse(res({ url: "https://h/blob/9f3a", mime: "image/jpeg", size: 30 * MB, acceptRanges: true }), opts).sniff);
  });
  it("never sniffs pages, non-GET requests, or when deep detection is off", () => {
    assert.equal(R.classifyResponse(res({ url: "https://h/watch", type: "main_frame", mime: "text/html" }), opts), null);
    assert.equal(R.classifyResponse(res({ url: "https://h/api", method: "POST", mime: "text/plain" }), opts), null);
    assert.equal(R.classifyResponse(res({ url: "https://h/x", mime: "application/octet-stream" }), { ...opts, deep: false }), null);
  });
  it("ignores failed and odd responses", () => {
    assert.equal(R.classifyResponse(res({ url: "https://h/a.mp4", mime: "video/mp4", status: 404 }), opts), null);
    assert.equal(R.classifyResponse(res({ url: "https://h/a.mp4", mime: "video/mp4", method: "PUT" }), opts), null);
  });
});

describe("sniffBytes", () => {
  const txt = (s) => new TextEncoder().encode(s);
  it("finds HLS (master or media), DASH and subtitles in text", () => {
    assert.deepEqual(
      [R.sniffBytes(txt("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nv.m3u8")).master, R.sniffBytes(txt("﻿#EXTM3U\n#EXTINF:4,\ns1.ts")).master],
      [true, false]);
    assert.equal(R.sniffBytes(txt('<?xml version="1.0"?>\n<MPD xmlns="urn:mpeg:dash:schema:mpd:2011">')).stream, "dash");
    assert.equal(R.sniffBytes(txt("WEBVTT\n\n00:00.000 --> 00:01.000\nhi")).kind, "subtitle");
    assert.equal(R.sniffBytes(txt("1\n00:00:01,000 --> 00:00:02,000\nhi")).mime, "application/x-subrip");
  });
  it("identifies containers by magic bytes", () => {
    assert.equal(R.sniffBytes(Uint8Array.from([0, 0, 0, 0x20, 0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6f, 0x6d])).mime, "video/mp4");
    assert.equal(R.sniffBytes(Uint8Array.from([0x1a, 0x45, 0xdf, 0xa3, 1])).mime, "video/webm");
    assert.equal(R.sniffBytes(Uint8Array.from([0x47, 0x40, 0, 0x10])).kind, "segment");
    assert.equal(R.sniffBytes(txt("ID3\u0004")).mime, "audio/mpeg");
  });
  it("returns null for ordinary documents", () => {
    assert.equal(R.sniffBytes(txt("<!doctype html><title>x</title>")), null);
    assert.equal(R.sniffBytes(txt('{"ok":true}')), null);
    assert.equal(R.sniffBytes(new Uint8Array()), null);
  });
});

describe("forwardHeaders", () => {
  it("keeps session, page and token headers only", () => {
    const got = R.forwardHeaders([
      { name: "Referer", value: "https://site.example/watch/1" },
      { name: "Cookie", value: "sid=abc" },
      { name: "Origin", value: "https://site.example" },
      { name: "Authorization", value: "Bearer t" },
      { name: "X-Playback-Session-Id", value: "42" },
      { name: "X-Client-Data", value: "variations" },
      { name: "Accept-Encoding", value: "gzip" },
      { name: "Range", value: "bytes=0-" },
      { name: "X-Evil", value: "a\r\nInjected: 1" },
    ]);
    assert.equal(got.referer, "https://site.example/watch/1");
    assert.equal(got.cookie, "sid=abc");
    assert.deepEqual(got.headers, { origin: "https://site.example", authorization: "Bearer t", "x-playback-session-id": "42" });
  });
});

describe("kinds with subtitles", () => {
  it("classifies subtitle entries and ranks them below video", () => {
    assert.equal(R.kindOf({ url: "https://h/en.vtt" }), "subtitle");
    assert.equal(R.kindOf({ url: "https://h/s?id=1", mime: "text/vtt" }), "subtitle");
    assert.ok(R.scoreMedia({ url: "https://h/en.vtt" }) < R.scoreMedia({ url: "https://h/s0.ts" }));
    assert.ok(R.scoreMedia({ url: "https://h/en.vtt" }) >= 0);
  });
});
