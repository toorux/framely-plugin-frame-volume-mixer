"""Exercise the real RPC controller against isolated audio/focus tools, never device audio."""
import json
import os
import pathlib
import select
import subprocess
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--no-deps', '--format-version=1', '--manifest-path', str(ROOT / 'backend/Cargo.toml')]))
backend = pathlib.Path(metadata['target_directory']) / 'debug/framely-volume-mixer'

TOOLS = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
path=pathlib.Path(os.environ['FOCUS_FIXTURE'])
s=json.loads(path.read_text()); program=pathlib.Path(sys.argv[0]).name
if program=='xprop':
 if s['focus'] is None: sys.exit(1)
 print('GAMESCOPE_FOCUSED_APP(CARDINAL) =',s['focus'])
elif program=='pw-dump':
 print(json.dumps([{'type':'PipeWire:Interface:Core','info':{'cookie':s['cookie']}}, *[
  {'id':n['id'],'type':'PipeWire:Interface:Node','info':{'state':'running','props':{
    'media.class':'Stream/Output/Audio','object.serial':n['serial'],'application.name':'Waydroid',
    'application.process.binary':'waydroid','application.process.id':1,
    'application.process.host':'lepton-steamlaunch-'+str(n['app']), 'media.name':n['name']}}}
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
    fixture = tmp / 'audio.json'
    state = {'cookie': 7, 'focus': 42, 'nodes': [
        {'id': 10, 'serial': 100, 'app': 42, 'name': 'A', 'volume': 60, 'muted': False},
        {'id': 11, 'serial': 101, 'app': 43, 'name': 'B', 'volume': 80, 'muted': True}]}
    fixture.write_text(json.dumps(state))
    env = {**os.environ, 'PATH': str(tmp) + ':' + os.environ['PATH'], 'XDG_STATE_HOME': str(tmp / 'saved'), 'FOCUS_FIXTURE': str(fixture), 'FRAMELY_MIXER_TEST_BIN': str(tmp)}
    process = None
    def start():
        global process
        process = subprocess.Popen([str(backend)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, text=True, bufsize=1)
    request_id = 0
    def rpc(method, **params):
        global request_id
        request_id += 1
        process.stdin.write(json.dumps({'id': request_id, 'method': method, 'params': params}) + '\n')
        process.stdin.flush()
        end = time.monotonic() + 12
        while time.monotonic() < end:
            if not select.select([process.stdout], [], [], 1)[0]: continue
            line = process.stdout.readline()
            assert line, process.stderr.read()
            message = json.loads(line)
            if message.get('id') == request_id:
                assert 'error' not in message, message
                return message['result']
        raise AssertionError('RPC timeout')
    def update(**values):
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
        print('Global/app policy, manual mute preservation, missing focus, crash recovery and reopened-app preferences passed')
    finally:
        if process and process.poll() is None:
            process.stdin.close(); process.wait(timeout=12)
        if process:
            errors = process.stderr.read()
            if errors: print(errors)
