#!/usr/bin/env bash
# replace-release-assets.sh —— 重传 v5.0.8 Release 附件(横幅修复重构建,旧附件被按名去重跳过)
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
    "https://api.github.com/repos/EasySSHCN/easyssh/releases/assets/$aid" -o /dev/null -w ''
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

echo '==> Gitee: 删除旧附件并重传'
GID=$(curl -s "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/tags/v$VER?access_token=$GITEE" | python3 -c "import json,sys;print(json.load(sys.stdin).get('id',''))")
[ -n "$GID" ] || { echo '!! 取 Gitee Release id 失败'; exit 1; }
curl -s --max-time 60 "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$GID?access_token=$GITEE" | python3 -c "
import json,sys
d=json.load(sys.stdin)
for a in d.get('assets',[]):
    if a.get('name','').startswith('EasySSH-$VER'):
        print(a['id'], a['name'])" | while read -r aid aname; do
  curl -s --max-time 60 -X DELETE \
    "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$GID/attach_files/$aid?access_token=$GITEE" -o /dev/null
  echo "    已删除 $aname"
done
for f in "${FILES[@]}"; do
  curl -s --max-time 600 -X POST \
    "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$GID/attach_files?access_token=$GITEE" \
    -F "file=@$f" | python3 -c "import json,sys;d=json.load(sys.stdin);print('    Gitee 附件', d.get('name') or d.get('message'),'=', d.get('browser_download_url','')[:60])" | head -1
done
echo '✔ v$VER 附件替换完成'
