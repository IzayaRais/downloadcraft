// Unit tests for the Facebook reel caption/thumbnail helpers in content.js.
// Run: `node --test fb.test.js`
//
// The helpers are DOM-driven at runtime, so the whole Facebook block is
// extracted and evaluated with stubs; only the pure rankers are exercised.
const { describe, it } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const src = fs.readFileSync(path.join(__dirname, "content.js"), "utf8");
const start = src.indexOf("// ---------- Facebook reels support");
const end = src.indexOf("// ---------- end Facebook reels support");
assert.ok(start > 0 && end > start, "Facebook block markers moved");

const fn = new Function(
  "document",
  "window",
  `${src.slice(start, end)}
   return { fbCleanCaption, fbCaptionUsable, fbPickCaption, fbJsonTexts,
            fbThumbOk, fbThumbScore, fbPickThumb, fbQuality, fbQualityCap, fbFilename,
            fbReelKeyOf, fbPickVideo, fbVideoKey, fbAudioItem, fbReelMedia, fbMenuRows,
            fbJsonIsCurrent, fbIdsNear, fbOrderFor, fbPermalink };`
);
const fb = fn({ title: "" }, {});

// What the user's reel caption looks like in the DOM.
const CAPTION = "আীবার সপক বজায় রেখে চলানা ইসলামিক পরিবার গড়া 🖤 মা-বাবার জন্য দোয়া";
const STALE = "Old caption from the page that loaded first";

describe("caption cleaning", () => {
  it("strips site chrome and the unread badge", () => {
    assert.equal(fb.fbCleanCaption("(2) My clip | Facebook"), "My clip");
    assert.equal(fb.fbCleanCaption("My clip on Facebook"), "My clip");
  });

  it("collapses real newlines and runs of spaces", () => {
    assert.equal(fb.fbCleanCaption("line one\nline two   spaced"), "line one line two spaced");
  });

  it("rejects chrome, the audio line, counters and the author line", () => {
    for (const bad of [
      "",
      "facebook",
      "Log in",
      "See posts, photos and more on Facebook.",
      "\u266a Smrity's Home Diary \u00b7 Original audio",
      "1.2K",
      "90",
      "Smrity's Home Diary \U0001f1e7\U0001f1e9 \u2022 Follow",
      "too short",
    ]) {
      assert.equal(fb.fbCaptionUsable(fb.fbCleanCaption(bad)), false, bad);
    }
  });

  it("accepts a real description", () => {
    assert.equal(fb.fbCaptionUsable(fb.fbCleanCaption(CAPTION)), true);
  });
});

describe("caption ranking", () => {
  it("prefers what is on screen over a stale meta", () => {
    const picked = fb.fbPickCaption([
      { text: STALE, kind: 0.9 },
      { text: CAPTION, kind: 1 },
    ]);
    assert.equal(picked, CAPTION);
  });

  it("falls back to the meta when the DOM has no caption", () => {
    const picked = fb.fbPickCaption([
      { text: "", kind: 1 },
      { text: STALE, kind: 0.9 },
    ]);
    assert.equal(picked, STALE);
  });

  it("keeps the caption ahead of the audio and author lines", () => {
    const picked = fb.fbPickCaption([
      { text: "Smrity's Home Diary \U0001f1e7\U0001f1e9 \u2022 Follow", kind: 1 },
      { text: CAPTION, kind: 1 },
    ]);
    assert.equal(picked, CAPTION);
  });

  it("within one source, the longer text wins", () => {
    const picked = fb.fbPickCaption([
      { text: "short caption", kind: 1 },
      { text: "a much longer caption over here", kind: 1 },
    ]);
    assert.equal(picked, "a much longer caption over here");
  });

  it("returns empty when everything is chrome", () => {
    assert.equal(
      fb.fbPickCaption([
        { text: "Facebook", kind: 1 },
        { text: "1.2K", kind: 0.9 },
        { text: "\u266a Audio \u00b7 Original audio", kind: 0.8 },
      ]),
      ""
    );
  });

  it("caps the returned caption", () => {
    assert.equal(fb.fbPickCaption([{ text: "x".repeat(300), kind: 1 }]).length, 200);
  });

  it("reads message text out of the page JSON", () => {
    const html = '{"message":{"text":"Hi \\u2705 there"}} and {"message":{"text":"Second one"}}';
    assert.deepEqual(fb.fbJsonTexts(html), ["Hi \u2705 there", "Second one"]);
  });
});

