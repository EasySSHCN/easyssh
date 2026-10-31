#!/usr/bin/env bash
# EasySSH 一键发布脚本：语法检查 → 构建 → zip → git 提交/打 tag/push → GitHub Release 同步发布 → Gitee Release 同步发布
# 用法: GITHUB_TOKEN=ghp_xxx [GITEE_TOKEN=xxx] bash scripts/release.sh "本次版本说明(可选,默认取 README 最新条目)"
# Gitee 凭据:未设置 GITEE_TOKEN 时自动从加密库 /opt/dsh/.gitee-vault 解密读取(AES-256-CBC,密钥同目录 key 文件)
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(node -p "require('./package.json').version")
TAG="v$VERSION"
REPO="EasySSHCN/easyssh"
EXE="release/EasySSH-$VERSION.exe"
ZIP="dist/EasySSH-$VERSION-win32-x64-portable.zip"

echo "==> 1/6 语法检查"
node --check main.js && node --check preload.js && node --check renderer.js

echo "==> 2/6 构建绿色 zip 包（v3.0.7 起取消单文件 exe，仅保留 zip）"
npm run package:win
[ -d "dist/EasySSH-win32-x64" ] && rm -rf "dist/EasySSH-$VERSION-win32-x64" && mv "dist/EasySSH-win32-x64" "dist/EasySSH-$VERSION-win32-x64"
# v3.0.8 zip 瘦身:仅保留界面语言语言包,移除 Chromium 许可证全文与软件 Vulkan 回退(详见 DEVELOPMENT.md)
PRUNE_DIR="dist/EasySSH-$VERSION-win32-x64"
find "$PRUNE_DIR/locales" -type f ! -name 'en-US.pak' ! -name 'zh-CN.pak' ! -name 'zh-TW.pak' ! -name 'ko.pak' -delete
rm -f "$PRUNE_DIR/LICENSES.chromium.html" "$PRUNE_DIR/vk_swiftshader.dll" "$PRUNE_DIR/vk_swiftshader_icd.json" "$PRUNE_DIR/vulkan-1.dll"
rm -f "$ZIP"
(cd dist && zip -qr "EasySSH-$VERSION-win32-x64-portable.zip" "EasySSH-$VERSION-win32-x64")
[ -f "$ZIP" ] || { echo "缺少 $ZIP"; exit 1; }

echo "==> 3/6 准备在线升级载荷（应用 exe 仅用于官网加密升级通道，不对外公开下载）"
rm -f "$EXE"
cp "dist/EasySSH-$VERSION-win32-x64/EasySSH.exe" "$EXE"
[ -f "$EXE" ] || { echo "缺少 $EXE"; exit 1; }

echo "==> 4/7 git 提交 + tag + push"
git add -A
git commit -m "EasySSH $TAG 发布" || echo "(无变更可提交)"
git tag -f "$TAG"
# push 凭据:优先用加密库 token,否则回退环境/凭据管理器
ASKPASS=$(mktemp)
if [ -f /opt/dsh/home/secrets/.gittoken ]; then printf '#!/bin/sh\ncat /opt/dsh/home/secrets/.gittoken\n' > "$ASKPASS"; chmod 700 "$ASKPASS"; fi
[ -n "${GITHUB_TOKEN:-}" ] && printf '#!/bin/sh\necho "$GITHUB_TOKEN"\n' > "$ASKPASS" && chmod 700 "$ASKPASS"
GIT_ASKPASS="$ASKPASS" GIT_TERMINAL_PROMPT=0 git push origin main --tags || echo "⚠ git push 失败(检查凭据)"
rm -f "$ASKPASS"
# 同步推送到 Gitee 镜像(remote gitee 已存在则复用)
if git remote get-url gitee >/dev/null 2>&1; then
  GITEE_URL=$(git remote get-url gitee)
  case "$GITEE_URL" in
    https://gitee.com/*) ;; # 无凭据的干净 URL
    *) GITEE_URL="https://gitee.com/easyssh/EasySSH.git";;
  esac
  if [ -n "${GITEE_TOKEN:-}" ]; then
    GITEE_PUSH_URL="https://easyssh:${GITEE_TOKEN}@gitee.com/easyssh/EasySSH.git"
  elif [ -f /opt/dsh/.gitee-vault/creds.enc ]; then
    GITEE_TOKEN_AUTO=$(openssl enc -aes-256-cbc -d -pbkdf2 -in /opt/dsh/.gitee-vault/creds.enc -pass file:/opt/dsh/.gitee-vault/key 2>/dev/null | sed -n 's/^api_token: //p')
    [ -n "$GITEE_TOKEN_AUTO" ] && GITEE_PUSH_URL="https://easyssh:${GITEE_TOKEN_AUTO}@gitee.com/easyssh/EasySSH.git"
  fi
  GIT_ASKPASS="$ASKPASS" GIT_TERMINAL_PROMPT=0 git push "${GITEE_PUSH_URL:-$GITEE_URL}" main --tags || echo "⚠ Gitee push 失败(检查凭据)"
