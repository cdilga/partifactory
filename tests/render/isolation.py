#!/usr/bin/env python3
"""Observe real render-container isolation and TLS/header parity against a running harness."""
import http.client
import json
import os
from pathlib import Path
import socket
import ssl
import subprocess
import sys

run = Path(sys.argv[1]).resolve()
sidecar = "pf-harness-partifactory-render-1"
config = json.loads(subprocess.check_output(["docker", "inspect", sidecar]))[0]
host = config["HostConfig"]
assert host["ReadonlyRootfs"] and config["Config"]["User"] == "1000:1000"
assert host["Memory"] == 768 * 1024 * 1024 and host["PidsLimit"] == 256
assert host["NanoCpus"] == 1_000_000_000 and "/tmp" in host["Tmpfs"]
assert set(config["NetworkSettings"]["Networks"]) == {"pf-harness_render"}
assert host["LogConfig"]["Config"] == {"max-size": "10m", "max-file": "3"}
network = json.loads(subprocess.check_output(["docker", "network", "inspect", "pf-harness_render"]))[0]
assert network["Internal"]

script = r'''
import http.client,json,os,socket,ssl
from pathlib import Path
for host,port in [('example.com',443),('172.30.77.10',443)]:
    try:
        socket.create_connection((host,port),timeout=2).close()
    except OSError:
        pass
    else:
        raise AssertionError('render can reach forbidden canary')
target=os.environ['PF_RENDER_GATEWAY']
host,sep,port=target.partition(':')
context=ssl.create_default_context(cadata=ssl.DER_cert_to_PEM_cert(Path('/run/pf-render/certificate.der').read_bytes()))
connection=context.wrap_socket(socket.create_connection((host,int(port) if sep else 443),timeout=5),server_hostname='pixel-part.'+os.environ['PF_RENDER_APEX'])
connection.sendall(('GET / HTTP/1.1\r\nHost: pixel-part.'+os.environ['PF_RENDER_APEX']+'\r\nConnection: close\r\n\r\n').encode())
response=http.client.HTTPResponse(connection);response.begin()
assert response.status==200,response.status
print(json.dumps(dict((k.lower(),v) for k,v in response.getheaders())))
'''
internal = json.loads(subprocess.check_output(["docker", "exec", "-i", sidecar, "python3", "-"], input=script.encode()))
context = ssl.create_default_context(cafile=str(run / "certs/ca.pem"))
apex = os.environ.get("PF_APEX", "pf.test")
with context.wrap_socket(socket.create_connection(("127.0.0.1", int(os.environ.get("PF_HTTPS_PORT", "18443"))), timeout=5), server_hostname="pixel-part." + apex) as connection:
    connection.sendall(("GET / HTTP/1.1\r\nHost: pixel-part." + apex + "\r\nConnection: close\r\n\r\n").encode())
    response = http.client.HTTPResponse(connection)
    response.begin()
    assert response.status == 200
    public = {k.lower(): v for k, v in response.getheaders()}
for name in ["content-security-policy", "cross-origin-opener-policy", "referrer-policy", "x-content-type-options", "content-type", "cache-control"]:
    assert public.get(name) == internal.get(name), name
assert internal["content-security-policy"]
print("PASS render: non-root/read-only/resources/internal-only network; egress and LAN blocked; verified internal TLS and identical security headers")
