(function () {
  'use strict';
  var list=document.getElementById('links'),status=document.getElementById('status');
  var kill=document.getElementById('kill'),retry=document.getElementById('retry-links'),timer;
  function api(path,body) {return fetch(path,body===undefined?{}:{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)}).then(function(r){return r.json().then(function(j){if(!r.ok)throw j.error;return j;});});}
  function bytes(s){return Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')),function(c){return c.charCodeAt(0);});}
  function b64(a){return btoa(String.fromCharCode.apply(null,new Uint8Array(a))).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');}
  function stepup(){return api('/admin/stepup/start',{}).catch(function(e){if(!(e&&e.details&&e.details.step_up_via==='provider'))throw e;status.textContent='Sign in again to confirm, then try once more.';location.assign(e.details.url+'?return='+encodeURIComponent(location.pathname+location.search));return new Promise(function(){});}).then(function(j){var p=j.publicKey;p.challenge=bytes(p.challenge);(p.allowCredentials||[]).forEach(function(c){c.id=bytes(c.id);});return navigator.credentials.get({publicKey:p}).then(function(c){return api('/admin/stepup/finish',{flow:j.flow,credential:{id:c.id,rawId:b64(c.rawId),type:c.type,response:{authenticatorData:b64(c.response.authenticatorData),clientDataJSON:b64(c.response.clientDataJSON),signature:b64(c.response.signature),userHandle:c.response.userHandle?b64(c.response.userHandle):null},extensions:c.getClientExtensionResults()}});});});}
  function mutate(path,body){return api(path,body).catch(function(e){if(e.code==='step_up_required'||(e.details&&e.details.step_up_required))return stepup().then(function(){return api(path,body);});throw e;});}
  function report(e){status.textContent=e.message||String(e);}
  if(location.hash==='#confirm-failed'){history.replaceState(null,'',location.pathname+location.search);status.textContent="Couldn't confirm: your identity provider didn't ask you to sign in again.";}
  function dateCell(row,value){var cell=document.createElement('td');if(value){var time=document.createElement('time'),date=new Date(value);time.dateTime=date.toISOString();time.textContent=date.toLocaleString();cell.appendChild(time);}else{cell.textContent='Never';}row.appendChild(cell);}
  function load(message){
    list.setAttribute('aria-busy','true');retry.hidden=true;
    return api('/api/admin/v1/links').then(function(j){
      list.replaceChildren();
      j.links.forEach(function(l){
        var row=document.createElement('tr'),name=document.createElement('td');name.textContent=l.name;row.appendChild(name);
        dateCell(row,l.created_at);dateCell(row,l.expires_at);
        var opens=document.createElement('td');opens.textContent=l.open_count;row.appendChild(opens);dateCell(row,l.last_opened_at);
        var cell=document.createElement('td'),button=document.createElement('button');button.textContent='Revoke';button.setAttribute('aria-label','Revoke link for '+l.name);
        button.onclick=function(){button.disabled=true;status.textContent='Revoking link…';mutate('/api/chrome/v1/'+encodeURIComponent(l.name)+'/links/'+encodeURIComponent(l.id),{action:'revoke'}).then(function(){return load('Link revoked.');}).catch(function(e){button.disabled=false;report(e);});};
        cell.appendChild(button);row.appendChild(cell);list.appendChild(row);
      });
      kill.disabled=!j.links.length;
      status.textContent=(message?message+' ':'')+(j.links.length?j.links.length+' live public '+(j.links.length===1?'link.':'links.'):'No live public links.');
    }).catch(function(e){report(e);retry.hidden=false;kill.disabled=true;}).finally(function(){list.setAttribute('aria-busy','false');});
  }
  function begin(){if(kill.disabled||timer)return;status.textContent='Keep holding to revoke every public link…';timer=setTimeout(function(){timer=null;kill.disabled=true;mutate('/api/admin/v1/links/revoke-all',{}).then(function(j){return load('Revoked '+j.revoked+' links.');}).catch(function(e){kill.disabled=false;report(e);});},1000);}
  function cancel(){if(timer){clearTimeout(timer);timer=null;status.textContent='Cancelled.';}}
  kill.addEventListener('pointerdown',begin);kill.addEventListener('pointerup',cancel);kill.addEventListener('pointerleave',cancel);kill.addEventListener('pointercancel',cancel);
  kill.addEventListener('keydown',function(e){if((e.key===' '||e.key==='Enter')&&!e.repeat){e.preventDefault();begin();}});kill.addEventListener('keyup',cancel);kill.addEventListener('blur',cancel);
  retry.addEventListener('click',function(){status.textContent='Loading public links…';load();});
  load();
})();
