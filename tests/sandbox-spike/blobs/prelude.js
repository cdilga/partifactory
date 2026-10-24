// Spike stand-in for the prelude's storage shims (item 4). Runs first in <head>, from the blob
// host, before any page script. Records what native access did and whether each redefinition
// took, in window.__pfPrelude.
(function () {
  var native = {};
  ['localStorage', 'sessionStorage', 'indexedDB', 'caches'].forEach(function (n) {
    try {
      var v = window[n];
      native[n] = v === undefined ? 'undefined' : 'available';
    } catch (e) {
      native[n] = 'threw ' + e.name;
    }
  });
  try {
    document.cookie;
    native.cookie = 'readable';
  } catch (e) {
    native.cookie = 'threw ' + e.name;
  }

  // In-memory Storage behind a Proxy, so both getItem/setItem and property access work.
  function mem() {
    var m = new Map();
    var api = {
      __pf: true,
      get length() { return m.size; },
      key: function (i) { var k = Array.from(m.keys())[i]; return k === undefined ? null : k; },
      getItem: function (k) { k = String(k); return m.has(k) ? m.get(k) : null; },
      setItem: function (k, v) { m.set(String(k), String(v)); },
      removeItem: function (k) { m.delete(String(k)); },
      clear: function () { m.clear(); },
    };
    return new Proxy(api, {
      get: function (t, k) { return k in t ? t[k] : typeof k === 'string' ? t.getItem(k) : undefined; },
      set: function (t, k, v) { t.setItem(k, v); return true; },
      deleteProperty: function (t, k) { t.removeItem(k); return true; },
      has: function (t, k) { return k in t || m.has(String(k)); },
      ownKeys: function () { return Array.from(m.keys()); },
      getOwnPropertyDescriptor: function (t, k) {
        return m.has(String(k)) ? { value: m.get(String(k)), enumerable: true, configurable: true, writable: true } : undefined;
      },
    });
  }
  var r = {};
  ['localStorage', 'sessionStorage'].forEach(function (n) {
    try {
      Object.defineProperty(window, n, { value: mem(), configurable: true, enumerable: true, writable: false });
      r[n] = 'ok';
    } catch (e) {
      r[n] = e.name + ': ' + e.message;
    }
  });
  var jar = '';
  try {
    Object.defineProperty(document, 'cookie', {
      configurable: true,
      get: function () { return jar; },
      set: function (v) { var kv = String(v).split(';')[0]; jar = jar ? jar + '; ' + kv : kv; },
    });
    r.cookie = 'ok';
  } catch (e) {
    r.cookie = e.name + ': ' + e.message;
  }
  ['indexedDB', 'caches'].forEach(function (n) {
    try {
      Object.defineProperty(window, n, { value: undefined, configurable: true });
      r[n] = 'ok';
    } catch (e) {
      r[n] = e.name + ': ' + e.message;
    }
  });
  window.__pfPrelude = { native: native, redefine: r };
})();
