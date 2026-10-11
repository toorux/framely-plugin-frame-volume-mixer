"""Exercise the real RPC controller against isolated audio/focus tools, never device audio."""
import json
import os
import pathlib
import queue
import threading
import subprocess
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--no-deps', '--format-version=1', '--manifest-path', str(ROOT / 'backend/Cargo.toml')]))
backend = pathlib.Path(os.environ.get('MIXER_TEST_BACKEND', str(pathlib.Path(metadata['target_directory']) / 'debug/framely-volume-mixer')))

TOOLS = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
path=pathlib.Path(os.environ['FOCUS_FIXTURE'])
s=json.loads(path.read_text()); program=pathlib.Path(sys.argv[0]).name
if program=='xprop':
 focus=json.loads(pathlib.Path(os.environ['FOCUS_VALUE']).read_text())
 if focus is None: sys.exit(1)
 print('GAMESCOPE_FOCUSED_APP(CARDINAL) =',focus)
elif program=='pw-dump':
 print(json.dumps([{'type':'PipeWire:Interface:Core','info':{'cookie':s['cookie']}}, *[
  {'id':n['id'],'type':'PipeWire:Interface:Node','info':{'state':'running','props':{
    'media.class':'Stream/Output/Audio','object.serial':n['serial'],'application.name':'Waydroid',
    'application.process.binary':'waydroid','application.process.id':1,
    'application.process.host':n.get('host','lepton-steamlaunch-'+str(n['app'])), 'media.name':n['name']}}}
  for n in s['nodes']]]))
elif program=='wpctl':
 node=next(n for n in s['nodes'] if n['id']==int(sys.argv[2]))
 if sys.argv[1]=='get-volume': print('Volume:',node['volume']/100,'[MUTED]' if node['muted'] else '')
 else:
  if sys.argv[1]=='set-volume':node['volume']=float(sys.argv[3].strip('%'))
  elif sys.argv[1]=='set-mute':node['muted']=sys.argv[3]=='1'
  path.write_text(json.dumps(s))
