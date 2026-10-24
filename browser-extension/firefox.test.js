const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const R=require('./rank.js');
const source=fs.readFileSync(__dirname+'/background.js','utf8');
const response={url:'https://h/export?id=1',method:'GET',type:'main_frame',statusCode:200,originUrl:'https://h/page',responseHeaders:[
 {name:'Content-Disposition',value:"attachment; filename=report.zip; filename*=UTF-8''report%20full.zip"},
 {name:'Content-Length',value:'2097152'},{name:'Content-Type',value:'application/zip'}]};
function setup({accept=true,enabled=true,blocked=false,seen=false,firefox=true}={}){
 const calls=[],listeners=[];
 const context={DCRank:R,settings:async()=>({autoCapture:enabled,minSizeKB:100}),freshSeen:()=>seen,hostBlocked:()=>blocked,
 sendToHost:async(...a)=>{calls.push(a);return accept;},markSeen:(...a)=>calls.push(['seen',...a]),hostOk:false,setBadge(){}};
 context.browser={runtime:firefox?{getBrowserInfo:()=>Promise.resolve({name:'Firefox'})}:{},
   webRequest:{onHeadersReceived:{addListener:(...a)=>listeners.push(a)}}};
 vm.createContext(context);
 vm.runInContext(source.slice(source.indexOf('async function captureAttachmentResponse('),source.indexOf('// ---- popup / content-script messages ----')),context);
 return {context,calls,listeners,run:details=>context.captureAttachmentResponse(details)};
}
test('Firefox registers a blocking response listener; Chromium browser namespace does not',()=>{
 assert.equal(setup().listeners.length,1);
 assert.deepEqual(Array.from(setup().listeners[0][2]),['blocking','responseHeaders']);
 // Chrome 148+ exposes `browser` too, but it must never register the Firefox-only listener.
 assert.equal(setup({firefox:false}).listeners.length,0);
});
test('attachment parsing handles opaque URLs, UTF-8 names and unsafe server paths',()=>{
 assert.equal(R.attachmentDownload(response).filename,'report full.zip');
 assert.equal(R.attachmentDownload({...response,responseHeaders:[{name:'content-disposition',value:'attachment; filename="../../report.zip"'}]}).filename,'report.zip');
 assert.equal(R.attachmentDownload({...response,responseHeaders:[{name:'content-disposition',value:"attachment; filename=ok.zip; filename*=UTF-8''bad%XX"}]}).filename,'ok.zip');
});
test('does not intercept playback, partials, internal requests, POST or error pages',()=>{
 for(const change of [{type:'media'},{type:'xmlhttprequest'},{method:'POST'},{statusCode:206},{statusCode:404},
 {responseHeaders:[{name:'content-disposition',value:'inline; filename=report.zip'}]}])assert.equal(R.attachmentDownload({...response,...change}),null);
});
test('Firefox cancels the response only after native host acceptance',async()=>{
 const h=setup();const result=await h.run(response);assert.equal(result.cancel,true);
 assert.equal(h.calls[0][0],response.url);assert.equal(h.calls[0][1],'report full.zip');
 assert.equal(h.calls[0][2].referer,response.originUrl);assert.equal(h.calls[0][2].size,2097152);
});
test('Firefox keeps original authenticated request on native host failure',async()=>{
 const h=setup({accept:false});assert.equal((await h.run(response)).cancel,undefined);
 assert.equal(h.calls.length,2);assert.equal(h.calls[1][0],'seen');
});
test('Firefox respects disabled capture, site filter, size filter and replay guard',async()=>{
 for(const options of [{enabled:false},{blocked:true},{seen:true}]){
  const h=setup(options);assert.equal((await h.run(response)).cancel,undefined);assert.equal(h.calls.length,0);
 }
 const h=setup();assert.equal((await h.run({...response,responseHeaders:[response.responseHeaders[0],{name:'content-length',value:'20'}]})).cancel,undefined);assert.equal(h.calls.length,0);
});
test('every browser package uses the same overlay and takeover code on every OS',()=>{
 for(const platform of ['windows','linux','macos'])for(const browser of ['firefox','chrome']){
  const manifest=JSON.parse(fs.readFileSync(__dirname+(browser==='firefox'?'/manifest-firefox.json':'/manifest.json')));
  assert.deepEqual(manifest.content_scripts[0].js,['rank.js','content.js'],platform+'/'+browser);
  assert.equal(manifest.content_scripts[0].all_frames,true);
  assert.ok(manifest.permissions.includes('nativeMessaging'));
  assert.ok(manifest.permissions.includes('downloads'));
  if(browser==='chrome') assert.ok(!manifest.permissions.includes('webRequestBlocking'));
 }
});
test('download bar follows fullscreen and X dismisses the associated menu',()=>{
 const content=fs.readFileSync(__dirname+'/content.js','utf8');
 const element=()=>({isConnected:true,style:{},remove(){this.isConnected=false;},getBoundingClientRect(){return {width:640,height:360,right:700,left:60,top:20,bottom:380};},appendChild(e){e.parentNode=this;}});
 const root=element(),fullscreen=element(),button=element(),video=element(),panel=element();
 button.__dcVideo=video;button.__dcClose=element();fullscreen.contains=x=>x===video;
 const context={activeBarDrag:null,applySavedBarPosition:()=>false,barWidth:()=>200,Set,DCRank:R,location:{href:'https://h/v'},button,panel,fullscreen,
 document:{documentElement:root,fullscreenElement:null,querySelectorAll:s=>s==='[data-dc-pill]'?[button]:[],addEventListener(){}},window:{innerWidth:1000,innerHeight:800,addEventListener(){}}};
 vm.createContext(context);vm.runInContext(content.slice(content.indexOf('  const dismissedVideos ='),content.indexOf('  function ensureButton(')),context);
 vm.runInContext("addDismissControl(button,'v',panel);syncDismissControls()",context);assert.equal(button.parentNode,root);
 vm.runInContext('document.fullscreenElement=fullscreen;syncDismissControls()',context);assert.equal(button.parentNode,fullscreen);
 vm.runInContext('document.fullscreenElement=null;syncDismissControls()',context);assert.equal(button.parentNode,root);
 button.__dcClose.onclick({preventDefault(){},stopPropagation(){}});assert.equal(button.isConnected,false);assert.equal(panel.isConnected,false);
});
