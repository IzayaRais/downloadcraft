const {it}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const R=require('./rank.js');
const fragment='https://cdn.example.net/movie/seg-002.m4s?token=signed';
const root='https://cdn.example.net/movie/master.m3u8?token=signed';
const context={tabId:7,frameId:4,pageUrl:'https://example.org/watch/123'};
it('recognizes generic fragments without using size or duration',()=>{
 for(const url of [fragment,'https://cdn.example.net/seg-001.ts','https://cdn.example.net/s0.ts','https://cdn.example.net/v.mp4?range=100-200'])assert.equal(R.kindOf({url}),'segment');
 for(const url of ['https://cdn.example.net/short.mp4','https://cdn.example.net/movie.ts','https://cdn.example.net/dashcam.mp4','https://cdn.example.net/v.mp4?range=notes','https://cdn.example.net/file.zip?range=100-200'])assert.equal(R.kindOf({url,size:512}),'file');
});
it('recognizes opaque manifests by MIME',()=>{
 assert.equal(R.kindOf({url:'https://cdn.example.net/stream?id=123',mime:'application/vnd.apple.mpegurl'}),'playlist');
 assert.equal(R.kindOf({url:'https://cdn.example.net/stream?id=123',mime:'application/dash+xml'}),'playlist');
});
it('extracts exact signed child URLs from real playlist references',()=>{
 const text='#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI="a.m3u8?token=one"\n#EXT-X-STREAM-INF:BANDWIDTH=123\nv.m3u8?token=two\n';
 assert.deepEqual(R.playlistReferences(text,root),['https://cdn.example.net/movie/a.m3u8?token=one','https://cdn.example.net/movie/v.m3u8?token=two']);
});
it('finds the master through observed parent/child links',()=>{
 const child={...context,url:'https://cdn.example.net/movie/v.m3u8',children:[fragment]};
 const master={...context,url:root,master:true,children:[child.url]};
 const ad={...context,url:'https://cdn.example.net/ad/master.m3u8',master:true,children:[]};
 assert.equal(R.completePlaylist({...context,url:fragment},[child,master,ad]),master);
 assert.equal(R.completePlaylist(child,[child,master,ad]),master);
});
it('never borrows a playlist from a different frame or chooses between unrelated roots',()=>{
 const capture={...context,url:fragment};
 assert.equal(R.completePlaylist(capture,[{...context,frameId:5,url:root,master:true}]),null);
 assert.equal(R.completePlaylist(capture,[{...context,url:root,master:true},{...context,url:root+'&other=1',master:true}]),null);
});
const source=fs.readFileSync(path.join(__dirname,'background.js'),'utf8');
const block=source.slice(source.indexOf('async function sendToHost('),source.indexOf('// ---- IDM-style network sniffer'));
function harness(items,frame={pageUrl:context.pageUrl,videoCount:1,durationSecs:2700,audioExpected:true}){
 const calls=[];
 const api={tabs:{sendMessage:async()=>frame},runtime:{sendNativeMessage:async(h,m)=>{calls.push(m);return {type:'ok'}}}};
 const make=new Function('DCRank','tabMedia','api','cookiesFor','stripFbSlice','HOST','console',`let lastHostError='';${block};return {sendToHost,error:()=>lastHostError}`);
 return {...make(R,new Map([[7,items]]),api,async()=>null,u=>u,'test',{error(){}}),calls};
}
it('hands a generic fragment to its complete playlist, preserving player expectations',async()=>{
 const h=harness([{...context,url:fragment},{...context,url:root,master:true}]);
 assert.equal(await h.sendToHost(fragment,'movie.mp4',{size:512}),true);
 assert.equal(h.calls[0].url,root);assert.equal(h.calls[0].size,null);
 assert.equal(h.calls[0].dur,2700);assert.equal(h.calls[0].audioExpected,true);
});
it('uses the exact single-player page as fragment recovery evidence',async()=>{
 const h=harness([{...context,url:fragment}]);
 assert.equal(await h.sendToHost(fragment,'movie.mp4',{}),true);
 assert.equal(h.calls[0].url,fragment);assert.equal(h.calls[0].referer,context.pageUrl);
});
it('refuses ambiguous fragment captures without starting a download',async()=>{
 const h=harness([{...context,url:fragment}],{pageUrl:context.pageUrl,videoCount:2});
 assert.equal(await h.sendToHost(fragment,'movie.mp4',{}),false);assert.equal(h.calls.length,0);
});
it('promotes a selected HLS child to its observed full master',async()=>{
 const child={...context,url:'https://cdn.example.net/movie/v.m3u8'};
 const h=harness([child,{...context,url:root,master:true,children:[child.url]}]);
 assert.equal(await h.sendToHost(child.url,'movie.mp4',{}),true);
 assert.equal(h.calls[0].url,root);
});

