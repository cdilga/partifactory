// A local stand-in for partifactory's prelude, for developing parts on a laptop without a
// server: window.storage over localStorage, data.changed over BroadcastChannel, so two tabs on
// the same origin behave like two people on one part. Never shipped into a part; on a real
// partifactory host the prelude provides all of this.
(function () {
  if (window.storage && window.partifactory) return;
  const NS = "pf-dev-shim:";
  const chan = new BroadcastChannel("pf-dev-shim");
  const me = sessionStorage.getItem(NS + "me") || "Guest " + Math.random().toString(16).slice(2, 6);
  sessionStorage.setItem(NS + "me", me);
  const listeners = { "data.changed": [] };
  const k = (key, shared) => NS + (shared ? "s:" : "p:") + key;
  const meta = (key, shared) => {
    try { return JSON.parse(localStorage.getItem(k(key, shared) + "\u0000meta")) || null; } catch { return null; }
  };
  const latency = () => new Promise(r => setTimeout(r, 20 + Math.random() * 60));

  window.storage = {
    async get(key, shared) {
      await latency();
      const v = localStorage.getItem(k(key, shared));
      return v === null ? null : { key, value: v, shared: !!shared, by: meta(key, shared)?.by, updated_at: meta(key, shared)?.at };
    },
    async getMany(keys, shared) { return Promise.all(keys.map(x => this.get(x, shared))); },
    async set(key, value, shared) {
      await latency();
      localStorage.setItem(k(key, shared), String(value));
      localStorage.setItem(k(key, shared) + "\u0000meta", JSON.stringify({ by: { kind: "guest", name: me }, at: Date.now() }));
      if (shared) chan.postMessage({ key, op: "set", by: { kind: "guest", name: me } });
      return { key, value: String(value), shared: !!shared };
    },
    async delete(key, shared) {
      await latency();
      localStorage.removeItem(k(key, shared));
      localStorage.removeItem(k(key, shared) + "\u0000meta");
      if (shared) chan.postMessage({ key, op: "delete", by: { kind: "guest", name: me } });
      return { key, deleted: true, shared: !!shared };
    },
    async list(prefix = "", shared, opts = {}) {
      if (typeof shared === "object") { opts = shared; shared = !!opts.shared; }
      await latency();
      const base = k("", shared);
      const keys = Object.keys(localStorage)
        .filter(x => x.startsWith(base + prefix) && !x.endsWith("\u0000meta"))
        .map(x => x.slice(base.length)).sort();
      const out = { keys, prefix, shared: !!shared, next_cursor: null };
      if (opts.values) {
        out.values = {};
        out.meta = {};
        for (const key of keys) { out.values[key] = localStorage.getItem(k(key, shared)); out.meta[key] = meta(key, shared); }
      }
      return out;
    },
  };
  chan.onmessage = e => listeners["data.changed"].forEach(fn => fn(e.data));
  window.partifactory = {
    __shim: true,
    basePath: "./",
    ready() { console.log("[pf-dev-shim] ready()"); },
    on(event, fn) { (listeners[event] = listeners[event] || []).push(fn); },
    async user() { return { kind: "guest", name: me }; },
  };
})();
