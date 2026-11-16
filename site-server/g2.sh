#!/bin/bash
GITEE=d4774aa35e89075a9427c36b4e9a8736
curl -s --max-time 1800 --retry 1 -X POST \
  "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/1306215/attach_files?access_token=$GITEE" \
  -F "file=@/opt/dsh/workspace/incoming/v5.1.3/EasySSH-5.1.3-portable.exe" \
  | python3 -c "import json,sys
try:
    d=json.load(sys.stdin); print('结果:', d.get('name') or d.get('message'), d.get('browser_download_url',''))
except Exception as e:
    print('解析失败:', e); sys.exit(1)"
echo '--- 最终附件清单:'
curl -s --max-time 30 "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/1306215/attach_files?access_token=$GITEE" | python3 -c "import json,sys;d=json.load(sys.stdin);[print(' ', a.get('name')) for a in d]"
