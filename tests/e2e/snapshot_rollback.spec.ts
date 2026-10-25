import { test, expect, waitForLog, serverLog } from './harness/fixtures';
import * as fs from 'node:fs';

const fixtures=()=>JSON.parse(fs.readFileSync(process.env.PF_PART_FIXTURE ?? '/run/pf/run/part-fixture.json','utf8'));
const token=()=>fs.readFileSync(process.env.PF_DEV_API_TOKEN_FILE ?? '/run/pf/run/secrets/api_token','utf8').trim();

test('part_data reads runtime documents and restores a snapshot into an open board',async({page,request},info)=>{
  const name=`pixel-undo-${info.project.name}`;
  await page.goto(fixtures()[name].url);await page.waitForFunction(()=>!!(window as any).__pfReadyAt);
  await page.locator('[data-key="px:0:0"]').click();
  const data=async(args:Record<string,unknown>)=>{
    const response=await request.post('/api/v1/part_data',{headers:{Authorization:`Bearer ${token()}`},data:{name,...args}});
    expect(response.ok(),await response.text()).toBe(true);return response.json();
  };
  await expect.poll(async()=> (await data({action:'get',doc_id:'px:0:0'})).doc?.data).toBe('#e4572e');
  const snapshots=await data({action:'snapshot_list'});expect(snapshots.policy).toEqual({mode:'daily',keep:7});
  expect(snapshots.snapshots.length).toBeGreaterThan(0);
  const id=snapshots.snapshots[0].id;
  const canary=`unlogged-${info.project.name}-canary-data-value`;
  await data({action:'set',doc_id:'px:0:0',data:canary,request_id:`bad-write-${info.project.name}`});
  await expect(page.locator('[data-key="px:0:0"]')).toHaveCSS('background-color','rgba(0, 0, 0, 0)',{timeout:1000});
  const restored=await data({action:'snapshot_restore',snapshot_id:id,request_id:`undo-${info.project.name}`});expect(restored.docs).toBe(1);
  await expect(page.locator('[data-key="px:0:0"]')).toHaveCSS('background-color','rgb(228, 87, 46)',{timeout:1000});
  expect((await waitForLog({event:'snapshot.restore'})).some(line=>line.outcome==='ok')).toBe(true);
  expect(JSON.stringify(await serverLog())).not.toContain(canary);
});
