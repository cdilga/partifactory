import {expect, type CDPSession, type Locator, type Page} from '@playwright/test';

export async function countAssertions(cdp:CDPSession,page:Page) {
  const hardware={assertions:0,biometrics:0,verified:0};
  cdp.on('WebAuthn.credentialAsserted',()=>hardware.assertions++);
  // CDP may report several CTAP assertions for one credentials.get ceremony. Observe
  // the genuine browser call plus its real server verification, and retain raw CTAP counts.
  await page.exposeBinding('__pfObservedBiometric',()=>{hardware.biometrics++;});
  const observe=()=>{
    const credentials=navigator.credentials,get=credentials.get;
    credentials.get=async function(options){
      const result=await get.call(this,options);
      if(result?.type==='public-key')await (window as any).__pfObservedBiometric();
      return result;
    };
  };
  await page.addInitScript(observe);
  await page.evaluate(observe);
  page.on('response',response=>{
    if(response.status()===200&&response.url().endsWith('/stepup')&&response.request().method()==='POST'&&response.request().postDataJSON()?.credential)hardware.verified++;
  });
  return hardware;
}

// Record actual user actions and actual virtual-hardware assertions separately.
export class ExactBudget {
  readonly actions:string[]=[];
  readonly start:number;
  readonly assertedStart:number;
  readonly verifiedStart:number;
  constructor(readonly page:Page,readonly hardware:{assertions:number,biometrics:number,verified:number},readonly label:string,readonly interactions:number,readonly biometrics:number) {this.start=hardware.biometrics;this.assertedStart=hardware.assertions;this.verifiedStart=hardware.verified;}
  async click(target:Locator,label:string) {this.actions.push(label);await target.click();}
  async select(target:Locator,value:string,label:string) {this.actions.push(label);await target.selectOption(value);}
  async openLink(url:string) {this.actions.push('Open agent-posted link');await this.page.goto(url);}
  async check() {
    const evidence=()=>JSON.stringify({label:this.label,actions:this.actions,biometrics:this.hardware.biometrics-this.start});
    expect(this.actions.length,evidence()).toBe(this.interactions);
    await expect.poll(()=>this.hardware.biometrics-this.start,{message:evidence()}).toBe(this.biometrics);
    await expect.poll(()=>this.hardware.verified-this.verifiedStart,{message:'Successful server verification for '+this.label}).toBe(this.biometrics);
    expect(this.hardware.assertions-this.assertedStart,'Real virtual-hardware assertions').toBeGreaterThanOrEqual(this.biometrics);
    return {hardwareAssertions:this.hardware.assertions-this.assertedStart,serverVerified:this.hardware.verified-this.verifiedStart,label:this.label,actions:this.actions,interactions:this.actions.length,biometrics:this.hardware.biometrics-this.start};
  }
}
