#!/usr/bin/env bash
set -euo pipefail
cd /opt/dsh/workspace/easyssh-site
node tools/sync.js 2>&1 | tail -5
python3 - <<'EOF'
import json
d = json.load(open('/www/wwwroot/easyssh.cn/data/site-data.json'))
for c in d['changelog'][:4]:
    print(c['version'], c['date'], 'notes:', len(c['notes']))
EOF
