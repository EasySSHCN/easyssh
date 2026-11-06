#!/usr/bin/env bash
# replace-release-assets.sh —— 同版本号重发时替换 GitHub/Gitee Release 附件
# 背景:repo-sync.sh 按附件名去重,同版本重构建后旧附件不会被覆盖,须删除后重挂。
# 用法:在发布机(la)执行:bash replace-release-assets.sh   (VER 在下方修改)
set -euo pipefail
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken)
VER=5.0.8
DIR=/opt/dsh/workspace/incoming/v$VER
[ -d "$DIR" ] || DIR=/opt/dsh/workspace/easyssh-site/site/downloads
PATS=("EasySSH-$VER.exe" "EasySSH-$VER-portable.exe" "EasySSH-$VER-win32-x64-portable.zip")
FILES=()
for p in "${PATS[@]}"; do f=$(ls "$DIR"/$p 2>/dev/null | head -1); [ -n "$f" ] && FILES+=("$f"); done
echo "待重传附件: ${FILES[*]}"

echo '==> GitHub: 删除旧附件'
GH_REL=$(curl -s --max-time 60 "https://api.github.com/repos/EasySSHCN/easyssh/releases/tags/v$VER")
GH_ID=$(printf '%s' "$GH_REL" | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
[ -n "$GH_ID" ] || { echo '!! 取 GitHub Release id 失败'; exit 1; }
printf '%s' "$GH_REL" | python3 -c "
import json,sys
d=json.load(sys.stdin)
for a in d.get('assets',[]):
    if a.get('name','').startswith('EasySSH-$VER'):
        print(a['id'], a['name'])" | while read -r aid aname; do
  curl -s --max-time 60 -X DELETE -H "Authorization: token $TOKEN" \
    "https://api.github.com/repos/EasySSHCN/easyssh/releases/assets/$aid" -o /dev/null
  echo "    已删除 $aname"
done
echo '==> GitHub: 上传新附件'
for f in "${FILES[@]}"; do
  n=$(basename "$f")
  curl -s --max-time 600 -X POST -H "Authorization: token $TOKEN" -H "Content-Type: application/octet-stream" \
    --data-binary @"$f" \
    "https://uploads.github.com/repos/EasySSHCN/easyssh/releases/$GH_ID/assets?name=$n" \
    | python3 -c "import json,sys;d=json.load(sys.stdin);print('    GitHub 附件', d.get('name'),'=',d.get('state') or d.get('message'))"
done

echo '==> GitHub: 同版本重发时同步 Release 说明(README 最新内容)'
NOTES_RAW=$(awk -v ver="### v$VER " 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' /opt/dsh/workspace/EasySSH/README.md)
[ -n "$NOTES_RAW" ] && export VER GH_ID TOKEN && printf '%s' "$NOTES_RAW" | python3 -c "
import json,sys,os
body = '## EasySSH v' + os.environ['VER'] + '\n\n**官方网站（下载 / 在线升级 / 文档）：https://EasySSH.CN**\n\n### 本版更新\n' + sys.stdin.read()
payload = json.dumps({'body': body}).encode()
req = urllib.request.Request('https://api.github.com/repos/EasySSHCN/easyssh/releases/' + os.environ['GH_ID'],
                             data=payload, method='PATCH',
                             headers={'Content-Type': 'application/json',
                                      'Authorization': 'token ' + os.environ['TOKEN'],
                                      'User-Agent': 'easyssh-release'})
print('    GitHub 说明已更新:', urllib.request.urlopen(req, timeout=60).status)
" 

echo '==> Gitee: 附件无 id 不可单独删,整体删除 Release 后重建并挂新附件'
OLD_GE=$(curl -s "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/tags/v$VER?access_token=$GITEE" | python3 -c "import json,sys;d=json.load(sys.stdin);print(d.get('id','') if isinstance(d,dict) else '')")
if [ -n "$OLD_GE" ]; then
  curl -s --max-time 60 -X DELETE "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$OLD_GE?access_token=$GITEE" -o /dev/null
  echo "    已删除 release $OLD_GE"
fi
README=/opt/dsh/workspace/EasySSH/README.md
NOTES_RAW=$(awk -v ver="### v$VER " 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' "$README")
[ -n "$NOTES_RAW" ] || NOTES_RAW="$VER 版本更新内容见官网 https://easyssh.cn/changelog.html"
BODY="## EasySSH v$VER

**官方网站（下载 / 在线升级 / 文档）：https://EasySSH.CN**

### 本版更新
$NOTES_RAW"
BODY_FILE=$(mktemp); printf '%s' "$BODY" > "$BODY_FILE"
export VER
NEW=$(python3 - "$BODY_FILE" <<'PYEOF'
import json, sys, os, urllib.request
body = open(sys.argv[1], encoding='utf-8').read()
token = open('/opt/dsh/home/secrets/.giteetoken').read().strip()
ver = os.environ['VER']
payload = json.dumps({"access_token": token, "tag_name": "v" + ver, "target_commitish": "main",
                      "name": "EasySSH v" + ver, "body": body, "prerelease": False}).encode()
req = urllib.request.Request("https://gitee.com/api/v5/repos/easyssh/EasySSH/releases", data=payload,
                             headers={"Content-Type": "application/json"})
print(urllib.request.urlopen(req, timeout=60).read().decode())
PYEOF
) | python3 -c "import json,sys;d=json.load(sys.stdin);print(d.get('id') or d.get('original_id',''))"
rm -f "$BODY_FILE"
[ -n "$NEW" ] || { echo '!! Gitee Release 重建失败'; exit 1; }
echo "    新 release id: $NEW"
for f in "${FILES[@]}"; do
  curl -s --max-time 600 -X POST \
    "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$NEW/attach_files?access_token=$GITEE" \
    -F "file=@$f" | python3 -c "import json,sys;d=json.load(sys.stdin);print('    Gitee 附件', d.get('name') or d.get('message'))" | head -1
done
echo '==> 完成'
