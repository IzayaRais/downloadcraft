// Unit tests for rank.js (P4). Run: `node --test rank.test.js`
const { describe, it } = require("node:test");
const assert = require("node:assert/strict");
const DCRank = require("./rank.js");

describe("kinds", () => {
  it("classifies playlists, segments, files", () => {
    assert.equal(DCRank.kindOf({ url: "https://h/v/pl.m3u8?x=1" }), "playlist");
    assert.equal(DCRank.kindOf({ url: "https://h/v/manifest.mpd" }), "playlist");
    assert.equal(DCRank.kindOf({ url: "https://h/v/seg-1.m4s" }), "segment");
    assert.equal(DCRank.kindOf({ url: "https://h/v/s0.ts" }), "segment");
    assert.equal(DCRank.kindOf({ url: "https://h/v/clip.mp4?x=1" }), "file");
    assert.equal(DCRank.kindOf({ url: "https://h/v/clip.mp4" }), "file");
  });
});

describe("canonical", () => {
  it("collapses slice variants", () => {
    const a = "https://h/v.mp4?oh=1&bytestart=0&byteend=100";
    const b = "https://h/v.mp4?oh=1&bytestart=101&byteend=200";
    assert.equal(DCRank.canonicalUrl(a), DCRank.canonicalUrl(b));
    assert.ok(!DCRank.canonicalUrl(a).includes("bytestart"));
  });
});

describe("scoring", () => {
  const file = { url: "https://h/v.mp4", mime: "video/mp4", size: 9e6 };
  const list = { url: "https://h/pl.m3u8", mime: "application/vnd.apple.mpegurl" };
  const seg = { url: "https://h/s0.ts", mime: "video/mp2t", size: 2e5 };
  it("orders file > playlist > segment", () => {
    const ctx = { hasPlaylist: false };
    assert.ok(DCRank.scoreMedia(file, ctx) > DCRank.scoreMedia(list, ctx));
    assert.ok(DCRank.scoreMedia(list, ctx) > DCRank.scoreMedia(seg, ctx));
  });
  it("demotes segments when a playlist is known (ad-skip)", () => {
    assert.equal(DCRank.scoreMedia(seg, { hasPlaylist: true }), -1);
    assert.ok(DCRank.scoreMedia(file, { hasPlaylist: true }) > 0);
  });
  it("prefers bigger sizes", () => {
    const small = { url: "https://h/a.mp4", mime: "video/mp4", size: 50e3 };
    const big = { url: "https://h/b.mp4", mime: "video/mp4", size: 50e6 };
    assert.ok(DCRank.scoreMedia(big, {}) > DCRank.scoreMedia(small, {}));
  });
});

describe("collapseDupes", () => {
  it("keeps the biggest, counts tabs", () => {
    const out = DCRank.collapseDupes([
      { url: "https://h/v.mp4?bytestart=0&byteend=9", size: 10, tabId: 1 },
      { url: "https://h/v.mp4?bytestart=10&byteend=99", size: 90, tabId: 2 },
      { url: "https://h/other.mp4", size: 5, tabId: 1 },
    ]);
    assert.equal(out.length, 2);
    const v = out.find((m) => m.url.includes("v.mp4"));
    assert.equal(v.size, 90);
    assert.equal(v.tabs, 2);
  });
});

describe("fmtDur", () => {
  it("formats m:ss", () => {
    assert.equal(DCRank.fmtDur(206), "3:26");
    assert.equal(DCRank.fmtDur(0), "");
    assert.equal(DCRank.fmtDur(-5), "");
  });
});

describe("panelTokens", () => {
  it("covers dark and light with the same keys", () => {
    const dark = DCRank.panelTokens("dark");
    const light = DCRank.panelTokens("light");
    assert.deepEqual(Object.keys(dark).sort(), Object.keys(light).sort());
    assert.equal(light.mode, "light");
    assert.equal(dark.mode, "dark");
    assert.equal(light.bg, "#ffffff");
    assert.equal(dark.bg, "#0f172a");
    assert.notEqual(dark.accent, light.accent);
    assert.ok(dark.toggle !== light.toggle);
    // unknown mode falls back to dark (the mockup's palette)
    assert.equal(DCRank.panelTokens().mode, "dark");
  });
});

