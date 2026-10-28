#!/usr/bin/env bash
# EasySSH 一键发布脚本：语法检查 → 构建 → zip → git 提交/打 tag/push → GitHub Release 同步发布
# 用法: GITHUB_TOKEN=ghp_xxx bash scripts/release.sh "本次版本说明(可选,默认取 README 最新条目)"
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(node -p "require('./package.json').version")
TAG="v$VERSION"
REPO="EasySSHCN/easyssh"
EXE="release/EasySSH-$VERSION.exe"
ZIP="dist/EasySSH-$VERSION-win32-x64-portable.zip"

echo "==> 1/6 语法检查"
node --check main.js && node --check preload.js && node --check renderer.js

echo "==> 2/6 构建单文件 exe"
npm run build:exe
[ -f "$EXE" ] || { echo "缺少 $EXE"; exit 1; }

echo "==> 3/6 构建文件夹版 + zip"
npm run package:win
[ -d "dist/EasySSH-win32-x64" ] && rm -rf "dist/EasySSH-$VERSION-win32-x64" && mv "dist/EasySSH-win32-x64" "dist/EasySSH-$VERSION-win32-x64"
rm -f "$ZIP"
(cd dist && zip -qr "EasySSH-$VERSION-win32-x64-portable.zip" "EasySSH-$VERSION-win32-x64")
[ -f "$ZIP" ] || { echo "缺少 $ZIP"; exit 1; }

echo "==> 4/6 git 提交 + tag + push"
git add -A
git commit -m "EasySSH $TAG 发布" || echo "(无变更可提交)"
git tag -f "$TAG"
git push origin HEAD || echo "⚠ git push 失败(检查凭据),本地发布已完成"
git push origin "$TAG" || echo "⚠ tag push 失败"

echo "==> 5/6 生成 Release 说明(取 README 最新版本条目)"
NOTES=$(awk "/^### $TAG /{f=1;next} /^### /{f=0} f" README.md)
[ -n "${NOTES// }" ] || NOTES="EasySSH $VERSION"
[ -n "${1:-}" ] && NOTES="$1

$NOTES"
printf '%s\n' "$NOTES" > /tmp/easyssh-release-notes.md
# 正文加中文文档头(官网地址醒目)
{ printf '## EasySSH %s\n\n**官方网站（下载 / 在线升级 / 文档）：https://EasySSH.CN**\n\n### 本版更新\n\n' "$VERSION"; cat /tmp/easyssh-release-notes.md; } > /tmp/easyssh-release-notes.md.tmp && mv /tmp/easyssh-release-notes.md.tmp /tmp/easyssh-release-notes.md

echo "==> 6/6 GitHub Release 同步发布"
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
      upload_asset "$UPLOAD_BASE?name=$(basename "$EXE")" "$EXE"
      upload_asset "$UPLOAD_BASE?name=$(basename "$ZIP")" "$ZIP"
      echo "GitHub Release $TAG 已创建: https://github.com/$REPO/releases/tag/$TAG"
    else
      echo "⚠ Release 可能已存在或创建失败，请到 GitHub 检查 $TAG"
    fi
  fi
else
  command -v gh >/dev/null && gh release create "$TAG" "$EXE" "$ZIP" \
    --repo "$REPO" --title "EasySSH $VERSION" --notes-file /tmp/easyssh-release-notes.md \
    || echo "⚠ 未设置 GITHUB_TOKEN 且无 gh CLI，跳过 GitHub Release（本地发布已完成）"
fi
echo "✅ 发布流程结束: $TAG"