else
  echo "⚠ 未配置 gitee 远程,跳过 Gitee 代码推送"
fi

echo "==> 5/7 生成 Release 说明(取 README 最新版本条目)"
NOTES=$(awk "/^### $TAG /{f=1;next} /^### /{f=0} f" README.md)
[ -n "${NOTES// }" ] || NOTES="EasySSH $VERSION"
[ -n "${1:-}" ] && NOTES="$1

$NOTES"
printf '%s\n' "$NOTES" > /tmp/easyssh-release-notes.md
# 正文加中文文档头(官网地址醒目)
{ printf '## EasySSH %s\n\n**官方网站（下载 / 在线升级 / 文档）：https://EasySSH.CN**\n\n### 本版更新\n\n' "$VERSION"; cat /tmp/easyssh-release-notes.md; } > /tmp/easyssh-release-notes.md.tmp && mv /tmp/easyssh-release-notes.md.tmp /tmp/easyssh-release-notes.md

echo "==> 6/7 GitHub Release 同步发布"
upload_asset() { # url file
  curl -sf -X POST -H "Authorization: Bearer $GITHUB_TOKEN" \
    -H "Content-Type: application/octet-stream" \
    --data-binary @"$2" "$1" >/dev/null && echo "  上传完成: $(basename "$2")" \
    || echo "  ⚠ 上传失败: $(basename "$2")"
}
if [ -n "${GITHUB_TOKEN:-}" ]; then
  RESP=$(curl -sf -X POST -H "Authorization: Bearer $GITHUB_TOKEN" \
    -H "Content-Type: application/json" \
    -d "{\"tag_name\":\"$TAG\",\"name\":\"EasySSH $VERSION\",\"body\":$(node -e "console.log(JSON.stringify(require('fs').readFileSync('/tmp/easyssh-release-notes.md','utf8')))"),"draft":false,"prerelease":false}" \
    "https://api.github.com/repos/$REPO/releases") || true
  if [ -n "${RESP:-}" ]; then
    UPLOAD_BASE=$(node -e "const r=JSON.parse(process.argv[1]);console.log(r.upload_url||'')" "$RESP" | sed 's/{.*}//')
    if [ -n "$UPLOAD_BASE" ]; then
      # v3.0.7 exe 仅作升级加密载荷，不对外发布
      upload_asset "$UPLOAD_BASE?name=$(basename "$ZIP")" "$ZIP"
      echo "GitHub Release $TAG 已创建: https://github.com/$REPO/releases/tag/$TAG"
    else
      echo "⚠ Release 可能已存在或创建失败，请到 GitHub 检查 $TAG"
    fi
  fi
else
  command -v gh >/dev/null && gh release create "$TAG" "$ZIP" \
    --repo "$REPO" --title "EasySSH $VERSION" --notes-file /tmp/easyssh-release-notes.md \
    || echo "⚠ 未设置 GITHUB_TOKEN 且无 gh CLI，跳过 GitHub Release（本地发布已完成）"
fi

echo "==> 7/7 Gitee Release 同步发布"
# 解析 Gitee token:环境变量 → 加密库
GITEE_T="${GITEE_TOKEN:-}"
if [ -z "$GITEE_T" ] && [ -f /opt/dsh/.gitee-vault/creds.enc ]; then
  GITEE_T=$(openssl enc -aes-256-cbc -d -pbkdf2 -in /opt/dsh/.gitee-vault/creds.enc -pass file:/opt/dsh/.gitee-vault/key 2>/dev/null | sed -n 's/^api_token: //p')
