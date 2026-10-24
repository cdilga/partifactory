// Spike rig server for pf-6qma. Throwaway: the part host bead re-implements what this proves.
//
// One process serves every host of the test domain (pf.test for Playwright, localtest.me for
// the iOS Simulator, plus <via>.<apex> when reached through a reverse proxy in Docker):
//   parts.<apex>       workspace: sets the Lax session cookie, /grant, /api/chrome/v1, /webauthn
//   a-part, b-part     part hosts: / = trusted wrapper, /c/<cap>/... = sandboxed part content
//   blobs-part.<apex>  shared blobs (prelude, wrapper JS, modules), ACAO *
//   canary.<apex>      sibling service that records the cookies it receives
//   spike.<apex>       the runner page that drives items and forwards their results
// Every host also answers /_spike/* (collector and probes).
//
// Listeners: HTTPS on SPIKE_TLS_PORT (443 on macOS, bound on all interfaces because macOS only
// lets unprivileged processes bind low ports on the wildcard address), plain HTTP on
// SPIKE_HTTP_PORT for the Docker proxies to forward to, and an HTTP CONNECT proxy on
// SPIKE_PROXY_PORT that resolves *.pf.test and *.localtest.me to 127.0.0.1 for Playwright.

import fs from 'node:fs';
import path from 'node:path';
import https from 'node:https';
import http from 'node:http';
import net from 'node:net';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const OUT = process.env.SPIKE_OUT || path.resolve(HERE, '../../target/sandbox-spike');
const TLS_PORT = +(process.env.SPIKE_TLS_PORT || 443);
const HTTP_PORT = +(process.env.SPIKE_HTTP_PORT || 8080);
const PROXY_PORT = +(process.env.SPIKE_PROXY_PORT || 3128);
const HB_MS = +(process.env.SPIKE_HB_MS || 25000);
const CERTS = path.join(OUT, 'certs');
fs.mkdirSync(path.join(OUT, 'ios-sim'), { recursive: true });

const ROOT_APEXES = ['pf.test', 'localtest.me'];
const SANDBOX =
  'sandbox allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals ' +
  'allow-downloads allow-top-navigation-by-user-activation';

// ---------------------------------------------------------------- state

const LOG = []; // every request: {ts, method, host, url, referer}
const PROBES = new Map(); // probe/nav id -> recorded request headers
const SEEN = new Map(); // t -> [{k, target, cookies, origin, sfs, method}]
const GRANTS = []; // {ts, part, session, referer, path}
const WA = { creds: [] };
const DONE = new Set();

function logLine(s) {
  fs.appendFileSync(path.join(OUT, 'server.events.log'), `${new Date().toISOString()} ${s}\n`);
}

function cookieNames(req) {
  return (req.headers.cookie || '')
    .split(';')
    .map((c) => c.trim().split('=')[0])
    .filter(Boolean);
}

function parseHost(hostHeader) {
  if (!hostHeader) return null;
  const m = /^([^:]+)(?::(\d+))?$/.exec(hostHeader.toLowerCase());
  if (!m) return null;
  const hostname = m[1];
  const port = m[2];
  const root = ROOT_APEXES.find((a) => hostname === a || hostname.endsWith('.' + a));
  if (!root) return null;
  const rest = hostname === root ? '' : hostname.slice(0, -(root.length + 1));
  const labels = rest ? rest.split('.') : [];
  const label = labels[0] || '';
  const via = labels[1] || 'direct';
  const apex = labels.slice(1).concat([root]).join('.');
  const portSuffix = port && port !== '443' ? ':' + port : '';
  return {
    hostname,
    label,
    via,
    apex,
    root,
    portSuffix,
    origin: `https://${hostname}${portSuffix}`,
    url: (l) => `https://${l}.${apex}${portSuffix}`,
  };
}

// ---------------------------------------------------------------- CSP

