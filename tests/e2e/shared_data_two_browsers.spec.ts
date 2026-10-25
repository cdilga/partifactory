import { test, expect, waitForLog, serverLog } from './harness/fixtures';
import * as fs from 'node:fs';

const fixtures = () => JSON.parse(fs.readFileSync(process.env.PF_PART_FIXTURE ?? '/run/pf/run/part-fixture.json', 'utf8'));

test('the contract board paints across two pages within one second and loads 256 keys in one list', async ({page, context}, info) => {
  const key = info.project.name === 'chromium-desktop' ? 'pixel' : `pixel-${info.project.name}`;
  const url = fixtures()[key].url;
  const methods: string[]=[];
  context.on('request',r => {if(r.url().includes('/_rt/')) methods.push(r.method());});
  const other=await context.newPage();
  await Promise.all([page.goto(url),other.goto(url)]);
  await Promise.all([page.waitForFunction(()=>!!(window as any).__pfReadyAt),other.waitForFunction(()=>!!(window as any).__pfReadyAt)]);
  await other.evaluate(()=>{(window as any).changes=[];(window as any).partifactory.on('data.changed',(e:any)=>(window as any).changes.push(e));});
  await page.evaluate(()=>{(window as any).echoes=[];(window as any).partifactory.on('data.changed',(e:any)=>(window as any).echoes.push(e));});
  const started=Date.now();
  await page.locator('[data-key="px:0:0"]').click();
  await expect(other.locator('[data-key="px:0:0"]')).toHaveCSS('background-color','rgb(228, 87, 46)',{timeout:1000});
  expect(Date.now()-started).toBeLessThan(1000);
  expect(await page.evaluate(()=>(window as any).echoes.length)).toBe(0);
  await page.evaluate(async()=>{
    const storage=(window as any).storage;
    for(let y=0;y<16;y++) for(let x=0;x<16;x++) await storage.set(`px:${x}:${y}`,'#334455',true);
  });
  let listCalls=0;
  page.on('request',r=>{if(r.url().includes('/_rt/v1') && r.postDataJSON()?.op==='storage.list') listCalls++;});
  await page.reload();await page.waitForFunction(()=>!!(window as any).__pfReadyAt);
  expect(listCalls).toBe(1);
  await expect(page.locator('[data-key="px:15:15"]')).toHaveCSS('background-color','rgb(51, 68, 85)');
  const data=await page.evaluate(async()=>(window as any).storage.list('px:',true,{values:true}));
  expect(data.keys).toHaveLength(256);expect(data.next_cursor).toBeNull();
  await page.evaluate(async()=>{await (window as any).storage.delete('px:0:0',true);});
  await expect.poll(()=>other.evaluate(()=>(window as any).changes.some((e:any)=>e.key==='px:0:0'&&e.op==='delete')),{timeout:1000}).toBe(true);
  expect(methods.filter(m=>m==='OPTIONS')).toEqual([]);
  expect((await waitForLog({event:'data.changed'})).length).toBeGreaterThan(0);
  expect((await waitForLog({event:'sse.deliver'})).some(line=>line.type==='data.changed')).toBe(true);
  await other.close();
});
