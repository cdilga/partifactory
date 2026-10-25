import { test, expect, waitForLog, serverLog } from './harness/fixtures';
import * as fs from 'node:fs';

test('renderer checks throwing blank and interactive pages without changing shared data', async({request}, info)=>{
  test.skip(info.project.name!=='chromium-desktop','The renderer owns its own browser; one API lane exercises it.');
  test.setTimeout(180000);
  const headers={Authorization:`Bearer ${fs.readFileSync(process.env.PF_DEV_API_TOKEN_FILE??'/run/pf/run/secrets/api_token','utf8').trim()}`};
  const call=(op:string,data:unknown)=>request.post(`/api/v1/${op}`,{headers,data});
  const probe=await call('part_check',{name:'pixel'});
  if(process.env.PF_E2E_RENDER==='1') expect(probe.ok(),`Enabled renderer: ${await probe.text()}`).toBeTruthy();
  if(probe.status()===503){
    expect((await probe.json()).error.code).toBe('renderer_unavailable');
    const created=await call('part_create',{name:'render-off',title:'Renderer off',kind:'html',files:{'index.html':'<!doctype html><body>renderer optional</body>'},live:'after_check',request_id:'render-off-create'});
    expect(created.ok(),await created.text()).toBeTruthy();
    await expect.poll(async()=> (await (await call('part_get',{name:'render-off'})).json()).live).toBe(1);
    const part=await (await call('part_get',{name:'render-off'})).json();
    expect(JSON.stringify(part.build.warnings)).toContain('check_skipped');
    return;
  }
  expect(probe.ok(),await probe.text()).toBeTruthy();
  const create=async(name:string,html:string)=>{
    const r=await call('part_create',{name,title:name,kind:'html',files:{'index.html':html},live:'after_check',request_id:`${name}-create`});
    expect(r.ok(),await r.text()).toBeTruthy();
    await expect.poll(async()=> (await (await call('part_get',{name})).json()).build.status).toBe('ok');
  };
  await create('render-throw','<!doctype html><body>throwing<script>throw new Error("render-throw-canary")</script></body>');
  let r=await call('part_check',{name:'render-throw',include_base64:true});
  expect(r.ok(),await r.text()).toBeTruthy();let result=await r.json();
  expect(result.passed).toBe(false);expect(JSON.stringify(result.errors)).toContain('render-throw-canary');
  expect(result.screenshot.base64).toBeTruthy();
  fs.writeFileSync(info.outputPath('throw.png'),Buffer.from(result.screenshot.base64,'base64'));
  expect((await (await call('part_get',{name:'render-throw'})).json()).live).toBeNull();
  r=await call('part_edit',{name:'render-throw',edits:[{op:'replace',old_str:'throw new Error("render-throw-canary")',new_str:'partifactory.ready()'}],live:'after_check',request_id:'render-throw-fix',wait:true});
  expect(r.ok(),await r.text()).toBeTruthy();
  await expect.poll(async()=> (await (await call('part_get',{name:'render-throw'})).json()).live).toBe(2);
  await create('render-blank','<!doctype html><body style="background:white"></body>');
  result=await (await call('part_check',{name:'render-blank'})).json();
  expect(result.blank).toBe(true);expect(result.passed).toBe(false);expect(result.timing.ready_ms).toBeNull();
  const screenshot=await request.get(result.screenshot.url,{headers});
  expect(screenshot.ok()).toBeTruthy();expect(screenshot.headers()['cache-control']).toBe('private, no-store');
  fs.writeFileSync(info.outputPath('blank.png'),await screenshot.body());
  const before=await (await call('part_data',{name:'pixel',action:'list'})).json();
  result=await (await call('part_check',{name:'pixel',include_base64:true,actions:[{do:'click',selector:'[data-key="px:1:1"]'},{do:'wait_ms',ms:100},{do:'eval_return',expr:'window.storage.get("px:1:1",true)'}]})).json();
  expect(result.passed).toBe(true);expect(result.actions[2].value.value).toBe('#e4572e');
  fs.writeFileSync(info.outputPath('pixel.png'),Buffer.from(result.screenshot.base64,'base64'));
  expect(await (await call('part_data',{name:'pixel',action:'list'})).json()).toEqual(before);
  expect((await waitForLog({event:'render.job'})).some(l=>l.outcome==='failed')).toBe(true);
  expect((await waitForLog({event:'live.switch'})).some(l=>l.gate==='after_check')).toBe(true);
  expect(JSON.stringify(await serverLog())).not.toMatch(/\/c\/[A-Za-z0-9_-]{20}/);
});