'''
with tempfile.TemporaryDirectory(prefix='mixer-policy-') as temporary:
    tmp = pathlib.Path(temporary)
    for name in ('xprop', 'pw-dump', 'wpctl'):
        p = tmp / name
        p.write_text(TOOLS)
        p.chmod(0o755)
    focus_file = tmp / 'focus.json'
    focus_file.write_text('42')
    fixture = tmp / 'audio.json'
    state = {'cookie': 7, 'focus': 42, 'nodes': [
        {'id': 10, 'serial': 100, 'app': 42, 'name': 'A', 'volume': 60, 'muted': False},
        {'id': 11, 'serial': 101, 'app': 43, 'name': 'B', 'volume': 80, 'muted': True}]}
    fixture.write_text(json.dumps(state))
    env = {**os.environ, 'PATH': str(tmp) + ':' + os.environ['PATH'], 'XDG_STATE_HOME': str(tmp / 'saved'), 'FOCUS_VALUE': str(focus_file), 'FOCUS_FIXTURE': str(fixture), 'FRAMELY_MIXER_TEST_BIN': str(tmp)}
    process = None
    def start():
        global process
        process = subprocess.Popen([str(backend)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, text=True, bufsize=1)
        global messages
        messages = queue.Queue()
        def read_messages():
            for line in process.stdout:
                messages.put(json.loads(line))
        threading.Thread(target=read_messages, daemon=True).start()
    request_id = 0
    def rpc(method, expect_error=False, **params):
        global request_id
        request_id += 1
        process.stdin.write(json.dumps({'id': request_id, 'method': method, 'params': params}) + '\n')
        process.stdin.flush()
        end = time.monotonic() + 12
        while time.monotonic() < end:
            try: message = messages.get(timeout=1)
            except queue.Empty: continue
            if message.get('id') == request_id:
                if expect_error:
                    assert 'error' in message, message
                    return message['error']
                assert 'error' not in message, message
                return message['result']
        raise AssertionError('RPC timeout')
    def update(**values):
        if 'focus' in values: focus_file.write_text(json.dumps(values.pop('focus')))
        if values:
            s = json.loads(fixture.read_text()); s.update(values); fixture.write_text(json.dumps(s))
    def until(predicate):
        for _ in range(30):
            result = rpc('status.get')
            if predicate(result): return result
            time.sleep(.15)
        raise AssertionError(result)
    def target(result, index=0):
        s = result['streams'][index]; return {'id': s['id'], 'serial': s['serial']}
    def set_app(result, enabled):
        return rpc('focus.set', cookie=7, target=target(result), enabled=enabled)
    try:
        start(); initial = rpc('status.get'); assert initial.get('streams'), initial
        result = rpc('streams.set', cookie=7, targets=[target(initial)], volume=37)
        assert result['streams'][0]['volume'] == 37
        rpc('focus.global', enabled=True)
        result = until(lambda r: r['streams'][0]['focused'] is True)
        assert not result['streams'][0]['muted'] and result['streams'][1]['muted']
        update(focus=43)
        result = until(lambda r: r['streams'][0]['autoMuted'] and r['streams'][1]['focused'] is True)
        assert result['streams'][0]['volume'] == 37 and result['streams'][0]['userMuted'] is False
        assert result['streams'][1]['muted'], 'Foreground must preserve manual mute'
        result = rpc('streams.set', cookie=7, targets=[target(result, 1)], muted=False)
        assert not result['streams'][1]['muted']
        result = set_app(result, True)
        result = rpc('focus.global', enabled=False)
        assert result['streams'][0]['autoMuted'], 'Global off must preserve independent app policy'
        result = set_app(result, False)
        assert not result['streams'][0]['muted'], 'Restore original manual mute'
        rpc('focus.global', enabled=True)
        update(focus=None)
        result = until(lambda r: r['streams'][0]['focused'] is None)
        assert not result['streams'][0]['muted'], 'Unknown focus must release owned mute'
        update(focus=43)
        until(lambda r: r['streams'][0]['autoMuted'])
        process.kill(); process.wait(); start()
        result = rpc('focus.global', enabled=False)
        assert not result['streams'][0]['muted'], 'Crash recovery must release owned mute'
        update(nodes=[]); rpc('status.get')
        update(nodes=[{'id': 20, 'serial': 200, 'app': 42, 'name': 'New media', 'volume': 100, 'muted': False},
                      {'id': 21, 'serial': 201, 'app': 43, 'name': 'B', 'volume': 100, 'muted': True}])
        result = rpc('status.get')
        assert result['streams'][0]['volume'] == 37, 'Reopened app must restore volume across new stream names'
        assert not result['streams'][1]['muted'], 'Reopened app must restore manual unmute'
        assert not result['backgroundMute']
        external=json.loads(fixture.read_text());external['nodes'][0]['volume']=25;fixture.write_text(json.dumps(external))
        assert rpc('status.get')['streams'][0]['volume']==25
        update(nodes=[]);rpc('status.get')
        external['nodes'][0].update(id=30,serial=300,volume=100,name='A')
        fixture.write_text(json.dumps(external))
        assert next(s for s in rpc('status.get')['streams'] if s['host']=='lepton-steamlaunch-42')['volume']==25, 'External manual volume must also persist' 
        update(focus=42)
        rpc('focus.global', enabled=True)
        until(lambda r: next(s for s in r['streams'] if s['host']=='lepton-steamlaunch-42')['focused'] is True)
        rpc('framely.lifecycle.start')
        timings = []
        for focus in [43, 42] * 6:
            started = time.monotonic()
            update(focus=focus)
            wanted = focus == 43
            while time.monotonic() - started < 3:
                try: actual = json.loads(fixture.read_text())['nodes'][0]['muted']
                except json.JSONDecodeError: continue
                if actual == wanted: break
                time.sleep(.005)
            else: raise AssertionError('Autonomous focus switch did not change mute')
            elapsed = time.monotonic() - started
            timings.append(elapsed * 1000)
            if not os.environ.get('MIXER_TEST_BACKEND'):
                assert elapsed < .8, f'Focus switch too slow: {elapsed:.3f}s'
            # Let status publication finish before the next independent transition.
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                message = messages.get(timeout=3)
                if message.get('event') == 'status' and next(s for s in message['data']['streams'] if s['host']=='lepton-steamlaunch-42').get('focused') == (focus == 42): break
            else: raise AssertionError('No focus status event')
            time.sleep(.04)
        print('Autonomous focus-to-mute latency (ms):', ', '.join(f'{n:.0f}' for n in timings))
        rpc('framely.lifecycle.stop')
        rpc('focus.global', enabled=False)
        rpc('apps.set', cookie=7, appKey='steam:42', muted=True)
        live = json.loads(fixture.read_text())['nodes']
        update(nodes=[])
        result = rpc('status.get')
        row = next(a for a in result['rememberedApps'] if a['appKey']=='steam:42')
        assert row['userMuted'] and row['volume']==25
        assert not result['streams'], 'Remembered apps must not masquerade as live audio nodes'
        process.stdin.close(); process.wait(timeout=12); start()
        assert any(a['appKey']=='steam:42' for a in rpc('status.get')['rememberedApps']), 'Muted app must survive backend restart'
        before = fixture.read_text()
        rpc('apps.set', expect_error=True, cookie=8, appKey='steam:42', muted=False)
        rpc('apps.set', expect_error=True, cookie=7, appKey='steam:unknown', muted=False)
        rpc('apps.set', expect_error=True, cookie=7, appKey='steam:42', volume=101)
        rpc('apps.set', expect_error=True, cookie=7, appKey='steam:42', volume=50, muted=False)
        assert fixture.read_text()==before, 'Invalid remembered-app requests must never write audio'
        rpc('apps.set', cookie=7, appKey='steam:42', volume=46)
        result = rpc('apps.set', cookie=7, appKey='steam:42', muted=False)
        assert not any(a['appKey']=='steam:42' for a in result['rememberedApps'])
        live[0].update(id=40, serial=400, volume=100, muted=True)
        update(nodes=live)
        result = rpc('status.get')
        app = next(s for s in result['streams'] if s['host']=='lepton-steamlaunch-42')
        assert app['volume']==46 and not app['muted'], 'Absent-app edits must apply to new audio nodes'
        assert not any(a['appKey']=='steam:42' for a in result['rememberedApps']), 'No duplicate remembered/live app'
        # Background policy also remains reachable after the stream vanishes.
        rpc('apps.set', cookie=7, appKey='steam:42', backgroundMute=True)
        update(nodes=[])
        result = rpc('status.get')
        assert next(a for a in result['rememberedApps'] if a['appKey']=='steam:42')['backgroundMute']
        result = rpc('apps.set', cookie=7, appKey='steam:42', backgroundMute=False)
        assert not any(a['appKey']=='steam:42' for a in result['rememberedApps'])
        # A global automatic mute retains the app without becoming its manual mute preference.
        update(nodes=live, focus=43)
        rpc('focus.global', enabled=True)
        until(lambda r: next(s for s in r['streams'] if s['host']=='lepton-steamlaunch-42')['autoMuted'])
        update(nodes=[])
        row = next(a for a in rpc('status.get')['rememberedApps'] if a['appKey']=='steam:42')
        assert not row['userMuted']
        result = rpc('focus.global', enabled=False)
        assert not any(a['appKey']=='steam:42' for a in result['rememberedApps'])
        update(nodes=[{'id':70,'serial':700,'app':0,'host':'unresolved-container','name':'No process','volume':55,'muted':False}])
        result = rpc('status.get')
        assert not result['streams'][0]['focusSupported']
        rpc('streams.set',cookie=7,targets=[target(result)],muted=True)
        update(nodes=[])
        row = next(a for a in rpc('status.get')['rememberedApps'] if a['appKey'].startswith('audio:'))
        assert row['userMuted'] and not row['focusSupported'], 'Unresolved processes must also remain visible when muted'
        rpc('apps.set', expect_error=True, cookie=7, appKey=row['appKey'], backgroundMute=True)
        result = rpc('apps.set',cookie=7,appKey=row['appKey'],muted=False)
        assert not result['rememberedApps']
        process.stdin.close(); process.wait(timeout=12)
        legacy_home = tmp / 'legacy-home'
        env['HOME'] = str(legacy_home)
        registry = legacy_home / '.local/share/framely/apk-manager/state.json'
        registry.parent.mkdir(parents=True)
        registry.write_text(json.dumps({'records':{'android':{'steamAppId':42,'metadata':{'name':'旧安卓应用'}}}}))
        steam = legacy_home / '.local/share/Steam'
        (steam / 'steamapps').mkdir(parents=True)
        (steam / 'steamapps/appmanifest_11.acf').write_text('"AppState" {\n"appid" "11"\n"name" "Old Steam game"\n}')
        shortcuts = steam / 'userdata/1/config/shortcuts.vdf'
        shortcuts.parent.mkdir(parents=True)
        shortcuts.write_bytes(b'\x00shortcuts\x00\x000\x00\x02appid\x00'+(2838518023).to_bytes(4,'little')+b'\x01AppName\x00Old shortcut\x00\x08\x08\x08')
        prefs = {
            'exe:/missing/framely-vr':{'volume':35,'muted':True},
            'steam:42':{'volume':37,'muted':True},
            'steam:11':{'volume':50,'muted':True},
            'steam:2838518023':{'volume':60,'muted':True},
            'steam:1':{'volume':80,'muted':True},
            'steam:99':{'volume':20,'muted':False},
            'exe:/missing/custom':{'volume':40,'muted':False,'streams':{'Music':{'muted':True}}},
        }
        old = {'global':False,'enabled':['steam:900'],'owned':{},'preferences':prefs,
               'applications':{'exe:/missing/custom':{'app':'Existing custom name','binary':'custom'}}}
        saved_path = tmp / 'saved/framely-volume-mixer/focus.json'
        saved_path.write_text(json.dumps(old))
        update(nodes=[])
        start()
        result = rpc('status.get')
        rows = {a['appKey']:a for a in result['rememberedApps']}
        assert rows['exe:/missing/framely-vr']['app']=='framely-vr'
        assert rows['steam:42']['app']=='旧安卓应用' and rows['steam:42']['iconKey']=='lepton:42'
        assert rows['steam:11']['app']=='Old Steam game'
        assert rows['steam:2838518023']['app']=='Old shortcut'
        assert rows['steam:1']['app']=='Steam 1', 'Missing local metadata must not hide old muted apps'
        assert rows['steam:900']['backgroundMute'], 'Legacy rule without volume preference must stay reachable'
        assert rows['exe:/missing/custom']['app']=='Existing custom name', 'Migration must preserve newer display records'
        assert 'steam:99' not in rows
        migrated = json.loads(saved_path.read_text())
        def unchanged(expected, actual):
            for field, value in expected.items():
                if isinstance(value, dict): unchanged(value, actual[field])
                else: assert actual[field]==value, (field, value, actual[field])
        for key, pref in prefs.items(): unchanged(pref, migrated['preferences'][key])
        assert migrated['enabled']==old['enabled'] and migrated['global']==old['global']
        before = saved_path.read_bytes()
        process.stdin.close(); process.wait(timeout=12); start(); rpc('status.get')
        assert saved_path.read_bytes()==before, 'Legacy migration must be idempotent'
        result = rpc('apps.set',cookie=7,appKey='exe:/missing/framely-vr',muted=False)
        assert not any(a['appKey']=='exe:/missing/framely-vr' for a in result['rememberedApps'])
        print('Legacy preferences migrated without audio streams; names, fallback IDs, settings and restart verified')
        print('Global/app policy, manual mute, recovery, reopen preferences and remembered muted apps passed')
    finally:
        if process and process.poll() is None:
            process.stdin.close(); process.wait(timeout=12)
        if process:
            errors = process.stderr.read()
            if errors: print(errors)
