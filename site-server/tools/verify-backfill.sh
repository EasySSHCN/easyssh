#!/usr/bin/env bash
python3 - <<'EOF'
import json
d = json.load(open('/www/wwwroot/data/stats-db.json'))
for i, c in d['clients'].items():
    f = c.get('fields', {})
    print(i, '=>', f.get('地域'), '|', f.get('系统语言代码'))
EOF
echo "admin.html LANG_ZH_LCID: $(grep -c LANG_ZH_LCID /www/wwwroot/easyssh.cn/admin.html)"
chown www:www /www/wwwroot/data/stats-db.json
chmod 664 /www/wwwroot/data/stats-db.json
ls -l /www/wwwroot/data/stats-db.json
