// Real release-image fixture for the native Safari wrapper/regrant lane.
import {test,expect} from './fixtures';
import {signIn} from '../lib/public-auth';

test('create a private wrapper probe with a real scoped device agent',async({page,request},info)=>{
  test.skip(!process.env.PF_IOS_WRAPPER||info.project.name!=='chromium-desktop','Explicit native wrapper lane');
  test.setTimeout(120_000);
  const nonce=process.env.PF_IOS_WRAPPER!;
  const base=process.env.PF_BASE_URL!,apex=new URL(base).hostname.slice('parts.'.length);
  await signIn(page);
  const device=await (await request.post('/oauth/device_authorization',{form:{client_id:'part-cli',agent_name:'Safari wrapper fixture',scope:'parts:read parts:write data:read'}})).json();
  await page.goto(device.verification_uri_complete);
  await page.getByRole('button',{name:'Continue',exact:true}).click();
  await page.getByRole('button',{name:'Allow',exact:true}).click();
  const granted=await request.post('/oauth/token',{form:{client_id:'part-cli',grant_type:'urn:ietf:params:oauth:grant-type:device_code',device_code:device.device_code}});
  expect(granted.status()).toBe(200);
  const headers={Authorization:`Bearer ${(await granted.json()).access_token}`};
  const content=`<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><h1>Wrapper regrant probe</h1><p id="status">Reading real storage…</p><button id="read">Read with current capability</button><script>
const status=document.getElementById('status');
document.getElementById('read').onclick=async()=>{try{await window.storage.get('wrapper-probe',true);status.textContent='Current capability read succeeded';}catch(e){status.textContent='Waiting for a fresh capability';}};
(async()=>{
  await window.storage.get('wrapper-probe',true);
  status.textContent='Wrapper ready with real storage';
  window.partifactory.ready();
  const response=await fetch(${JSON.stringify(`https://blobs-part.${apex}/__harness/report`)},{method:'POST',credentials:'omit',headers:{'Content-Type':'text/plain'},body:JSON.stringify({event:'device.check',item:'wrapper_frame_loaded',lane:'safari-content',outcome:'pass',run:${JSON.stringify(nonce)}})});
  if(response.status!==204)throw Error('Collector refused real frame report');
})();
</script>`;
  const created=await request.post('/api/v1/part_create',{headers,data:{name:'ios-wrapper',title:'Wrapper regrant probe',kind:'html',content,request_id:'ios-wrapper-'+nonce}});
  expect(created.status(),await created.text()).toBe(200);
  await expect.poll(async()=> (await (await request.post('/api/v1/part_get',{headers,data:{name:'ios-wrapper'}})).json()).build?.status,{timeout:45_000}).toBe('ok');
  // Leave its host entirely unopened: Safari must perform its own genuine first /grant hop.
});
