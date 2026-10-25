// Exercise the actual native-host handoff with browser APIs stubbed.
const { it } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const R = require('./rank.js');
const source = fs.readFileSync(path.join(__dirname, 'background.js'), 'utf8');
const block = source.slice(source.indexOf('async function sendToHost('), source.indexOf('// ---- IDM-style network sniffer'));
const fragment = 'https://vod.vimeocdn.com/v2/range/prot/token/video.mp4?range=3938712-4982381';
const player = 'https://player.vimeo.com/video/123?h=unlisted';
function harness(capture, framePage, capturedUrl = fragment) {
  const calls = [], cookies = [];
  const api = {
    tabs: { sendMessage: async (tab, msg, opts) => {
      assert.equal(tab, 7); assert.equal(opts.frameId, 4);
      return { pageUrl: framePage };
    } },
    runtime: { sendNativeMessage: async (host, msg) => { calls.push(msg); return {type:'ok'}; } },
  };
  const make = new Function('DCRank', 'tabMedia', 'mediaAuth', 'api', 'cookiesFor', 'stripFbSlice', 'HOST', 'console',
    `let lastHostError=''; const hostSend = (m) => api.runtime.sendNativeMessage(HOST, m); ${block}; return { sendToHost, error: () => lastHostError };`);
  const result = make(R, new Map([[7, capture ? [{url:capturedUrl, ...capture}] : []]]), new Map(), api,
    async url => { cookies.push(url); return 'session=example'; }, u => u, 'test-host', {error(){}});
  return {...result, calls, cookies};
}
it('routes a signed fragment through its captured Vimeo player and discards fragment metadata', async () => {
  const h = harness({pageUrl:player});
  assert.equal(await h.sendToHost(fragment, 'video.mp4', {pageUrl:'https://example.com/watch', size:1043670, pairUrl:'wrong-part', cookie:'wrong-domain'}), true);
  assert.equal(h.calls[0].url, player);
  assert.equal(h.calls[0].size, null);
  assert.equal(h.calls[0].pairUrl, null);
  assert.equal(h.calls[0].cookie, 'session=example');
  assert.deepEqual(h.cookies, [player]);
});
it('obtains the exact requesting iframe URL when Chromium omits documentUrl', async () => {
  const h = harness({frameId:4}, player);
  assert.equal(await h.sendToHost(fragment, 'video.mp4', {pageUrl:'https://example.com/watch'}), true);
  assert.equal(h.calls[0].url, player);
});
it('refuses unresolved fragments without starting a native download', async () => {
  const h = harness(null);
  assert.equal(await h.sendToHost(fragment, 'video.mp4', {pageUrl:'https://example.com/feed'}), false);
  assert.equal(h.calls.length, 0);
  assert.match(h.error(), /Vimeo playback fragment/);
});
it('leaves ordinary full-file MP4 handoffs unchanged', async () => {
  const h = harness(null);
  const url = 'https://vod.vimeocdn.com/full.mp4';
  assert.equal(await h.sendToHost(url, 'video.mp4', {size:12345}), true);
  assert.equal(h.calls[0].url, url);
  assert.equal(h.calls[0].size, 12345);
});

it('routes captured Vimeo HLS manifests through the player, not video-only HLS assembly', async () => {
  const manifest = 'https://vod-adaptive-ak.vimeocdn.com/video/master.m3u8?token=signed';
  const h = harness({pageUrl:player}, null, manifest);
  assert.equal(await h.sendToHost(manifest, 'video.mp4', {size:100}), true);
  assert.equal(h.calls[0].url, player);
  assert.equal(h.calls[0].size, null);
});
it('preserves a complete Vimeo manifest when no player can be recovered', async () => {
  const manifest = 'https://vod-adaptive-ak.vimeocdn.com/video/master.mpd?token=signed';
  const h = harness(null);
  assert.equal(await h.sendToHost(manifest, 'video.mp4', {pageUrl:'https://example.com/watch', size:100}), true);
  assert.equal(h.calls[0].url, manifest);
  assert.equal(h.calls[0].size, null);
});
it('rejects standalone adaptive MP4 legs without an exact player', async () => {
  const h = harness(null);
  assert.equal(await h.sendToHost('https://vod-adaptive-ak.vimeocdn.com/video.mp4', 'video.mp4', {}), false);
  assert.equal(h.calls.length, 0);
});
