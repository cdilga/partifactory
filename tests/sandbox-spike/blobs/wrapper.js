// Spike stand-in for the trusted wrapper at a part host's root (item 10, PLAN.md §2 "The link
// to post"). Runs first in <head>. Takes the cap from #pf= (clearing it with replaceState before
// any other script), else from sessionStorage, else hops client-side to the workspace /grant.
// Spike-only: the cap key is scoped by ?run= so each run starts with no stored cap.
(function () {
  var L = location;
  var q = new URLSearchParams(L.search);
  var first = L.hostname.split('.')[0];
  var part = first.replace(/-part$/, '');
  var rest = L.host.slice(first.length + 1);
  var WS = 'https://parts.' + rest;
  var K = 'pf:' + part + ':' + (q.get('run') || '');
  var m = /(?:^#|&)pf=([^&]*)/.exec(L.hash);
  if (m) {
    history.replaceState(null, '', L.pathname + L.search);
    var cap = decodeURIComponent(m[1]);
    try { sessionStorage.setItem(K + ':cap', cap); } catch (e) {}
    window.__pf = { cap: cap, hadFragment: true, hashAfter: L.hash, WS: WS, K: K, part: part };
  } else {
    var stored = null;
    try { stored = sessionStorage.getItem(K + ':cap'); } catch (e) {}
    if (!stored) {
      var other = L.hash.replace(/^#/, '');
      try {
        if (other) sessionStorage.setItem(K + ':frag', other);
        sessionStorage.setItem(K + ':hopped', String(Date.now()));
      } catch (e) {}
      L.replace(WS + '/grant?part=' + encodeURIComponent(part) + '&path=' + encodeURIComponent(L.pathname + L.search) + '&mode=wrap');
      window.__pfHopping = true;
      return;
    }
    window.__pf = { cap: stored, hadFragment: false, WS: WS, K: K, part: part };
  }

  var R = [];
  function rec(item, check, outcome, detail) {
    R.push({ item: String(item), check: check, outcome: outcome, detail: typeof detail === 'string' ? detail : JSON.stringify(detail) });
  }
  function sleep(ms) { return new Promise(function (r) { setTimeout(r, ms); }); }
  function rid() { return Math.random().toString(36).slice(2, 10); }
  async function getJSON(u) { var r = await fetch(u); return r.json(); }
  function frame(src) {
    var f = document.createElement('iframe');
    f.setAttribute('allow', 'clipboard-write *; fullscreen *; web-share *');
    f.style.cssText = 'width:100%;height:140px;border:1px solid #999';
    f.src = src;
    document.body.appendChild(f);
    return f;
  }
  function waitHello(f, ms) {
    return new Promise(function (res) {
      var t = setTimeout(function () { window.removeEventListener('message', h); res(null); }, ms);
      function h(e) {
        if (e.source === f.contentWindow && e.data && e.data.type === 'pf:hello') {
          clearTimeout(t); window.removeEventListener('message', h); res(e.data);
        }
      }
      window.addEventListener('message', h);
    });
  }

  async function run() {
    var pf = window.__pf;
    var late = window.__pfLateSaw || {};
    var out = document.getElementById('out');
    var hopped = null, frag = null;
    try { hopped = sessionStorage.getItem(pf.K + ':hopped'); frag = sessionStorage.getItem(pf.K + ':frag'); } catch (e) {}

    var grants = [];
    try { grants = await getJSON(pf.WS + '/_spike/grants?part=' + pf.part); } catch (e) {}
    var runId = q.get('run') || '';
    var g = grants.filter(function (x) { return x.path.indexOf('run=' + runId) >= 0; }).pop();
    if (g) g = { session: g.session, referer: g.referer, sfs: g.sfs };
    rec(10, '1_nocap_location_replace_to_grant', hopped && g ? 'pass' : 'fail',
      { hopped: !!hopped, grantSeen: !!g, grantHadSession: g && g.session, grantReferer: g && g.referer, grantSecFetchSite: g && g.sfs });
    rec(10, '2_pf_fragment_survives_302', pf.hadFragment ? 'pass' : 'fail', { hadFragment: pf.hadFragment });
    rec(10, '3a_replacestate_clears_before_other_scripts',
      pf.hadFragment && pf.hashAfter === '' && late.hash === '' ? 'pass' : 'fail', { hashAfterReplace: pf.hashAfter, laterScriptSaw: late });
    var leak = { hits: ['leak check failed'] };
    try { leak = await getJSON(pf.WS + '/_spike/leak'); } catch (e) {}
    rec(10, '3b_fragment_never_in_referer_or_request_logs', leak.hits.length === 0 ? 'pass' : 'fail', leak);

    var navid = rid();
    var src = '/c/' + pf.cap + L.pathname + L.search + (L.search ? '&' : '?') + 'nav=' + navid;
    var f = frame(src);
    var hello = await waitHello(f, 10000);
    rec(10, '4_explicit_frame_ancestors_admits_same_host_wrapper', hello ? 'pass' : 'fail', hello ? { partOrigin: hello.r.origin } : 'no pf:hello within 10s');
    if (hello) {
      var r = hello.r;
      rec(10, '6_framed_part_cannot_read_parent', !r.parent_document.ok && !r.parent_sessionStorage.ok ? 'pass' : 'fail', r);
    } else {
      rec(10, '6_framed_part_cannot_read_parent', 'fail', 'part frame never loaded');
    }
    try {
      var nh = await getJSON(pf.WS + '/_spike/headers/' + navid);
      rec(2, 'wrapper_framed_part_nav_headers', nh && nh['sec-fetch-dest'] === 'iframe' ? 'pass' : (nh && !nh['sec-fetch-dest'] ? 'info' : 'fail'),
        nh && { site: nh['sec-fetch-site'], mode: nh['sec-fetch-mode'], dest: nh['sec-fetch-dest'], accept: (nh.accept || '').slice(0, 40) });
    } catch (e) {}
    var f2 = frame('/c/' + pf.cap + '/fa-self?csp=fa-self');
    var hello2 = await waitHello(f2, 6000);
    rec(10, 'info_frame_ancestors_self_admits_wrapper', 'info', hello2 ? "'self' admitted the same-host wrapper" : "'self' did NOT admit the same-host wrapper (keep the host explicit)");

    try {
      var resp = await fetch(pf.WS + '/api/chrome/v1/' + pf.part + '/echo', {
        method: 'POST', credentials: 'include', headers: { 'Content-Type': 'application/json' }, body: '{}',
      });
      var j = await resp.json();
      rec(10, '5_credentialed_same_site_fetch_carries_lax_cookie', j.cookies.indexOf('__Host-pf_sess') >= 0 ? 'pass' : 'fail', j);
    } catch (e) {
      rec(10, '5_credentialed_same_site_fetch_carries_lax_cookie', 'fail', e.name + ': ' + e.message);
    }
    rec(10, '7_share_fragment_survives_hop', frag === 'share=public' ? 'pass' : 'fail', 'restored=' + frag);

    out.textContent = R.map(function (x) { return x.outcome.toUpperCase() + '  ' + x.item + '/' + x.check + '  ' + x.detail.slice(0, 200); }).join('\n');
    try {
      await fetch(pf.WS + '/_spike/result', { method: 'POST', body: JSON.stringify({ type: 'spike:results', lane: q.get('lane'), run: q.get('run'), ua: navigator.userAgent, results: R }) });
    } catch (e) {}
    await sleep(+(q.get('linger') || 1500));
    var back = q.get('back');
    if (back) L.replace(back);
  }
  document.addEventListener('DOMContentLoaded', run);
})();
