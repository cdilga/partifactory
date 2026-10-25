#!/usr/bin/env python3
"""Private file-command bridge to the existing genuine native rmcp client.

The browser container cannot execute a macOS binary. Requests/results live only in
the disposable mode-0700 harness run directory; tokens never enter public evidence.
Each completion file is an explicit barrier, never a fabricated protocol response.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import threading


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run',type=Path)
    parser.add_argument('--client',type=Path,required=True)
    parser.add_argument('--apex',default='pf.test')
    parser.add_argument('--port',type=int,default=443)
    args=parser.parse_args()
    directory=args.run/'communal-private'
    directory.mkdir(mode=0o700,parents=True,exist_ok=True)
    stopped=threading.Event()
    for sig in (signal.SIGINT,signal.SIGTERM):signal.signal(sig,lambda *_:stopped.set())
    (directory/'ready').touch(mode=0o600)
    while not stopped.is_set():
        for request in sorted(directory.glob('*.request.json')):
            result=request.with_name(request.name.replace('.request.json','.result.json'))
            if result.exists():continue
            command=json.loads(request.read_text())
            env=dict(os.environ,PF_MCP_URL=f'https://parts.{args.apex}/mcp',PF_MCP_CONNECT=f'127.0.0.1:{args.port}',PARTIFACTORY_CA_CERT=str(args.run/'certs/ca.pem'),PARTIFACTORY_TOKEN=command['token'])
            for key in ('HTTPS_PROXY','https_proxy','ALL_PROXY','all_proxy'):env.pop(key,None)
            try:
                run=subprocess.run([str(args.client.resolve())],input=json.dumps([{'op':command['op'],'input':command['input']}]),env=env,capture_output=True,text=True,timeout=120,check=True)
                value=json.loads(run.stdout)
            except (subprocess.SubprocessError,ValueError):
                # Never publish raw client stderr: protocol errors may carry credentials.
                value={'broker_error':'Real rmcp client failed; private local diagnostics required'}
            temporary=result.with_suffix('.tmp')
            temporary.write_text(json.dumps(value));temporary.chmod(0o600);temporary.replace(result)
        stopped.wait(.05)


if __name__=='__main__':main()
