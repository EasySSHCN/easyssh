#!/usr/bin/env bash
# 构建单文件引导器（v3.0.4 起替代 electron-builder portable 常驻自解压模式）
# 产物：release/EasySSH-<version>.exe
#   - 首次双击：解压到 exe 同目录 EasySSH\ 并建桌面快捷方式，之后日常秒开
#   - 再次双击：直接启动 EasySSH\EasySSH.exe（不解压）
# makensis 取自 electron-builder 缓存（与 CI 一致的 Windows 目标工具链）
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(node -p "require('./package.json').version")
OUT="release/EasySSH-$VERSION.exe"

MAKENSIS_CANDIDATES=(
  "$HOME/.cache/electron-builder/nsis"/*/linux/makensis
  /opt/dsh/.cache/electron-builder/nsis/*/*/linux/makensis
  /opt/dsh/.cache/electron-builder/nsis/*/linux/makensis
)
MAKENSIS=""
for c in "${MAKENSIS_CANDIDATES[@]}"; do
  [ -x "$c" ] && MAKENSIS="$c" && break
done
[ -n "$MAKENSIS" ] || { echo "未找到 makensis（electron-builder 缓存），请先运行一次 npm run build:exe 旧流程"; exit 1; }
echo "==> 使用 makensis: $MAKENSIS"

echo "==> 1/2 生成 win-unpacked（electron-builder --dir）"
npx electron-builder --win dir
APP_DIR="$(pwd)/release/win-unpacked"
[ -f "$APP_DIR/EasySSH.exe" ] || { echo "缺少 $APP_DIR/EasySSH.exe"; exit 1; }

echo "==> 2/2 编译引导器 NSIS"
mkdir -p release
"$MAKENSIS" \
  -DAPP_DIR="$APP_DIR" \
  -DOUTFILE="$(pwd)/$OUT" \
  -DICON="$(pwd)/icon.ico" \
  nsis/bootstrap.nsi

[ -f "$OUT" ] || { echo "构建失败：缺少 $OUT"; exit 1; }
echo "✅ 单文件引导器构建完成: $OUT ($(du -h "$OUT" | cut -f1))"
