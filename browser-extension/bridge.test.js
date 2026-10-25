// The app link: request context capture (headers, POST bodies, IP, proxy)
// and the persistent port's request/reply matching, with browser APIs stubbed.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(__dirname + '/bridge.js', 'utf8');

function load() {
  const listeners = {};
  const posted = [];
  let portMsg, portGone;
  const ev = (name) => ({ addListener: (f) => { listeners[name] = f; } });
  const chrome = {
    runtime: {
      lastError: null,
      connectNative: () => ({
        onMessage: { addListener: (f) => { portMsg = f; } },
        onDisconnect: { addListener: (f) => { portGone = f; } },
        postMessage: (m) => posted.push(m),
      }),
      sendNativeMessage: async (h, m) => ({ type: 'ok', via: 'oneshot', m }),
    },
    webRequest: {
      onBeforeRequest: ev('before'), onSendHeaders: ev('headers'),
      onBeforeRedirect: ev('redirect'), onResponseStarted: ev('started'),
      OnSendHeadersOptions: { EXTRA_HEADERS: 'extraHeaders' },
    },
    proxy: { settings: { get: async () => ({ value: { mode: 'fixed_servers', rules: { singleProxy: { scheme: 'socks5', host: '10.0.0.2', port: 1080 } } } }), onChange: ev('proxy') } },
  };
  const ctx = { chrome, console, setTimeout, clearTimeout, TextEncoder, URL, btoa: (s) => Buffer.from(s, 'binary').toString('base64'), Date, Map, Set, Promise, Object, String, Array, Number, JSON, Uint8Array };
  vm.createContext(ctx);
  vm.runInContext(source + '\nthis.DCBridge = DCBridge;', ctx);
  ctx.DCBridge.start('com.download_craft');
  return { B: ctx.DCBridge, listeners, posted, port: () => portMsg, gone: () => portGone };
}

const dec = (b64) => Buffer.from(b64, 'base64').toString('utf8');

test('a form POST is rebuilt with its headers, IP and the browser proxy', async () => {
  const { B, listeners } = load();
  await new Promise((r) => setTimeout(r, 5));
  listeners.before({ requestId: '1', url: 'https://bank.example/export', method: 'POST',
    requestBody: { formData: { from: ['2026-01-01'], 'q x': ['a&b'] } } });
  listeners.headers({ requestId: '1', requestHeaders: [
    { name: 'User-Agent', value: 'UA/1' }, { name: 'Content-Type', value: 'application/x-www-form-urlencoded' },
    { name: 'Cookie', value: 'sid=1' }, { name: 'Referer', value: 'https://bank.example/' }, { name: 'Range', value: 'bytes=0-' }] });
  listeners.started({ requestId: '1', url: 'https://bank.example/export', ip: '198.51.100.7' });
  const c = B.browserContext('https://bank.example/export');
  assert.equal(c.method, 'POST');
  assert.equal(dec(c.body), 'from=2026-01-01&q+x=a%26b');
  assert.deepEqual([...Object.keys(c.headers)].sort(), ['content-type', 'user-agent']);
  assert.equal(c.serverIp, '198.51.100.7');
  assert.equal(c.proxy, 'socks5://10.0.0.2:1080');
  assert.equal(c._cookie, 'sid=1');
  assert.equal(c._referer, 'https://bank.example/');
  assert.equal(c.fromExtension, true);
});

test('multipart bodies keep their boundary; uploaded files are not repeatable', () => {
  const { B, listeners } = load();
  listeners.before({ requestId: '2', url: 'https://s.example/r', method: 'POST', requestBody: { formData: { a: ['1'] } } });
  listeners.headers({ requestId: '2', requestHeaders: [{ name: 'Content-Type', value: 'multipart/form-data; boundary=XyZ' }] });
  const c = B.browserContext('https://s.example/r');
  assert.equal(dec(c.body), '--XyZ\r\nContent-Disposition: form-data; name="a"\r\n\r\n1\r\n--XyZ--\r\n');
  listeners.before({ requestId: '3', url: 'https://s.example/up', method: 'POST', requestBody: { raw: [{ bytes: new Uint8Array([1]).buffer }, { file: 'C:/x' }] } });
  const u = B.browserContext('https://s.example/up');
  assert.equal(u.method, undefined);
  assert.equal(u._post, true);
});

test('redirect targets map back to the request; GET drops Content-Type', () => {
  const { B, listeners } = load();
  listeners.before({ requestId: '4', url: 'https://a.example/get', method: 'GET' });
  listeners.headers({ requestId: '4', requestHeaders: [{ name: 'Accept-Language', value: 'bn' }, { name: 'Content-Type', value: 'x' }] });
  listeners.redirect({ requestId: '4', redirectUrl: 'https://cdn.example/file.zip' });
  const c = B.browserContext('https://cdn.example/file.zip', { visitorData: 'Cgt4' });
  assert.deepEqual(JSON.parse(JSON.stringify(c.headers)), { 'accept-language': 'bn' });
  assert.equal(c.visitorData, 'Cgt4');
  assert.equal(B.browserContext('https://unknown.example/').headers, undefined);
});

test('replies find their request by id; pushes reach their handlers', async () => {
  const { B, posted, port } = load();
  const hello = posted.find((m) => m.type === 'hello');
  port()({ type: 'pong', version: 2, rid: hello.rid });
  await new Promise((r) => setTimeout(r, 5));
  assert.equal(B.live(), true);
  const got = [];
  B.on('event', (m) => got.push(m.event));
  const p = B.send({ type: 'list' });
  const sent = posted.find((m) => m.type === 'list');
  port()({ type: 'event', event: 'done', id: 1 });
  port()({ type: 'list', items: [], rid: sent.rid });
  assert.equal((await p).items.length, 0);
  assert.deepEqual(got, ['done']);
});

test('without the live port every request still goes through the one-shot host', async () => {
  const { B } = load();
  const r = await B.send({ type: 'ping' });
  assert.equal(r.via, 'oneshot');
});
