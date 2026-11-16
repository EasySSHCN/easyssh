#!/bin/bash
set -u
GITEE=d4774aa35e89075a9427c36b4e9a8736
REL=1306215
DIR=/opt/dsh/workspace/incoming/v5.1.3
# 已有附件清单
echo '--- Gitee 现有附件:'
curl -s --max-time 30 "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$REL/attach_files?access_token=$GITEE" | python3 -c "import json,sys;d=json.load(sys.stdin);[print(' ',a.get('name')) for a in d]" || true
for f in EasySSH-5.1.3-portable.exe EasySSH-5.1.3-win32-x64-portable.zip; do
  echo "--- 上传 $f:"
  curl -s --max-time 1800 --retry 2 --retry-delay 10 -X POST \
    "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$REL/attach_files?access_token=$GITEE" \
    -F "file=@$DIR/$f" | python3 -c "import json,sys;d=json.load(sys.stdin);print('  结果:', d.get('name') or d.get('message'), d.get('browser_download_url',''))"
done
echo '--- 复查附件清单:'
curl -s --max-time 30 "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$REL/attach_files?access_token=$GITEE" | python3 -c "import json,sys;d=json.load(sys.stdin);[print(' ',a.get('name')) for a in d]"
