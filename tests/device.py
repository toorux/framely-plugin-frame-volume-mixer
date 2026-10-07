"""Run on the headset with an ARM64 backend path. Only touches our silent streams."""
import json, os, pathlib, select, subprocess, sys, tempfile, time, wave
backend=sys.argv[1]
children=[]
def run(*args): return subprocess.check_output(args,text=True,timeout=5)
def dump(): return json.loads(run('pw-dump'))
def nodes():
    return [o for o in dump() if o['type']=='PipeWire:Interface:Node' and o.get('info',{}).get('props',{}).get('media.class')=='Stream/Output/Audio' and o['info']['props'].get('application.name') in ('Framely Mixer Native Test','Framely Mixer Pulse Test')]
with tempfile.TemporaryDirectory(prefix='framely-mixer-test-') as folder:
    wav=str(pathlib.Path(folder)/'silence.wav')
    with wave.open(wav,'wb') as f:
        f.setnchannels(2);f.setsampwidth(2);f.setframerate(48000);f.writeframes(b'\0'*(48000*4*90))
    try:
        children.append(subprocess.Popen(['pw-play','--properties=application.name="Framely Mixer Native Test"',wav],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,env={**os.environ,'SteamAppId':'1'}))
        children.append(subprocess.Popen(['paplay','--client-name=Framely Mixer Pulse Test',wav],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE))
        deadline=time.monotonic()+8
        while time.monotonic()<deadline:
            found=nodes()
            if len(found)==2:break
            time.sleep(.2)
        assert len(found)==2,found
        worker=subprocess.Popen([backend],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1,env={**os.environ,'XDG_STATE_HOME':str(pathlib.Path(folder)/'state')})
        children.append(worker)
        def rpc(method,params={}):
            worker.stdin.write(json.dumps({'id':1,'method':method,'params':params})+'\n');worker.stdin.flush()
            while True:
                ready,_,_=select.select([worker.stdout],[],[],20)
                assert ready,'RPC timed out'
                line=worker.stdout.readline();assert line,'backend exited'
                obj=json.loads(line)
                if obj.get('id')==1:return obj
        state=rpc('status.get')['result']
        assert state['connected'],state
        master=state['master'];assert master and master['volume'] is not None,state
        native_master=run('wpctl','get-volume','@DEFAULT_AUDIO_SINK@').split()
        assert abs(master['volume']-float(native_master[1])*100)<.1,(master,native_master)
        master_params={'cookie':state['cookie'],'targets':[{'id':master['id'],'serial':master['serial']}],'volume':master['volume']}
        # Write back the existing volume, preserving the user's current output level.
        same=rpc('master.set',master_params);assert 'error' not in same,same
        assert same['result']['master']['volume']==master['volume'],same
        assert any(s['serial']==master['serial'] for s in state['outputs']),state
        same=rpc('outputs.set',master_params);assert 'error' not in same,same
        same=rpc('outputs.default',{'cookie':state['cookie'],'target':master_params['targets'][0]});assert 'error' not in same,same
        assert same['result']['master']['serial']==master['serial'],same
        master_params['targets'][0]['serial']+=100000
        assert 'error' in rpc('master.set',master_params),'stale output accepted'
        assert 'error' in rpc('outputs.set',master_params),'stale output control accepted'
        assert 'error' in rpc('outputs.default',{'cookie':state['cookie'],'target':master_params['targets'][0]}),'stale default output accepted'
        a=next(s for s in state['streams'] if s['app']=='Framely Mixer Native Test')
        b=next(s for s in state['streams'] if s['app']=='Framely Mixer Pulse Test')
        assert not a['system'] and not b['system'],(a,b)
        assert a['pid']==children[0].pid and b['pid']==children[1].pid,(a,b)
        def params(s,**kw):return {'cookie':state['cookie'],'targets':[{'id':s['id'],'serial':s['serial']}],**kw}
        result=rpc('streams.set',params(a,volume=35));assert 'error' not in result,result
        updated=result['result']['streams']
        aa=next(s for s in updated if s['serial']==a['serial']);bb=next(s for s in updated if s['serial']==b['serial'])
        assert abs(aa['volume']-35)<1,aa
        assert bb['volume']==b['volume'],(b,bb)
        result=rpc('streams.set',params(b,muted=True));assert 'error' not in result,result
        bb=next(s for s in result['result']['streams'] if s['serial']==b['serial']);assert bb['muted'],bb
        aa=next(s for s in result['result']['streams'] if s['serial']==a['serial']);assert not aa['muted'],aa
        result=rpc('streams.set',params(b,muted=False));assert 'error' not in result,result
        # Only our silent test app gets the automatic policy; real apps are untouched.
        result=rpc('focus.set',{'cookie':state['cookie'],'target':{'id':a['id'],'serial':a['serial']},'enabled':True});assert 'error' not in result,result
        deadline=time.monotonic()+8
        while time.monotonic()<deadline:
            result=rpc('status.get');aa=next(s for s in result['result']['streams'] if s['serial']==a['serial'])
            if aa['autoMuted']:break
            time.sleep(.15)
        assert aa['autoMuted'] and aa['muted'] and aa['userMuted'] is False,aa
        assert abs(aa['volume']-35)<1,aa
        result=rpc('focus.set',{'cookie':state['cookie'],'target':{'id':a['id'],'serial':a['serial']},'enabled':False});assert 'error' not in result,result
        aa=next(s for s in result['result']['streams'] if s['serial']==a['serial']);assert not aa['muted'],aa
        old=params(a,volume=10);old['targets'][0]['serial']+=100000
        assert 'error' in rpc('streams.set',old),'stale stream accepted'
        children[0].terminate();children[0].wait(timeout=3)
        time.sleep(.2)
        assert 'error' in rpc('streams.set',params(a,volume=10)),'exited stream accepted'
        system=next(s for s in state['streams'] if s['system'])
        assert 'error' in rpc('streams.set',params(system,volume=10)),'system chain accepted'
        # Verify lifecycle works and exits on stdin close without an audio worker left behind.
        assert rpc('framely.lifecycle.start')['result']['ready']
        started=time.monotonic()
        run('wpctl','set-volume',str(b['id']),'42%')
        observed=False
        while time.monotonic()-started<2:
            ready,_,_=select.select([worker.stdout],[],[],2)
            assert ready,'no status event'
            event=json.loads(worker.stdout.readline())
            if event.get('event')=='status':
                actual=next(s for s in event['data']['streams'] if s['serial']==b['serial'])
                if abs(actual['volume']-42)<.5:
                    observed=True;break
        assert observed,'external volume change was not synchronized'
        print('external volume event latency:',round((time.monotonic()-started)*1000),'ms')
        assert rpc('framely.lifecycle.stop')['result']['stopped']
        worker.stdin.close();worker.wait(timeout=5);assert worker.returncode==0
        print(json.dumps({'passed':True,'native':{'pid':a['pid'],'volume':aa['volume']},'pulse':{'pid':b['pid'],'isolated':True},'checks':['background mute+original mute restoration','native+Pulse enumeration','PID attribution','35% readback','independent volume','mute/unmute','stale serial rejection','exited stream rejection','system chain rejection','lifecycle+EOF','default output read+same-value write','stale output rejection']},ensure_ascii=False))
    finally:
        for child in children:
            if child.poll() is None:
                child.terminate()
                try:child.wait(timeout=3)
                except subprocess.TimeoutExpired:child.kill();child.wait()