function partCsp(ctx, variant) {
  const blob = ctx.url('blobs-part');
  const self = ctx.origin;
  const ws = ctx.url('parts');
  const embedder = `https://spike.${ctx.root}`;
  const fa = variant === 'fa-self' ? "frame-ancestors 'self'" : `frame-ancestors ${self} ${ws} ${embedder}`;
  const wssSelf = self.replace(/^https:/, 'wss:');
  if (variant === 'selfonly' || variant === 'explicit' || variant === 'explicithttps') {
    // selfonly: 'self' everywhere. explicithttps: the host spelled out as https://host only.
    // explicit: https://host plus wss://host (an https host-source does not cover wss).
    const s = variant === 'selfonly' ? "'self'" : self;
    const c = variant === 'explicit' ? `${self} ${wssSelf}` : s;
    return [SANDBOX, "default-src 'none'", `script-src ${s} 'unsafe-inline'`, `connect-src ${c}`,
      `img-src ${s}`, `form-action ${s}`, `frame-src ${s}`, "style-src 'unsafe-inline'", fa].join('; ');
  }
  if (variant === 'ro' || variant === 'meta') return fa; // sandbox comes from elsewhere (or not at all)
  const connect = variant === 'relaxed8'
    ? `connect-src 'self' ${self} ${wssSelf} ${blob} ${ctx.url('canary')} ${ws}`
    : `connect-src 'self' ${self} ${wssSelf} ${blob}`; // explicit host: WebKit's 'self' misses fetch (item 1)
  // The recommended policy after item 1: every 'self' also spells the part host out, because
  // WebKit's 'self' does not match the part's own host for fetch from a sandboxed document.
  const S = `'self' ${self}`;
  return [SANDBOX, "default-src 'none'",
    `script-src ${S} 'unsafe-inline' 'unsafe-eval' blob: ${blob}`,
    `style-src ${S} 'unsafe-inline' ${blob}`,
    `img-src ${S} data: blob: ${blob}`,
    `media-src ${S} data: blob: ${blob}`,
    `font-src ${S} data: ${blob}`,
    connect,
    `worker-src ${S} blob:`,
    `frame-src ${S}`,
    `form-action ${S}`,
    "base-uri 'none'",
    "object-src 'none'",
    fa].join('; ');
}

function wrapperCsp(ctx) {
  const blob = ctx.url('blobs-part');
  const ws = ctx.url('parts');
  return [`default-src 'none'`, `script-src ${blob}`, `style-src ${blob} 'unsafe-inline'`,
    `img-src ${blob} data:`, `connect-src ${ws}`, `frame-src 'self'`, `frame-ancestors ${ws}`,
    `base-uri 'none'`, `form-action 'none'`].join('; ');
}

// Part responses: Origin: null gets ACAO null + Vary: Origin; never ACAC, never CORP.
function partHeaders(req, extra = {}) {
  const h = {
    'Referrer-Policy': 'no-referrer',
    'X-Robots-Tag': 'noindex, nofollow',
    'X-Content-Type-Options': 'nosniff',
    'Cache-Control': 'no-store',
    ...extra,
  };
  if (req.headers.origin === 'null') {
    h['Access-Control-Allow-Origin'] = 'null';
    h['Vary'] = 'Origin';
  }
  return h;
}

// ---------------------------------------------------------------- helpers

const TYPES = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8', '.json': 'application/json', '.txt': 'text/plain; charset=utf-8',
  '.gif': 'image/gif', '.css': 'text/css' };
const GIF = Buffer.from('R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==', 'base64');
const SPIKE_JS = fs.readFileSync(path.join(HERE, 'items/spike.js'), 'utf8');

function send(res, status, headers, body) {
  res.writeHead(status, headers);
  res.end(body);
}
function json(res, obj, headers = {}) {
  send(res, 200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', ...headers }, JSON.stringify(obj));
}
const ANY = { 'Access-Control-Allow-Origin': '*' };

function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
  });
}

function template(ctx, q, text) {
  const cap = (q && q.cap) || '';
  return text
    .replaceAll('{{SPIKE_JS}}', `<script>${SPIKE_JS}\nspike.blob=${JSON.stringify(ctx.url('blobs-part'))};</script>`)
    .replaceAll('{{BLOB}}', ctx.url('blobs-part'))
    .replaceAll('{{SELF}}', ctx.origin)
    .replaceAll('{{WS}}', ctx.url('parts'))
    .replaceAll('{{CANARY}}', ctx.url('canary'))
    .replaceAll('{{SIBLING}}', ctx.url('b-part'))
    .replaceAll('{{CAP}}', cap)
    .replaceAll('{{META}}', q && q.csp === 'meta'
      ? '<meta http-equiv="Content-Security-Policy" content="sandbox allow-scripts">' : '');
}

