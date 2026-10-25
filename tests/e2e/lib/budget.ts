import {expect, type CDPSession, type Locator, type Page} from '@playwright/test';

export function countAssertions(cdp:CDPSession) {
  const hardware={assertions:0};
  cdp.on('WebAuthn.credentialAsserted',()=>hardware.assertions++);
  return hardware;
}

// Record actual user actions and actual virtual-hardware assertions separately.
export class ExactBudget {
  readonly actions:string[]=[];
  readonly start:number;
  constructor(readonly page:Page,readonly hardware:{assertions:number},readonly label:string,readonly interactions:number,readonly biometrics:number) {this.start=hardware.assertions;}
  async click(target:Locator,label:string) {this.actions.push(label);await target.click();}
  async select(target:Locator,value:string,label:string) {this.actions.push(label);await target.selectOption(value);}
  async openLink(url:string) {this.actions.push('Open agent-posted link');await this.page.goto(url);}
  async check() {
    const evidence=()=>JSON.stringify({label:this.label,actions:this.actions,biometrics:this.hardware.assertions-this.start});
    expect(this.actions.length,evidence()).toBe(this.interactions);
    await expect.poll(()=>this.hardware.assertions-this.start,{message:evidence()}).toBe(this.biometrics);
    return {label:this.label,actions:this.actions,interactions:this.actions.length,biometrics:this.hardware.assertions-this.start};
  }
}
