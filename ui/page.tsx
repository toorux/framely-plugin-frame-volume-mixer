import React, {useEffect, useRef, useState} from 'react';
import {framely, registerPlugin} from '@framely/sdk';
import {SpeakerLoudIcon, SpeakerOffIcon, ExternalLinkIcon, ReloadIcon, ChevronDownIcon} from '@radix-ui/react-icons';
import {css} from './styles';
import {LanguageProvider,useLocale} from './i18n';
const style=document.createElement('style');style.textContent=css;document.head.append(style);
type Target={id:number;serial:number};
type Stream=Target & {app:string;binary:string;pid:number|null;host?:string;iconKey?:string|null;name:string;state:string;system:boolean;virtualDevice?:boolean;volume:number|null;muted:boolean|null;error:string|null};
type Status={connected:boolean;cookie:number|null;master:Stream|null;outputs?:Stream[];streams:Stream[];error:string|null};
type Change={targets:Target[];volume?:number;muted?:boolean};
const icons=new Map<string,Promise<string|null>>();
function AppIcon({stream}:{stream:Stream}) {
  const key=stream.iconKey||stream.binary;
  const [src,setSrc]=useState<string|null>(null);
  useEffect(()=>{let active=true;setSrc(null);if(key){let request=icons.get(key);if(!request){request=framely.call<{src:string|null}>('icon.get',{key}).then(v=>v.src?.startsWith('data:image/png;base64,')?v.src:null).catch(()=>null);icons.set(key,request)}void request.then(v=>{if(active)setSrc(v)})}return()=>{active=false}},[key]);
  return <span className="app-icon" aria-hidden="true">{src?<img src={src} alt="" onError={()=>setSrc(null)}/>:<span className="empty-icon"/>}</span>;
}
function Volume({name,streams,disabled,apply}:{name:string;streams:Stream[];disabled:boolean;apply:(v:Change)=>Promise<void>}) {
  const {t}=useLocale();
  const values=streams.map(s=>s.volume).filter((v):v is number=>v!==null);
  const value=values.length?Math.max(...values):0;
  const mixed=new Set(values.map(Math.round)).size>1;
  const muted=streams.every(s=>s.muted===true);
  const [draft,setDraft]=useState<number|null>(null),[pending,setPending]=useState(false);
  const editing=useRef(false),draftRef=useRef<number|null>(null);
  useEffect(()=>{if(!editing.current&&!pending&&draft!==null&&streams.every(s=>s.volume!==null&&Math.abs(s.volume-draft)<0.5)){setDraft(null);draftRef.current=null}},[streams,draft,pending]);
  useEffect(()=>{if(pending||draft===null)return;const timer=setTimeout(()=>{if(!editing.current){setDraft(null);draftRef.current=null}},2000);return()=>clearTimeout(timer)},[draft,pending]);
  const blocked=disabled||pending||streams.some(s=>s.volume===null||s.muted===null);
  const targets=streams.map(({id,serial})=>({id,serial}));
  async function commit(v:Change){editing.current=false;setPending(true);try{await apply(v)}catch{setDraft(null);draftRef.current=null}finally{setPending(false)}}
  function finish(){if(draftRef.current!==null&&editing.current)void commit({targets,volume:draftRef.current})}
  return <div className="volume-control">
    <div className="capsule-control"><input aria-label={name===t('系统总音量')?name:t('{name} 音量',{name})} type="range" min="0" max="100" step="1" value={draft??Math.min(value,100)} disabled={blocked} style={{'--volume':`${draft??Math.min(value,100)}%`} as React.CSSProperties} onChange={e=>{editing.current=true;draftRef.current=+e.target.value;setDraft(+e.target.value)}} onPointerUp={finish} onPointerCancel={()=>{editing.current=false;draftRef.current=null;setDraft(null)}} onKeyUp={e=>{if(['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End','PageUp','PageDown'].includes(e.key))finish()}} onBlur={finish}/>
    <output title={mixed?t('各播放流音量不同，调节后统一设置'):undefined}>{streams.some(s=>s.volume===null)?'-':`${Math.round(draft??value)}%`}{mixed&&draft===null&&<small>*</small>}</output></div>
    <button className={'mute '+(muted?'is-muted':'')} disabled={blocked} aria-label={t(muted?'取消静音 {name}':'静音 {name}',{name})} aria-pressed={muted} onClick={()=>void commit({targets,muted:!muted})}>{muted?<SpeakerOffIcon/>:<SpeakerLoudIcon/>}</button>
  </div>
}
function Mixer({quick=false}:{quick?:boolean}) {
  const {t,errorText}=useLocale();
  const [s,setS]=useState<Status|null>(null),[error,setError]=useState(''),[busy,setBusy]=useState(false),[received,setReceived]=useState(0),[now,setNow]=useState(Date.now());
  const mounted=useRef(true),writing=useRef(false),generation=useRef(0);
  function accept(v:Status){setS(v);setReceived(Date.now())}
  async function refresh(){const requestGeneration=generation.current;try{const v=await framely.call<Status>('status.get');if(mounted.current&&!writing.current&&requestGeneration===generation.current){accept(v);setError('')}}catch(e){if(mounted.current&&requestGeneration===generation.current)setError(String(e))}}
  useEffect(()=>{mounted.current=true;void refresh();const off=framely.onEvent((e:any)=>{if(mounted.current&&!writing.current&&e.type==='status')accept(e.data)});const timer=setInterval(()=>setNow(Date.now()),1000);return()=>{mounted.current=false;off();clearInterval(timer)}},[]);
  async function apply(method:string,change:Change){if(writing.current||!s?.connected)throw new Error(t('正在同步音量'));writing.current=true;generation.current++;setBusy(true);setError('');try{const v=await framely.call<Status>(method,{cookie:s.cookie,...change});if(mounted.current)accept(v)}catch(e){if(mounted.current){setError(String(e).replace(/^Error: /,''));try{const current=await framely.call<Status>('status.get');if(mounted.current)accept(current)}catch{}}throw e}finally{writing.current=false;if(mounted.current)setBusy(false)}}
  const stale=received>0&&now-received>6000,disabled=busy||stale||!s?.connected;
  const grouped=new Map<string,Stream[]>();
  for(const x of s?.streams??[]){if(x.system)continue;const key=x.pid?`host:${x.host??''}:pid:${x.pid}`:`stream:${x.serial}`;grouped.set(key,[...(grouped.get(key)??[]),x])}
  return <main className={quick?'mixer quick':'mixer'}>
    <section className="master-panel" aria-labelledby="master-title">
      <div className="section-heading"><h1 id="master-title">{t('系统总音量')}</h1><div className="actions"><button aria-label={t('刷新音量')} disabled={busy} onClick={()=>void refresh()}><ReloadIcon/></button>{quick&&<button aria-label={t('打开音量合成器窗口')} onClick={()=>void framely.windows.open('main').catch(e=>setError(String(e)))}><ExternalLinkIcon/></button>}</div></div>
      {s?.master?<><Volume key={`${s.cookie}:${s.master.serial}`} name={t('系统总音量')} streams={[s.master]} disabled={disabled} apply={v=>apply('master.set',v)}/>{s.master.error&&<p className="error" role="alert">{errorText(s.master.error)}</p>}</>:!s?<div className="skeleton" aria-label={t('正在读取系统音量')} role="status"/>:<p className="unavailable">{t('暂无默认播放设备')}</p>}
    </section>
    {(error||s?.error||stale)&&<div className="error" role="alert">{errorText(error||s?.error||(stale?t('音频连接已中断，请刷新。'):''))}</div>}
    <section className="applications" aria-labelledby="apps-title"><div className="section-heading apps-heading"><h2 id="apps-title">{t('应用音量')}</h2>{s&&<small>{t('{count} 个应用',{count:grouped.size})}</small>}</div>
      <div className="application-list">
        {[...grouped.entries()].map(([key,xs])=><div className="application" key={`${s?.cookie}:${key}`}>
          <div className="app-row"><div className="app-identity"><AppIcon stream={xs[0]}/><div className="app-copy"><h3 title={xs[0].app}>{xs[0].app||t('未命名应用')}</h3><small>{xs.every(x=>x.muted===true)?t('已静音'):xs.some(x=>x.state==='running')?t('播放中'):t('已暂停')}</small></div></div><Volume name={xs[0].app} streams={xs} disabled={disabled} apply={v=>apply('streams.set',v)}/></div>
          {xs.some(x=>x.error)&&<p className="error" role="alert">{t('无法读取应用音量，请刷新。')}</p>}
          {xs.length>1&&<details><summary aria-label={t('分别调节播放流')} title={t('分别调节播放流')}><ChevronDownIcon/></summary>{xs.map(x=><div className="app-row substream" key={x.serial}><div className="app-copy"><h3 title={x.name}>{x.name}</h3></div><Volume name={x.name} streams={[x]} disabled={disabled} apply={v=>apply('streams.set',v)}/></div>)}</details>}
        </div>)}
        {!s&&[0,1].map(i=><div key={i} className="skeleton app-skeleton" aria-hidden="true"/>)}
        {s?.connected&&!grouped.size&&<div className="empty">{t('暂无应用播放声音')}<small>{t('播放游戏、视频或音乐后，会自动显示在这里。')}</small></div>}
        {s&&!s.connected&&<div className="empty">{t('等待音频服务连接')}</div>}
      </div>
    </section>

  </main>
}
registerPlugin({QuickPage:()=> <LanguageProvider><Mixer quick/></LanguageProvider>,WindowPage:()=> <LanguageProvider><Mixer/></LanguageProvider>});
