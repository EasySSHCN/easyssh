#!/usr/bin/env bash
# EasySSH v4 单文件引导器（NSIS）：打包 Tauri exe + WebView2Loader.dll
# 产物：release/EasySSH-<ver>-setup.exe（单文件下载版）
# 依赖：~/.cargo/config.toml（mingw 工具链，同 build-v4.sh）、electron-builder NSIS 缓存 makensis
set -euo pipefail
cd "$(dirname "$0")/../src-tauri"

source "$HOME/.cargo/env" 2>/dev/null || true
# MSVC 交叉工具链（clang-cl/lld-link/llvm-rc/nasm + libtinfo 补丁 + xwin SDK）
PATH="$HOME/llvm/clang+llvm-18.1.8-x86_64-linux-gnu-ubuntu-18.04/bin:$HOME/nasm:$HOME/mingw-prefix/bin:$PATH"
LD_LIBRARY_PATH="$HOME/libhack:${LD_LIBRARY_PATH:-}"
XWIN_ARCH=x86_64
export LD_LIBRARY_PATH XWIN_ARCH

VERSION=$(grep -o '"version": *"[^"]*"' tauri.conf.json | head -1 | cut -d'"' -f4)
OUT="../dist-v4/EasySSH-$VERSION.exe"

MAKENSIS_CANDIDATES=(
  "$HOME/.cache/electron-builder/nsis"/*/linux/makensis
  /opt/dsh/.cache/electron-builder-nsis-manual/nsis-3.0.4.1/linux/makensis
)
MAKENSIS=""
for c in "${MAKENSIS_CANDIDATES[@]}"; do [ -x "$c" ] && MAKENSIS="$c" && break; done
[ -n "$MAKENSIS" ] || { echo "未找到 makensis"; exit 1; }

# 确保 Tauri exe + Loader 已构建
# v4.2.0:全部切换 MSVC 构建(WebView2Loader 静态链接,AV 误报率显著低于 mingw 构建)
cargo xwin build --release --target x86_64-pc-windows-msvc
EXE="../target/x86_64-pc-windows-msvc/release/easyssh.exe"
WV=$(ls ../target/x86_64-pc-windows-msvc/release/build/webview2-com-sys-*/out/x64/WebView2Loader.dll 2>/dev/null | head -1)
ICON="icons/icon.ico"
[ -f "$EXE" ] && [ -f "$WV" ] && [ -f "$ICON" ] || { echo "缺少构建产物"; exit 1; }

echo "==> 编译 NSIS 单文件引导器"
mkdir -p ../dist-v4
"$MAKENSIS" \
  -DAPP_EXE="$(pwd)/$EXE" \
  -DLOADER_DLL="$(pwd)/$WV" \
  -DOUTFILE="$(pwd)/$OUT" \
  -DICON="$(pwd)/icons/icon.ico" \
  -DVERSION="$VERSION" \
  -DVER4="$VERSION.0" \
  ../nsis/bootstrap-v4.nsi

[ -f "$OUT" ] || { echo "构建失败：缺少 $OUT"; exit 1; }
echo "✅ 单文件引导器构建完成: $OUT ($(du -h "$OUT" | cut -f1))"

# 发布目录只保留最新 3 个版本（用户要求，v4.1.5 起纳入流程）
bash "$(dirname "$0")/prune-old-releases.sh" || true
