(function () {
  'use strict';
  var list=document.getElementById('links'),status=document.getElementById('status');
  function api(path,body) {return fetch(path,body===undefined?{}:{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(r){return r.json().then(function(j){if(!r.ok)throw j.error;return j;});});}
  function bytes(s){return Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')),function(c){return c.charCodeAt(0);});}
  function b64(a){return btoa(String.fromCharCode.apply(null,new Uint8Array(a))).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');}
  function stepup(){return api('/admin/stepup/start',{}).then(function(j){var p=j.publicKey;p.challenge=bytes(p.challenge);(p.allowCredentials||[]).forEach(function(c){c.id=bytes(c.id);});return navigator.credentials.get({publicKey:p}).then(function(c){return api('/admin/stepup/finish',{flow:j.flow,credential:{id:c.id,rawId:b64(c.rawId),type:c.type,response:{authenticatorData:b64(c.response.authenticatorData),clientDataJSON:b64(c.response.clientDataJSON),signature:b64(c.response.signature),userHandle:c.response.userHandle?b64(c.response.userHandle):null},extensions:c.getClientExtensionResults()}});});});}
  function mutate(path,body){return api(path,body).catch(function(e){if(e.code==='step_up_required'||(e.details&&e.details.step_up_required))return stepup().then(function(){return api(path,body);});throw e;});}
  function report(e){status.textContent=e.message||String(e);}
  function load(){api('/api/admin/v1/links').then(function(j){list.replaceChildren();j.links.forEach(function(l){var row=document.createElement('tr');[l.name,new Date(l.created_at).toLocaleString(),l.expires_at?new Date(l.expires_at).toLocaleString():'Never',l.open_count,l.last_opened_at?new Date(l.last_opened_at).toLocaleString():'Never'].forEach(function(v){var c=document.createElement('td');c.textContent=v;row.appendChild(c);});var cell=document.createElement('td'),button=document.createElement('button');button.textContent='Revoke';button.onclick=function(){mutate('/api/chrome/v1/'+encodeURIComponent(l.name)+'/links/'+encodeURIComponent(l.id),{action:'revoke'}).then(load).catch(report);};cell.appendChild(button);row.appendChild(cell);list.appendChild(row);});if(!j.links.length)status.textContent='No live public links.';}).catch(report);}
  var kill=document.getElementById('kill'),timer;
  function begin(){clearTimeout(timer);status.textContent='Keep holding to revoke every public link…';timer=setTimeout(function(){timer=null;mutate('/api/admin/v1/links/revoke-all',{}).then(function(j){status.textContent='Revoked '+j.revoked+' links.';load();}).catch(report);},1000);}
  function cancel(){if(timer){clearTimeout(timer);timer=null;status.textContent='Cancelled.';}}
  kill.addEventListener('pointerdown',begin);kill.addEventListener('pointerup',cancel);kill.addEventListener('pointerleave',cancel);kill.addEventListener('pointercancel',cancel);
  kill.addEventListener('keydown',function(e){if((e.key===' '||e.key==='Enter')&&!e.repeat){e.preventDefault();begin();}});kill.addEventListener('keyup',cancel);kill.addEventListener('blur',cancel);
  load();
})();
