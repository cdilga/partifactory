import { test, expect, waitForLog } from './harness/fixtures';
import { signIn, nativePasskey } from './lib/public-auth';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { randomBytes } from 'node:crypto';

// Runs in its own disposable small-volume harness; pressure and the quota loop intentionally
// affect all writers. tests/storage/gate.sh owns image attachment and teardown even on failure.
test('real volume pressure refuses writes while content reads and deletion continue',async({page,request,context},info)=>{
  test.skip(process.env.PF_STORAGE_GATE!=='1'||info.project.name!=='chromium-desktop','Dedicated small-volume lane');
  test.setTimeout(240_000);
  const directory=process.env.PF_STORAGE_GATE_DATA!;
  const token=fs.readFileSync('/run/pf/run/secrets/api_token','utf8').trim();
  const metricsToken=fs.readFileSync('/run/pf/run/secrets/metrics_token','utf8').trim();
  const metrics=async()=>{
    const r=await request.get(process.env.PF_STORAGE_METRICS_URL??'http://host.docker.internal:19090/metrics',{headers:{Authorization:`Bearer ${metricsToken}`}});
    expect(r.status()).toBe(200);return r.text();
  };
  const metric=(body:string,name:string)=>{const hit=body.match(new RegExp('^'+name+' ([0-9]+)$','m'));expect(hit,name).not.toBeNull();return Number(hit![1]);};
  const call=(op:string,data:unknown)=>request.post('/api/v1/'+op,{headers:{Authorization:`Bearer ${token}`},data});
  await signIn(page);
  await nativePasskey(page,context);
  fs.writeFileSync(path.join(directory,'pressure-seed.request'),'seed');
  await expect.poll(()=>fs.existsSync(path.join(directory,'pressure-seed.ready')),{timeout:30_000}).toBe(true);
  const cases=JSON.parse(fs.readFileSync(path.join(directory,'pressure-seed.ready'),'utf8'));
  expect(cases.stop_cases.sort()).toEqual(['blobs','data_docs','data_snapshots','mirror_cache','previews','shared_blobs']);
  for(const store of cases.prune_cases){
    const r=await page.request.post('/admin/health/run-now',{headers:{Origin:'https://parts.pf.test'},form:{store}});
    expect(r.status(),await r.text()).toBe(200);
    const body=await metrics();
    const size=metric(body,`pf_store_bytes\\{store="${store}"\\}`);
    const cap=metric(body,`pf_store_cap_bytes\\{store="${store}"\\}`);
    expect(size,store).toBeLessThanOrEqual(cap);
  }
  expect((await waitForLog({event:'janitor.job'})).some(l=>l.outcome==='ok')).toBe(true);
  fs.writeFileSync(info.outputPath('metrics-stores.txt'),await metrics());
  await page.goto('https://notes-part.pf.test/');
  await expect(page.frameLocator('#pf-frame').getByRole('heading',{name:'Serving fixture'})).toBeVisible();
  const contentUrl=page.frames().find(f=>f.url().includes('/c/'))!.url();
  let stopped=false,created=0;
  for(let i=0;i<10_000;i++){
    const r=await call('part_create',{name:'quota-loop-'+i,title:'Quota fixture',request_id:'quota-loop-'+i});
    if(r.status()===429){expect(await r.json()).toMatchObject({error:{code:'quota_exceeded',details:{kind:'space_parts',limit:2000}}});stopped=true;break;}
    expect(r.status(),await r.text()).toBe(200);created++;
  }
  expect(stopped).toBe(true);expect(created).toBeGreaterThan(0);expect(created).toBeLessThanOrEqual(2000);
  for(let i=0;i<10;i++)expect((await call('part_create',{name:'quota-retry',title:'Retry',request_id:'quota-retry-'+i})).status()).toBe(429);
  expect((await waitForLog({event:'quota.hit'})).filter(l=>l.kind==='space_parts')).toHaveLength(1);
  const before=await metrics();const budget=metric(before,'pf_disk_budget_bytes');const used=metric(before,'pf_disk_used_bytes');
  const fill=Math.max(0,Math.ceil(budget*.95)-used+1024*1024);
  expect(fill).toBeLessThan(120*1024*1024);
  const filler=path.join(directory,'pressure.fixture');const fd=fs.openSync(filler,'wx');
  try{
    const chunk=randomBytes(1024*1024);let remaining=fill;
    while(remaining>0){const n=Math.min(chunk.length,remaining);fs.writeSync(fd,chunk,0,n);remaining-=n;}fs.fsyncSync(fd);
  }finally{fs.closeSync(fd);}
  try{
    await expect.poll(async()=>metric(await metrics(),'pf_disk_budget_state'),{timeout:45_000}).toBe(2);
    const refused=await call('part_create',{name:'storage-full',title:'Full fixture',request_id:'storage-full'});
    expect(refused.status(),await refused.text()).toBe(507);expect(await refused.json()).toMatchObject({error:{code:'storage_full'}});
    const read=await call('part_get',{name:'notes'});expect(read.status()).toBe(200);
    const content=await request.get(contentUrl);expect(content.status()).toBe(200);expect(await content.text()).toContain('Serving fixture');
    const deleted=await call('part_edit',{name:'quota-loop-0',delete:true,request_id:'pressure-delete'});expect(deleted.status(),await deleted.text()).toBe(200);
    expect((await waitForLog({event:'disk.budget_state'})).some(l=>l.state==='full')).toBe(true);
    expect((await waitForLog({event:'op'})).some(l=>l.op==='part_create'&&l.outcome==='storage_full')).toBe(true);
    await page.goto('/admin/health');
    await expect(page.locator('[data-health-row="Disk budget"] .red')).toBeVisible();
    await page.screenshot({path:info.outputPath('health-disk-full.png'),fullPage:true});
    fs.writeFileSync(info.outputPath('metrics-full.txt'),await metrics());
  }finally{fs.unlinkSync(filler);}
});