it('does not attach video duration/audio to subtitles or ordinary files',()=>{
 const videos=[{duration:2700,currentSrc:'blob:movie',mozHasAudio:true}];
 for(const url of ['https://example.org/subs.vtt','https://example.org/file.zip','https://example.org/song.mp3']){
  assert.deepEqual(R.playerEvidence(videos,url,'','https://example.org/watch',null),{dur:null,audioExpected:false});
 }
});
it('keeps genuine short clips and uses positive player audio evidence only',()=>{
 const video={duration:1.2,currentSrc:'https://cdn.example.org/short.mp4',webkitAudioDecodedByteCount:128};
 assert.deepEqual(R.playerEvidence([video],video.currentSrc,'video/mp4','https://example.org/watch',null),{dur:1,audioExpected:true});
 assert.deepEqual(R.playerEvidence([{duration:3,src:video.currentSrc}],video.currentSrc,'video/mp4','https://example.org/watch',null),{dur:3,audioExpected:false});
});
it('does not guess a duration across multiple unmatched players',()=>{
 assert.deepEqual(R.playerEvidence([{duration:20},{duration:2700}],'https://cdn.example.org/v.mp4','video/mp4','https://example.org/watch',null),{dur:null,audioExpected:false});
});

it('demotes observed MP4 segment children without declaring all MP4 files fragments',()=>{
 const part={...context,url:'https://cdn.example.net/piece.mp4',segment:true,mime:'video/mp4'};
 assert.equal(R.kindOf(part),'segment');
 assert.equal(R.scoreMedia(part,{hasPlaylist:true}),-1);
 assert.equal(R.kindOf({url:part.url,mime:'video/mp4'}),'file');
});

it('hides browser UI only after explicit opt-in with connected automatic capture',()=>{
 assert.equal(R.hideDownloadUI({autoCapture:true,hideBrowserUI:true},true),true);
 assert.equal(R.hideDownloadUI({autoCapture:true,hideBrowserUI:false},true),false);
 assert.equal(R.hideDownloadUI({autoCapture:false,hideBrowserUI:true},true),false);
 assert.equal(R.hideDownloadUI({autoCapture:true,hideBrowserUI:true},false),false);
});
it('applies and releases Chromium download UI suppression through the real controller',async()=>{
 const start=source.indexOf('async function syncBrowserDownloadUI()');
 const end=source.indexOf('function hostBlocked(',start);
 const make=new Function('api','settings','DCRank','hostOk',`${source.slice(start,end)}; return syncBrowserDownloadUI;`);
 for(const [cfg,connected,enabled] of [[{autoCapture:true,hideBrowserUI:true},true,false],[{autoCapture:false,hideBrowserUI:true},true,true],[{autoCapture:true,hideBrowserUI:true},false,true],[{autoCapture:true,hideBrowserUI:false},true,true]]){
  const calls=[];
  const sync=make({downloads:{setUiOptions:async v=>calls.push(v)}},async()=>cfg,R,connected);
  assert.equal((await sync()).supported,true);
  assert.deepEqual(calls,[{enabled}]);
 }
});
it('handles browsers without the Chromium download UI API',async()=>{
 const start=source.indexOf('async function syncBrowserDownloadUI()');
 const end=source.indexOf('function hostBlocked(',start);
 const make=new Function('api','settings','DCRank','hostOk',`${source.slice(start,end)}; return syncBrowserDownloadUI;`);
 assert.deepEqual(await make({downloads:{}},async()=>({}),R,true)(),{supported:false});
});
