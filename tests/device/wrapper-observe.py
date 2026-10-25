#!/usr/bin/env python3
"""Observe a real Safari first-open and restart the actual release service for regrant."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import threading
import time


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run',type=Path)
    parser.add_argument('out',type=Path)
    parser.add_argument('nonce')
    args=parser.parse_args()
    apex=os.environ['PF_APEX']
    port=os.environ['PF_HTTPS_PORT']
    collector=f'https://blobs-part.{apex}:{port}/__harness'
    pause=threading.Event()

    def http(route,body=None):
        command=['curl','-fsS','--max-time','10','--cacert',str(args.run/'certs/ca.pem'),'--resolve',f'blobs-part.{apex}:{port}:127.0.0.1',collector+route]
        if body is not None:command+=['-H','Content-Type: text/plain','--data-binary',json.dumps(body)]
        response=subprocess.run(command,capture_output=True,check=True,timeout=12)
        return json.loads(response.stdout) if response.stdout else None

    def reports():return http('/reports?run='+args.nonce)
    def observed(item):return any(r.get('item')==item and r.get('outcome')=='pass' for r in reports())
    def frames():return sum(r.get('item')=='wrapper_frame_loaded' and r.get('outcome')=='pass' for r in reports())
    def logs():
        # Read the container itself after restart: a Docker log-follow process can exit
        # on stop, and cannot be the authority for the new process's boot epoch.
        snapshot=subprocess.run(['tests/e2e/harness/compose.sh','--profile','container','logs','--no-color','--no-log-prefix','partifactory'],env={**os.environ,'PF_RUN_DIR':str(args.run)},capture_output=True,text=True,check=True,timeout=20).stdout
        result=[]
        for line in snapshot.splitlines():
            if line.startswith('{'):
                try:result.append(json.loads(line))
                except ValueError:pass
        return result
    def wait(check,label,seconds=180):
        deadline=time.monotonic()+seconds
        while time.monotonic()<deadline:
            if check():return
            pause.wait(.15)
        raise AssertionError('No actual observation: '+label)
    def barrier(item):
        http('/report',{'event':'device.check','item':item,'lane':'harness-observer','outcome':'pass','run':args.nonce})
    def chrome(rows):return [r for r in rows if r.get('event')=='chrome.grant' and r.get('part')=='ios-wrapper' and r.get('outcome')=='ok']
    def boot(rows):return [r['boot_epoch'] for r in rows if r.get('event')=='store.ready'][-1]

    wait(lambda:observed('wrapper_first_open'),'native Safari first open')
    before=logs()
    initial_grants=[r for r in before if r.get('event')=='http.request' and r.get('path')=='/grant' and r.get('method')=='GET']
    assert len(initial_grants)==1,initial_grants
    initial_frames=frames()
    assert initial_frames==1,initial_frames
    old_boot=boot(before)
    # Persist the session/key files and database; invalidate real caps by a genuine start.
    subprocess.run(['tests/e2e/harness/compose.sh','--profile','container','restart','partifactory'],env={**os.environ,'PF_RUN_DIR':str(args.run)},check=True,timeout=90)
    wait(lambda:boot(logs())>old_boot,'actual incremented boot epoch',60)
    barrier('wrapper_service_restarted')
    def renewed():return len(chrome(logs()[len(before):]))>=1 and frames()>initial_frames
    wait(renewed,'new real frame after silent chrome grant')
    after=logs()[len(before):]
    grants=chrome(after)
    assert len(grants)==1,grants
    assert any(r.get('event')=='cap.verify' and r.get('outcome')=='stale_epoch' for r in after),after
    assert any(r.get('event')=='http.request' and r.get('status')==401 and r.get('path','').startswith('/c/') for r in after),after
    assert any(r.get('event')=='http.request' and r.get('method')=='POST' and r.get('path')=='/api/chrome/v1/ios-wrapper/grant' and r.get('status')==200 for r in after),after
    assert not any(r.get('event')=='wrapper.serve' or r.get('path') in ['/grant','/signin','/oidc/callback'] for r in after),after
    barrier('wrapper_regrant_observed')
    wait(lambda:observed('wrapper_regrant'),'native fresh-cap read and clean address')
    final=logs()[len(before):]
    assert len(chrome(final))==1,chrome(final)
    assert not any(r.get('event')=='wrapper.serve' or r.get('path') in ['/grant','/signin','/oidc/callback'] for r in final),final
    assert sum(r.get('event')=='rt.op' and r.get('op')=='storage.get' and r.get('outcome')=='ok' for r in final)>=2,final
    fields={'timestamp','event','outcome','part','op','path','status','method','request_id','boot_epoch'}
    trace=[{k:v for k,v in r.items() if k in fields} for r in final if r.get('event') in ['store.ready','cap.verify','chrome.grant','rt.op','wrapper.serve'] or r.get('path')=='/api/chrome/v1/ios-wrapper/grant']
    report={'pass':True,'harness':'release','image':os.environ['PF_HARNESS_IMAGE'],'checks':{'wrapper_first_open':True,'wrapper_regrant':True},'firstOpenGrantHops':len(initial_grants),'chromeRegrants':1,'topLevelNavigationsDuringRegrant':0,'bootEpochBefore':old_boot,'bootEpochAfter':boot(logs()),'initialFrames':initial_frames,'renewedFrames':frames(),'events':trace}
    (args.out/'observer-report.json').write_text(json.dumps(report,indent=2))
    (args.out/'collector.json').write_text(json.dumps(reports(),indent=2))
    print('Safari wrapper: one first-open hop, real boot invalidation, one silent chrome regrant, no top navigation PASS')


if __name__=='__main__':main()
