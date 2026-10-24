// The spike runner on https://spike.<apex>/run. Frames item pages (it is listed as an embedder in
// the part CSP's frame-ancestors), forwards their postMessage results to the collector, and
// navigates top-level for the items that need it (login, 2top, 10, 13), which come back here
// with ?i=<next>. Query: lane, items (comma list, default all), run, wa (virt|none|sim),
// actwait (s), hold (s, item 12), proxies (1 = also run item 12 through the Docker proxies).
(function () {
  var q = new URLSearchParams(location.search);
  var ROOT = location.hostname.replace(/^spike\./, '');
  var A = 'https://a-part.' + ROOT;
  var WS = 'https://parts.' + ROOT;
  var lane = q.get('lane') || 'manual';
  var run = q.get('run') || ('r' + Date.now().toString(36));
  if (!q.get('run')) { q.set('run', run); history.replaceState(null, '', '?' + q.toString()); }
  var ALL = 'login,1,1x,1h,2,2top,3,4,5,6,6src,7,8,8c,9a,9b,9c,L,dl,10,12,13';
  var items = (q.get('items') || ALL).split(',');
  var CAP = 'v1.spike.' + run;
  var hold = +(q.get('hold') || 75);
  var VIAS = { direct: A, caddy: 'https://a-part.caddy.' + ROOT + ':9443', nginx: 'https://a-part.nginx.' + ROOT + ':9444',
    nginxnaive: 'https://a-part.nginxnaive.' + ROOT + ':9445', traefik: 'https://a-part.traefik.' + ROOT + ':9446' };
  var STEPS = {
    login: { item: 'login', top: function () { return WS + '/login'; } },
    '1': { item: '1', file: '1' }, '1x': { item: '1', file: '1x', csp: 'explicit' }, '1h': { item: '1', file: '1h', csp: 'explicithttps' },
    '2': { item: '2', file: '2' }, '2top': { item: '2', top: function () { return A + '/c/' + CAP + '/items/2top.html?nav=' + rid(); } },
    '3': { item: '3', file: '3' }, '4': { item: '4', file: '4' }, '5': { item: '5', file: '5' },
    '6': { item: '6', file: '6', timeout: 40 }, '6src': { item: '6', file: '6', timeout: 40, allow: 'src' }, '7': { item: '7', file: '7' },
    '8': { item: '8', file: '8' }, '8c': { item: '8', file: '8c', csp: 'canonical' },
    '9a': { item: '9', file: '9a' }, '9b': { item: '9', file: '9b', csp: 'ro' }, '9c': { item: '9', file: '9c', csp: 'meta' },
    L: { item: 'L', file: 'L' }, dl: { item: 'L', file: 'dl', timeout: 40 },
    '10': { item: '10', top: function () { return A + '/deep/path?x=1'; }, frag: '#share=public' },
    '12': { item: '12', multi: true },
    '13': { item: '13', top: function () { return WS + '/webauthn'; } },
  };
  function rid() { return Math.random().toString(36).slice(2, 10); }
  function common() {
    var c = new URLSearchParams();
    ['lane', 'run', 'wa', 'actwait', 'hold', 'hb'].forEach(function (k) { if (q.get(k)) c.set(k, q.get(k)); });
    c.set('run', run);
    return c.toString();
  }
  function selfUrl(i) { var c = new URLSearchParams(q); c.set('i', String(i)); return location.origin + location.pathname + '?' + c.toString(); }
  function status(s) { document.getElementById('status').textContent = s; }
  async function post(payload) {
    payload.lane = lane; payload.run = run;
    await fetch('/_spike/result', { method: 'POST', body: JSON.stringify(payload) });
  }
  function waitResults(f, secs) {
    return new Promise(function (res) {
      var t = setTimeout(function () { window.removeEventListener('message', h); res(null); }, secs * 1000);
      function h(e) {
        if (e.source === f.contentWindow && e.data && e.data.type === 'spike:results') {
          clearTimeout(t); window.removeEventListener('message', h); res(e.data);
        }
      }
      window.addEventListener('message', h);
    });
  }
  // A sandboxed (opaque-origin) frame never matches the default 'src' allowlist, so delegated
  // features need "<feature> *". Step 6src frames item 6 with the default to show the difference.
  var ALLOW_STAR = 'clipboard-write *; fullscreen *; web-share *';
  var ALLOW_SRC = 'clipboard-write; fullscreen; web-share';
  function mkFrame(src, allow) {
    var f = document.createElement('iframe');
    f.className = 'item';
    f.setAttribute('allow', allow === 'src' ? ALLOW_SRC : ALLOW_STAR);
    f.style.cssText = 'width:100%;height:260px;border:1px solid #888';
    f.src = src;
    document.getElementById('frames').appendChild(f);
    return f;
  }
  async function runFrame(key, st) {
    var src = A + '/c/' + CAP + '/items/' + st.file + '.html?' + common() + '&nav=' + rid() + (st.csp ? '&csp=' + st.csp : '') + (st.allow ? '&allow=' + st.allow : '');
    var f = mkFrame(src, st.allow);
    var secs = st.timeout || 20;
    var r = await waitResults(f, secs);
    if (!r) r = { ua: navigator.userAgent, results: [{ item: st.item, check: 'report_' + key, outcome: 'fail', detail: 'no report within ' + secs + 's (frame blocked or script failed)' }] };
    await post({ type: 'spike:results', ua: r.ua, results: r.results });
    f.remove();
  }
  async function runMulti() {
    var vias = q.get('proxies') === '1' ? Object.keys(VIAS) : ['direct'];
    await Promise.all(vias.map(async function (v) {
      var f = mkFrame(VIAS[v] + '/c/' + CAP + '/items/12.html?' + common());
      var r = await waitResults(f, hold + 40);
      if (!r) r = { ua: navigator.userAgent, results: [{ item: '12', check: 'report@' + v, outcome: 'fail', detail: 'no report (proxy down or frame blocked)' }] };
      await post({ type: 'spike:results', ua: r.ua, results: r.results });
      f.remove();
    }));
  }
  async function summary() {
    var rows = await (await fetch('/_spike/results?lane=' + encodeURIComponent(lane))).json();
    var latest = {};
    rows.forEach(function (r) { latest[r.item + '/' + r.check] = r; });
    var t = document.getElementById('t');
    t.innerHTML = '';
    Object.keys(latest).sort().forEach(function (k) {
      var r = latest[k];
      var tr = document.createElement('tr');
      var color = { pass: '#cfc', fail: '#fcc', not_assertable: '#eee', info: '#def' }[r.outcome] || '#fff';
      tr.innerHTML = '<td style="background:' + color + '">' + r.outcome + '</td><td>' + k + '</td><td>' + String(r.detail).slice(0, 90).replace(/</g, '&lt;') + '</td>';
      t.appendChild(tr);
    });
  }
  async function main() {
    document.getElementById('h').textContent = 'pf-6qma spike · lane ' + lane + ' · run ' + run;
    var i = +(q.get('i') || 0);
    while (i < items.length) {
      var key = items[i];
      var st = STEPS[key];
      status('step ' + (i + 1) + '/' + items.length + ': ' + key);
      document.title = 'spike:step:' + key;
      if (!st) { i++; continue; }
      if (st.top) {
        var u = st.top();
        var sep = u.indexOf('?') >= 0 ? '&' : '?';
        var back = key === 'login' ? selfUrl(i + 1) : selfUrl(i + 1);
        if (key === 'login') location.href = u + sep + 'back=' + encodeURIComponent(back);
        else location.href = u + sep + common() + '&back=' + encodeURIComponent(back) + (st.frag || '');
        return;
      }
      if (st.multi) await runMulti(); else await runFrame(key, st);
      i++;
    }
    await post({ type: 'spike:done' });
    status('DONE · ' + items.join(','));
    await summary();
    document.title = 'spike:done';
  }
  main().catch(function (e) { status('runner error: ' + e); document.title = 'spike:error'; });
})();
