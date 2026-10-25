import { test, expect, waitForLog } from './harness/fixtures';
import { signIn } from './lib/public-auth';
import * as fs from 'node:fs';
import * as path from 'node:path';

test('Stockroom gallery hydrates, switches theme, and serves only local font and wasm assets',async({page},info)=>{
  test.skip(!['chromium-desktop','webkit-mobile'].includes(info.project.name));
  const assets:string[]=[];page.on('request',r=>{if(/woff2|\.wasm/.test(r.url()))assets.push(r.url());});
  const errors:string[]=[];page.on('pageerror',e=>errors.push(e.message));
  await signIn(page);await expect(page.locator('html')).toHaveAttribute('data-pf-hydrated','1');
  await expect(page.getByRole('heading',{name:'Parts',exact:true})).toBeVisible();
  expect(assets.some(u=>u.endsWith('.wasm'))).toBe(true);
  expect(assets.every(u=>new URL(u).origin==='https://parts.pf.test')).toBe(true);
  await page.getByRole('button',{name:'Toggle theme'}).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme','dark');
  const evidence=path.join('/repo/target/evidence','pf-wmcw',info.project.name);fs.mkdirSync(evidence,{recursive:true});
  await page.screenshot({path:path.join(evidence,'gallery-dark.png')});
  await page.getByRole('button',{name:'Toggle theme'}).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme','light');
  await page.screenshot({path:path.join(evidence,'gallery-light.png')});
  await page.setViewportSize({width:360,height:740});await page.screenshot({path:path.join(evidence,'gallery-360.png')});
  await page.getByRole('link',{name:'Open notes',exact:true}).click();
  await expect(page.frameLocator('#pf-frame').getByRole('heading',{name:'Serving fixture'})).toBeVisible();
  await page.screenshot({path:path.join(evidence,'part-360-light.png')});
  await page.emulateMedia({colorScheme:'dark'});await page.screenshot({path:path.join(evidence,'part-360-dark.png')});
  expect(errors).toEqual([]);
  expect((await waitForLog({event:'ui.render'})).some(l=>l.page==='gallery')).toBe(true);
});
