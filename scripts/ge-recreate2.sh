#!/usr/bin/env bash
# ge-recreate2.sh —— Gitee v5.0.8 Release 重建并挂最新附件
set -euo pipefail
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
VER=5.0.8
DIR=/opt/dsh/workspace/incoming/v$VER
[ -d "$DIR" ] || DIR=/opt/dsh/workspace/easyssh-site/site/downloads
README=/opt/dsh/workspace/EasySSH/README.md
NOTES_RAW=$(awk -v ver="### v$VER " 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' "$README")
BODY="## EasySSH v$VER

**官方网站（下载 / 在线升级 / 文档）：https://EasySSH.CN**

### 本版更新
$NOTES_RAW"

BODY_FILE=$(mktemp); printf '%s' "$BODY" > "$BODY_FILE"

echo '==> 删除现有 Release(若有)'
OLD=$(curl -s --max-time 60 "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/tags/v$VER?access_token=$GITEE" | python3 -c "import json,sys;d=json.load(sys.stdin);print(d.get('id','') if isinstance(d,dict) else '')")
if [ -n "$OLD" ]; then
  curl -s --max-time 60 -X DELETE "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$OLD?access_token=$GITEE" -o /dev/null
  echo "    已删除 release $OLD"
fi

echo '==> 重建 Release'
python3 - "$BODY_FILE" > /tmp/ge-new.json <<'PYEOF'
import json, sys, urllib.request
body = open(sys.argv[1], encoding='utf-8').read()
token = open('/opt/dsh/home/secrets/.giteetoken').read().strip()
payload = json.dumps({"access_token": token, "tag_name": "v5.0.8", "target_commitish": "main",
                      "name": "EasySSH v5.0.8", "body": body, "prerelease": False}).encode()
req = urllib.request.Request("https://gitee.com/api/v5/repos/easyssh/EasySSH/releases", data=payload,
                             headers={"Content-Type": "application/json"})
print(urllib.request.urlopen(req, timeout=60).read().decode())
PYEOF
GID=$(python3 -c "import json;d=json.load(open('/tmp/ge-new.json'));print(d.get('id') or d.get('original_id',''))")
[ -n "$GID" ] || { echo '!! 重建失败:'; head -c 300 /tmp/ge-new.json; exit 1; }
echo "    新 release id: $GID"

echo '==> 挂新附件'
for pat in "EasySSH-$VER.exe" "EasySSH-$VER-portable.exe" "EasySSH-$VER-win32-x64-portable.zip"; do
  f=$(ls "$DIR"/$pat 2>/dev/null | head -1); [ -n "$f" ] || continue
  curl -s --max-time 600 -X POST \
    "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$GID/attach_files?access_token=$GITEE" \
    -F "file=@$f" | python3 -c "import json,sys;d=json.load(sys.stdin);print('    附件', d.get('name') or d.get('message'))" | head -1
done
echo '==> 附件清单'
curl -s --max-time 60 "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$GID?access_token=$GITEE" | python3 -c "
import json,sys
d=json.load(sys.stdin)
for a in d.get('assets',[]):
    print(' -', a.get('name'))"
rm -f /tmp/ge-new.json "$BODY_FILE"
echo '完成'
