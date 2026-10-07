import React,{createContext,useContext,useEffect,useState} from 'react';
import {framely} from '@framely/sdk';
export const messages:Record<string,string>={
'音量合成器':'Volume mixer','系统总音量':'Master volume','应用音量':'App volume','输出设备':'Output devices',
'刷新音量':'Refresh volume','打开音量合成器窗口':'Open volume mixer','正在读取系统音量':'Reading master volume',
'暂无默认播放设备':'No default output device','音频连接已中断，请刷新。':'Audio connection lost. Refresh to reconnect.',
'{count} 个应用':'{count} apps','{count} 个设备':'{count} devices','未命名应用':'Unnamed app',
'已静音':'Muted','播放中':'Playing','已暂停':'Paused','分别调节播放流':'Adjust individual streams',
'无法读取应用音量，请刷新。':'Unable to read app volume. Please refresh.',
'暂无应用播放声音':'No apps are playing audio','播放游戏、视频或音乐后，会自动显示在这里。':'Apps appear here when they play audio.',
'等待音频服务连接':'Waiting for the audio service','未命名输出设备':'Unnamed output device',
'默认输出':'Default output','虚拟输出':'Virtual output','暂无输出设备':'No output devices',
'设为默认输出：{name}':'Set default output: {name}','{name} 音量':'{name} volume',
'取消静音 {name}':'Unmute {name}','静音 {name}':'Mute {name}',
'各播放流音量不同，调节后统一设置':'Streams have different volumes. Adjusting sets them to the same level.',
'正在同步音量':'Synchronizing volume',
'请以 SteamOS 音频会话用户运行，无需 root':'Run as the SteamOS audio session user, without root',
'请求过大':'Request too large','未知方法':'Unknown method','pw-dump 未返回对象列表':'pw-dump did not return an object list',
'缺少 PipeWire 服务标识':'Missing PipeWire service identifier','读取输出失败':'Unable to read output',
'读取错误失败':'Unable to read diagnostic output','音频服务返回的数据过大':'Audio service response is too large',
'缺少用户目录':'Missing user home directory','无效图标名称':'Invalid icon name','无法读取音量':'Unable to read volume',
'缺少音量值':'Missing volume value','音量值无效':'Invalid volume value',
'音频服务已重启，请刷新后重试':'Audio service restarted. Refresh and try again',
'请选择 1–64 个播放流':'Select 1–64 playback streams','一次只能设置音量或静音':'Set either volume or mute at a time',
'音量必须在 0–100% 之间':'Volume must be between 0 and 100%',
'输出设备已退出或改变，请刷新后重试':'Output device disappeared or changed. Refresh and try again',
'默认播放设备已切换或不可用，请刷新后重试':'Default output changed or is unavailable. Refresh and try again',
'重复的播放流':'Duplicate playback stream',
'播放流已退出、改变或属于系统链路，请刷新后重试':'Playback stream disappeared, changed, or belongs to a system chain. Refresh and try again',
'插件页面错误':'Plugin page error',
};
export function resolveLocale(language:unknown):'zh'|'en'{return typeof language==='string'&&/^zh(?:[-_]|$)/i.test(language)?'zh':'en'}
export function translate(locale:'zh'|'en',key:string,args:Record<string,string|number>={}){let text=locale==='zh'?key:messages[key]??key;if(locale==='en'&&args.count===1){if(key==='{count} 个应用')text='{count} app';if(key==='{count} 个设备')text='{count} device';}for(const [name,value] of Object.entries(args))text=text.replaceAll(`{${name}}`,String(value));return text}
export function translateError(locale:'zh'|'en',error:string){if(locale==='zh')return error;let text=error;for(const key of Object.keys(messages).sort((a,b)=>b.length-a.length))text=text.replaceAll(key,messages[key]);return text.replace(/启动 (.*?)（请确认已安装 PipeWire\/WirePlumber）/g,'Unable to start $1 (check that PipeWire/WirePlumber is installed)').replace(/(\S+) 超时（3 秒）/g,'$1 timed out (3 seconds)').replace(/设置失败；已更新 (\d+) 个流，请刷新确认/g,'Update failed after changing $1 streams. Refresh to check');}
const Locale=createContext<'zh'|'en'>('en');
export function LanguageProvider({children}:{children:React.ReactNode}){
 const [locale,setLocale]=useState<'zh'|'en'|null>(null);
 useEffect(()=>{let active=true,revision=0;const accept=(language:unknown)=>{if(active)setLocale(resolveLocale(language))};const off=framely.onEvent((event:any)=>{if(event.type==='language.changed'){revision++;accept(event.data?.language)}});const initial=revision;void framely.language.get().then(value=>{if(revision===initial)accept(value.language)}).catch(()=>{if(revision===initial)accept('en')});return()=>{active=false;off()}},[]);
 useEffect(()=>{if(locale){document.documentElement.lang=locale==='zh'?'zh-CN':'en';document.title=translate(locale,'音量合成器')}},[locale]);
 return locale?<Locale.Provider value={locale}>{children}</Locale.Provider>:null;
}
export function useLocale(){const locale=useContext(Locale);return {t:(key:string,args?:Record<string,string|number>)=>translate(locale,key,args),errorText:(text:string)=>translateError(locale,text)}}