describe("thumbnail candidates", () => {
  const POSTER = "https://scontent.xx.fbcdn.net/v/t15.5256-10/poster.jpg?_nc_cat=1";
  const OG = "https://scontent.xx.fbcdn.net/v/t39.30808-6/og.jpg";
  const SPRITE = "https://static.xx.fbcdn.net/rsrc.php/v3/y1/r/abcd1234.png";

  it("never accepts Facebook UI sprites or non-http urls", () => {
    assert.equal(fb.fbThumbOk(SPRITE), false);
    assert.equal(fb.fbThumbOk("data:image/png;base64,AAAA"), false);
    assert.equal(fb.fbThumbOk("blob:https://www.facebook.com/123"), false);
    assert.equal(fb.fbThumbOk(POSTER), true);
  });

  it("prefers the player poster over a page meta", () => {
    assert.equal(
      fb.fbPickThumb([
        { url: POSTER, kind: 1, dist: 0 },
        { url: OG, kind: 0.6 },
      ]),
      POSTER
    );
  });

  it("skips a sprite posing as the poster", () => {
    assert.equal(
      fb.fbPickThumb([
        { url: SPRITE, kind: 1, dist: 0 },
        { url: OG, kind: 0.6 },
      ]),
      OG
    );
  });

  it("within one source, nearer then larger wins", () => {
    const near = "https://scontent.xx.fbcdn.net/v/t15.5256-10/near.jpg";
    const far = "https://scontent.xx.fbcdn.net/v/t15.5256-10/far.jpg";
    assert.equal(
      fb.fbPickThumb([
        { url: far, kind: 0.8, dist: 400 },
        { url: near, kind: 0.8, dist: 12 },
      ]),
      near
    );
    const big = "https://scontent.xx.fbcdn.net/v/t39.30808-6/big.jpg";
    const small = "https://scontent.xx.fbcdn.net/v/t39.30808-6/small.jpg";
    assert.equal(
      fb.fbPickThumb([
        { url: small, kind: 0.8, dist: 10, w: 120, h: 90 },
        { url: big, kind: 0.8, dist: 10, w: 720, h: 1280 },
      ]),
      big
    );
  });

  it("returns empty for junk", () => {
    assert.equal(fb.fbPickThumb([{ url: SPRITE, kind: 1 }, { url: "", kind: 0.8 }]), "");
  });
});

