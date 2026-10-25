#!/usr/bin/env python3
"""Seed isolated pressure fixtures on the running harness database; never production data."""
import hashlib
import json
import pathlib
import sqlite3
import sys
import time

root = pathlib.Path(sys.argv[1]).resolve()
assert root.name == 'data' and 'e2e-runs' in root.parts, root
request = root / 'pressure-seed.request'
for _ in range(1200):
    if request.exists():
        break
    time.sleep(.1)
else:
    raise SystemExit('storage browser never requested pressure fixtures')
now = int(time.time() * 1000)
c = sqlite3.connect(root / 'partifactory.db', timeout=30)
c.execute('PRAGMA foreign_keys=ON')
with c:
    owner, space = c.execute("SELECT owner_user_id,space_id FROM parts WHERE name='notes'").fetchone()
    c.execute("INSERT INTO parts(id,name,space_id,title,kind,owner_user_id,created_by_user,created_at,updated_at) VALUES('storage-fixture','store-pressure',?1,'Storage fixture','html',?2,?2,?3,?3)", (space, owner, now))
    c.execute("INSERT INTO versions(id,part_id,n,manifest_hash,entry,author_user,via,created_at) VALUES('storage-version','storage-fixture',1,'storage-version','index.html',?1,'api',?2)", (owner, now))
    hashes = {}
    for name, size in [('small',512),('other',512),('large',768),('log',1500)]:
        data = (name.encode() * size)[:size]
        h = hashlib.sha256(data).hexdigest()
        p = root / 'blobs' / h[:2] / h[2:4] / h
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)
        c.execute('INSERT INTO blobs(hash,size,created_at) VALUES(?,?,?) ON CONFLICT DO NOTHING', (h,size,now))
        hashes[name] = h
    for h in [hashes['small'],hashes['other']]:
        c.execute("INSERT INTO shared_blobs(hash,source,label,added_at,content_type) VALUES(?,'runtime','pressure',?,'text/plain')",(h,now))
        c.execute("INSERT INTO mirror_cache(url,resolved_url,hash,content_type,pinned,fetched_at) VALUES(?,? ,?,'text/plain',1,?)",(h,h,h,now))
        c.execute("INSERT INTO data_snapshots(part_id,taken_at,blob_hash,docs,bytes) VALUES('storage-fixture',?,?,0,512)",(now,h))
    c.execute("INSERT INTO part_previews(part_id,version_id,source,poster_hash,video_hash,state,updated_at) VALUES('storage-fixture','storage-version','render',?,?,'ready',?)",(hashes['small'],hashes['other'],now))
    c.execute("INSERT INTO data_docs(part_id,scope,collection,doc_id,body,rev,updated_at,updated_by) VALUES('storage-fixture','shared','fixture','old',?,1,?,'system')",(json.dumps('x'*1022).encode(),now))
    c.execute("INSERT INTO data_snapshot_policies(part_id,mode,keep,set_by,set_at) VALUES('storage-fixture','off',7,'system',?)",(now,))
    queries = {
        'blobs': 'SELECT COALESCE(sum(size),0) FROM blobs',
        'shared_blobs': 'SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT hash FROM shared_blobs)',
        'mirror_cache': 'SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT hash FROM mirror_cache)',
        'data_snapshots': 'SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT blob_hash FROM data_snapshots)',
        'previews': 'SELECT COALESCE(sum(size),0) FROM blobs WHERE hash IN (SELECT poster_hash FROM part_previews UNION SELECT video_hash FROM part_previews)',
        'data_docs': 'SELECT COALESCE(sum(length(body)),0) FROM data_docs',
    }
    for store, sql in queries.items():
        cap = c.execute(sql).fetchone()[0]
        c.execute('INSERT INTO kv(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value',('store_cap:'+store,str(cap)))
    large = hashes['large']
    cases = {
        'blobs': ("INSERT INTO blobs(hash,size,created_at) VALUES('pressure-extra',1,0)",()),
        'shared_blobs': ("INSERT INTO shared_blobs(hash,source,label,added_at) VALUES(?,'runtime','pressure',0)",(large,)),
        'mirror_cache': ("INSERT INTO mirror_cache(url,resolved_url,hash,content_type,pinned,fetched_at) VALUES('pressure-extra','pressure-extra',?,'text/plain',1,0)",(large,)),
        'data_snapshots': ("INSERT INTO data_snapshots(part_id,taken_at,blob_hash,docs,bytes) VALUES('storage-fixture',0,?,0,768)",(large,)),
        'previews': ("UPDATE part_previews SET poster_hash=? WHERE part_id='storage-fixture'",(large,)),
        'data_docs': ("UPDATE data_docs SET body=zeroblob(1025) WHERE part_id='storage-fixture' AND doc_id='old'",()),
    }
    for store, (sql, args) in cases.items():
        try:
            c.execute(sql,args)
        except sqlite3.IntegrityError as e:
            assert str(e) == 'pf_storage_full:'+store, (store,str(e))
        else:
            raise AssertionError(store+' accepted a write over its cap')
    # Restore ordinary write capacity for the API quota loop. Pressure-stop cases above are
    # transactionally observed on this running server's real database.
    for store in queries:
        c.execute('DELETE FROM kv WHERE key=?',('store_cap:'+store,))
    body = json.dumps('x'*1400)
    c.execute("INSERT INTO events(at,type,payload) VALUES(?,'pressure',?)",(now,body))
    c.execute("INSERT INTO runtime_errors(part_id,version_id,fingerprint,kind,message,first_at,last_at) VALUES('storage-fixture','storage-version','pressure','error',?,?,?)",('e'*1400,now,now))
    c.execute("INSERT INTO audit(at,action,target_kind,target_id,after) VALUES(?,'pressure','part','storage-fixture',?)",(now,body))
    c.execute("INSERT INTO idempotency(principal,request_id,operation,response,created_at) VALUES('system','pressure','fixture',?,?)",(body,now))
    for n in range(5):
        c.execute("INSERT INTO quota_hits(principal,kind,day,at,limit_value) VALUES(?,'fixture',0,?,1)",('fixture:'+str(n),now))
    c.execute("INSERT INTO builds(version_id,cache_key,toolchain,method,status,log_hash,queued_at) VALUES('storage-version','pressure','fixture','server_build','ok',?,?)",(hashes['log'],now))
    for store in ['events','runtime_errors','audit','idempotency','quota_hits','build_logs']:
        c.execute('INSERT INTO kv(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value',('store_cap:'+store,'1024'))
c.close()
(root / 'pressure-seed.ready').write_text(json.dumps({'stop_cases':list(cases),'prune_cases':['events','runtime_errors','audit','idempotency','quota_hits','build_logs']}))
print('running-database stop-cap cases passed: '+', '.join(cases), flush=True)
