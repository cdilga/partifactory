import { test, expect } from '../e2e/harness/fixtures';
import { signIn, nativePasskey } from '../e2e/lib/public-auth';
import * as fs from 'node:fs';
import * as path from 'node:path';

// Imported by tests/e2e/device_pass.spec.ts so the accumulated harness also runs this matrix.
// Reports contain item names and engine versions; capability/public-link URLs stay out of logs.
test('shipping device matrix: standalone, part root, reload, guest and sign-in', async ({ page, context, browserName, browser }, info) => {
  test.skip(process.env.PF_DEVICE_PASS!=='1','Dedicated device lane');
  test.skip(!['chromium-mobile','webkit-mobile','firefox-desktop'].includes(info.project.name),'Shipping device matrix');
  test.setTimeout(90_000);
  const run=process.env.PF_E2E_RUN??'local';
  const directory=process.env.PF_DEVICE_STATE_DIR??`/repo/target/e2e-runs/${run}/device-pass`;
  fs.mkdirSync(directory,{recursive:true,mode:0o700});
  const fixture=JSON.parse(fs.readFileSync(process.env.PF_PART_FIXTURE??'/run/pf/run/part-fixture.json','utf8'));
  const reports:Record<string,unknown>[]=[];
  const report=async(item:string,outcome='pass')=>{
    const row={event:'device.check',item,lane:info.project.name,outcome,browser:browser.version()};
    reports.push(row);
    const endpoint=process.env.PF_HARNESS_RELEASE==='1'?(process.env.PF_DEVICE_REPORT_URL??'https://blobs-part.pf.test/__harness/report'):'https://parts.pf.test/_dev/report';
    const r=await context.request.post(endpoint,{data:row});
    expect(r.status()).toBe(204);
  };
  try {
    await page.goto(fixture.notes.url);
    await expect(page.getByRole('heading',{name:'Serving fixture'})).toBeVisible();
    await report('standalone');
    const previousAuthDirectory=process.env.PF_BROWSER_AUTH_DIR;
    process.env.PF_BROWSER_AUTH_DIR=path.join(directory,'auth',info.project.name);
    try {await signIn(page);} finally {
      if(previousAuthDirectory===undefined)delete process.env.PF_BROWSER_AUTH_DIR;
      else process.env.PF_BROWSER_AUTH_DIR=previousAuthDirectory;
    }
    await expect.poll(async()=>(await page.request.get('https://parts.pf.test/api/session')).status()).toBe(200);
    await report('sign_in');
    if(browserName==='chromium'){
      const credentialFile=process.env.PF_HARNESS_RELEASE==='1'
        ?path.join('/repo/target/e2e-runs',run,'release-private/authenticator.json'):undefined;
      await nativePasskey(page,context,credentialFile);
      await report('passkey_step_up');
    }else await report('passkey_step_up','not_assertable');
    await page.goto('https://notes-part.pf.test/');
    await expect(page.frameLocator('#pf-frame').getByRole('heading',{name:'Serving fixture'})).toBeVisible();
    await report('part_root_first_open');
    await page.reload();
    await expect(page.frameLocator('#pf-frame').getByRole('heading',{name:'Serving fixture'})).toBeVisible();
    await report('part_root_reload');
    const linkFile=path.join(directory,'guest-link.json');
    if(info.project.name==='chromium-mobile'){
      await page.getByRole('button',{name:'Share',exact:true}).click();
      await page.getByRole('button',{name:'Anyone with the link',exact:true}).click();
      await page.getByRole('button',{name:'Copy link',exact:true}).click();
      const input=page.getByLabel('Public link',{exact:true});
      await expect(input).toBeVisible();
      fs.writeFileSync(linkFile,JSON.stringify({url:await input.inputValue()}),{mode:0o600});
    }
    await expect.poll(()=>fs.existsSync(linkFile),{timeout:45_000}).toBe(true);
    const guestUrl=JSON.parse(fs.readFileSync(linkFile,'utf8')).url;
    // Clearing workspace cookies proves the /l/ visit is an anonymous guest in this engine.
    await context.clearCookies();
    await page.goto(guestUrl);
    await expect(page.frameLocator('#pf-frame').getByRole('heading',{name:'Serving fixture'})).toBeVisible();
    expect((await context.cookies('https://parts.pf.test/')).some(c=>c.name.startsWith('__Host-'))).toBe(false);
    await report('guest_link');
    await page.screenshot({path:path.join(directory,`${info.project.name}-guest.png`),fullPage:true});
  } finally {
    fs.writeFileSync(path.join(directory,`${info.project.name}-report.json`),JSON.stringify(reports,null,2));
    await info.attach('device-report',{body:Buffer.from(JSON.stringify(reports,null,2)),contentType:'application/json'});
  }
});