function appendResults(payload, req) {
  const items = Array.isArray(payload) ? payload : [payload];
  for (const p of items) {
    if (!p || typeof p !== 'object') continue;
    const lane = String(p.lane || 'unknown');
    const run = p.run || null;
    if (p.type === 'spike:done') {
      DONE.add(run);
      logLine(`event="spike.done" lane=${lane} run=${run}`);
      continue;
    }
    const file = lane === 'ios-sim' ? path.join(OUT, 'ios-sim/results.jsonl') : path.join(OUT, 'results.jsonl');
    for (const r of p.results || []) {
      const line = {
        event: 'spike.result', ts: new Date().toISOString(), lane, run,
        item: String(r.item), check: String(r.check), outcome: String(r.outcome),
        detail: r.detail == null ? '' : String(r.detail).slice(0, 4000),
        ua: p.ua || req.headers['user-agent'] || '',
      };
      fs.appendFileSync(file, JSON.stringify(line) + '\n');
      console.log(`event="spike.result" item=${line.item} check=${line.check} lane=${lane} outcome=${line.outcome}`);
    }
  }
}

function readResults(lane) {
  const file = lane === 'ios-sim' ? path.join(OUT, 'ios-sim/results.jsonl') : path.join(OUT, 'results.jsonl');
  if (!fs.existsSync(file)) return [];
  return fs.readFileSync(file, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l))
    .filter((r) => !lane || r.lane === lane);
}

function fakeCap() {
  return 'v1.spike.' + crypto.randomBytes(18).toString('base64url') + '.' + crypto.randomBytes(8).toString('base64url');
}

// ---------------------------------------------------------------- /_spike/* (any host)

