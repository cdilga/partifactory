import {test,expect,serverLog} from './harness/fixtures';
import {signIn,nativePasskey} from './lib/public-auth';
import {ExactBudget,countAssertions} from './lib/budget';
import {agentDriver,freshAdmin,signedOutGuest,observePaint} from './lib/pixel-acceptance';
import {contractPixel,pixelFixture} from './fixtures/pixel';
import * as fs from 'node:fs';
import * as path from 'node:path';
import {createHash} from 'node:crypto';

test('pixel fixture is the published worked session with its own documented edit applied',()=>{
  const published=contractPixel();
  expect(published.edit).toMatchObject({old_str:'loadBoard().finally',new_str:'load().finally'});
  expect(published.source).toContain('async function load()');
  expect(published.fixed).toBe(pixelFixture);
});

test('communal_pixel_editor: real device agents, exact admin budgets and signed-out guest matrix',async({page,context,request,playwright},info)=>{
  test.skip(process.env.PF_COMMUNAL_ACCEPTANCE!=='1','Explicit fresh-harness acceptance lane');
  test.skip(!['chromium-desktop','chromium-mobile'].includes(info.project.name),'Two genuine virtual-authenticator admin profiles');
  test.setTimeout(360_000);
  const mobile=info.project.name==='chromium-mobile';
  const base=process.env.PF_BASE_URL??'https://parts.pf.test',apex=new URL(base).hostname.slice('parts.'.length);
  const run=path.join('/repo/target/e2e-runs',process.env.PF_E2E_RUN!);
  const evidence=path.resolve('../../target/e2e-logs',process.env.PF_E2E_RUN!,'communal_pixel_editor',info.project.name);
  fs.mkdirSync(evidence,{recursive:true});
  const offset=(await serverLog()).length;
  await signIn(page);
  const {cdp}=await nativePasskey(page,context);
  const hardware=await countAssertions(cdp,page);
  const agents:any[]=[];
  for(const surface of ['http','mcp'] as const) {
    const device=await (await request.post('/oauth/device_authorization',{form:{client_id:'part-cli',agent_name:`Pixel test agent ${surface} ${info.project.name}`,scope:'parts:read parts:write data:read data:write'}})).json();
    await page.goto(device.verification_uri_complete);
    await page.getByRole('button',{name:'Continue',exact:true}).click();
    await page.getByRole('button',{name:'Allow',exact:true}).click();
    await expect(page.getByRole('status')).toHaveText('Agent connected. You can return to your terminal.');
    const grant=await request.post('/oauth/token',{form:{client_id:'part-cli',grant_type:'urn:ietf:params:oauth:grant-type:device_code',device_code:device.device_code}});
    expect(grant.status()).toBe(200);
    const token=(await grant.json()).access_token;
    const call=agentDriver(surface,token,request,run);
    const name=mobile?`communal-mobile-${surface}`:surface==='http'?'pixel':'communal-mcp';
    // Disable only periodic snapshots, through the public create option: the contribution
    // snapshot is then deterministically newest regardless of the wall-clock minute.
    const created=await call('part_create',{name,title:'Communal pixel board',kind:'html',content:pixelFixture,live:'after_check',snapshots:'off',request_id:`communal-${name}-create`});
    expect(created.head).toBe(1);
    expect(created.url).toBe(`https://${name}-part.${apex}/`);
    const checked=await call('part_check',{name,actions:[{do:'click',selector:'[data-key="px:0:0"]'},{do:'wait_ms',ms:300},{do:'eval_return',expr:'window.storage.get("px:0:0", true).then(r => r && r.value)'}]});
    expect(checked.passed).toBe(true);expect(checked.blank).toBe(false);expect(checked.errors).toEqual([]);
    expect(checked.actions[2].value).toBe('#e4572e');
    const read=await call('part_get',{name});expect(read.head).toBe(1);expect(read.live).toBe(1);
    expect((await call('part_data',{name,action:'list',prefix:'px:'})).docs).toEqual([]);
    agents.push({surface,name,call,id:read.id,url:created.url,check:{passed:checked.passed,blank:checked.blank,errors:checked.errors,head:read.head,live:read.live}});
  }
  const primary=agents.find(agent=>agent.surface===(mobile?'mcp':'http'))!;
  await freshAdmin(page,base);
  const adminEvents=page.waitForResponse(response=>response.url().includes('/_rt/events')&&response.status()===200);
  await page.goto(primary.url);await expect(page.frameLocator('#pf-frame').locator('#b')).toBeVisible();await adminEvents;
  expect(page.url()).toBe(primary.url);expect(page.url()).not.toContain('#pf=');
  const budgets:any[]=[];
  const share=new ExactBudget(page,hardware,'Can contribute + Copy',4,1);
  await share.click(page.getByRole('button',{name:'Share',exact:true}),'Share');
  await share.click(page.getByRole('button',{name:'Anyone with the link',exact:true}),'Anyone with the link');
  await share.select(page.getByLabel('Link access'),'contribute','Can contribute');
  await share.click(page.getByRole('button',{name:'Copy link',exact:true}),'Copy link');
  const link=page.getByLabel('Public link',{exact:true});await expect(link).toBeVisible();
  const url=await link.inputValue();budgets.push(await share.check());
  const createdLinks=await (await page.request.get(base+`/api/chrome/v1/${primary.name}/links`,{headers:{Origin:new URL(primary.url).origin}})).json();
  const linkId=createdLinks.links.find((value:any)=>value.mode==='contribute').id;
  await page.screenshot({path:path.join(evidence,'admin-share.png'),mask:[link]});
  // Each independently budgeted action starts with the sheet closed.
  await page.keyboard.press('Escape');
  const guests:Awaited<ReturnType<typeof signedOutGuest>>[]=[];
  const timings:Record<string,number[]>={};
  try {
    for(const [browserIndex,kind] of ['webkit','firefox','chromium'].entries()) {
      const guest=await signedOutGuest(playwright,kind,apex);guests.push(guest);
      const events=guest.page.waitForResponse(response=>response.url().includes('/_rt/events')&&response.status()===200);
      await guest.page.goto(url);await expect(guest.page.frameLocator('#pf-frame').locator('#b')).toBeVisible();await events;
      expect((await guest.context.cookies()).some(cookie=>cookie.name==='__Host-pf_session')).toBe(false);
      timings[kind]=[];
      for(let sample=0;sample<3;sample++)timings[kind].push(await observePaint(page,guest.page,`px:${browserIndex*3+sample}:0`));
      await guest.page.screenshot({path:path.join(evidence,`${kind}-painted.png`)});
    }
    const docs=(await primary.call('part_data',{name:primary.name,action:'list',prefix:'px:'})).docs;
    expect(docs).toHaveLength(9);expect(docs.every((doc:any)=>doc.provenance==='link_viewer')).toBe(true);
    const snapshots=await primary.call('part_data',{name:primary.name,action:'snapshot_list'});
    expect(snapshots.snapshots[0]).toMatchObject({reason:'contribute',docs:0});
    const rollback=new ExactBudget(page,hardware,'Roll back contribution snapshot',3,0);
    await rollback.click(page.getByRole('button',{name:'Share',exact:true}),'Share');
    await rollback.click(page.getByRole('button',{name:'Roll back data',exact:true}),'Roll back data');
    const selected=page.getByLabel('Snapshot');await expect(selected).toHaveValue(String(snapshots.snapshots[0].id));
    await expect(selected.locator('option:checked')).toContainText('contribute');
    const restored=page.waitForResponse(response=>response.request().method()==='POST'&&response.url().endsWith(`/api/chrome/v1/${primary.name}/data`)&&response.request().postDataJSON()?.action==='snapshot_restore');
    await rollback.click(page.getByRole('button',{name:'Restore snapshot',exact:true}),'Restore snapshot');
    expect((await restored).status()).toBe(200);await expect(page.getByRole('status')).toHaveText('Data restored.');
    budgets.push(await rollback.check());
    for(const tab of [page,...guests.map(guest=>guest.page)])await expect.poll(()=>tab.frameLocator('#pf-frame').locator('#b i').evaluateAll(cells=>cells.every(cell=>getComputedStyle(cell).backgroundColor==='rgba(0, 0, 0, 0)')),{timeout:3000}).toBe(true);
    expect((await primary.call('part_data',{name:primary.name,action:'list',prefix:'px:'})).docs).toEqual([]);
    await page.keyboard.press('Escape');
    const freeze=new ExactBudget(page,hardware,'Freeze',2,0);
    await freeze.click(page.getByRole('button',{name:'Share',exact:true}),'Share');
    const frozen=page.waitForResponse(response=>response.request().method()==='POST'&&response.url().endsWith(`/links/${linkId}`));
    await freeze.click(page.getByRole('button',{name:'Freeze',exact:true}),'Freeze');
    expect((await frozen).status()).toBe(200);await expect(page.getByText('Frozen.',{exact:true})).toBeVisible();
    budgets.push(await freeze.check());
    for(const [index,guest] of guests.entries()) {
      const frame=guest.page.frames().find(frame=>frame.url().includes('/c/'))!;
      await frame.evaluate(()=>{
        const set=(window as any).storage.set;
        (window as any).storage.set=async(...args:any[])=>{try{return await set(...args);}catch(error:any){(window as any).refused={name:error.name,code:error.code};throw error;}};
      });
      const response=guest.page.waitForResponse(response=>response.request().method()==='POST'&&response.url().includes('/_rt/v1')&&response.request().postDataJSON()?.op==='storage.set');
      await guest.page.frameLocator('#pf-frame').locator('[data-key="px:9:0"]').click();
      expect((await response).status()).toBe(403);
      await expect.poll(()=>frame.evaluate(()=>(window as any).refused)).toMatchObject({name:'PfError',code:'forbidden_view_only'});
      await expect(guest.page.frameLocator('#pf-frame').locator('[data-key="px:9:0"]')).not.toHaveCSS('background-color','rgb(228, 87, 46)');
      await guest.page.screenshot({path:path.join(evidence,`${['webkit','firefox','chromium'][index]}-frozen.png`)});
    }
    const trace=(await serverLog()).slice(offset);
    let cursor=-1;
    const ordered=(label:string,predicate:(row:any)=>boolean)=>{const next=trace.findIndex((row,index)=>index>cursor&&predicate(row));expect(next,`${label} after ${cursor}`).toBeGreaterThan(cursor);cursor=next;return {label,...trace[next]};};
    const orderedEvents=[
      ordered('device agent created board',row=>row.event==='part.write'&&row.op==='part_create'&&row.part_id===primary.id&&row.outcome==='ok'),
      ordered('gate render passed',row=>row.event==='render.job'&&row.purpose==='gate'&&row.outcome==='passed'),
      ordered('live moved after gate',row=>row.event==='live.switch'&&row.part_id===primary.id&&row.result==='moved'),
      ordered('real in-sheet passkey assertion',row=>row.event==='stepup.verify'&&row.outcome==='ok'),
      ordered('contribute link created',row=>row.event==='link.create'&&row.mode==='contribute'),
      ordered('signed-out guest capability',row=>row.event==='link.go'&&row.mode==='contribute'),
      // Committed data.changed is emitted before the enclosing runtime operation returns.
      ordered('guest write committed data event',row=>row.event==='data.changed'&&row.part_id===primary.id),
      ordered('guest shared write returned',row=>row.event==='rt.op'&&row.op==='storage.set'&&row.principal?.startsWith(`guest:${linkId}:`)&&row.outcome==='ok'),
      ordered('UI snapshot restored',row=>row.event==='snapshot.restore'&&row.outcome==='ok'),
      ordered('Freeze epoch committed',row=>row.event==='epoch.bump'&&row.key===`minter:link:${linkId}`),
      ordered('next guest write refused',row=>row.event==='rt.op'&&row.op==='storage.set'&&row.principal?.startsWith(`guest:${linkId}:`)&&row.outcome==='forbidden_view_only'),
    ];
    await page.screenshot({path:path.join(evidence,'admin-after-freeze.png'),mask:[page.getByLabel('Public link',{exact:true})]});
    // A fresh real session makes the deep-link's actual in-sheet biometric observable.
    const posted=await primary.call('part_link_for',{name:primary.name,action:'contribute'});
    expect(posted.url).toBe(primary.url+'#share=contribute');
    await freshAdmin(page,base);
    const deep=new ExactBudget(page,hardware,'Agent contribute link',2,1);
    await deep.openLink(posted.url);await expect(page.getByRole('dialog')).toBeVisible();
    await deep.click(page.getByRole('button',{name:'Copy link',exact:true}),'Confirm contribute link');
    await expect(page.getByLabel('Public link',{exact:true})).toBeVisible();budgets.push(await deep.check());
    const report={pass:true,project:info.project.name,agents:agents.map(({surface,name,url,check})=>({surface,name,url,check})),budgets,timings,p100Ms:Math.max(...Object.values(timings).flat()),provenance:'link_viewer',rollbackEmptyTabs:4,guestRefusals:3,orderedEvents,fixtureSha256:createHash('sha256').update(pixelFixture).digest('hex')};
    fs.writeFileSync(path.join(evidence,'report.json'),JSON.stringify(report,null,2));
    console.log(`communal_pixel_editor ${info.project.name} PASS HTTP+rmcp,3guest browsers x3paints,p100=${report.p100Ms}ms,exactbudgets,rollback4tabs,Freeze3refusals`);
  } finally {for(const guest of guests)await guest.close();}
});
