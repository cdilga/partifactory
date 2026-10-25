import { test, expect, waitForLog } from './harness/fixtures';
import * as fs from 'node:fs';

const fixtures = () => JSON.parse(fs.readFileSync(process.env.PF_PART_FIXTURE ?? '/run/pf/run/part-fixture.json', 'utf8'));

test('the contract board paints across two pages within one second and loads 256 keys in one list', async ({page, context}, info) => {
  const key = info.project.name === 'chromium-desktop' ? 'pixel' : `pixel-${info.project.name}`;
  const url = fixtures()[key].url;
  const methods: string[]=[];
  let writerTab: string | undefined;
  context.on('request',r => {
    if(r.url().includes('/_rt/')) {
      methods.push(r.method());
      if(r.method()==='POST' && r.postDataJSON()?.op==='storage.set') writerTab=r.postDataJSON().args.tab;
    }
  });
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
  await page.evaluate(async()=>{await (window as any).storage.delete('px:0:0',true);});
  await expect.poll(()=>other.evaluate(()=>(window as any).changes.some((e:any)=>e.key==='px:0:0'&&e.op==='delete')),{timeout:1000}).toBe(true);
  await other.close();
  expect(writerTab).toBeTruthy();
  await page.evaluate(async(tab)=>{
    const set:Record<string,string>={};
    for(let y=0;y<16;y++) for(let x=0;x<16;x++) set[`px:${x}:${y}`]='#334455';
    const response=await fetch('/_rt/v1',{
      method:'POST',headers:{'Content-Type':'text/plain'},
      body:JSON.stringify({op:'storage.sync',args:{shared:true,set,delete:[],tab}})
    });
    if(!response.ok) throw new Error(`bulk seed failed: ${response.status} ${await response.text()}`);
  },writerTab);
  let listCalls=0;
  page.on('request',r=>{if(r.url().includes('/_rt/v1') && r.postDataJSON()?.op==='storage.list') listCalls++;});
  await page.reload();await page.waitForFunction(()=>!!(window as any).__pfReadyAt);
  expect(listCalls).toBe(1);
  await expect(page.locator('[data-key="px:15:15"]')).toHaveCSS('background-color','rgb(51, 68, 85)');
  const data=await page.evaluate(async()=>(window as any).storage.list('px:',true,{values:true}));
  expect(data.keys).toHaveLength(256);expect(data.next_cursor).toBeNull();
  expect(methods.filter(m=>m==='OPTIONS')).toEqual([]);
  expect((await waitForLog({event:'data.changed'})).length).toBeGreaterThan(0);
  expect((await waitForLog({event:'sse.deliver'})).some(line=>line.type==='data.changed')).toBe(true);
});
