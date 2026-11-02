#!/usr/bin/env bash
# EasySSH v4 (Tauri 2) Windows 交叉构建（Linux + mingw-w64，无需 Windows 环境）
# 依赖：
#   1) rustup + rust（stable）+ target x86_64-pc-windows-gnu
#   2) mingw-w64 工具链（EL8: mingw64-gcc/binutils/crt/headers rpms 解包到 ~/mingw-prefix）
#   3) ~/.cargo/config.toml（linker/rustflags/env 配置，见仓库内 scripts/setup-mingw.md）
# 产物：dist-v4/EasySSH-v4-win64.zip（EasySSH.exe + WebView2Loader.dll）
set -euo pipefail
cd "$(dirname "$0")/../src-tauri"

source "$HOME/.cargo/env" 2>/dev/null || true
PATH="$HOME/mingw-prefix/bin:$PATH"

echo "==> cargo build --release (x86_64-pc-windows-gnu)"
cargo build --release --target x86_64-pc-windows-gnu

EXE="target/x86_64-pc-windows-gnu/release/easyssh.exe"
# v4.3.0:workspace 根 target 目录 + find 定位 DLL（构建哈希目录会变化）
WV=$(find target/x86_64-pc-windows-gnu/release/build -path '*webview2-com-sys*' -name 'WebView2Loader.dll' -path '*x64*' 2>/dev/null | head -1)
[ -f "$EXE" ] || { echo "缺少 $EXE"; exit 1; }
[ -f "$WV" ] || { echo "缺少 WebView2Loader.dll"; exit 1; }

OUT="../dist-v4"
VERSION=$(grep -o '"version": *"[^"]*"' tauri.conf.json | head -1 | cut -d'"' -f4)
mkdir -p "$OUT"
ROOT="$PWD/.."
PKG="/tmp/EasySSH-pkg-$VERSION"
rm -rf "$PKG" && mkdir -p "$PKG"
cp "$EXE" "$PKG/EasySSH.exe"
cp "$WV" "$PKG/WebView2Loader.dll"
(cd "$PKG" && zip -qr "$ROOT/dist-v4/EasySSH-v$VERSION-win64.zip" .)
echo "✅ zip 构建完成: dist-v4/EasySSH-v$VERSION-win64.zip ($(du -h "$ROOT/dist-v4/EasySSH-v$VERSION-win64.zip" | cut -f1))"

# 单文件 NSIS 引导安装器（与历史发布物 EasySSH-$VERSION.exe 同构）
MAKENSIS=$(find /opt/dsh/.cache/electron-builder-nsis-manual -name makensis -path '*linux*' 2>/dev/null | head -1)
if [ -n "${MAKENSIS:-}" ]; then
  "$MAKENSIS" -V2 \
    -DOUTFILE="$OUT/EasySSH-$VERSION.exe" \
    -DVERSION="$VERSION" -DVER4="$VERSION.0" \
    -DICON="$ROOT/src-tauri/icons/icon.ico" \
    -DAPP_EXE="$ROOT/$EXE" -DLOADER_DLL="$ROOT/$WV" \
    "$ROOT/nsis/bootstrap-v4.nsi" && \
    echo "✅ 安装器构建完成: dist-v4/EasySSH-$VERSION.exe"
else
  echo "⚠ 未找到 makensis，跳过单文件安装器"
fi
