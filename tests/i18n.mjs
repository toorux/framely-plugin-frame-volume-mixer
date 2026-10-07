import {build} from 'esbuild';
import {createRequire} from 'node:module';
import assert from 'node:assert/strict';
import {mkdtemp,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import path from 'node:path';
const root=path.resolve(import.meta.dirname,'..');
const folder=await mkdtemp(path.join(tmpdir(),'mixer-i18n-'));
try {
 const file=path.join(folder,'i18n.cjs');
 await build({entryPoints:[path.join(root,'ui/i18n.tsx')],outfile:file,bundle:true,platform:'node',format:'cjs'});
 const {resolveLocale,translate,translateError,messages}=createRequire(import.meta.url)(file);
 for(const language of ['zh','zh-CN','zh-TW','ZH-hans','zh_HK'])assert.equal(resolveLocale(language),'zh');
 for(const language of ['en-US','fr-FR','ja-JP','auto','',undefined])assert.equal(resolveLocale(language),'en');
 assert.equal(translate('en','{count} 个应用',{count:3}),'3 apps');
 assert.equal(translate('zh','{count} 个应用',{count:3}),'3 个应用');
 assert.equal(translate('en','静音 {name}',{name:'哔哩哔哩HD'}),'Mute 哔哩哔哩HD');
 assert.equal(translateError('en','设置失败；已更新 2 个流，请刷新确认: wpctl 超时（3 秒）'),'Update failed after changing 2 streams. Refresh to check: wpctl timed out (3 seconds)');
 assert.equal(translateError('zh','音量值无效'),'音量值无效');
 const source=await readFile(path.join(root,'ui/page.tsx'),'utf8');
 for(const match of source.matchAll(/\bt\('([^']+)'/g))assert.ok(messages[match[1]],`Missing English message: ${match[1]}`);
 console.log('Locale rules, interpolation, error translation and UI message coverage passed');
} finally {await rm(folder,{recursive:true,force:true})}
