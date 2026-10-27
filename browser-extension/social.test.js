// rank.js: post links on Instagram / TikTok / X / Reddit, and the
// remembered-quality pick. Run: `node --test social.test.js`
const { it } = require("node:test");
const assert = require("node:assert/strict");
const R = require("./rank.js");

it("recognises one video post per site, and only posts", () => {
  const ok = {
    "https://www.instagram.com/reel/C9xYz_AbC12/?igsh=x": "https://www.instagram.com/reel/C9xYz_AbC12/",
    "https://www.instagram.com/reels/C9xYz_AbC12/": "https://www.instagram.com/reel/C9xYz_AbC12/",
    "https://www.instagram.com/someone/p/C9xYzAbC12/": "https://www.instagram.com/p/C9xYzAbC12/",
    "https://www.tiktok.com/@a.b/video/7301234567890123456?lang=en": "https://www.tiktok.com/@a.b/video/7301234567890123456",
    "https://x.com/someone/status/1790000000000000000/video/1": "https://x.com/someone/status/1790000000000000000",
    "https://mobile.twitter.com/someone/status/1790000000000000000": "https://x.com/someone/status/1790000000000000000",
    "https://old.reddit.com/r/videos/comments/1abcde/title/": "https://www.reddit.com/r/videos/comments/1abcde/",
  };
  for (const [u, want] of Object.entries(ok)) assert.equal(R.socialPostUrl(u), want, u);
  assert.equal(R.socialPostUrl("/r/videos/comments/1abcde/x/", "https://www.reddit.com/"), "https://www.reddit.com/r/videos/comments/1abcde/");
  for (const u of ["https://www.instagram.com/someone/", "https://www.tiktok.com/foryou", "https://x.com/home",
    "https://x.com/i/status/123", "https://www.reddit.com/r/videos/", "https://example.com/reel/abc", "javascript:alert(1)", ""]) {
    assert.equal(R.socialPostUrl(u), "", u);
  }
  assert.equal(R.socialSite("www.instagram.com"), "instagram.com");
  assert.equal(R.socialSite("old.reddit.com"), "reddit.com");
  assert.equal(R.socialSite("www.youtube.com"), "");
});

it("picks the remembered quality, else the best below, else the lowest above", () => {
  const hs = [1080, 720, 480, 360];
  assert.equal(R.pickHeight(hs, "720p"), 1);
  assert.equal(R.pickHeight(hs, "600p"), 2);
  assert.equal(R.pickHeight([1080, 720], "240p"), 1);
  assert.equal(R.pickHeight(hs, "audio"), -1);
  assert.equal(R.pickHeight([], "720p"), -1);
});
