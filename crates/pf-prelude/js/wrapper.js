window.pfShareChrome = function (part, WS) {
  'use strict';
  var links = [], isAdmin = false, sheet, status, list, mode, expiry, audience;
  var writers='viewers',guestEdits=0;
  function el(tag, text, parent) { var e = document.createElement(tag); if (text) e.textContent = text; if (parent) parent.appendChild(e); return e; }
  function button(text, parent, action) { var b = el('button', text, parent); b.type = 'button'; b.onclick = action; return b; }
  function api(path, body) {
    var opts = { credentials: 'include' };
    if (body !== undefined) { opts.method = 'POST'; opts.headers = { 'Content-Type': 'application/json' }; opts.body = JSON.stringify(body); }
    return fetch(WS + '/api/chrome/v1/' + encodeURIComponent(part) + path, opts).then(function (r) {
      return r.json().then(function (j) { if (!r.ok) throw j.error || { message: 'Please try again.' }; return j; });
    });
  }
  function bytes(s) { return Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), function (c) { return c.charCodeAt(0); }); }
  function b64(a) { return btoa(String.fromCharCode.apply(null, new Uint8Array(a))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, ''); }
  function stepup() {
    status.textContent = 'Confirm with your admin passkey…';
    return api('/stepup', {}).then(function (j) {
      var p = j.publicKey; p.challenge = bytes(p.challenge);
      (p.allowCredentials || []).forEach(function (c) { c.id = bytes(c.id); });
      return navigator.credentials.get({ publicKey: p }).then(function (c) {
        return api('/stepup', { flow: j.flow, credential: { id: c.id, rawId: b64(c.rawId), type: c.type, response: { authenticatorData: b64(c.response.authenticatorData), clientDataJSON: b64(c.response.clientDataJSON), signature: b64(c.response.signature), userHandle: c.response.userHandle ? b64(c.response.userHandle) : null }, extensions: c.getClientExtensionResults() } });
      });
    });
  }
  function mutation(path, body) {
    return api(path, body).catch(function (e) {
      if (e.code === 'step_up_required' || (e.details && e.details.step_up_required)) return stepup().then(function () { return api(path, body); });
      throw e;
    });
  }
  function report(e) { status.textContent = e.message || String(e); }
  function refresh() {
    return api('/links').then(function (j) { isAdmin = true; links = j.links; writers=j.shared_data_writers;guestEdits=j.guest_edits_today;render(); }, function () { isAdmin = false; links = []; render(); });
  }
  function render() {
    list.replaceChildren();
    audience.textContent = links.length ? '◉ Public link' : 'Only me';
    audience.className = links.length ? 'pf-audience pf-public' : 'pf-audience';
    document.getElementById('pf-public-rung').hidden = !isAdmin;
    if (!isAdmin) { options.hidden = true; el('p', 'Only the admin can make a public link.', list); return; }
    el('p',guestEdits+' guest edits today',list);
    button(writers==='editors'?'Data: view only':'Data: viewers can write',list,function(){api('/writers',{writers:writers==='editors'?'viewers':'editors'}).then(refresh).catch(report);});
    button('Roll back data',list,function(){data('snapshot_list').then(function(j){status.replaceChildren();var select=el('select',null,status);select.setAttribute('aria-label','Snapshot');j.snapshots.forEach(function(s){var o=el('option',new Date(s.taken_at).toLocaleString()+' · '+s.reason,select);o.value=s.id;});button('Restore snapshot',status,function(){data('snapshot_restore',{snapshot_id:Number(select.value)}).then(function(){status.textContent='Data restored.';}).catch(report);}).disabled=!j.snapshots.length;}).catch(report);});
    button('Reset data',list,function(){status.replaceChildren();el('p','Clear all shared data? An undo snapshot will be saved first.',status);button('Confirm reset',status,function(){data('reset',{confirm:true}).then(function(){status.textContent='Data reset. Roll back to undo.';}).catch(report);});});
    links.forEach(function (l) {
      var row = el('section', null, list);
      el('p', (l.mode === 'contribute' ? 'Can contribute' : 'Can view') + ' · opened ' + l.open_count + ' times · last ' + (l.last_opened_at ? new Date(l.last_opened_at).toLocaleString() : 'never'), row);
      if (l.mode === 'contribute') button('Freeze', row, function () { change(l, 'freeze'); });
      else el('p', 'Data: view only', row);
      button('Revoke', row, function () { change(l, 'revoke'); }).className = 'pf-danger';
    });
  }
  function data(action,extra){var body={name:part,action:action,scope:'shared'};if(action!=='snapshot_list')body.request_id=crypto.randomUUID();Object.assign(body,extra||{});return api('/data',body);}
  function change(l, action) {
    mutation('/links/' + encodeURIComponent(l.id), { action: action }).then(function () {
      status.replaceChildren(); el('span', action === 'freeze' ? 'Frozen. ' : 'Link revoked. ', status);
      var undo = button('Undo', status, function () { mutation('/links/' + encodeURIComponent(l.id), { action: action, undo: true }).then(function () { status.textContent = 'Restored.'; refresh(); }).catch(report); });
      setTimeout(function () { undo.remove(); }, 10000); refresh();
    }).catch(report);
  }
  function share() { sheet.showModal(); refresh(); }
  var style = el('style', null, document.head);
  style.textContent = ':root{color-scheme:light dark;--pf-bg:#fff;--pf-fg:#20221f;--pf-line:#d0d4cc;--pf-amber:#724300;--pf-amber-bg:#fff0c2;--pf-danger:#9e2525}@media(prefers-color-scheme:dark){:root{--pf-bg:#1c211c;--pf-fg:#f0f3ec;--pf-line:#596154;--pf-amber:#ffdc8b;--pf-amber-bg:#493811;--pf-danger:#ffb4ac}}body{margin:0;background:var(--pf-bg);color:var(--pf-fg);font:15px system-ui,sans-serif}#pf-bar{height:48px;box-sizing:border-box;display:flex;align-items:center;gap:12px;padding:0 16px;border-bottom:1px solid var(--pf-line)}button,select{font:inherit;color:inherit;background:var(--pf-bg);border:1px solid var(--pf-line);border-radius:8px;padding:9px 14px;cursor:pointer}button:focus-visible,select:focus-visible{outline:3px solid #4d7bca;outline-offset:2px}button:disabled{opacity:.55;cursor:default}.pf-audience{margin-left:auto;font-size:13px}.pf-public{background:var(--pf-amber-bg);color:var(--pf-amber);padding:5px 9px;border-radius:20px}.pf-danger{color:var(--pf-danger)}#pf-share{box-sizing:border-box;width:min(480px,calc(100% - 24px));max-height:90dvh;overflow:auto;border:1px solid var(--pf-line);border-radius:16px;padding:24px;background:var(--pf-bg);color:var(--pf-fg)}#pf-share::backdrop{background:#0008}#pf-share h2{margin:0 0 18px}#pf-share header{display:flex;justify-content:space-between;align-items:start}#pf-share section{border-top:1px solid var(--pf-line);padding:8px 0}#pf-share button{margin:4px 4px 4px 0}#pf-share label{display:block;margin:14px 0}#pf-share select{display:block;width:100%;margin-top:6px}#pf-status{min-height:24px;font-size:14px;margin-top:16px}#pf-public-rung{color:var(--pf-amber);background:var(--pf-amber-bg)}@media(max-width:520px){#pf-share{width:100%;max-width:none;margin:auto 0 0;border-radius:18px 18px 0 0;padding:22px;max-height:88dvh}#pf-bar{padding:0 10px}}';
  var bar = el('nav', null, document.body); bar.id = 'pf-bar'; bar.setAttribute('aria-label', 'Part controls');
  el('strong', 'partifactory', bar); audience = el('span', 'Only me', bar); audience.className = 'pf-audience';
  button('Share', bar, share);
  sheet = el('dialog', null, document.body); sheet.id = 'pf-share'; sheet.setAttribute('aria-labelledby', 'pf-share-title');
  var head = el('header', null, sheet); el('h2', 'Share', head).id = 'pf-share-title'; button('Close', head, function () { sheet.close(); });
  el('p', 'Choose who can open this part.', sheet);
  button('Only me', sheet, function () {}).disabled = true;
  button('People you add', sheet, function () {}).disabled = true;
  var rung = button('Anyone with the link', sheet, function () { options.hidden = false; }); rung.id = 'pf-public-rung'; rung.hidden = true;
  var options = el('div', null, sheet); options.id = 'pf-public-options'; options.hidden = true;
  el('p', 'Anyone with the link can use it. Only you and people you add can change it.', options);
  var ml = el('label', 'Link access', options); mode = el('select', null, ml); mode.setAttribute('aria-label', 'Link access');
  [['view', 'Can view'], ['contribute', 'Can contribute']].forEach(function (v) { var o = el('option', v[1], mode); o.value = v[0]; });
  var xl = el('label', 'Expires', options); expiry = el('select', null, xl); expiry.setAttribute('aria-label', 'Expires');
  [['', 'Never'], ['1', '1 day'], ['7', '7 days'], ['30', '30 days']].forEach(function (v) { var o = el('option', v[1], expiry); o.value = v[0]; });
  button('Copy link', options, function () {
    mutation('/links', { name: part, mode: mode.value, expires_days: expiry.value ? Number(expiry.value) : null }).then(function (j) {
      status.replaceChildren();
      var field = el('input', null, status); field.value = j.url; field.readOnly = true; field.setAttribute('aria-label', 'Public link'); field.style.width = '100%'; field.select();
      if (navigator.clipboard) navigator.clipboard.writeText(j.url).catch(function () {});
      refresh();
    }).catch(report);
  });
  list = el('div', null, sheet); status = el('div', null, sheet); status.id = 'pf-status'; status.setAttribute('role', 'status');
  refresh().then(function () {
    if (/(?:^#|&)share=(public|contribute)(?:&|$)/.test(location.hash)) { mode.value = location.hash.indexOf('share=contribute') >= 0 ? 'contribute' : 'view'; options.hidden = !isAdmin; share(); }
  });
};
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
  var publicLink = H.hasAttribute('data-pf-link');
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
  if (publicLink) cap = null;
  if (!cap && !publicLink) { hop(); return; }
  window.__pfWrapper = { part: part, cap: true };

  function start() {
    if (publicLink && !cap) {
      fetch(L.pathname + '/go', { method: 'POST' }).then(function (r) {
        if (!r.ok) throw new Error('This link is no longer available.');
        return r.json();
      }).then(function (j) { cap = j.cap; start(); }).catch(function (e) { document.body.textContent = e.message; });
      return;
    }
    var card = document.getElementById('pf-card'); if (card) card.remove();
    if (!publicLink && window.pfShareChrome) window.pfShareChrome(part, WS);
    var f = document.createElement('iframe');
    f.id = 'pf-frame';
    f.title = document.title || part;
    f.setAttribute('allow', 'clipboard-write *; fullscreen *; web-share *');
    f.style.cssText = 'position:fixed;inset:' + (publicLink ? '0' : '48px 0 0') + ';width:100%;height:' + (publicLink ? '100%' : 'calc(100% - 48px)') + ';border:0;display:block;transition:opacity .2s';
    f.src = '/c/' + cap + (publicLink ? '/' : L.pathname + L.search) + L.hash;
    document.body.appendChild(f);
    var regranting = false;
    window.addEventListener('message', function (e) {
      if (e.source !== f.contentWindow || !e.data || typeof e.data !== 'object') return;
      var t = e.data.type;
      if (t === 'pf:ready') H.setAttribute('data-pf-ready', '1');
      if (t === 'pf:version') { f.style.opacity = '0'; f.onload = function () { f.style.opacity = '1'; f.onload = null; }; f.src = f.src; }
      if (t === 'pf:regrant' && !regranting) {
        regranting = true;
        // The credentialed chrome API mints a new cap with no navigation; until it exists (or if
        // it refuses) fall back to a top-level hop through /grant.
        fetch(publicLink ? L.pathname + '/go' : WS + '/api/chrome/v1/' + encodeURIComponent(part) + '/grant', {
          method: 'POST', credentials: 'include', headers: { 'Content-Type': 'application/json' }, body: '{}'
        }).then(function (r) { return r.ok ? r.json() : Promise.reject(r.status); }).then(function (j) {
          cap = j.cap; if(!publicLink) store(K + ':cap', cap); regranting = false;
          var u = new URL(f.src); u.pathname = u.pathname.replace(/^\/c\/[^/]+/, '/c/' + cap); f.src = u.href;
        }).catch(function () { if(publicLink) document.body.textContent = 'This link is no longer available.'; else hop(); });
      }
    });
  }
  if (document.body) start(); else document.addEventListener('DOMContentLoaded', start);
})();