async function spikeRoute(req, res, u) {
  const p = u.pathname;
  if (req.method === 'OPTIONS') {
    return send(res, 204, { ...ANY, 'Access-Control-Allow-Methods': 'GET, POST', 'Access-Control-Allow-Headers': 'Content-Type', 'Access-Control-Max-Age': '600' });
  }
  if (p === '/_spike/ping') return json(res, { ok: true }, ANY);
  if (p === '/_spike/result' && req.method === 'POST') {
    const body = await readBody(req);
    try { appendResults(JSON.parse(body), req); } catch (e) { return send(res, 400, ANY, 'bad json: ' + e.message); }
    return json(res, { ok: true }, ANY);
  }
  if (p === '/_spike/r') { // image beacon fallback
    try { appendResults(JSON.parse(Buffer.from(u.searchParams.get('d') || '', 'base64').toString('utf8')), req); } catch {}
    return send(res, 200, { 'Content-Type': 'image/gif', ...ANY }, GIF);
  }
  if (p.startsWith('/_spike/headers/')) {
    const id = p.slice('/_spike/headers/'.length);
    const h = PROBES.get(id) || null;
    const opt = PROBES.get(id + ':OPTIONS') || null;
    return json(res, h ? { ...h, preflight: !!opt } : null, ANY);
  }
  if (p === '/_spike/seen') return json(res, SEEN.get(u.searchParams.get('t')) || [], ANY);
  if (p === '/_spike/grants') {
    const part = u.searchParams.get('part');
    return json(res, GRANTS.filter((g) => g.part === part), ANY);
  }
  if (p === '/_spike/leak') {
    // The #pf= fragment must never reach a server: no request line or Referer may contain it.
    const hits = LOG.filter((l) => /[#&?]pf=/.test(l.url) || /pf=/.test(l.referer || ''))
      .map((l) => ({ host: l.host, url: l.url.slice(0, 120), referer: (l.referer || '').slice(0, 120) }));
    return json(res, { scanned: LOG.length, hits }, ANY);
  }
  if (p === '/_spike/results') return json(res, readResults(u.searchParams.get('lane')), ANY);
  if (p === '/_spike/done') return json(res, { done: DONE.has(u.searchParams.get('run')) }, ANY);
  if (p === '/_spike/wa/store' && req.method === 'POST') {
    const b = JSON.parse(await readBody(req));
    WA.creds.push(b.id);
    return json(res, { ok: true }, ANY);
  }
  if (p === '/_spike/wa/creds') return json(res, WA.creds, ANY);
  return send(res, 404, ANY, 'no such spike route');
}

// ---------------------------------------------------------------- hosts

function serveFile(res, file, headers, transform) {
  if (!fs.existsSync(file)) return send(res, 404, headers, 'not found');
  let body = fs.readFileSync(file);
  if (transform) body = transform(body.toString('utf8'));
  return send(res, 200, { 'Content-Type': TYPES[path.extname(file)] || 'application/octet-stream', ...headers }, body);
}

function workspace(req, res, u, ctx) {
  const p = u.pathname;
  if (p === '/login') {
    const back = u.searchParams.get('back');
    const cookies = [
      '__Host-pf_sess=s1; Path=/; Secure; HttpOnly; SameSite=Lax',
      `pf_dom_lax=1; Domain=${ctx.apex}; Path=/; Secure; SameSite=Lax`,
      `pf_dom_none=1; Domain=${ctx.apex}; Path=/; Secure; SameSite=None`,
    ];
    res.setHeader('Set-Cookie', cookies);
    if (back) return send(res, 302, { Location: back }, '');
    return send(res, 200, { 'Content-Type': 'text/html' }, '<p>signed in (spike)</p>');
  }
  if (p === '/grant') {
    const part = u.searchParams.get('part') || '';
    const gp = u.searchParams.get('path') || '/';
    const session = cookieNames(req).includes('__Host-pf_sess');
    GRANTS.push({ ts: Date.now(), part, session, referer: req.headers.referer || null, path: gp,
      sfs: req.headers['sec-fetch-site'] || null });
    if (!/^[a-z0-9-]+$/.test(part) || !gp.startsWith('/')) return send(res, 400, {}, 'bad grant');
    if (!session) return send(res, 200, { 'Content-Type': 'text/html' }, '<h1>Sign in (spike stub)</h1>');
    const loc = `https://${part}-part.${ctx.apex}${ctx.portSuffix}${gp}#pf=${fakeCap()}`;
    return send(res, 302, { Location: loc, 'Cache-Control': 'no-store' }, '');
  }
  const m = /^\/api\/chrome\/v1\/([a-z0-9-]+)\/echo$/.exec(p);
  if (m) {
    const allowed = `https://${m[1]}-part.${ctx.apex}${ctx.portSuffix}`;
    const h = { Vary: 'Origin', 'Cache-Control': 'no-store' };
    if (req.headers.origin === allowed) {
      Object.assign(h, { 'Access-Control-Allow-Origin': allowed, 'Access-Control-Allow-Credentials': 'true',
        'Access-Control-Allow-Methods': 'GET, POST', 'Access-Control-Allow-Headers': 'Content-Type',
        'Access-Control-Max-Age': '600' });
    }
    if (req.method === 'OPTIONS') return send(res, 204, h, '');
    return json(res, { cookies: cookieNames(req), origin: req.headers.origin || null,
      sfs: req.headers['sec-fetch-site'] || null, mode: req.headers['sec-fetch-mode'] || null }, h);
  }
  if (p === '/echo') return echo(req, res, u, 'workspace', {}); // never CORS for Origin: null
  if (p === '/webauthn') return serveFile(res, path.join(HERE, 'items/webauthn-register.html'), { 'Cache-Control': 'no-store' });
  return send(res, 404, {}, 'workspace: not found');
}

function echo(req, res, u, target, headers) {
  const t = u.searchParams.get('t');
  if (t) {
    if (!SEEN.has(t)) SEEN.set(t, []);
    SEEN.get(t).push({ k: u.searchParams.get('k'), target, method: req.method, cookies: cookieNames(req),
      origin: req.headers.origin || null, sfs: req.headers['sec-fetch-site'] || null });
  }
  return json(res, { ok: true }, headers);
}

function canary(req, res, u, ctx) {
  if (u.pathname === '/echo') {
    const h = req.headers.origin ? { 'Access-Control-Allow-Origin': req.headers.origin, Vary: 'Origin' } : {};
    return echo(req, res, u, 'canary', h);
  }
  if (u.pathname === '/popup') return serveFile(res, path.join(HERE, 'items/popup.html'), { 'Cache-Control': 'no-store' });
  return send(res, 404, {}, 'canary: not found');
}

function blobs(req, res, u, ctx) {
  const m = /^\/b\/([a-z0-9-]+\.m?js)$/.exec(u.pathname);
  if (!m) return send(res, 404, ANY, 'blob not found');
  return serveFile(res, path.join(HERE, 'blobs', m[1]),
    { ...ANY, 'Cache-Control': 'public, max-age=31536000, immutable', 'X-Content-Type-Options': 'nosniff' });
}

const ITEM_ALIASES = { '2top': '2', '9a': '9', '9b': '9', '9c': '9', '8c': '8', '1x': '1', '1h': '1', '6src': '6' };

function partHost(req, res, u, ctx) {
  const p = u.pathname;
  const cm = /^\/c\/(v1\.[A-Za-z0-9_.-]+)(\/.*)?$/.exec(p);
  if (!cm) {
    if (p === '/_pf/webauthn') {
      return serveFile(res, path.join(HERE, 'items/webauthn-get.html'),
        { 'Content-Security-Policy': wrapperCsp(ctx), 'Cache-Control': 'no-store' }, (t) => template(ctx, null, t));
    }
    if (p.startsWith('/c/')) return send(res, 404, {}, 'not found'); // bad cap looks like no part
    // The trusted wrapper: served at / and any non-reserved path; same HTML for every load.
    return serveFile(res, path.join(HERE, 'items/wrapper.html'),
      { 'Content-Security-Policy': wrapperCsp(ctx), 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' },
      (t) => template(ctx, null, t));
  }
  const cap = cm[1];
  const rest = cm[2] || '/';
  const q = { cap, csp: u.searchParams.get('csp') || '' };
  if (req.method === 'OPTIONS') return send(res, 204, partHeaders(req), '');

  let im = /^\/items\/([A-Za-z0-9]+)\.html$/.exec(rest);
  if (im) {
    const key = im[1];
    const file = path.join(HERE, 'items', (ITEM_ALIASES[key] || key) + '.html');
    const variant = q.csp || { '1x': 'explicit', '1': 'selfonly', '8': 'relaxed8', '9b': 'ro', '9c': 'meta' }[key] || 'canonical';
    if (variant === 'explicit' || variant === 'selfonly' || variant === 'explicithttps') q.csp = variant;
    if (key === '9b') q.csp = 'ro';
    if (key === '9c') q.csp = 'meta';
    const extra = { 'Content-Security-Policy': partCsp(ctx, variant) };
    if (variant === 'ro') extra['Content-Security-Policy-Report-Only'] = 'sandbox allow-scripts';
    return serveFile(res, file, partHeaders(req, extra), (t) => template(ctx, q, t));
  }
  im = /^\/assets\/([a-z0-9-]+\.(?:js|mjs|json))$/.exec(rest);
  if (im) return serveFile(res, path.join(HERE, 'items/assets', im[1]), partHeaders(req));
  if (rest === '/probe') {
    const h = {};
    for (const k of ['origin', 'sec-fetch-site', 'sec-fetch-mode', 'sec-fetch-dest', 'cookie', 'referer']) h[k] = req.headers[k] || null;
    return json(res, h, partHeaders(req));
  }
  if (rest === '/formsink') {
    const id = (u.searchParams.get('id') || '').replace(/[^a-z0-9]/g, '');
    return send(res, 200, partHeaders(req, { 'Content-Type': 'text/html' }),
      `<script>parent.postMessage('formsink:${id}', '*')</script>${req.method}`);
  }
  if (rest === '/px.gif') return send(res, 200, partHeaders(req, { 'Content-Type': 'image/gif' }), GIF);
  if (rest === '/dl/file.txt') {
    const extra = { 'Content-Type': 'text/plain' };
    if (u.searchParams.get('dl') === '1') extra['Content-Disposition'] = 'attachment; filename="pf-report.txt"';
    return send(res, 200, partHeaders(req, extra), 'spike download\n');
  }
  if (rest === '/api/sse' || rest === '/_rt/events') return sse(req, res, ctx, rest === '/_rt/events' ? 'rt' : 'api');
  // Anything else under /c/<cap>/ is the part's entry document (used by the wrapper flow).
  const variant = q.csp || 'canonical';
  return serveFile(res, path.join(HERE, 'items/part.html'),
    partHeaders(req, { 'Content-Security-Policy': partCsp(ctx, variant) }), (t) => template(ctx, q, t));
}

function sse(req, res, ctx, kind) {
  const h = partHeaders(req, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' });
  if (kind === 'rt') h['X-Accel-Buffering'] = 'no'; // what the shipped /_rt/events will send
  res.writeHead(200, h);
  const t0 = Date.now();
  res.write(`retry: 2000\nevent: hello\ndata: ${JSON.stringify({ origin: req.headers.origin || null, via: ctx.via, kind })}\n\n`);
  let n = 0;
  const iv = setInterval(() => res.write(`event: hb\ndata: ${++n}\n\n`), HB_MS);
  req.on('close', () => {
    clearInterval(iv);
    logLine(`event="spike.sse.close" via=${ctx.via} kind=${kind} after_ms=${Date.now() - t0} hbs=${n}`);
  });
}

// ---------------------------------------------------------------- main handler

async function handle(req, res) {
  const ctx = parseHost(req.headers.host);
  const u = new URL(req.url, 'https://x');
  LOG.push({ ts: Date.now(), method: req.method, host: req.headers.host, url: req.url, referer: req.headers.referer || null });
  if (LOG.length > 20000) LOG.shift();
  for (const key of ['probe', 'nav']) {
    const id = u.searchParams.get(key);
    if (id) {
      const rec = { method: req.method, path: u.pathname };
      for (const k of ['origin', 'sec-fetch-site', 'sec-fetch-mode', 'sec-fetch-dest', 'sec-fetch-user', 'accept', 'referer']) {
        rec[k] = req.headers[k] || null;
      }
      rec.cookies = cookieNames(req);
      PROBES.set(req.method === 'OPTIONS' ? id + ':OPTIONS' : id, rec);
    }
  }
  try {
    if (u.pathname.startsWith('/_spike/')) return await spikeRoute(req, res, u);
    if (!ctx) return send(res, 404, {}, 'unknown host');
    switch (ctx.label) {
      case 'parts': return workspace(req, res, u, ctx);
      case 'canary': return canary(req, res, u, ctx);
      case 'blobs-part': return blobs(req, res, u, ctx);
      case 'spike':
        if (u.pathname === '/run') return serveFile(res, path.join(HERE, 'runner.html'), { 'Cache-Control': 'no-store' });
        if (u.pathname === '/runner.js') return serveFile(res, path.join(HERE, 'runner.js'), { 'Cache-Control': 'no-store' });
        return send(res, 404, {}, 'spike: not found');
      default:
        if (ctx.label.endsWith('-part')) return partHost(req, res, u, ctx);
        return send(res, 404, {}, 'not a part host');
    }
  } catch (e) {
    console.error(e);
    if (!res.headersSent) send(res, 500, {}, String(e));
  }
}

// ---------------------------------------------------------------- WebSocket

const wss = new WebSocketServer({ noServer: true });
function onUpgrade(req, socket, head) {
  const u = new URL(req.url, 'https://x');
  if (!/^\/c\/v1\.[A-Za-z0-9_.-]+\/api\/ws$/.test(u.pathname)) return socket.destroy();
  const ctx = parseHost(req.headers.host) || { via: '?' };
  const origin = req.headers.origin || null;
  wss.handleUpgrade(req, socket, head, (ws) => {
    const t0 = Date.now();
    ws.send(JSON.stringify({ type: 'hello', origin, via: ctx.via }));
    let n = 0;
    const iv = setInterval(() => ws.send(JSON.stringify({ type: 'hb', n: ++n })), HB_MS);
    ws.on('message', (m) => ws.send(JSON.stringify({ type: 'echo', data: String(m) })));
    ws.on('close', (code, reason) => {
      clearInterval(iv);
      logLine(`event="spike.ws.close" via=${ctx.via} origin=${origin} code=${code} reason="${reason}" after_ms=${Date.now() - t0} hbs=${n}`);
    });
  });
}

// ---------------------------------------------------------------- listeners

const tls = https.createServer({ key: fs.readFileSync(path.join(CERTS, 'key.pem')), cert: fs.readFileSync(path.join(CERTS, 'fullchain.pem')) }, handle);
tls.on('upgrade', onUpgrade);
tls.listen(TLS_PORT, '::', () => console.log(`spike: https on :${TLS_PORT}`));

const plain = http.createServer(handle);
plain.on('upgrade', onUpgrade);
plain.listen(HTTP_PORT, '::', () => console.log(`spike: http (proxy backend + collector) on :${HTTP_PORT}`));

// CONNECT proxy for Playwright: *.pf.test / *.localtest.me -> 127.0.0.1 (443 -> TLS_PORT).
const proxy = http.createServer((req, res) => send(res, 405, {}, 'CONNECT only'));
proxy.on('connect', (req, client, head) => {
  const [host, portStr] = req.url.split(':');
  const port = +(portStr || 443);
  if (!ROOT_APEXES.some((a) => host === a || host.endsWith('.' + a))) {
    client.end('HTTP/1.1 403 Forbidden\r\n\r\n');
    return;
  }
  const upstream = net.connect(port === 443 ? TLS_PORT : port, '127.0.0.1', () => {
    client.write('HTTP/1.1 200 Connection Established\r\n\r\n');
    upstream.write(head);
    upstream.pipe(client);
    client.pipe(upstream);
  });
  upstream.on('error', () => client.destroy());
  client.on('error', () => upstream.destroy());
});
proxy.listen(PROXY_PORT, '127.0.0.1', () => console.log(`spike: CONNECT proxy on 127.0.0.1:${PROXY_PORT}`));
