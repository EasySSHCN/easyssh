#!/usr/bin/env bash
# intake-release.sh —— 接收开发机交付的发布数据并完成官网发布（本机发布流程唯一入口）
#
# 用法: bash tools/intake-release.sh <交付目录>
#
# 交付目录必须包含（命名固定，版本号 <ver> 自动识别）:
#   EasySSH-<ver>.exe                            NSIS 单文件引导安装器
#   EasySSH-<ver>-update.exe                     裸 MSVC exe（在线升级加密载荷）
#   EasySSH-<ver>-portable.exe                   裸 MSVC exe（下载页便携版）
#   EasySSH-<ver>-win32-x64-portable.zip         便携 zip（内含 EasySSH.exe）
#   README.md                                    EasySSH 仓库 README（已含 ### v<ver> 更新历史小节）
# 可选:
#   features.html                                官网功能总览页（若本版功能有变化）
#
# 执行: 落位产物 → 换 README → 功能页检查 → sync.js → 自检 → 校验 latestVersion
set -euo pipefail
IN="${1:?用法: intake-release.sh <交付目录>}"
[ -d "$IN" ] || { echo "!! 交付目录不存在: $IN"; exit 1; }

SITE=/opt/dsh/workspace/easyssh-site
APP=/opt/dsh/workspace/EasySSH

EXE=$(ls "$IN"/EasySSH-*.exe 2>/dev/null | grep -v -- -update.exe | grep -v -- -portable.exe | head -1) || true
UPD=$(ls "$IN"/EasySSH-*-update.exe 2>/dev/null | head -1) || true
PORT=$(ls "$IN"/EasySSH-*-portable.exe 2>/dev/null | head -1) || true
ZIPF=$(ls "$IN"/EasySSH-*-win32-x64-portable.zip 2>/dev/null | head -1) || true
VER=$(basename "${UPD:-${EXE:-}}" | grep -oP 'EasySSH-\K[0-9]+(\.[0-9]+)*' || true)
[ -n "$VER" ] || { echo "!! 无法从交付文件名识别版本号"; exit 1; }
echo "==> 版本 v$VER"

for f in "$EXE" "$UPD" "$PORT" "$ZIPF" "$IN/README.md"; do
  [ -f "$f" ] || { echo "!! 缺少交付文件: $f"; exit 1; }
done
grep -q "^### v$VER " "$IN/README.md" || { echo "!! README.md 缺少 ### v$VER 更新历史小节（官网 changelog/升级 notes 的权威源）"; exit 1; }

echo "==> [1/5] 落位产物"
cp -f "$EXE"  "$APP/release/EasySSH-$VER.exe"
cp -f "$UPD"  "$APP/dist/EasySSH-$VER-update.exe"
cp -f "$PORT" "$APP/dist/EasySSH-$VER-portable.exe"
cp -f "$ZIPF" "$APP/dist/EasySSH-$VER-win32-x64-portable.zip"
cp -f "$IN/README.md" "$APP/README.md"
cp -f "$EXE"  "$SITE/site/downloads/EasySSH-$VER.exe"

echo "==> [2/5] 功能总览页检查"
if [ -f "$IN/features.html" ]; then
  cp -f "$IN/features.html" "$SITE/site/features.html"
  echo "    已用交付的 features.html 更新功能页"
elif grep -q "$VER" "$SITE/site/features.html"; then
  echo "    现有 features.html 已含 v$VER 标注 ✓"
else
  echo "⚠ features.html 未含 v$VER 标注且交付目录未提供新版——请人工更新后重跑"
  exit 1
fi

echo "==> [3/5] 官网同步"
cd "$SITE" && node tools/sync.js | tail -3

echo "==> [4/5] 自检"
node tools/update-selftest.js >/dev/null && echo "    加密协议自测 ✓"
bash tools/healthcheck.sh | tail -1

echo "==> [5/5] 校验 latestVersion"
LV=$(python3 -c "import json;print(json.load(open('site/data/site-data.json'))['latestVersion'])")
[ "$LV" = "$VER" ] || { echo "!! site-data latestVersion=$LV != $VER"; exit 1; }
echo "✔ v$VER 发布完成: latestVersion=$LV, 升级接口已指向新版"
echo "  （webroot 页面文件由 root 侧定时任务自动从 site/ 同步，约 5 分钟内生效）"