describe("panel paint", () => {
  // Minimal fake DOM: style bag + role-tagged children addressed by
  // `data-dc-p` (the same query the real painter issues).
  function fakeEl(role) {
    const attrs = {};
    if (role) attrs["data-dc-p"] = role;
    return {
      style: {},
      textContent: "",
      attrs,
      setAttribute(k, v) { this.attrs[k] = v; },
      getAttribute(k) { return this.attrs[k] === undefined ? null : this.attrs[k]; },
      querySelectorAll() { return []; },
    };
  }
  function fakePanel(kids) {
    return {
      _dcT: null,
      style: {},
      kids,
      querySelectorAll(sel) {
        if (sel === "[data-dc-p]") return this.kids.filter((k) => k.attrs["data-dc-p"]);
        return [];
      },
    };
  }
  function withGlobals(t, fn) {
    const g = globalThis;
    const keepW = g.window;
    const keepD = g.document;
    g.window = {
      __dcTheme: t.theme,
      matchMedia: () => ({ matches: t.theme !== "dark" && t.systemLight }),
    };
    g.document = { createElement: () => fakeEl(null) };
    try {
      delete g.__dcTheme;
      if (t.theme === "light" || t.theme === "dark") g.__dcTheme = t.theme;
      return fn();
    } finally {
      delete g.__dcTheme;
      if (keepW === undefined) delete g.window; else g.window = keepW;
      if (keepD === undefined) delete g.document; else g.document = keepD;
    }
  }

  it("defaults to dark without an override or light OS", () => {
    withGlobals({ systemLight: false }, () => {
      assert.equal(DCRank.dcThemeMode(), "dark");
    });
  });

  it("follows prefers-color-scheme and honors the override", () => {
    withGlobals({ systemLight: true }, () => {
      assert.equal(DCRank.dcThemeMode(), "light");
    });
    withGlobals({ theme: "dark", systemLight: true }, () => {
      assert.equal(DCRank.dcThemeMode(), "dark");
    });
  });

  it("paints a panel light without touching layout", () => {
    withGlobals({ theme: "light" }, () => {
      const txt = fakeEl("text");
      const div = fakeEl("divider");
      const item = fakeEl("item");
      const panel = fakePanel([txt, div, item]);
      panel.style.display = "none";
      DCRank.paintDcPanel(panel);
      assert.equal(panel.style.background, "#ffffff");
      assert.equal(panel.style.display, "none"); // layout untouched
      assert.equal(panel._dcT.mode, "light");
      assert.equal(txt.style.color, "#0f172a");
      assert.equal(div.style.background, "#e2e8f0");
      assert.equal(item.style.background, "#f8fafc");
      assert.equal(item.style.borderColor, "#e2e8f0");
    });
  });

  it("highlights the selected cell and flips the bubble arrow", () => {
    withGlobals({ theme: "dark" }, () => {
      const sel = fakeEl("item");
      sel.attrs["data-dc-sel"] = "1";
      const above = fakeEl("arrow");
      above.attrs["data-dc-arrow"] = "bottom";
      DCRank.paintDcPanel(fakePanel([sel, above]));
      assert.equal(sel.style.borderColor, "#3b82f6");
      assert.equal(sel.style.background, "rgba(59,130,246,.16)");
      // Arrow below the panel: the two bottom borders draw the point.
      assert.equal(above.style.borderRightColor, "#334155");
      assert.equal(above.style.borderBottomColor, "#334155");
      assert.equal(above.style.borderLeftColor, "transparent");
    });
  });

  it("writes the toggle glyph for the active theme", () => {
    withGlobals({ theme: "dark" }, () => {
      const tog = fakeEl("toggle");
      DCRank.paintDcPanel(fakePanel([tog]));
      assert.equal(tog.textContent, "☀");
    });
  });
});

const VIMEO_FRAGMENT = "https://vod-adaptive-ak.vimeocdn.com/exp=1~hmac=example/v2/range/prot/cmFuZ2U9MzkzODcxMi00OTgyMzgx/avf/video.mp4?pathsig=example&range=3938712-4982381";
describe("Vimeo range captures", () => {
  it("classifies signed MP4 range URLs as fragments without rewriting them", () => {
    assert.equal(DCRank.kindOf({ url: VIMEO_FRAGMENT }), "segment");
    assert.equal(DCRank.canonicalUrl(VIMEO_FRAGMENT), VIMEO_FRAGMENT);
    assert.equal(DCRank.isVimeoFragment(VIMEO_FRAGMENT.split("?")[0]), true);
    assert.equal(DCRank.isVimeoFragment("https://vod.vimeocdn.com/video.mp4?range=0-99"), true);
  });
  it("chooses a manifest over a Vimeo MP4 fragment", () => {
    const fragment = { url: VIMEO_FRAGMENT, mime: "video/mp4", size: 1043670 };
    const manifest = { url: "https://vod.vimeocdn.com/master.mpd" };
    const pool = [fragment, manifest].filter(m => DCRank.scoreMedia(m, {hasPlaylist:true}) >= 0);
    assert.deepEqual(pool, [manifest]);
    assert.equal(DCRank.kindOf({ url: "https://vod.vimeocdn.com/full.mp4" }), "file");
    assert.equal(DCRank.isVimeoFragment("https://vimeocdn.com.evil.test/v2/range/a.mp4"), false);
  });
  it("accepts exact video pages and preserves unlisted access parameters", () => {
    for (const page of ["https://player.vimeo.com/video/123?h=secret&app_id=1", "https://vimeo.com/123/secret"]) {
      assert.equal(DCRank.vimeoVideoPage(page), page);
    }
    for (const page of ["https://vimeo.com/", "https://vimeo.com/user123", "https://player.vimeo.com.evil.test/video/123", "ftp://vimeo.com/123"]) {
      assert.equal(DCRank.vimeoVideoPage(page), null);
    }
  });
});
