// Paired with genuine Mobile Safari XCTest; never included in ordinary browser runs.
import {test, expect} from './fixtures';
import {signIn, nativePasskey} from '../lib/public-auth';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as https from 'node:https';

test('Simulator guest paints and Chromium admin freezes the communal board', async ({page, context, request}, info) => {
  test.skip(!process.env.PF_IOS_PIXEL || info.project.name !== 'chromium-desktop', 'Explicit paired Simulator lane');
  test.setTimeout(600_000);
  const nonce=process.env.PF_IOS_PIXEL!;
  const base=process.env.PF_BASE_URL!;
  const apex=new URL(base).hostname.slice('parts.'.length);
  const collector=`https://blobs-part.${apex}/__harness`;
  const run=path.join('/repo/target/e2e-runs',process.env.PF_E2E_RUN!);
  const dir=path.join(run,'device-pixel');
  fs.mkdirSync(dir,{recursive:true,mode:0o700});
  // Node's APIRequestContext does not use Chromium's wildcard resolver. Preserve
  // the collector hostname/SNI and ordinary CA verification, overriding only its IP.
  const collect=(route:string,row?:Record<string,string>)=>new Promise<{status:number,rows:any[]}>((resolve,reject)=>{
    const operation=https.request(collector+route,{
      method:row?'POST':'GET',headers:row?{'Content-Type':'text/plain'}:{},
      lookup:(_hostname,options,callback)=>{
        const address=process.env.PF_HARNESS_IP!;
        if((options as any).all)(callback as any)(null,[{address,family:4}]);
        else callback(null,address,4);
      },
    },response=>{
      let body='';response.setEncoding('utf8');response.on('data',chunk=>body+=chunk);
      response.on('end',()=>{try{resolve({status:response.statusCode!,rows:body?JSON.parse(body):[]});}catch(error){reject(error);}});
    });
    operation.setTimeout(35_000,()=>operation.destroy(new Error('Collector barrier timed out')));
    operation.on('error',reject);operation.end(row?JSON.stringify(row):undefined);
  });
  const report=async(item:string,extra:Record<string,string>={})=>{
    const response=await collect('/report',{event:'device.check',item,lane:'chromium-admin',outcome:'pass',run:nonce,...extra});
    expect(response.status).toBe(204);
  };
  const wait=async(item:string)=>{
    for(let attempt=0;attempt<15;attempt++) {
      const response=await collect('/wait?'+new URLSearchParams({run:nonce,item}));
      expect(response.status).toBe(200);
      const rows=response.rows;
      if(rows.length)return rows[0];
    }
    throw new Error('No Simulator collector barrier: '+item);
  };
  const logs=async(event:string)=>{
    if(process.env.PF_HARNESS_RELEASE==='1')return fs.readFileSync(path.join(run,'server.jsonl'),'utf8').split('\n').filter(line=>line.startsWith('{')).flatMap(line=>{try{return [JSON.parse(line)];}catch{return [];}}).filter(row=>row.event===event);
    return (await request.get(base+'/_dev/logs',{params:{event}})).json();
  };
  await signIn(page);
  const device=await (await request.post('/oauth/device_authorization',{form:{client_id:'part-cli',agent_name:'Simulator pixel gate',scope:'parts:read parts:write data:read data:write'}})).json();
  await page.goto(device.verification_uri_complete);
  await page.getByRole('button',{name:'Continue',exact:true}).click();
  const consent=await (await context.request.get('/api/agents/consent',{params:{code:device.user_code}})).json();
  const space=consent.spaces[0].slug;
  await page.getByRole('button',{name:'Allow',exact:true}).click();
  const granted=await request.post('/oauth/token',{form:{client_id:'part-cli',grant_type:'urn:ietf:params:oauth:grant-type:device_code',device_code:device.device_code}});
  expect(granted.status()).toBe(200);
  const headers={Authorization:`Bearer ${(await granted.json()).access_token}`};
  // Exercise the published board, adding only accessibility and test observation hooks.
  const contract=fs.readFileSync('/repo/docs/agent-contract.md','utf8');
  let content=contract.split('Hermes writes `board.html`:')[1].split('```html\n')[1].split('\n```')[0];
  content=content.replace('el.dataset.key = key;', 'el.setAttribute("role", "button"); el.setAttribute("aria-label", "Pixel " + x + " " + y); el.dataset.key = key;');
  content=content.replace('note.textContent = ""; }', 'note.textContent = ""; await devicePainted(); }');
  content=content.replace('"Not saved: " + e.message; }', '"Not saved: " + e.message; await deviceRefused(e, key, before); }');
  content=content.replace('loadBoard().finally(() => window.partifactory.ready());', `
const deviceCollector=${JSON.stringify(collector)}, deviceNonce=${JSON.stringify(nonce)};
async function devicePost(item, extra) {
  const response=await fetch(deviceCollector+"/report", {method:"POST",credentials:"omit",headers:{"Content-Type":"text/plain"},body:JSON.stringify(Object.assign({event:"device.check",item,lane:"safari-guest",outcome:"pass",run:deviceNonce},extra))});
  if(response.status!==204)throw new Error("Device collector refused report");
}
async function deviceWait(item) {
  for (;;) {const response=await fetch(deviceCollector+"/wait?run="+deviceNonce+"&item="+item,{credentials:"omit"}); if((await response.json()).length)return;}
}
async function devicePainted() {
  const at=Date.now();
  await devicePost("guest_painted",{at:String(at)});
  note.textContent="Paint accepted";
  await deviceWait("admin_frozen");
  cells["px:1:0"].setAttribute("aria-label","Paint frozen pixel");
}
async function deviceRefused(e,key,before) {
  if(e.name!=="PfError" || !["stale_epoch","forbidden_view_only"].includes(e.code) || cells[key].style.background!==before) {
    note.textContent="Freeze observed: "+e.name+" "+e.code;
    await devicePost("guest_refusal_mismatch",{outcome:"fail",error:e.name});
    throw new Error("Frozen pixel was not refused and reverted: "+e.name+" "+e.code);
  }
  await devicePost("guest_refused",{code:e.code,error:e.name});
  note.textContent="PfError: frozen pixel reverted";
}
loadBoard().then(async()=>{window.partifactory.ready();await deviceWait("admin_ready");cells["px:0:0"].setAttribute("aria-label","Paint communal pixel");});`);
  const name='ios-pixel';
  const created=await request.post('/api/v1/part_create',{headers,data:{name,space,title:'Simulator pixel board',kind:'html',content,request_id:'ios-pixel-'+nonce}});
  expect(created.status(),await created.text()).toBe(200);
  await expect.poll(async()=> (await (await request.post('/api/v1/part_get',{headers,data:{name}})).json()).build?.status,{timeout:45_000}).toBe('ok');
  await page.goto(base+'/');
  await nativePasskey(page,context);
  await page.goto(`https://${name}-part.${apex}/`);
  const board=page.frameLocator('#pf-frame');
  await expect(board.locator('#b')).toBeVisible();
  await page.getByRole('button',{name:'Share',exact:true}).click();
  await page.getByRole('button',{name:'Anyone with the link',exact:true}).click();
  await page.getByLabel('Link access').selectOption('contribute');
  await page.getByRole('button',{name:'Copy link',exact:true}).click();
  const url=await page.getByLabel('Public link',{exact:true}).inputValue();
  const frame=page.frames().find(f=>f.url().includes('/c/'))!;
  await frame.evaluate(()=>{
    (window as any).devicePixelSeen=new Promise<number>(resolve=>{
      const cell=document.querySelector('[data-key="px:0:0"]')!;
      const observer=new MutationObserver(()=>{if(getComputedStyle(cell).backgroundColor==='rgb(228, 87, 46)'){observer.disconnect();resolve(Date.now());}});
      observer.observe(cell,{attributes:true,attributeFilter:['style']});
    });
  });
  await report('admin_ready');
  fs.writeFileSync(path.join(dir,'guest.json'),JSON.stringify({url,nonce}),{mode:0o600});
  const painted=await wait('guest_painted');
  const seen=await frame.evaluate(()=> (window as any).devicePixelSeen);
  const elapsed=Math.max(0,seen-Number(painted.at));
  expect(elapsed,'guest storage.set response to admin computed style').toBeLessThan(1000);
  await expect(board.locator('[data-key="px:0:0"]')).toHaveCSS('background-color','rgb(228, 87, 46)');
  await report('admin_live_under_1s',{at:String(elapsed)});
  await page.getByRole('button',{name:'Freeze',exact:true}).click();
  await report('admin_frozen');
  const refused=await wait('guest_refused');
  expect(refused.error).toBe('PfError');
  expect(['stale_epoch','forbidden_view_only']).toContain(refused.code);
  await expect(board.locator('[data-key="px:1:0"]')).not.toHaveCSS('background-color','rgb(228, 87, 46)');
  await page.screenshot({path:path.join(dir,'admin-after-freeze.png')});
  await expect.poll(async()=> (await logs('epoch.bump')).some((row:any)=>String(row.key).startsWith('minter:link:'))).toBe(true);
  const events=await logs('epoch.bump');
  expect(events.some(row=>String(row.key).startsWith('minter:link:'))).toBe(true);
  const writes=await logs('rt.op');
  expect(writes.some(row=>['stale_epoch','forbidden_view_only'].includes(String(row.outcome)))).toBe(true);
  fs.writeFileSync(path.join(dir,'admin-report.json'),JSON.stringify({pass:true,responseToComputedStyleMs:elapsed,refusal:refused.code,events,writes},null,2));
});
