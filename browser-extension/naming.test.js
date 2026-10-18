// Unit tests for the capture naming helpers in content.js.
// Run: `node --test naming.test.js`
//
// content.js is a page script (it touches `document` on load), so the helper
// block is extracted and evaluated against stubs instead of importing it.
const { describe, it } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const src = fs.readFileSync(path.join(__dirname, "content.js"), "utf8");
const start = src.indexOf("// ---------- naming ----------");
const end = src.indexOf("// ---------- Facebook reels support");
assert.ok(start > 0 && end > start, "naming block markers moved");

function helpers(docTitle, href, ogTitle) {
  const document = {
    title: docTitle,
    querySelector(sel) {
      if (ogTitle != null && String(sel).includes("og:title")) {
        return { getAttribute: () => ogTitle };
      }
      return null;
    },
  };
  const location = { href };
  const block = src.slice(start, end);
  const fn = new Function(
    "document",
    "location",
    `${block}\nreturn { mediaBase, pageTitle, urlBasename };`
  );
  return fn(document, location);
}

const POOJA =
  "https://stream.sunplex.net/Movies/South.Indian/2026/P/Pooja.Meri.Jaan/Pooja.Meri.Jaan.2026.1080p.x264.AAC.Hin.BTs.VTL.mp4";
const POOJA_STEM = "Pooja.Meri.Jaan.2026.1080p.x264.AAC.Hin.BTs.VTL";

describe("direct video pages", () => {
  it("prefers the release filename over a generic page title", () => {
    assert.equal(helpers("video.mp4", POOJA).mediaBase(POOJA), POOJA_STEM);
  });

  it("never appends a second media extension", () => {
    const base = helpers(`${POOJA_STEM}.mp4`, POOJA).mediaBase(POOJA);
    assert.equal(base, POOJA_STEM);
    assert.ok(!/\.mp4$/i.test(base));
  });

  it("keeps a real page title over a hashed CDN name", () => {
    const cdn = "https://scontent.xx.fbcdn.net/o1/v/t2/f2/AQPfJZEgJQBtMJzC9SsGPgy2.mp4?_nc_cat=1";
    const h = helpers("SOOPer Icon Atif Aslam Performances - Facebook", cdn);
    assert.equal(h.mediaBase(cdn), "SOOPer Icon Atif Aslam Performances");
  });

  it("decodes percent-encoded names", () => {
    const u = "https://h/Movies/%D9%85%D9%88%D9%81%D9%8A.2026.1080p.mp4";
    assert.equal(helpers("", u).mediaBase(u), "موفي.2026.1080p");
  });
});

describe("fallbacks", () => {
  it("ignores stream placeholders", () => {
    const h = helpers("Video", "https://h/videoplayback?expire=1");
    // The boring CDN stem is never used as a name; the page title stays.
    assert.equal(h.mediaBase("https://h/videoplayback?expire=1"), "Video");
  });

  it("strips site chrome from titles", () => {
    const h = helpers("My Clip | Example Site", "https://h/watch");
    assert.equal(h.mediaBase("https://h/watch"), "My Clip");
  });

  it("strips a known media extension from the title", () => {
    const h = helpers("holiday.webm", "https://h/v?id=9");
    assert.equal(h.mediaBase("https://h/v?id=9"), "holiday");
  });
});
