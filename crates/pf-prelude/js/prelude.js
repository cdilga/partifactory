/* partifactory prelude (PLAN §2 "The prelude"). First script in <head> of every part document,
   served from the shared blob host. It makes Claude-written pages run unchanged in the opaque-origin
   sandbox: storage shims, URL prefixing for the cap path, blob workers, window.storage and
   window.partifactory over /_rt, error reporting, and the wrapper's postMessage schema
   (crates/pf-prelude/PROTOCOL.md). Plain ES2017, no build step. */
(function () {
  'use strict';
  var W = window, D = document, L = location;
  if (W.partifactory && W.partifactory.__pf) return;
  var m = /^\/c\/([^/]+)\//.exec(L.pathname);
  var BASE = m ? '/c/' + m[1] + '/' : '/';
  var ORIGIN = L.protocol + '//' + L.host;
  var RT = ORIGIN + BASE + '_rt/v1';
  var TAB = Math.random().toString(36).slice(2, 10);
  var framed = false;
  try { framed = W.parent !== W; } catch (e) { framed = true; }
  var parentPost = function (msg) { if (framed) { try { W.parent.postMessage(msg, '*'); } catch (e) {} } };

  // ------------------------------------------------------------------ boot snapshot (lazy)
  // The server puts <script type="application/json" id="pf-boot"> right after this tag, so it is
  // parsed before any page script runs; read it on first use.
  var boot = null;
  function getBoot() {
    if (boot) return boot;
    boot = { personal: {}, scope: 'none' };
    try {
      var el = D.getElementById('pf-boot');
      if (el) boot = JSON.parse(el.textContent || '{}');
      if (!boot.personal) boot.personal = {};
    } catch (e) {}
    return boot;
  }

  // ------------------------------------------------------------------ /_rt client
  function stripCap(u) { return String(u || '').replace(/\/c\/[^/?#\s"']+/g, ''); }
  function PfError(e, status) {
    var err = new Error((e && e.message) || 'request failed');
    err.name = 'PfError';
    err.code = (e && e.code) || 'internal';
    err.retryable = err.code === 'rate_limited' || status >= 500;
    if (e && e.details && e.details.retry_after) err.retry_after = e.details.retry_after;
    err.details = (e && e.details) || {};
    return err;
  }
  var regranting = false;
  function regrant() {
    if (regranting) return;
    regranting = true;
    if (framed) parentPost({ type: 'pf:regrant' });
    else L.reload();
  }
  function rt(op, args) {
    return nativeFetch(RT, {
      method: 'POST', headers: { 'Content-Type': 'text/plain' }, credentials: 'omit',
      body: JSON.stringify({ op: op, args: args || {} })
    }).then(function (r) {
      if (r.status === 401 && r.headers.get('X-PF-Reauth')) regrant();
      return r.text().then(function (t) {
        var j = null;
        try { j = JSON.parse(t); } catch (e) {}
        if (!r.ok) throw PfError(j && j.error, r.status);
        return j ? j.ok : null;
      });
    });
  }

  // ------------------------------------------------------------------ URL prefixing
  function fix(u) {
    if (u == null) return u;
    var s = typeof u === 'string' ? u : (u instanceof URL ? u.href : null);
    if (s === null) return u;
    var url;
    try { url = new URL(s, L.href); } catch (e) { return u; }
    if (url.protocol !== L.protocol || url.host !== L.host) return u;
    if (url.pathname.indexOf('/c/') === 0) return u;
    if (s.charAt(0) !== '/' && !/^[a-z]+:/i.test(s)) return u; // relative: already under the cap
    url.pathname = BASE + url.pathname.slice(1);
    return url.href;
  }
  var nativeFetch = W.fetch.bind(W);
  W.fetch = function (input, init) {
    if (typeof Request !== 'undefined' && input instanceof Request) {
      var f = fix(input.url);
      if (f !== input.url) input = new Request(f, input);
      return nativeFetch(input, init);
    }
    return nativeFetch(fix(input), init);
  };
  if (W.XMLHttpRequest) {
    var xo = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (method, url) {
      var a = Array.prototype.slice.call(arguments);
      a[1] = fix(url);
      return xo.apply(this, a);
    };
  }
  var NES = W.EventSource;
  if (NES) {
    var ES = function EventSource(url, opts) { return new NES(fix(url), opts); };
    ES.prototype = NES.prototype;
    ES.CONNECTING = 0; ES.OPEN = 1; ES.CLOSED = 2;
    W.EventSource = ES;
  }

  // ------------------------------------------------------------------ workers from blob: URLs
  // A worker can't be started from the part's URL in an opaque origin (spike item 5), so start a
  // classic worker from a blob: URL that loads the real script by absolute URL. Relative URLs in
  // the worker resolve against the script's real location.
  function workerSrc(abs, module) {
    var shim = 'var __pfBase=' + JSON.stringify(abs) + ';' +
      '(function(){var f=self.fetch,i=self.importScripts;' +
      'var r=function(u){return typeof u==="string"?new URL(u,__pfBase).href:u};' +
      'self.fetch=function(u,o){return f.call(self,r(u),o)};' +
      'self.importScripts=function(){return i.apply(self,Array.prototype.map.call(arguments,r))};})();';
    return shim + (module
      ? 'import(__pfBase).catch(function(e){setTimeout(function(){throw e})});'
      : 'importScripts(__pfBase);');
  }
  function wrapWorker(Native) {
    if (!Native) return Native;
    var Wrapped = function (url, opts) {
      var s = String(url);
      if (/^(blob|data):/.test(s)) return new Native(url, opts);
      var abs = new URL(fix(s), L.href).href;
      var module = opts && opts.type === 'module';
      var o = {};
      if (opts && opts.name) o.name = opts.name;
      var b = new Blob([workerSrc(abs, module)], { type: 'text/javascript' });
      return new Native(URL.createObjectURL(b), o);
    };
    Wrapped.prototype = Native.prototype;
    return Wrapped;
  }
  W.Worker = wrapWorker(W.Worker);
  if (W.SharedWorker) W.SharedWorker = wrapWorker(W.SharedWorker);

  // Script-created images load with CORS so canvases stay readable (the server answers ACAO null).
  if (W.Image) {
    var NI = W.Image;
    var Img = function Image(w, h) { var i = new NI(w, h); i.crossOrigin = 'anonymous'; return i; };
    Img.prototype = NI.prototype;
    W.Image = Img;
  }

  // ------------------------------------------------------------------ storage shims
  function makeStorage(map, onChange) {
    var api = {
      get length() { return map.size; },
      key: function (i) { var k = Array.from(map.keys())[i]; return k === undefined ? null : k; },
      getItem: function (k) { k = String(k); return map.has(k) ? map.get(k) : null; },
      setItem: function (k, v) { k = String(k); v = String(v); map.set(k, v); if (onChange) onChange(k, v); },
      removeItem: function (k) { k = String(k); if (map.has(k)) { map.delete(k); if (onChange) onChange(k, null); } },
      clear: function () { Array.from(map.keys()).forEach(function (k) { api.removeItem(k); }); }
    };
    return new Proxy(api, {
      get: function (t, k) { return k in t ? t[k] : (typeof k === 'string' ? t.getItem(k) : undefined); },
      set: function (t, k, v) { t.setItem(k, v); return true; },
      deleteProperty: function (t, k) { t.removeItem(k); return true; },
      has: function (t, k) { return k in t || map.has(String(k)); },
      ownKeys: function () { return Array.from(map.keys()); },
      getOwnPropertyDescriptor: function (t, k) {
        return map.has(String(k)) ? { value: map.get(String(k)), enumerable: true, configurable: true, writable: true } : undefined;
      }
    });
  }
  // localStorage: seeded from pf-boot, written behind to personal state (collection __storage).
  var lsMap = null, dirty = {}, timer = null;
  function lsSeed() {
    if (lsMap) return lsMap;
    lsMap = new Map();
    var p = getBoot().personal;
    Object.keys(p).forEach(function (k) { lsMap.set(k, String(p[k])); });
    return lsMap;
  }
  function takeDirty() {
    var set = {}, del = [], n = 0;
    Object.keys(dirty).forEach(function (k) { n++; if (dirty[k] === null) del.push(k); else set[k] = dirty[k]; });
    dirty = {};
    return n ? { set: set, delete: del, tab: TAB } : null;
  }
  function flush() {
    timer = null;
    var b = takeDirty();
    if (b) rt('storage.sync', b).catch(function (e) { reportError('error', 'localStorage sync failed: ' + e.code + ': ' + e.message); });
  }
  function lsChanged(k, v) {
    if (getBoot().scope !== 'person') return; // public links, guests, render: memory only
    dirty[k] = v;
    if (!timer) timer = setTimeout(flush, 300);
  }
  var lsProxy = null;
  function ls() { if (!lsProxy) lsProxy = makeStorage(lsSeed(), lsChanged); return lsProxy; }
  function define(obj, name, desc) { try { Object.defineProperty(obj, name, desc); } catch (e) {} }
  define(W, 'localStorage', { configurable: true, enumerable: true, get: ls });
  var ss = makeStorage(new Map(), null);
  define(W, 'sessionStorage', { configurable: true, enumerable: true, get: function () { return ss; } });
  W.addEventListener('pagehide', function () {
    if (timer) { clearTimeout(timer); timer = null; }
    var b = takeDirty();
    if (!b) return;
    // A text/plain beacon uses no-cors, which the runtime's Fetch Metadata guard rejects.
    // Keepalive survives teardown while explicit cors preserves the opaque-origin contract.
    nativeFetch(RT, {
      method: 'POST', mode: 'cors', credentials: 'omit', keepalive: true,
      headers: { 'Content-Type': 'text/plain' },
      body: JSON.stringify({ op: 'storage.sync', args: b })
    }).catch(function () {});
  });
  // document.cookie: an inert in-memory string, never sent anywhere.
  var jar = {};
  define(D, 'cookie', {
    configurable: true,
    get: function () { return Object.keys(jar).map(function (k) { return k + '=' + jar[k]; }).join('; '); },
    set: function (v) {
      var kv = String(v).split(';')[0], i = kv.indexOf('=');
      var k = (i < 0 ? '' : kv.slice(0, i)).trim(), val = i < 0 ? kv : kv.slice(i + 1);
      if (/max-age=0|expires=thu, 01 jan 1970/i.test(String(v))) delete jar[k]; else jar[k] = val;
    }
  });
  // indexedDB and caches are reachable natively but throw; make feature detection fall back.
  define(W, 'indexedDB', { configurable: true, value: undefined });
  define(W, 'caches', { configurable: true, value: undefined });

  // ------------------------------------------------------------------ window.storage
  function sharedArg(s) { return s === true || (s && typeof s === 'object' && s.shared === true); }
  W.storage = {
    get: function (key, shared) { return rt('storage.get', { key: key, shared: sharedArg(shared) }); },
    getMany: function (keys, shared) { return rt('storage.getMany', { keys: keys, shared: sharedArg(shared) }); },
    set: function (key, value, shared) {
      return rt('storage.set', { key: key, value: String(value), shared: sharedArg(shared), tab: TAB });
    },
    delete: function (key, shared) { return rt('storage.delete', { key: key, shared: sharedArg(shared), tab: TAB }); },
    list: function (prefix, shared, opts) {
      if (shared && typeof shared === 'object') { opts = shared; shared = shared.shared; }
      opts = opts || {};
      return rt('storage.list', {
        prefix: prefix || '', shared: !!shared, cursor: opts.cursor || null,
        limit: opts.limit || 1000, values: !!opts.values, meta: !!opts.meta
      });
    }
  };

  // ------------------------------------------------------------------ live events
  var listeners = {};
  var es = null;
  function emit(name, data) { (listeners[name] || []).slice().forEach(function (fn) { try { fn(data); } catch (e) { setTimeout(function () { throw e; }); } }); }
  function openEvents() {
    if (es || !NES || getBoot().events === false) return;
    es = new NES(ORIGIN + BASE + '_rt/events');
    es.addEventListener('data.changed', function (e) {
      var d; try { d = JSON.parse(e.data); } catch (x) { return; }
      if (d.tab && d.tab === TAB) return; // this tab's own write
      delete d.tab;
      emit('data.changed', d);
    });
    es.addEventListener('pf:version', function (e) {
      if (framed) parentPost({ type: 'pf:version' });
      else L.reload();
    });
    es.onerror = function () {
      if (es && es.readyState === 2) { es = null; nativeFetch(L.href, { method: 'HEAD' }).then(function (r) { if (r.status === 401) regrant(); }); }
    };
  }

  // ------------------------------------------------------------------ window.partifactory
  var readyCalled = false, userP = null;
  W.partifactory = {
    __pf: true,
    basePath: BASE,
    ready: function () {
      if (readyCalled) return;
      readyCalled = true;
      W.__pfReadyAt = Date.now();
      parentPost({ type: 'pf:ready' });
      try { D.dispatchEvent(new Event('pf:ready')); } catch (e) {}
    },
    on: function (name, fn) {
      (listeners[name] = listeners[name] || []).push(fn);
      openEvents();
      return function () { listeners[name] = (listeners[name] || []).filter(function (f) { return f !== fn; }); };
    },
    user: function () {
      if (!userP) userP = rt('user', {}).catch(function (e) { userP = null; throw e; });
      return userP;
    },
    refresh: function () { return rt('refresh', {}); }
  };

  // ------------------------------------------------------------------ error reporting
  var sent = {}, nSent = 0;
  function reportError(kind, message, stack, url) {
    var fp = kind + '|' + message + '|' + String(stack || '').split('\n')[1];
    if (sent[fp] || nSent >= 20) return;
    sent[fp] = 1; nSent++;
    var args = { kind: kind, message: String(message).slice(0, 2000), stack: stack ? stripCap(stack).slice(0, 8000) : null, url: url ? stripCap(url) : stripCap(L.pathname) };
    W.__pfErrors = (W.__pfErrors || []).concat([args]);
    rt('errors.report', args).catch(function () {});
  }
  W.addEventListener('error', function (e) {
    var t = e.target;
    if (t && t !== W && (t.src || t.href)) {
      reportError('blocked_load', 'failed to load ' + (t.tagName || '').toLowerCase(), null, t.src || t.href);
    } else {
      reportError('error', e.message || 'error', e.error && e.error.stack, e.filename);
    }
  }, true);
  W.addEventListener('unhandledrejection', function (e) {
    var r = e.reason;
    reportError('unhandledrejection', r && r.message ? r.message : String(r), r && r.stack, null);
  });
  D.addEventListener('securitypolicyviolation', function (e) {
    reportError('csp', 'blocked by CSP ' + e.effectiveDirective + ': ' + stripCap(e.blockedURI), null, e.sourceFile);
  });

  // ------------------------------------------------------------------ wrapper hand-shake
  parentPost({ type: 'pf:hello', basePath: BASE, v: 1 });
  W.addEventListener('message', function (e) {
    var d = e.data;
    if (!d || typeof d !== 'object' || e.source !== W.parent) return;
    if (d.type === 'pf:ping') parentPost({ type: 'pf:pong', ready: readyCalled });
  });
  W.addEventListener('load', function () { setTimeout(openEvents, 0); });
})();
