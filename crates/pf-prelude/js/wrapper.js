/* partifactory part-host wrapper, minimal (PLAN §2 "The link to post"). Served at the part host's
   root and any non-reserved path. It takes the cap from #pf= (clearing it before anything else
   runs), else from sessionStorage, else hops to the workspace's /grant?mode=wrap. It frames
   /c/<cap><path> and answers the prelude's pf:regrant. The chrome (title bar, Share, comments)
   comes with pf-wmcw; this file is the hand-off and the frame only. */
(function () {
  'use strict';
  var L = location, H = document.documentElement;
  var part = H.getAttribute('data-pf-part');
  var WS = H.getAttribute('data-pf-ws');
  var K = 'pf:' + part;
  function store(k, v) { try { if (v === null) sessionStorage.removeItem(k); else sessionStorage.setItem(k, v); } catch (e) {} }
  function load(k) { try { return sessionStorage.getItem(k); } catch (e) { return null; } }
  function hop() {
    var other = L.hash.replace(/^#/, '');
    if (other) store(K + ':frag', other);
    store(K + ':cap', null);
    L.replace(WS + '/grant?part=' + encodeURIComponent(part) + '&path=' +
      encodeURIComponent(L.pathname + L.search) + '&mode=wrap');
  }
  var cap = null;
  var mm = /(?:^#|&)pf=([^&]*)/.exec(L.hash);
  if (mm) {
    cap = decodeURIComponent(mm[1]);
    var rest = L.hash.replace(/^#/, '').split('&').filter(function (x) { return x.indexOf('pf=') !== 0; }).join('&');
    var frag = rest || load(K + ':frag') || '';
    store(K + ':frag', null);
    history.replaceState(null, '', L.pathname + L.search + (frag ? '#' + frag : ''));
    store(K + ':cap', cap);
  } else {
    cap = load(K + ':cap');
  }
  if (!cap) { hop(); return; }
  window.__pfWrapper = { part: part, cap: true };

  function start() {
    var f = document.createElement('iframe');
    f.id = 'pf-frame';
    f.title = document.title || part;
    f.setAttribute('allow', 'clipboard-write *; fullscreen *; web-share *');
    f.style.cssText = 'position:fixed;inset:0;width:100%;height:100%;border:0;display:block';
    f.src = '/c/' + cap + L.pathname + L.search + L.hash;
    document.body.appendChild(f);
    var regranting = false;
    window.addEventListener('message', function (e) {
      if (e.source !== f.contentWindow || !e.data || typeof e.data !== 'object') return;
      var t = e.data.type;
      if (t === 'pf:ready') H.setAttribute('data-pf-ready', '1');
      if (t === 'pf:version') f.src = f.src;
      if (t === 'pf:regrant' && !regranting) {
        regranting = true;
        // The credentialed chrome API mints a new cap with no navigation; until it exists (or if
        // it refuses) fall back to a top-level hop through /grant.
        fetch(WS + '/api/chrome/v1/' + encodeURIComponent(part) + '/grant', {
          method: 'POST', credentials: 'include', headers: { 'Content-Type': 'application/json' }, body: '{}'
        }).then(function (r) { return r.ok ? r.json() : Promise.reject(r.status); }).then(function (j) {
          cap = j.cap; store(K + ':cap', cap); regranting = false;
          var u = new URL(f.src); u.pathname = u.pathname.replace(/^\/c\/[^/]+/, '/c/' + cap); f.src = u.href;
        }).catch(function () { hop(); });
      }
    });
  }
  if (document.body) start(); else document.addEventListener('DOMContentLoaded', start);
})();
