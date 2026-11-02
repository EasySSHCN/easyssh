#!/usr/bin/env bash
# repo-sync.sh —— 从开发机交付的 git bundle 同步仓库到 GitHub + Gitee
#
# 用法: bash tools/repo-sync.sh <bundle文件> <版本号>
# 数据流: 开发机 commit → git bundle → 本脚本 clone → push GitHub(EasySSHCN token) + Gitee(token)
set -euo pipefail
BUNDLE="${1:?用法: repo-sync.sh <bundle文件> <版本号>}"
VER="${2:?用法: repo-sync.sh <bundle文件> <版本号>}"
[ -f "$BUNDLE" ] || { echo "!! bundle 不存在: $BUNDLE"; exit 1; }
TOKEN=$(cat /opt/dsh/home/secrets/.gittoken)
GITEE=$(cat /opt/dsh/home/secrets/.giteetoken 2>/dev/null || true)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

git clone -q "$BUNDLE" "$WORK/pub"
cd "$WORK/pub"

GH_URL="https://EasySSHCN:${TOKEN}@github.com/EasySSHCN/easyssh.git"
echo '==> push GitHub'
git push -f "$GH_URL" HEAD:refs/heads/main | tail -1
git push -f "$GH_URL" HEAD:refs/heads/v4-dev | tail -1
git tag -f "v$VER"
git push -f "$GH_URL" "refs/tags/v$VER" | tail -1

if [ -n "$GITEE" ]; then
  # Gitee 私人令牌认证：用户名=Gitee 账号(easyssh)，密码=私人令牌
  GE_URL="https://easyssh:${GITEE}@gitee.com/easyssh/EasySSH.git"
  echo '==> push Gitee'
  git push -f "$GE_URL" HEAD:refs/heads/main | tail -1 || echo '!! Gitee main 失败'
  git push -f "$GE_URL" HEAD:refs/heads/v4-dev | tail -1 || echo '!! Gitee v4-dev 失败'
  git tag -f "v$VER"
  git push -f "$GE_URL" "refs/tags/v$VER" | tail -1 || echo '!! Gitee tag 失败'
else
  echo '⚠ 未配置 Gitee token(/opt/dsh/home/secrets/.giteetoken)，跳过 Gitee'
fi
echo "✔ v$VER git 同步完成(GitHub main+v4-dev+tag)"

# ================= Release 对象创建（幂等：已存在自动跳过） =================
# 附件来源优先级：incoming/v<ver>/（本轮交付）→ site/downloads/（已上站产物）
ART_IN="/opt/dsh/workspace/incoming/v$VER"
ART_DL="/opt/dsh/workspace/easyssh-site/site/downloads"
README=/opt/dsh/workspace/EasySSH/README.md
NOTES_RAW=$(awk -v ver="### v$VER " 'index($0,ver)==1{f=1;next} f&&/^### /{exit} f{print}' "$README")
[ -n "$NOTES_RAW" ] || NOTES_RAW="$VER 版本更新内容见官网 https://easyssh.cn/changelog.html"
HEAD_LINE="## EasySSH v$VER

**官方网站（下载 / 在线升级 / 文档）：https://EasySSH.CN**

### 本版更新
"
BODY="$HEAD_LINE$NOTES_RAW"
ATTACH_DIR="$ART_IN"
[ -d "$ATTACH_DIR" ] || ATTACH_DIR="$ART_DL"
# 附件清单：exe / update.exe / portable.exe / zip（有则挂）
ATT_FILES=()
for pat in "EasySSH-$VER.exe" "EasySSH-$VER-update.exe" "EasySSH-$VER-portable.exe" "EasySSH-$VER-win32-x64-portable.zip"; do
  f=$(ls "$ATTACH_DIR"/$pat 2>/dev/null | head -1) || true
  [ -n "$f" ] && ATT_FILES+=("$f")
done

echo "==> GitHub Release v$VER"
EX=$(curl -s "https://api.github.com/repos/EasySSHCN/easyssh/releases/tags/v$VER" | grep -o '"id": *[0-9]*' | head -1 || true)
if [ -n "$EX" ]; then
  echo '    已存在,跳过'
else
  REL=$(mktemp)
  curl -s -X POST -H "Authorization: token $TOKEN" -H "Accept: application/vnd.github+json" \
    https://api.github.com/repos/EasySSHCN/easyssh/releases \
    -d "{\"tag_name\":\"v$VER\",\"name\":\"EasySSH v$VER\",\"body\":$(python3 -c "import json,sys;print(json.dumps(sys.stdin.read()))" <<< "$BODY")}" > "$REL"
  REL_ID=$(python3 -c "import json;print(json.load(open('$REL')).get('id',''))")
  [ -n "$REL_ID" ] || { echo '!! GitHub Release 创建失败:'; cat "$REL"; }
  rm -f "$REL"
  if [ -n "${REL_ID:-}" ]; then
    for f in "${ATT_FILES[@]:-}"; do
      [ -f "$f" ] || continue
      curl -s -X POST -H "Authorization: token $TOKEN" -H "Content-Type: application/octet-stream" \
        --data-binary @"$f" \
        "https://uploads.github.com/repos/EasySSHCN/easyssh/releases/$REL_ID/assets?name=$(basename "$f")" \
        | python3 -c "import json,sys;d=json.load(sys.stdin);print('    GitHub 附件', d.get('name'),'=',d.get('state') or d.get('message'))"
    done
  fi
fi

if [ -n "$GITEE" ]; then
  echo "==> Gitee Release v$VER"
  GID=$(curl -s "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/tags/v$VER?access_token=$GITEE" | python3 -c "import json,sys
try:
  d=json.load(sys.stdin)
  print(d.get('id','') if isinstance(d,dict) else '')
except Exception:
  print('')" || true)
  if [ -n "$GID" ]; then
    echo '    已存在,跳过'
  else
    GREL=$(mktemp)
    curl -s -X POST -H "Content-Type: application/json" \
      "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases" \
      -d "{\"access_token\":\"$GITEE\",\"tag_name\":\"v$VER\",\"target_commitish\":\"main\",\"name\":\"EasySSH v$VER\",\"body\":$(python3 -c "import json,sys;print(json.dumps(sys.stdin.read()))" <<< "$BODY"),\"prerelease\":false}" > "$GREL"
    GID=$(python3 -c "import json;d=json.load(open('$GREL'));print(d.get('id','') or d.get('original_id','') if isinstance(d,dict) else '')")
    [ -n "$GID" ] || { echo '!! Gitee Release 创建失败:'; cat "$GREL"; }
    rm -f "$GREL"
  fi
  if [ -n "$GID" ]; then
    for f in "${ATT_FILES[@]:-}"; do
      [ -f "$f" ] || continue
      curl -s -X POST "https://gitee.com/api/v5/repos/easyssh/EasySSH/releases/$GID/attach_files?access_token=$GITEE" \
        -F "file=@$f" | python3 -c "import json,sys;d=json.load(sys.stdin);print('    Gitee 附件', d.get('name') or d.get('message'),'=', d.get('browser_download_url','')[:60])" | head -1
    done
  fi
fi
echo "✔ v$VER 仓库同步+Release 完成"
