#!/usr/bin/env python3
"""Observe build isolation and optional-renderer behavior through verified harness HTTPS."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import threading
import time
import tomllib
from urllib.parse import urlsplit


REPO = Path(__file__).resolve().parents[2]
RUN = Path(os.environ['PF_RUN_DIR'])
APEX = os.environ.get('PF_APEX', 'pf.test')
BASE = f'https://parts.{APEX}'
PORT = os.environ.get('PF_HTTPS_PORT', '18443')
TOKEN = (RUN / 'secrets/api_token').read_text().strip()
CANARY = 'BUILD_GATE_SOURCE_MUST_NOT_APPEAR_IN_LOGS'
QUERY = 'build_gate_query_must_not_appear_in_logs'


def http(url, data=None, token=False):
    host = urlsplit(url).hostname
    command = ['curl', '-sS', '--max-time', '15', '--cacert', str(RUN / 'certs/ca.pem'),
               '--connect-to', f'{host}:443:127.0.0.1:{PORT}', '-w', '\n%{http_code}', url]
    if token:
        command += ['-H', 'Authorization: Bearer ' + TOKEN]
    if data is not None:
        command += ['-H', 'Content-Type: application/json', '--data-binary', '@-']
    try:
        result = subprocess.run(command, input=json.dumps(data) if data is not None else None,
                                capture_output=True, text=True, timeout=18)
    except subprocess.TimeoutExpired:
        raise AssertionError('HTTPS request timed out: ' + urlsplit(url).path) from None
    assert result.returncode == 0, ('verified HTTPS failed', urlsplit(url).path, result.returncode)
    body, status = result.stdout.rsplit('\n', 1)
    return int(status), body


def call(op, data, expected=200):
    status, body = http(BASE + '/api/v1/' + op, data, token=True)
    assert status == expected, (op, status, body[:1500])
    return json.loads(body)


def dev(op, data):
    if os.environ['PF_HARNESS_MODE'] == 'host':
        command = [os.environ['PF_BIN'], 'dev', '--socket', str(RUN / 'data/run/dev.sock')]
    else:
        command = [str(REPO / 'tests/e2e/harness/compose.sh'), '--profile', 'container',
                   'exec', '-T', 'partifactory', '/usr/local/bin/partifactory', 'dev']
    r = subprocess.run(command + ['request', op], input=json.dumps(data), text=True,
                       capture_output=True, timeout=30, check=True)
    return json.loads(r.stdout)


def settled(name):
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        part = call('part_get', {'name': name})
        if part['build']['status'] not in ['queued', 'running']:
            return part
        time.sleep(.05)
    raise AssertionError('Build did not settle: ' + name)


def create(name, files, live='always'):
    call('part_create', {'name': name, 'title': name, 'kind': 'html', 'files': files,
                         'live': live, 'request_id': name + '-create'})
    part = settled(name)
    assert part['build']['status'] == 'ok', part
    if live == 'after_check':
        deadline = time.monotonic() + 10
        while part['live'] != part['head'] and time.monotonic() < deadline:
            time.sleep(.05)
            part = call('part_get', {'name': name})
    return part


def logs():
    result = []
    for line in (RUN / 'server.jsonl').read_text().splitlines():
        if line.startswith('{'):
            result.append(json.loads(line))
    return result


def rows(sql, parameters=()):
    with sqlite3.connect((RUN / 'data/partifactory.db').as_uri() + '?mode=ro', uri=True) as db:
        db.row_factory = sqlite3.Row
        return [dict(r) for r in db.execute(sql, parameters)]


def build_gate():
    name = 'build-gate-faults'
    marker = 'previous healthy live build'
    initial = create(name, {'index.html': f'<!doctype html><h1>{marker}</h1>'})
    assert initial['live'] == 1, initial
    cap = dev('cap', {'name': name})['url']
    checks = []
    source = {'index.html': f'<!doctype html><h1>{marker}</h1>'}
    stop = threading.Event()
    health = []

    def watch_health():
        while not stop.is_set():
            try:
                status, body = http(BASE + '/healthz')
                health.append(status == 200 and json.loads(body)['status'] == 'ok')
            except Exception:
                health.append(False)
            stop.wait(.1)

    watcher = threading.Thread(target=watch_health)
    watcher.start()
    try:
        for index, (fault, reason) in enumerate([
            ('private', 'private_address'), ('outside', 'not_allowlisted'), ('panic', 'builder_crashed')
        ], start=2):
            if fault == 'panic':
                edits = [{'op': 'write', 'file': 'index.html', 'content': '<h1>' + CANARY + '</h1>'},
                         {'op': 'write', 'file': '.pf/fault', 'content': 'panic'}]
            else:
                url = ('https://127.0.0.1/private.js' if fault == 'private' else
                       'https://unpkg.com/off-allowlist.js') + '?canary=' + QUERY
                edits = [{'op': 'write', 'file': 'index.html',
                          'content': f'<!doctype html><h1>{CANARY}</h1><script src="{url}"></script>'}]
            for edit in edits:
                previous = source.get(edit['file'])
                edit['if_match'] = hashlib.sha256(previous.encode()).hexdigest() if previous is not None else None
                source[edit['file']] = edit['content']
            call('part_edit', {'name': name, 'edits': edits, 'request_id': name + '-' + fault})
            part = settled(name)
            assert part['head'] == index and part['live'] == 1, part
            assert part['build']['status'] == 'failed', part
            errors = json.dumps(part['build']['errors'])
            assert reason in errors, errors
            if fault != 'panic':
                assert 'mirror_failed' in errors, errors
            status, body = http(cap)
            assert status == 200 and marker in body and CANARY not in body, ('old live stopped serving', fault)
            assert http(BASE + '/healthz')[0] == 200
            checks.append({'fault': fault, 'head': part['head'], 'live': part['live'],
                           'build': part['build'], 'saveStatus': 200, 'liveStatus': status})
        # Removing the injected fault and publishing a new build proves the pool respawns.
        call('part_edit', {'name': name, 'edits': [
            {'op': 'delete', 'file': '.pf/fault', 'if_match': hashlib.sha256(b'panic').hexdigest()},
            {'op': 'write', 'file': 'index.html', 'content': '<h1>worker recovered</h1>',
             'if_match': hashlib.sha256(source['index.html'].encode()).hexdigest()}
        ], 'request_id': name + '-recovery'})
        recovered = settled(name)
        assert recovered['build']['status'] == 'ok' and recovered['live'] == 5, recovered
    finally:
        stop.set()
        watcher.join(timeout=20)
    assert health and all(health), health
    create('build-gate-deps', {'index.html': '<!doctype html><h1>Mirrored dependency evidence</h1>'
                  '<script type="module" src="https://unpkg.com/widget@1.2.3/main.js"></script>'
                  '<script src="https://unpkg.com/unknown.js"></script>'})
    dependencies = rows('''SELECT d.* FROM build_deps d JOIN builds b ON b.id=d.build_id
        JOIN versions v ON v.id=b.version_id JOIN parts p ON p.id=v.part_id WHERE p.name=?''', ('build-gate-deps',))
    assert any(r['ecosystem'] == 'npm' and r['name'] == 'widget' and r['version'] == '1.2.3' for r in dependencies), dependencies
    assert any(r['ecosystem'] == 'unknown' and r['name'] == 'https://unpkg.com/unknown.js' for r in dependencies), dependencies
    cache = rows("SELECT * FROM mirror_cache WHERE url LIKE 'https://unpkg.com/%'")
    assert cache and all(r['hash'] for r in cache), cache
    history = rows('''SELECT p.name,v.n,b.id,b.status,b.errors,b.warnings,b.gate_check FROM builds b
        JOIN versions v ON v.id=b.version_id JOIN parts p ON p.id=v.part_id
        WHERE p.name IN (?,?) ORDER BY p.name,v.n''', (name, 'build-gate-deps'))
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        events = logs()
        if all(any(r.get('event') == 'build.finished' and r.get('build_id') == b['id'] for r in events) for b in history):
            break
        time.sleep(.05)
    assert all(any(r.get('event') == 'build.finished' and r.get('build_id') == b['id'] for r in events) for b in history), history
    assert any(r.get('event') == 'egress.refused' and r.get('reason') == 'private_address' for r in events)
    raw = (RUN / 'server.jsonl').read_text()
    assert CANARY not in raw and QUERY not in raw, 'source bytes or URL query leaked into server JSONL'
    return {'pass': True, 'healthPolls': len(health), 'healthFailures': health.count(False),
            'faults': checks, 'recoveredLive': recovered['live'], 'builds': history,
            'build_deps': dependencies, 'mirror_cache': cache,
            'events': [r for r in events if r.get('event') in ['build.finished', 'egress.refused']]}


def renderer_off():
    assert os.environ.get('PF_E2E_RENDER', '0') == '0', 'requires a genuinely renderer-off harness'
    configured = tomllib.loads((RUN / 'site.toml').read_text()).get('render', {}).get('enabled', False)
    assert configured is False, 'site configuration unexpectedly enables the renderer'
    running = subprocess.check_output([str(REPO / 'tests/e2e/harness/compose.sh'),
        '--profile', 'render', 'ps', '--services', '--status', 'running'], text=True).splitlines()
    assert 'partifactory-render' not in running, running
    name = 'render-off-gate'
    first = create(name, {'index.html': '<!doctype html><h1>renderer optional v1</h1>'}, 'after_check')
    assert first['live'] == 1 and 'check_skipped' in json.dumps(first['build']['warnings']), first
    unavailable = call('part_check', {'name': name}, expected=503)
    assert unavailable['error']['code'] == 'renderer_unavailable', unavailable
    call('part_edit', {'name': name, 'edits': [{'op': 'replace', 'old_str': 'optional v1', 'new_str': 'optional v2'}],
                       'live': 'after_check', 'request_id': name + '-edit'})
    second = settled(name)
    deadline = time.monotonic() + 10
    while second['live'] != 2 and time.monotonic() < deadline:
        time.sleep(.05)
        second = call('part_get', {'name': name})
    assert second['live'] == 2 and 'check_skipped' in json.dumps(second['build']['warnings']), second
    status, body = http(dev('cap', {'name': name})['url'])
    assert status == 200 and 'renderer optional v2' in body
    persisted = rows('''SELECT b.id,b.status,b.gate_check,b.warnings FROM builds b
        JOIN versions v ON v.id=b.version_id JOIN parts p ON p.id=v.part_id WHERE p.name=? ORDER BY v.n''', (name,))
    assert len(persisted) == 2 and all(r['status'] == 'ok' and r['gate_check'] == 'skipped' for r in persisted), persisted
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        switched = [r for r in logs() if r.get('event') == 'live.switch' and r.get('build') in [b['id'] for b in persisted]]
        if len(switched) == 2:
            break
        time.sleep(.05)
    assert len(switched) == 2 and all(r.get('gate') == 'after_check' and r.get('result') == 'skipped' for r in switched), switched
    return {'pass': True, 'renderConfiguredEnabled': configured, 'runningServices': running,
            'partCheckStatus': 503, 'partCheckError': unavailable['error']['code'],
            'liveAfterCreate': first['live'], 'liveAfterEdit': second['live'], 'servedStatus': status,
            'builds': persisted, 'liveSwitchEvents': switched}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--case', choices=['builds', 'renderer-off', 'all'], default='all')
    p.add_argument('--out', type=Path, required=True)
    args = p.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    dev('sign-in', {'username': 'admin'})
    report = {'sourceCommit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()}
    binary = Path(os.environ.get('PF_BIN', REPO / 'target/debug/partifactory'))
    report['binarySha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
    if args.case in ['builds', 'all']:
        report['builds'] = build_gate()
    if args.case in ['renderer-off', 'all']:
        report['rendererOff'] = renderer_off()
    report['pass'] = True
    (args.out / 'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({'event': 'build.running_gate', 'case': args.case, 'outcome': 'pass', 'evidence': str(args.out)}))


if __name__ == '__main__':
    main()
