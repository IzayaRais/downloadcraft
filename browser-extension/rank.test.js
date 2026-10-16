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

describe("menuTokens", () => {
  it("covers dark and light with the same keys", () => {
    const dark = DCRank.menuTokens("dark");
    const light = DCRank.menuTokens("light");
    assert.deepEqual(Object.keys(dark).sort(), Object.keys(light).sort());
    assert.equal(dark.mode, "dark");
    assert.equal(light.mode, "light");
    assert.equal(light.bg, "#ffffff");
    assert.ok(dark.bg !== light.bg);
    assert.ok(dark.toggle !== light.toggle);
    // unknown mode falls back to dark (historic look)
    assert.equal(DCRank.menuTokens().mode, "dark");
  });
});

describe("menu paint", () => {
  // Minimal fake DOM: style bag, role-tagged children, toggle button.
  function fakeEl(role) {
    return {
      role,
      style: {},
      textContent: "",
      title: "",
      attrs: {},
      setAttribute(k, v) { this.attrs[k] = v; },
      onclick: null,
    };
  }
  function fakeMenu(kids) {
    return {
      style: { display: "none" },
      _dcT: null,
      kids,
      querySelectorAll(sel) {
        const roles = sel.split(",").map((s) => {
          const m = s.match(/\[data-dc="([^"]+)"\]/);
          return m && m[1];
        }).filter(Boolean);
        return this.kids.filter((k) => roles.includes(k.attrs["data-dc"]));
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

  it("paints a menu light without touching layout", () => {
    withGlobals({ theme: "light" }, () => {
      const head = fakeEl(); head.attrs["data-dc"] = "head";
      const sec = fakeEl(); sec.attrs["data-dc"] = "sec";
      const item = fakeEl(); item.attrs["data-dc"] = "item";
      const menu = fakeMenu([head, sec, item]);
      DCRank.paintDcMenu(menu);
      assert.equal(menu.style.background, "#ffffff");
      assert.equal(menu.style.display, "none"); // layout untouched
      assert.equal(head.style.color, "#202124");
      assert.equal(sec.style.color, "#9aa0a6");
      assert.equal(item.style.background, "#ffffff");
      assert.equal(item.style.border, "1px solid #e1e3e6");
    });
  });

  it("toggle flips the theme and repaints", () => {
    withGlobals({ theme: "dark" }, () => {
      const menu = fakeMenu([]);
      const btn = DCRank.dcThemeToggle(menu, globalThis.document);
      assert.equal(btn.textContent, "");
      btn.onclick({ stopPropagation() {} });
      assert.equal(menu._dcT.mode, "light");
      assert.equal(menu.style.background, "#ffffff");
      // The toggle button itself gets the new glyph on repaint.
      const t2 = fakeEl(); t2.attrs["data-dc"] = "toggle";
      const menu2 = fakeMenu([t2]);
      DCRank.paintDcMenu(menu2);
      assert.equal(t2.textContent, DCRank.dcTokens().toggle);
    });
  });
});
