#!/usr/bin/env bash
# Validate existing optional records. Absence and reported failures never block release gates.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 - "${1:-$repo/tests/manual/results}" <<'PY'
import datetime,json,pathlib,re,sys
root=pathlib.Path(sys.argv[1])
files=sorted(root.glob('*.json')) if root.exists() else []
required={'sign_in','passkey_step_up','part_root_first_open','part_root_reload','standalone','guest_link','embedding','invite_acceptance','telegram_sign_in'}
for file in files:
    value=json.loads(file.read_text())
    assert set(value)=={'schema','recorded_at','device','os','browser','checks'},f'{file}: unexpected or missing fields'
    assert value['schema']==1,f'{file}: unsupported schema'
    timestamp=datetime.datetime.fromisoformat(value['recorded_at'].replace('Z','+00:00'))
    assert timestamp.tzinfo is not None,f'{file}: timestamp needs timezone'
    for key in ['device','os','browser']:
        assert isinstance(value[key],str) and 1<=len(value[key])<=100,f'{file}: invalid {key}'
        assert not re.search(r'https?://|[\r\n]|@|\b(?:\d{1,3}\.){3}\d{1,3}\b',value[key]),f'{file}: {key} must contain generic software/device versions only'
    assert set(value['checks'])==required,f'{file}: missing or unexpected checks'
    assert all(v in {'pass','fail','not_applicable'} for v in value['checks'].values()),f'{file}: invalid outcome'
    failures=sum(v=='fail' for v in value['checks'].values())
    print(f'{file.name}: valid; {failures} nonblocking reported failures')
print(f'manual records: {len(files)} existing files validated (none required)')
PY
