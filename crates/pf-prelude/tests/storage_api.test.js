const {test} = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');

function harness() {
  const calls = [], sources = [], hooks = {};
  let response = {ok: true, status: 200, value: {ok: null}};
  class Source {
    constructor(url) {this.url=url; this.listeners={};sources.push(this);}
    addEventListener(name,fn) {this.listeners[name]=fn;}
    close() {this.readyState=2;}
  }
  const document = {getElementById:()=>({textContent:'{"scope":"person","personal":{}}'}),addEventListener(){},dispatchEvent(){}};
  const window = {fetch:async(url,opts)=>{calls.push({url,opts,body:JSON.parse(opts.body)});return {ok:response.ok,status:response.status,headers:{get:()=>null},text:async()=>JSON.stringify(response.value)};},addEventListener:(name,fn)=>{hooks[name]=fn;},EventSource:Source};
  window.parent=window;
  const context={window,document,location:{protocol:'https:',host:'board-part.pf.test',pathname:'/c/cap/',href:'https://board-part.pf.test/c/cap/'},URL,Map,Proxy,JSON,Math,Error,setTimeout:()=>1,clearTimeout(){},Event:class{}};
  vm.runInNewContext(fs.readFileSync(require('node:path').join(__dirname,'../js/prelude.js'),'utf8'),context);
  return {window,calls,sources,hooks,setResponse:r=>{response=r;}};
}

test('storage signatures preserve order, shared options, paging and null',async()=>{
  const h=harness();
  assert.equal(await h.window.storage.get('absent',true),null);
  assert.deepEqual(h.calls.pop().body,{op:'storage.get',args:{key:'absent',shared:true}});
  await h.window.storage.getMany(['b','missing','a'],true);
  assert.deepEqual(h.calls.pop().body,{op:'storage.getMany',args:{keys:['b','missing','a'],shared:true}});
  await h.window.storage.list('px:',{shared:true,cursor:'px:2',limit:256,values:true});
  assert.deepEqual(h.calls.pop().body,{op:'storage.list',args:{prefix:'px:',shared:true,cursor:'px:2',limit:256,values:true,meta:false}});
  await h.window.storage.set('a',12,true);
  const set=h.calls.pop().body;assert.equal(set.args.value,'12');assert.equal(set.args.shared,true);assert.equal(typeof set.args.tab,'string');
  await h.window.storage.delete('a',true);assert.equal(h.calls.pop().body.args.tab,set.args.tab);
});

test('PfError carries contract fields and localStorage sync survives navigation',async()=>{
  const h=harness();
  h.setResponse({ok:false,status:429,value:{error:{code:'rate_limited',message:'wait',details:{retry_after:3}}}});
  await assert.rejects(h.window.storage.get('a'),e=>e.name==='PfError'&&e.code==='rate_limited'&&e.retryable&&e.retry_after===3);
  h.window.localStorage.setItem('keep','value');h.hooks.pagehide();
  const call=h.calls.pop();assert.equal(call.opts.keepalive,true);assert.equal(call.opts.mode,'cors');assert.equal(call.opts.headers['Content-Type'],'text/plain');assert.equal(call.body.op,'storage.sync');
});

test('one EventSource suppresses writer echo and dispatches deletion and resync',async()=>{
  const h=harness();let changes=[],resync=0;
  h.window.partifactory.on('data.changed',e=>changes.push(e));h.window.partifactory.on('pf:resync',()=>resync++);
  assert.equal(h.sources.length,1);
  await h.window.storage.set('a','value',true);const tab=h.calls.pop().body.args.tab;
  const source=h.sources[0];source.listeners['data.changed']({data:JSON.stringify({key:'a',op:'set',tab})});assert.equal(changes.length,0);
  source.listeners['data.changed']({data:JSON.stringify({key:'a',op:'delete',tab:'someone-else',by:{kind:'person',name:'A family member'}})});
  assert.equal(changes.length,1);assert.equal(changes[0].op,'delete');assert.equal(changes[0].tab,undefined);
  source.listeners['pf:resync']();assert.equal(resync,1);
});