fi
GITEE_REPO="easyssh/EasySSH"
if [ -n "$GITEE_T" ]; then
  # Gitee 若已有同 tag Release 则先取其 id(避免重复创建)
  REL_ID=$(curl -sf "https://gitee.com/api/v5/repos/$GITEE_REPO/releases/tags/$TAG?access_token=$GITEE_T" \
    | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{try{console.log(JSON.parse(s).id||'')}catch{console.log('')}})") || REL_ID=""
  if [ -z "$REL_ID" ]; then
    REL_ID=$(curl -sf -X POST "https://gitee.com/api/v5/repos/$GITEE_REPO/releases" \
      -H "Content-Type: application/json" \
      -d "{\"access_token\":\"$GITEE_T\",\"tag_name\":\"$TAG\",\"name\":\"EasySSH $VERSION\",\"body\":$(node -e "console.log(JSON.stringify(require('fs').readFileSync('/tmp/easyssh-release-notes.md','utf8')))"),"prerelease":false,"target_commitish":"main"}" \
      | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{try{console.log(JSON.parse(s).id||'')}catch{console.log('')}})") || REL_ID=""
  fi
  if [ -n "$REL_ID" ]; then
    gitee_upload() { # file — 单附件上限 100MB,超出自动跳过并提示
      local f="$1" sz
      sz=$(stat -c%s "$f" 2>/dev/null || echo 0)
      if [ "$sz" -gt 104857600 ]; then
        echo "  ⏭ 跳过 $(basename "$f") ($(numfmt --to=iec $sz 2>/dev/null || echo ${sz}B) 超 Gitee 100MB 附件上限)"
        return 0
      fi
      # token 必须放查询串:放表单字段(-F access_token=)时 Gitee 会挂起无响应
      curl -sf --max-time 7200 -X POST "https://gitee.com/api/v5/repos/$GITEE_REPO/releases/$REL_ID/attach_files?access_token=$GITEE_T" \
        -F "file=@$f" >/dev/null \
        && echo "  上传完成: $(basename "$f")" \
        || echo "  ⚠ 上传失败: $(basename "$f")"
    }
    # v3.0.7 exe 仅作升级加密载荷，不对外发布
    gitee_upload "$ZIP"
    echo "Gitee Release $TAG 已发布: https://gitee.com/$GITEE_REPO/releases/tag/$TAG"
  else
    echo "⚠ Gitee Release 创建失败(检查 token 权限/是否已存在)，请到 Gitee 检查 $TAG"
  fi
else
  echo "⚠ 未设置 GITEE_TOKEN 且加密库不可读，跳过 Gitee Release"
fi

echo "==> 8/8 官网(easyssh.cn)同步上线"
# 权威源是 README(功能总览/使用/版本历史),sync.js 重新生成 site-data.json 并打包下载产物;
# deploy.sh 同步 site/ -> /www/wwwroot/easyssh.cn(无权限时由 root 定时任务每 5 分钟自动上线)
SITE_DIR=/opt/dsh/workspace/easyssh-site
if [ -d "$SITE_DIR" ] && command -v node >/dev/null; then
  (cd "$SITE_DIR" && node tools/sync.js) && echo "  官网数据已重新生成(site-data.json + downloads)" \
    || echo "  ⚠ sync.js 执行失败,请检查官网同步工具"
  if bash "$SITE_DIR/tools/deploy.sh" >/dev/null 2>&1; then
    echo "  官网已部署上线: https://easyssh.cn"
  else
    echo "  ℹ 无官网目录写权限,已由定时任务在 5 分钟内自动上线(或手动: sudo $SITE_DIR/tools/deploy.sh)"
  fi
  bash "$SITE_DIR/tools/healthcheck.sh" >/dev/null 2>&1 && echo "  官网自检: OK" || echo "  ℹ 官网自检未通过/未执行,请稍后复查 https://easyssh.cn"
else
  echo "⚠ 未找到官网目录($SITE_DIR)或无 node,跳过官网同步"
fi
echo "✅ 发布流程结束: $TAG"
