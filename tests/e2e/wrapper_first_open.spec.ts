import { test, expect, waitForLog } from './harness/fixtures';
import { signIn } from './lib/public-auth';
import * as fs from 'node:fs';
const fixtures=()=>JSON.parse(fs.readFileSync(process.env.PF_PART_FIXTURE??'/run/pf/run/part-fixture.json','utf8'));

test('gallery opens the trusted wrapper in one click, hides its cap, preserves fragments and reloads without a grant',async({page,browserName},info)=>{
  test.skip(!['chromium-desktop','webkit-mobile'].includes(info.project.name));
  await signIn(page);
  await expect(page.locator('html')).toHaveAttribute('data-pf-hydrated','1');
  const grants:string[]=[];page.on('request',r=>{if(new URL(r.url()).pathname==='/grant')grants.push(r.url());});
  await page.getByRole('link',{name:'Open pixel',exact:true}).click();
  await expect(page.frameLocator('#pf-frame').locator('#b')).toBeVisible();
  expect(page.url()).toBe('https://pixel-part.pf.test/');expect(grants).toHaveLength(1);
  await expect(page.locator('.pf-revision')).toHaveText('REV 1');
  await page.reload();await expect(page.frameLocator('#pf-frame').locator('#b')).toBeVisible();expect(grants).toHaveLength(1);
  const isolation=await page.frames().find(f=>f.url().includes('/c/'))!.evaluate(()=>{
    let documentBlocked=false,storageBlocked=false;
    try{void parent.document.body;}catch{documentBlocked=true;}
    try{void parent.sessionStorage.length;}catch{storageBlocked=true;}
    return {documentBlocked,storageBlocked,origin:window.origin};
  });expect(isolation).toEqual({documentBlocked:true,storageBlocked:true,origin:'null'});
  const frameSrc=await page.locator('#pf-frame').getAttribute('src');
  await page.evaluate(()=>window.postMessage({type:'pf:regrant'},'*'));await page.waitForTimeout(100);
  expect(await page.locator('#pf-frame').getAttribute('src')).toBe(frameSrc);
  await page.goto('https://notes-part.pf.test/deep?x=1#share=contribute');
  await expect(page.getByRole('dialog')).toBeVisible();expect(page.url()).toBe('https://notes-part.pf.test/deep?x=1#share=contribute');
  expect(new URL((await page.locator('#pf-frame').getAttribute('src'))!,page.url()).pathname).toMatch(/\/c\/[^/]+\/deep$/);
  expect((await waitForLog({event:'wrapper.serve'})).some(l=>l.audience==='private')).toBe(true);
});

test('expired framed fetch makes one chrome regrant without a top-level navigation',async({page},info)=>{
  test.skip(!['chromium-desktop','webkit-mobile'].includes(info.project.name));
  await signIn(page);await page.goto('https://notes-part.pf.test/');
  await expect(page.frameLocator('#pf-frame').getByRole('heading',{name:'Serving fixture'})).toBeVisible();
  const navigation:string[]=[];page.on('framenavigated',f=>{if(f===page.mainFrame())navigation.push(f.url());});
  const chrome:string[]=[];page.on('request',r=>{if(r.url().includes('/api/chrome/v1/notes/grant'))chrome.push(r.url());});
  const expired=fixtures().expired.cap;
  const frame=page.frames().find(f=>f.url().includes('/c/'))!;
  await frame.evaluate(async(cap)=>{await Promise.all([1,2,3].map(()=>fetch('/c/'+cap+'/data.json')));},expired);
  await expect.poll(()=>chrome.length).toBe(1);
  await expect(page.frameLocator('#pf-frame').getByRole('heading',{name:'Serving fixture'})).toBeVisible();
  expect(navigation).toHaveLength(0);
  expect((await waitForLog({event:'chrome.grant'})).some(l=>l.outcome==='ok')).toBe(true);
});

test('live chrome CORS fuzz and JSON mutation requirements',async({request},info)=>{
  test.skip(info.project.name!=='chromium-desktop');
  for(const route of ['summary','links','grant','stepup']){
    const url='/api/chrome/v1/pixel/'+route;
    for(const origin of ['null','https://notes-part.pf.test','https://blobs-part.pf.test','https://old-pixel-part.pf.test','https://evil.example','http://pixel-part.pf.test']){
      const r=await request.fetch(url,{method:'OPTIONS',headers:{Origin:origin,'Access-Control-Request-Method':'POST','Access-Control-Request-Headers':'content-type'}});
      expect(r.headers()['access-control-allow-origin']).toBeUndefined();
    }
    const ok=await request.fetch(url,{method:'OPTIONS',headers:{Origin:'https://pixel-part.pf.test','Access-Control-Request-Method':'POST','Access-Control-Request-Headers':'content-type'}});
    expect(ok.status()).toBe(204);expect(ok.headers()['access-control-allow-origin']).toBe('https://pixel-part.pf.test');expect(ok.headers()['access-control-max-age']).toBe('600');
    if(route==='summary')continue;
    for(const headers of [{Origin:'https://pixel-part.pf.test','Content-Type':'application/json'},{Origin:'https://pixel-part.pf.test','Sec-Fetch-Site':'cross-site','Content-Type':'application/json'},{Origin:'https://pixel-part.pf.test','Sec-Fetch-Site':'same-site','Content-Type':'text/plain'}]){
      const r=await request.post(url,{headers,data:'{}'});expect(r.status()).toBe(403);
    }
  }
});

test('an unshared private part reveals no title and offers another sign-in',async({page},info)=>{
  test.skip(!['chromium-desktop','webkit-mobile'].includes(info.project.name));
  await signIn(page,'member-b');const response=await page.goto('/grant?part=notes');
  expect(response?.status()).toBe(403);
  await expect(page.getByRole('heading',{name:'403 · NOT SHARED WITH YOU'})).toBeVisible();
  await expect(page.getByRole('link',{name:'Sign in as someone else'})).toBeVisible();
  await expect(page.getByText('Serving fixture')).toHaveCount(0);
  expect(await page.locator('meta[property="og:description"]').getAttribute('content')).toBe('Sign in to open');
});

test('signed-out root and cards are content-free and identical for crawlers',async({request},info)=>{
  test.skip(info.project.name!=='chromium-desktop');
  const url='https://notes-part.pf.test/';
  const bot=await request.get(url,{headers:{'User-Agent':'Slackbot'}});const human=await request.get(url,{headers:{'User-Agent':'Mozilla/5.0'}});
  const body=await bot.text();expect(body).toBe(await human.text());expect(bot.headers()['set-cookie']).toBeUndefined();expect(bot.headers()['referrer-policy']).toBe('no-referrer');
  expect(body).not.toContain('Serving fixture');expect(body).not.toMatch(/v1\.[0-9]+\./);expect(body).toContain('<noscript>Sign in to open</noscript>');
  expect(bot.headers()['content-security-policy']).toContain("frame-src 'self'");expect(bot.headers()['content-security-policy']).not.toContain('unsafe-inline');
  const match=body.match(/property="og:image" content="([^"]+)"/);expect(match).not.toBeNull();
  const card=await request.get(match![1]);expect(card.headers()['content-type']).toBe('image/png');expect(card.headers()['cache-control']).toBe('public, max-age=31536000, immutable');
  const png=await card.body();expect(png.readUInt32BE(16)).toBe(1200);expect(png.readUInt32BE(20)).toBe(630);expect(png.length).toBeLessThanOrEqual(150000);
  const manifest=await request.get(url+'_pf/manifest.webmanifest');expect(await manifest.json()).toMatchObject({id:'/',scope:'/'});
});
