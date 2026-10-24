// Inlined into every item page by the server ({{SPIKE_JS}}). Collects {item, check, outcome,
// detail} and reports them: to the parent (the runner) when framed, otherwise straight to the
// collector on the page's own host, falling back to an image beacon on the blob host.
(function () {
  var q = new URLSearchParams(location.search);
  var S = (window.spike = {
    q: q,
    lane: q.get('lane') || 'manual',
    out: [],
    framed: window.parent !== window,
    blob: '',
    rec: function (item, check, outcome, detail) {
      S.out.push({
        item: String(item),
        check: check,
        outcome: outcome,
        detail: detail == null ? '' : typeof detail === 'string' ? detail : JSON.stringify(detail),
      });
    },
    tryit: function (fn) {
      try {
        return { ok: true, v: fn() };
      } catch (e) {
        return { ok: false, err: (e && e.name) + ': ' + (e && e.message) };
      }
    },
    err: function (e) {
      return (e && e.name) + ': ' + (e && e.message);
    },
    rid: function () {
      return Math.random().toString(36).slice(2, 10);
    },
    sleep: function (ms) {
      return new Promise(function (r) { setTimeout(r, ms); });
    },
    withTimeout: function (p, ms, label) {
      return Promise.race([
        p,
        new Promise(function (_, rej) {
          setTimeout(function () { rej(new Error('timeout ' + (label || '') + ' ' + ms + 'ms')); }, ms);
        }),
      ]);
    },
    // A button that runs `handler` inside the click (transient user activation). Resolves true
    // if clicked within actwait seconds, false otherwise (lanes with no input, e.g. simctl).
    activation: function (id, label, handler) {
      var wait = +(q.get('actwait') || 8) * 1000;
      return new Promise(function (res) {
        var b = document.createElement('button');
        b.id = id;
        b.textContent = label;
        b.style.cssText = 'font-size:18px;margin:4px;padding:8px';
        (document.body || document.documentElement).appendChild(b);
        var t = setTimeout(function () { res(false); }, wait);
        b.addEventListener('click', function () {
          clearTimeout(t);
          var p;
          try { p = handler(); } catch (e) { p = null; }
          Promise.resolve(p).then(function () { res(true); }, function () { res(true); });
        }, { once: true });
      });
    },
    done: async function () {
      var payload = { type: 'spike:results', lane: S.lane, run: q.get('run'), ua: navigator.userAgent, results: S.out };
      render();
      if (S.framed) {
        parent.postMessage(payload, '*');
        return;
      }
      await S.post(payload);
      await S.sleep(+(q.get('linger') || 800));
      var back = q.get('back');
      if (back) location.replace(back);
    },
    post: async function (payload) {
      try {
        var r = await fetch('/_spike/result', { method: 'POST', body: JSON.stringify(payload) });
        if (r.ok) return;
      } catch (e) {}
      await new Promise(function (res) {
        var i = new Image();
        i.onload = i.onerror = res;
        i.src = S.blob + '/_spike/r?d=' + encodeURIComponent(btoa(unescape(encodeURIComponent(JSON.stringify(payload)))));
      });
    },
  });
  function render() {
    var pre = document.createElement('pre');
    pre.style.cssText = 'white-space:pre-wrap;font-size:12px';
    pre.textContent = S.out.map(function (r) {
      return r.outcome.toUpperCase() + '  ' + r.item + '/' + r.check + '  ' + r.detail.slice(0, 300);
    }).join('\n');
    (document.body || document.documentElement).appendChild(pre);
  }
})();
