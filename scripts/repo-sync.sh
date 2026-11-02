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
  GE_URL="https://EasySSHCN:${GITEE}@gitee.com/easyssh/EasySSH.git"
  echo '==> push Gitee'
  git push -f "$GE_URL" HEAD:refs/heads/main | tail -1 || echo '!! Gitee main 失败'
  git push -f "$GE_URL" HEAD:refs/heads/v4-dev | tail -1 || echo '!! Gitee v4-dev 失败'
else
  echo '⚠ 未配置 Gitee token(/opt/dsh/home/secrets/.giteetoken)，跳过 Gitee'
fi
echo "✔ v$VER 仓库同步完成(GitHub main+v4-dev+tag v$VER)"
