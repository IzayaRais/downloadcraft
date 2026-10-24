const {test}=require('node:test');
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const source=fs.readFileSync(__dirname+'/background.js','utf8');
function worker(session){
 let listener;
 const chrome={runtime:{onMessage:{addListener:f=>listener=f}}};
 const api={storage:{session:{get:async key=>({[key]:session[key]}),set:async patch=>Object.assign(session,patch)}}};
 const context={chrome,api,Number};vm.createContext(context);
 vm.runInContext(source.slice(source.indexOf('// ---- popup / content-script messages ----')),context);
 return msg=>new Promise(resolve=>listener(msg,{},resolve));
}
test('video-bar position survives worker restart, but a new browser session starts at default',async()=>{
 const session={};let send=worker(session);
 assert.equal((await send({type:'getVideoBarPosition'})).position,null);
 assert.equal((await send({type:'setVideoBarPosition',position:{x:0.25,y:0.75}})).ok,true);
 send=worker(session);
 assert.deepEqual(JSON.parse(JSON.stringify((await send({type:'getVideoBarPosition'})).position)),{x:0.25,y:0.75});
 assert.equal((await worker({})({type:'getVideoBarPosition'})).position,null);
});
test('invalid video-bar coordinates never overwrite session position',async()=>{
 const session={videoBarPosition:{x:.5,y:.5}},send=worker(session);
 for(const p of [null,{}, {x:-1,y:0}, {x:1,y:Infinity},{x:NaN,y:0},{x:2,y:.2},{x:'0',y:.2}]){
  assert.equal((await send({type:'setVideoBarPosition',position:p})).ok,false);
 }
 assert.deepEqual(session.videoBarPosition,{x:.5,y:.5});
});
test('a saved bar spot applies in a normal window but never in fullscreen',()=>{
 const content=fs.readFileSync(__dirname+'/content.js','utf8');
 const fn=content.slice(content.indexOf('function applySavedBarPosition'),content.indexOf('function positionDraggedPanel'));
 const root={appendChild(){}};
 const document={fullscreenElement:null,documentElement:root};
 const context={document,window:{innerWidth:1000,innerHeight:600},barWidth:()=>200,Math,barPosition:{x:.5,y:.1}};
 vm.createContext(context);vm.runInContext(fn+';this.apply=applySavedBarPosition;',context);
 const host={parentNode:root,style:{}};
 assert.equal(context.apply(host),true);
 assert.equal(host.style.left,'400px');
 document.fullscreenElement={appendChild(){}};
 const fsHost={parentNode:root,style:{}};
 assert.equal(context.apply(fsHost),false);
 assert.equal(fsHost.style.left,undefined);
});
