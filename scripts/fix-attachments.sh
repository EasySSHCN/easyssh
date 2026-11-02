#!/usr/bin/env bash
# fix-attachments.sh —— 补齐 v<VER> Release 的缺失附件（幂等，可反复运行直到齐）
# 用法: fix-attachments.sh <版本号>   附件源: incoming/v<ver>/ 优先,回退 site/downloads/
set -uo pipefail
VER="${1:?用法: fix-attachments.sh <版本号>}"
GT=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken 2>/dev/null || true)
IN="/opt/dsh/workspace/incoming/v$VER"
DL="/opt/dsh/workspace/easyssh-site/site/downloads"
FILES=()
for pat in "EasySSH-$VER.exe" "EasySSH-$VER-update.exe" "EasySSH-$VER-portable.exe" "EasySSH-$VER-win32-x64-portable.zip"; do
  f=$(ls "$IN"/$pat 2>/dev/null | head -1)
  [ -z "$f" ] && f=$(ls "$DL"/$pat 2>/dev/null | head -1)
  [ -n "$f" ] && FILES+=("$f")
done
[ ${#FILES[@]} -eq 4 ] || { echo "!! 附件源不足(${#FILES[@]}/4)"; exit 1; }

GH_REL_JSON=$(curl -s --max-time 60 "https://api.github.com/repos/EasySSHCN/easyssh/releases/tags/v$VER")
GH_ID=$(printf '%s' "$GH_REL_JSON" | python3 -c "import json,sys
try: print(json.load(sys.stdin).get('id',''))
except Exception: print('')")
GH_HAVE=$(printf '%s' "$GH_REL_JSON" | python3 -c "import json,sys
try: print('\n'.join(a['name'] for a in json.load(sys.stdin).get('assets',[])))
except Exception: pass")

GE_REL_JSON=$(curl -s --max-time 60 "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/tags/v$VER?access_token=$GITEE")
GE_ID=$(printf '%s' "$GE_REL_JSON" | python3 -c "import json,sys
try:
  d=json.load(sys.stdin); print(d.get('id','') or d.get('original_id',''))
except Exception: print('')")
GE_HAVE=$(printf '%s' "$GE_REL_JSON" | python3 -c "import json,sys
try: print('\n'.join(a['name'] for a in json.load(sys.stdin).get('assets',[])))
except Exception: pass")
echo "GitHub id=$GH_ID assets=[$(echo $GH_HAVE | tr '\n' ' ')]"
echo "Gitee  id=$GE_ID assets=[$(echo $GE_HAVE | tr '\n' ' ')]"

MISS_GH=0; MISS_GE=0
for f in "${FILES[@]}"; do
  n=$(basename "$f")
  if [ -n "$GH_ID" ] && ! grep -qx "$n" <<<"$GH_HAVE"; then
    echo "GH 上传 $n ..."
    curl -s --max-time 900 -X POST -H "Authorization: token $GT" -H "Content-Type: application/octet-stream" \
      --data-binary @"$f" \
      "https://uploads.github.com/repos/EasySSHCN/easyssh/releases/$GH_ID/assets?name=$n" \
      | python3 -c "import json,sys
try:
  d=json.load(sys.stdin); print('   GH:', d.get('name'), d.get('state') or d.get('message'))
except Exception: print('   GH: 非JSON响应(可能网络中断)')" || true
    grep -qx "$n" <<<"$GH_HAVE" || MISS_GH=1
  fi
  if [ -n "$GE_ID" ] && ! grep -qx "$n" <<<"$GE_HAVE"; then
    echo "Gitee 上传 $n ..."
    curl -s --max-time 900 -X POST "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$GE_ID/attach_files?access_token=$GITEE" \
      -F "file=@$f" \
      | python3 -c "import json,sys
try:
  d=json.load(sys.stdin); print('   GE:', d.get('name') or d.get('message'))
except Exception: print('   GE: 非JSON响应(可能网络中断)')" || true
    grep -qx "$n" <<<"$GE_HAVE" || MISS_GE=1
  fi
done
echo "RESULT: MISS_GH=$MISS_GH MISS_GE=$MISS_GE"
