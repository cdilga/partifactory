window.pfShareChrome = function (part, WS) {
  'use strict';
  var links = [], isAdmin = false, sheet, status, list, mode, expiry, audience;
  var unsandboxed=document.documentElement.hasAttribute('data-pf-unsandboxed');
  if(unsandboxed){var bar=document.createElement('nav');bar.id='pf-bar';var back=document.createElement('a');back.href=WS+'/';back.textContent='‹ Parts';bar.appendChild(back);var shareLink=document.createElement('a');shareLink.href=WS+'/p/'+encodeURIComponent(part)+'/share';shareLink.textContent='Share';bar.appendChild(shareLink);document.body.appendChild(bar);return;}
  var writers='viewers',guestEdits=0, summary=null;
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
    var pub=links.length || summary && summary.audience==='public';
    audience.textContent = pub ? '◉ Public link' : summary && summary.audience==='shared' ? 'Shared with you' : 'Only me';
    audience.className = pub ? 'pf-audience pf-public' : 'pf-audience';
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
  var bar = el('nav', null, document.body); bar.id = 'pf-bar'; bar.setAttribute('aria-label', 'Part controls');
  var back=el('a','‹ Parts',bar);back.href=WS+'/';back.className='pf-back';
  var kind=el('span','',bar);kind.className='pf-kind';
  var title=el('strong',part,bar);title.className='pf-title';
  var nameLabel=el('span',part,bar);nameLabel.className='pf-name';
  var revision=el('span','',bar);revision.className='pf-revision';
  audience = el('span', 'Only me', bar); audience.className = 'pf-audience';
  var shareButton=button('Share',bar,share);shareButton.hidden=true;
  var more=el('details',null,bar);more.className='pf-more';var toggle=el('summary','⋯',more);toggle.setAttribute('aria-label','More part actions');
  var actions=el('div',null,more);actions.className='pf-actions';
  var copy=button('Copy link',actions,function(){navigator.clipboard.writeText(location.origin+'/').then(function(){more.open=false;});});copy.hidden=true;
  var standalone=el('a','Open standalone',actions);standalone.hidden=true;standalone.target='_blank';standalone.rel='noopener';
  var drafting=el('section',null,document.body);drafting.id='pf-drafting';drafting.hidden=true;
  var draftTitle=el('h1',part,drafting);el('p','being made…',drafting);el('p','Your agent is getting this part ready.',drafting);
  var priorScroll=0;window.addEventListener('message',function(e){var f=document.getElementById('pf-frame');if(!f||e.source!==f.contentWindow||!e.data)return;if(e.data.type==='pf:scroll'){var y=Number(e.data.y)||0;bar.classList.toggle('pf-bar-hidden',y>priorScroll&&y>48);priorScroll=y;}});
  function updateSummary(fromVersion) {
    return api('/summary').then(function(j){summary=j;title.textContent=j.title||part;draftTitle.textContent=j.title||part;document.title=(j.title||part)+' · partifactory';
      kind.textContent=String(j.kind||'').toUpperCase();revision.textContent=j.live===null?'DRAFT':'REV '+j.live;
      var features=j.features||{};shareButton.hidden=!features.share;copy.hidden=!features.copy_link;standalone.hidden=!features.open_standalone;
      shareButton.onclick=features.unsandboxed?function(){location.href=WS+'/p/'+encodeURIComponent(part)+'/share';}:share;
      var f=document.getElementById('pf-frame');if(f)standalone.href=f.src;
      var wasDraft=!drafting.hidden;drafting.hidden=j.live!==null;
      if(f&&wasDraft&&j.live!==null&&!fromVersion){f.classList.add('pf-switching');f.onload=function(){f.classList.remove('pf-switching');};f.src=f.src;}
      render();if(j.live===null)setTimeout(updateSummary,1000);
    }).catch(function(){shareButton.hidden=true;});
  }
  window.__pfRefreshSummary=updateSummary;
  updateSummary();
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
      var field = el('input', null, status); field.value = j.url; field.readOnly = true; field.setAttribute('aria-label', 'Public link'); field.className='pf-link-value'; field.select();
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
  var workspaceShare=document.querySelector('[data-pf-workspace-share]');
  if(workspaceShare){part=workspaceShare.getAttribute('data-pf-workspace-share');WS=location.origin;window.pfShareChrome(part,WS);document.getElementById('pf-share').showModal();return;}
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
    if(!H.hasAttribute('data-pf-unsandboxed')) f.setAttribute('sandbox','allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals allow-downloads allow-top-navigation-by-user-activation');
    f.src = '/c/' + cap + (publicLink ? '/' : L.pathname + L.search) + L.hash;
    document.body.appendChild(f);
    if(!publicLink){var ticks=document.createElement("div");ticks.id="pf-registration";ticks.setAttribute("aria-hidden","true");document.body.appendChild(ticks);}
    var regranting = false;
    window.addEventListener('message', function (e) {
      if (e.source !== f.contentWindow || !e.data || typeof e.data !== 'object') return;
      var t = e.data.type;
      if (t === 'pf:ready') H.setAttribute('data-pf-ready', '1');
      if (t === 'pf:version') { if(window.__pfRefreshSummary)window.__pfRefreshSummary(true); f.classList.add('pf-switching'); f.onload = function () { f.classList.remove('pf-switching'); f.onload = null; }; f.src = f.src; }
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
