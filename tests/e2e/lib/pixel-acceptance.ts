import {expect, devices, type APIRequestContext, type Page, type Playwright, type BrowserContext} from '@playwright/test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';
import {randomUUID} from 'node:crypto';

export async function freshAdmin(page:Page,base:string) {
  await page.context().clearCookies();
  await page.goto(base+'/signin?return=/');
  await page.locator('input[name=login]').fill('admin@example.com');
  await page.locator('input[name=password]').fill('password');
  await page.locator('button[type=submit]').click();
  await expect.poll(async()=> (await page.request.get(base+'/api/session')).status()).toBe(200);
}

export function agentDriver(surface:'http'|'mcp',token:string,request:APIRequestContext,run:string) {
  return async(op:string,input:Record<string,unknown>)=>{
    if(surface==='http') {
      const response=await request.post('/api/v1/'+op,{headers:{Authorization:`Bearer ${token}`},data:input});
      expect(response.status(),await response.text()).toBe(200);return response.json();
    }
    const directory=path.join(run,'communal-private');
    fs.mkdirSync(directory,{recursive:true,mode:0o700});
    expect(fs.existsSync(path.join(directory,'ready')),'Native rmcp bridge was started by the runner').toBe(true);
    const id=randomUUID(),file=path.join(directory,id+'.request.json'),result=path.join(directory,id+'.result.json');
    fs.writeFileSync(file+'.tmp',JSON.stringify({token,op,input}),{mode:0o600});fs.renameSync(file+'.tmp',file);
    await expect.poll(()=>fs.existsSync(result),{timeout:125_000,intervals:[50,100,250],message:'Real rmcp completion barrier for '+op}).toBe(true);
    const value=JSON.parse(fs.readFileSync(result,'utf8'));
    expect(value.broker_error).toBeUndefined();expect(value.is_error,JSON.stringify(value.result)).not.toBe(true);
    if(op==='part_check')expect(value.images).toBe(1);
    return value.result;
  };
}

export async function signedOutGuest(playwright:Playwright,kind:string,apex:string) {
  let context:BrowserContext;
  let close:()=>Promise<void>;
  if(kind==='firefox') {
    const profile=fs.mkdtempSync(path.join(os.tmpdir(),'pf-pixel-firefox-'));
    fs.cpSync(process.env.PF_FIREFOX_PROFILE!,profile,{recursive:true});
    context=await playwright.firefox.launchPersistentContext(profile,{viewport:{width:1280,height:800}});
    close=async()=>{await context.close();fs.rmSync(profile,{recursive:true,force:true});};
  } else {
    const browser=kind==='webkit'?await playwright.webkit.launch():await playwright.chromium.launch({args:[`--host-resolver-rules=MAP *.${apex} ${process.env.PF_HARNESS_IP}, EXCLUDE localhost`]});
    context=await browser.newContext(kind==='webkit'?devices['iPhone 15']:devices['Pixel 7']);
    close=async()=>{await context.close();await browser.close();};
  }
  expect(await context.cookies()).toEqual([]);
  return {context,page:await context.newPage(),close};
}

export async function observePaint(admin:Page,guest:Page,key:string) {
  const adminFrame=admin.frames().find(frame=>frame.url().includes('/c/'))!;
  const guestFrame=guest.frames().find(frame=>frame.url().includes('/c/'))!;
  await adminFrame.evaluate(key=>{
    const cell=document.querySelector(`[data-key="${key}"]`)!;
    (window as any).pixelSeen=new Promise<number>(resolve=>{
      const observer=new MutationObserver(()=>{if(getComputedStyle(cell).backgroundColor==='rgb(228, 87, 46)'){observer.disconnect();resolve(Date.now());}});
      observer.observe(cell,{attributes:true,attributeFilter:['style']});
    });
  },key);
  // Observe promise completion from the genuine prelude response; no mocked clocks or writes.
  await guestFrame.evaluate(()=>{
    (window as any).pixelWriteCompleted=0;
    const set=(window as any).storage.set;
    (window as any).storage.set=async(...args:any[])=>{const result=await set(...args);(window as any).pixelWriteCompleted=Date.now();return result;};
  });
  const response=guest.waitForResponse(response=>response.request().method()==='POST'&&response.url().includes('/_rt/v1')&&response.request().postDataJSON()?.op==='storage.set');
  await guest.frameLocator('#pf-frame').locator(`[data-key="${key}"]`).click();
  expect((await response).status()).toBe(200);
  await expect(admin.frameLocator('#pf-frame').locator(`[data-key="${key}"]`)).toHaveCSS('background-color','rgb(228, 87, 46)',{timeout:1000});
  await expect.poll(()=>guestFrame.evaluate(()=>(window as any).pixelWriteCompleted)).toBeTruthy();
  const [completed,seen]=await Promise.all([guestFrame.evaluate(()=>(window as any).pixelWriteCompleted),adminFrame.evaluate(()=>(window as any).pixelSeen)]);
  const elapsed=Math.max(0,seen-completed);
  expect(elapsed,'Guest write response to admin computed style').toBeLessThan(1000);
  return elapsed;
}
