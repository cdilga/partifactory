#!/usr/bin/env bash
# Fail if shipped compose/example variables are absent from the generated contract.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 - <<'PY'
import pathlib, re, sys
docs = pathlib.Path('docs/env.md').read_text()
paths = [pathlib.Path('deploy/compose.yaml'), pathlib.Path('deploy/.env.example')]
variables = set()
for path in paths:
    variables.update(re.findall(r'\bPF_[A-Z0-9_]+\b', path.read_text()))
missing = sorted(v for v in variables if f'`{v}`' not in docs and f'`{v} /' not in docs)
if missing:
    sys.exit('env contract missing: ' + ', '.join(missing))
print(f'env contract: {len(variables)} compose/example variables documented')
PY