describe("file naming and quality", () => {  it("names the file after the description", () => {
    const name = fb.fbFilename(CAPTION);
    assert.ok(name.endsWith(".mp4"));
    assert.ok(name.startsWith("\u0986\u09c0\u09ac\u09be\u09b0"));
    assert.equal(/[<>:"/\\|?*]/.test(name), false);
  });

  it("keeps a sane fallback and never doubles the extension", () => {
    assert.equal(fb.fbFilename(""), "facebook_video.mp4");
    assert.equal(fb.fbFilename("clip.mp4"), "clip.mp4");
    assert.ok(fb.fbFilename("a/b\\c:d*e?f").indexOf("_") > 0);
  });

  it("labels qualities the way the panel shows them", () => {
    assert.equal(fb.fbQuality(1920, 1080, true), "1080p HD");
    assert.equal(fb.fbQuality(1080, 1920, true), "1080p HD");
    assert.equal(fb.fbQuality(1280, 720, true), "720p HD");
    assert.equal(fb.fbQuality(854, 480, false), "480p");
    assert.equal(fb.fbQuality(640, 360, false), "360p");
    assert.equal(fb.fbQuality(0, 0, true), "HD");
    assert.equal(fb.fbQuality(0, 0, false), "SD");
  });

  it("turns a panel label into the numeric cap the app needs", () => {
    // The desktop app caps yt-dlp at this height; without it the picked row is
    // ignored and the absolute best stream is fetched instead.
    assert.equal(fb.fbQualityCap("1080p HD"), "1080p");
    assert.equal(fb.fbQualityCap("720p HD"), "720p");
    assert.equal(fb.fbQualityCap("480p"), "480p");
    // No number to cap on: leave it empty so the app uses best, not a guess.
    assert.equal(fb.fbQualityCap("HD"), "");
    assert.equal(fb.fbQualityCap("SD"), "");
    assert.equal(fb.fbQualityCap(""), "");
    assert.equal(fb.fbQualityCap(undefined), "");
  });
});

// The Facebook pill sits inside the player box, which is only as wide as the
// video — a panel anchored in there is clipped once the reel column is
// narrower than the panel. placeFbPanel anchors to the page and clamps.
describe("Facebook panel placement", () => {
  const pstart = src.indexOf("function placeFbPanel(");
  const pend = src.indexOf("// Presence pre-check");
  assert.ok(pstart > 0 && pend > pstart, "placeFbPanel markers moved");
  const placeFn = new Function(
    "document",
    "window",
    `${src.slice(pstart, pend)}\nreturn { placeFbPanel };`
  );

  function makePanel() {
    const arrow = { style: {} };
    const panel = {
      style: {},
      parentElement: null,
      querySelector: (sel) => (String(sel).includes("arrow") ? arrow : null),
      appendChild: () => {},
      arrow,
    };
    return panel;
  }
  function env(vw, vh, ox, oy) {
    const host = { appendChild: (el) => { el.parentElement = host; } };
    const document = {
      body: host,
      documentElement: {
        clientWidth: vw,
        clientHeight: vh,
        scrollLeft: ox,
        scrollTop: oy,
      },
    };
    const window = { innerWidth: vw, innerHeight: vh, pageXOffset: ox, pageYOffset: oy };
    return { document, window };
  }
  const pillAt = (left, top, w = 32, h = 32) => ({
    getBoundingClientRect: () => ({
      left,
      top,
      right: left + w,
      bottom: top + h,
      width: w,
      height: h,
    }),
  });

  it("keeps a 300px panel on screen when the player box is narrow", () => {
    const e = env(1366, 700, 0, 0);
    const panel = makePanel();
    placeFn(e.document, e.window).placeFbPanel(pillAt(750, 145), panel);
    assert.equal(panel.parentElement, e.document.body);
    assert.equal(panel.style.position, "absolute");
    assert.equal(panel.style.width, "300px");
    assert.equal(panel.style.left, "482px"); // pill right (782) - 300
    assert.equal(panel.style.top, "183px"); // 6px under the pill
    assert.equal(panel.style.right, "auto");
    assert.equal(panel.arrow.style.right, "auto");
    assert.ok(/^\d+px$/.test(panel.arrow.style.left), panel.arrow.style.left);
  });

  it("clamps to the left edge instead of overflowing it", () => {
    const e = env(1366, 700, 0, 0);
    const panel = makePanel();
    placeFn(e.document, e.window).placeFbPanel(pillAt(20, 145), panel);
    assert.equal(panel.style.left, "8px");
  });

  it("follows the page scroll and lifts when the pill is low", () => {
    const e = env(1366, 700, 0, 500);
    const panel = makePanel();
    placeFn(e.document, e.window).placeFbPanel(pillAt(750, 640), panel);
    // 640+32+6 = 678 → 678+300 > 692 → lift to 700-8-300 = 392 (+ scroll)
    assert.equal(panel.style.top, "892px");
  });

  it("shrinks on a phone-sized viewport", () => {
    const e = env(320, 640, 0, 0);
    const panel = makePanel();
    placeFn(e.document, e.window).placeFbPanel(pillAt(270, 100), panel);
    assert.equal(panel.style.width, "300px"); // min(300, 320-16=304)
    assert.equal(panel.style.left, "8px"); // clamped to the left margin; 8px+300px fits
  });
});

// Reels are an SPA: the reel on screen changes while the document, the page's
// embedded JSON and the tab's sniffed requests all still describe the reel the
// user scrolled past. These helpers keep the current reel ahead of the old one.
describe("reel identity and freshness", () => {
  it("reads the reel id out of every facebook URL shape", () => {
    assert.equal(fb.fbReelKeyOf("https://www.facebook.com/reel/1136332525739217"), "1136332525739217");
    assert.equal(fb.fbReelKeyOf("https://www.facebook.com/reel/1136332525739217/"), "1136332525739217");
    assert.equal(fb.fbReelKeyOf("https://www.facebook.com/watch/?v=12345"), "12345");
    assert.equal(fb.fbReelKeyOf("https://www.facebook.com/user/videos/777/"), "777");
    assert.equal(fb.fbReelKeyOf("https://www.facebook.com/"), "");
  });

  it("ranks sniffed legs newest first", () => {
    const out = fb.fbReelMedia([
      { url: "https://video.xx.fbcdn.net/old.mp4", seenAt: 10 },
      { url: "https://video.xx.fbcdn.net/new.mp4", seenAt: 30 },
      { url: "https://video.xx.fbcdn.net/mid.mp4", seenAt: 20 },
    ], 0);
    assert.deepEqual(out.map((m) => m.url), [
      "https://video.xx.fbcdn.net/new.mp4",
      "https://video.xx.fbcdn.net/mid.mp4",
      "https://video.xx.fbcdn.net/old.mp4",
    ]);
  });

  it("drops the reel the user scrolled away from", () => {
    const out = fb.fbReelMedia([
      { url: "https://video.xx.fbcdn.net/prev.mp4", seenAt: 1000 },
      { url: "https://video.xx.fbcdn.net/now.mp4", seenAt: 6000 },
    ], 5000);
    assert.deepEqual(out.map((m) => m.url), ["https://video.xx.fbcdn.net/now.mp4"]);
  });

  it("falls back to every leg rather than sending the page instead", () => {
    // Nothing captured after the switch (prefetch timings): the menu must still
    // offer the reel's own media, newest first.
    const out = fb.fbReelMedia([
      { url: "https://video.xx.fbcdn.net/prev.mp4", seenAt: 1000 },
      { url: "https://video.xx.fbcdn.net/now.mp4", seenAt: 2000 },
    ], 9999);
    assert.equal(out.length, 2);
    assert.equal(out[0].url, "https://video.xx.fbcdn.net/now.mp4");
  });

  it("ignores non-http legs", () => {
    assert.deepEqual(fb.fbReelMedia([{ url: "blob:https://www.facebook.com/1", seenAt: 5 }], 0), []);
  });

  it("treats a DASH audio leg as audio and a plain mp4 as video", () => {
    assert.equal(fb.fbAudioItem({ url: "https://video.xx.fbcdn.net/x.mp4", mime: "audio/mp4" }), true);
    assert.equal(fb.fbAudioItem({ url: "https://video.xx.fbcdn.net/x.mp4", mime: "video/mp4" }), false);
    assert.equal(fb.fbAudioItem({ url: "https://video.xx.fbcdn.net/x.m4a" }), true);
    assert.equal(fb.fbAudioItem({ url: "https://video.xx.fbcdn.net/x.mp4" }), false);
  });

  it("picks the player on screen, not the first video in the document", () => {
    const at = (left, top, playing, vw) => ({
      getBoundingClientRect: () => ({ left, top, right: left + 700, bottom: top + 1200, width: 700, height: 1200 }),
      paused: !playing,
      videoWidth: vw,
    });
    const off = at(-2000, 0, false, 0); // the reel scrolled past, still mounted
    const on = at(300, 0, true, 1080); // what the user is watching
    assert.equal(fb.fbPickVideo([off, on], 1366, 768), on);
    assert.equal(fb.fbPickVideo([off], 1366, 768), null);
  });
});

// What the reels panel offers, once the stale sources are gone.
describe("reel menu rows", () => {
  const NEW_LEG = "https://video.xx.fbcdn.net/now_v.mp4";
  const OLD_LEG = "https://video.xx.fbcdn.net/prev_v.mp4";
  const info = (over) => Object.assign({
    title: CAPTION,
    durSec: 30,
    w: 1080,
    h: 1920,
    hd: "",
    sd: "",
    current: true,
    since: 0,
    thumb: "",
  }, over || {});

  it("offers the live leg of the current reel, labelled with its size", () => {
    const d = fb.fbMenuRows(info(), [{ url: NEW_LEG, mime: "video/mp4", seenAt: 10, size: 5000000 }]);
    assert.equal(d.rows.length, 1);
    assert.equal(d.rows[0].url, NEW_LEG);
    assert.equal(d.rows[0].q, "1080p"); // portrait 1080x1920 -> shorter side
    assert.equal(d.rows[0]._size, 5000000);
  });

  it("never offers the previous reel's page URLs after a scroll", () => {
    // current: false is what fbJsonIsCurrent() reports once /reel/<id> changed:
    // the metas/JSON still hold the reel the user came from.
    const d = fb.fbMenuRows(
      info({
        current: false,
        hd: "https://video.xx.fbcdn.net/prev_hd.mp4",
        sd: "https://video.xx.fbcdn.net/prev_sd.mp4",
        since: 5000,
      }),
      [
        { url: OLD_LEG, mime: "video/mp4", seenAt: 1000 },
        { url: NEW_LEG, mime: "video/mp4", seenAt: 6000 },
      ]
    );
    assert.deepEqual(d.rows.map((r) => r.url), [NEW_LEG]);
  });

  it("puts the newest leg first even when the timestamps are unknown", () => {
    const d = fb.fbMenuRows(
      info({ current: false }),
      [
        { url: OLD_LEG, mime: "video/mp4", seenAt: 1000 },
        { url: NEW_LEG, mime: "video/mp4", seenAt: 2000 },
      ]
    );
    assert.deepEqual(d.rows.map((r) => r.url), [NEW_LEG, OLD_LEG]);
  });

  it("keeps the page's own HD/SD rows while the page still describes itself", () => {
    const d = fb.fbMenuRows(
      info({ current: true, hd: "https://video.xx.fbcdn.net/hd.mp4", sd: "https://video.xx.fbcdn.net/sd.mp4" }),
      []
    );
    assert.deepEqual(d.rows.map((r) => r.url), [
      "https://video.xx.fbcdn.net/hd.mp4",
      "https://video.xx.fbcdn.net/sd.mp4",
    ]);
    assert.equal(d.rows.every((r) => r.fromJson), true);
  });

  it("keeps the page's own HD row ahead of the captured leg while it is current", () => {
    const d = fb.fbMenuRows(
      info({ current: true, hd: "https://video.xx.fbcdn.net/hd.mp4", sd: "https://video.xx.fbcdn.net/sd.mp4" }),
      [{ url: NEW_LEG, mime: "video/mp4", seenAt: 10 }]
    );
    assert.deepEqual(d.rows.map((r) => r.url), [
      "https://video.xx.fbcdn.net/hd.mp4",
      "https://video.xx.fbcdn.net/sd.mp4",
      NEW_LEG,
    ]);
  });

  it("turns audio legs into chips and pairs the newest with the video", () => {
    const d = fb.fbMenuRows(info({ current: false }), [
      { url: NEW_LEG, mime: "video/mp4", seenAt: 10 },
      { url: "https://video.xx.fbcdn.net/a1.m4a", mime: "audio/mp4", seenAt: 11 },
      { url: "https://video.xx.fbcdn.net/a2.m4a", mime: "audio/mp4", seenAt: 12 },
    ]);
    // An audio-only leg is never offered as a video row: offering one saved
    // "with audio and no video".
    assert.deepEqual(d.rows.map((r) => r.url), [NEW_LEG]);
    assert.deepEqual(d.audio.map((a) => a.url), [
      "https://video.xx.fbcdn.net/a2.m4a",
      "https://video.xx.fbcdn.net/a1.m4a",
    ]);
    assert.equal(d.pair.url, "https://video.xx.fbcdn.net/a2.m4a");
  });

  it("does not invent a pair when nothing is audio", () => {
    const d = fb.fbMenuRows(info(), [{ url: NEW_LEG, mime: "video/mp4", seenAt: 10 }]);
    assert.equal(d.pair, null);
    assert.deepEqual(d.audio, []);
  });
});

// A tiny DOM: enough of parentElement / querySelectorAll / getAttribute.
function node(tag, attrs, kids) {
  const n = { tag, attrs: attrs || {}, kids: kids || [], parentElement: null };
  for (const k of n.kids) k.parentElement = n;
  n.getAttribute = (k) => (k in n.attrs ? n.attrs[k] : null);
  n.querySelectorAll = (sel) => {
    const out = [];
    const walk = (x) => {
      for (const k of x.kids) {
        if ((sel === "video" && k.tag === "video") || (sel === "a[href]" && k.tag === "a" && k.attrs.href)) out.push(k);
        walk(k);
      }
    };
    walk(n);
    return out;
  };
  return n;
}

describe("the clicked feed video", () => {
  // Two posts in a feed, each with its video and a link to it.
  const v1 = node("video");
  const v2 = node("video");
  const post1 = node("div", {}, [node("div", {}, [v1]), node("a", { href: "/somepage/videos/1111111111/" }), node("a", { href: "/hashtag/x" })]);
  const post2 = node("div", {}, [node("div", {}, [v2]), node("a", { href: "https://www.facebook.com/reel/2222222222?s=1" })]);
  node("div", {}, [post1, post2]);

  it("reads the id from its own post, never the neighbour's", () => {
    assert.deepEqual(fb.fbIdsNear(v1).map((x) => x.id), ["1111111111"]);
    assert.deepEqual(fb.fbIdsNear(v2).map((x) => x.id), ["2222222222"]);
  });

  const stored = [
    { id: "2222222222", dur: 30 }, // fetched last: the old pick
    { id: "1111111111", dur: 61 },
    { id: "3333333333", dur: 12 },
  ];

  it("puts the clicked video's data first", () => {
    const o = fb.fbOrderFor(stored, fb.fbIdsNear(v1), 61.2);
    assert.equal(o.list[0].id, "1111111111");
    assert.equal(o.known, true);
  });

  it("waits for a known id whose data has not arrived instead of offering another video", () => {
    const o = fb.fbOrderFor(stored.slice(0, 1), fb.fbIdsNear(v1), 61);
    assert.deepEqual(o.list, []);
    assert.equal(o.missing, "1111111111");
  });

  it("falls back to the one video of the same length, then to the old order", () => {
    assert.equal(fb.fbOrderFor(stored, [], 12.4).list[0].id, "3333333333");
    const o = fb.fbOrderFor(stored, [], 99);
    assert.equal(o.known, false);
    assert.equal(o.list[0].id, "2222222222");
  });

  it("builds a page yt-dlp can download that one video from", () => {
    assert.equal(fb.fbPermalink("2222222222", "/reel/2222222222"), "https://www.facebook.com/reel/2222222222");
    assert.equal(fb.fbPermalink("1111111111", "/p/videos/1111111111/"), "https://www.facebook.com/watch/?v=1111111111");
  });
});
