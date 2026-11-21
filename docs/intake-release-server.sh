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
# 执行: 发布前状态快照 → 落位产物 → 换 README → 功能页检查 → sync.js → 自检 → 校验 latestVersion
# v5.1.6: 任何落位动作前先快照「发布前状态」到 rollback/snapshot-v<ver>-<时间戳>/
# （site-data.json / SHA256SUMS / features.html / en/features.html / README.md / meta），
# 配合 tools/rollback-release.sh 可一键回滚到发布前版本。
set -euo pipefail
IN="${1:?用法: intake-release.sh <交付目录>}"
[ -d "$IN" ] || { echo "!! 交付目录不存在: $IN"; exit 1; }

SITE=/opt/dsh/workspace/easyssh-site
APP=/opt/dsh/workspace/EasySSH
RB="$SITE/rollback"

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

# ==> [0/5] v5.1.6 发布前状态快照（回滚依据；快照失败不阻断发布——回滚为尽力而为）
PREV=$(python3 -c "import json;print(json.load(open('$SITE/site/data/site-data.json'))['latestVersion'])" 2>/dev/null || echo unknown)
SNAP="$RB/snapshot-v$VER-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$SNAP" 2>/dev/null || SNAP=""
if [ -n "$SNAP" ]; then
  cp -f "$SITE/site/data/site-data.json" "$SNAP/site-data.json" 2>/dev/null || true
  cp -f "$SITE/site/downloads/SHA256SUMS" "$SNAP/SHA256SUMS" 2>/dev/null || true
  cp -f "$SITE/site/features.html" "$SNAP/features.html" 2>/dev/null || true
  cp -f "$SITE/site/en/features.html" "$SNAP/en.features.html" 2>/dev/null || true
  cp -f "$APP/README.md" "$SNAP/README.md" 2>/dev/null || true
  printf 'newVersion=%s\npreviousVersion=%s\ntakenAt=%s\n' "$VER" "$PREV" "$(date -Is)" > "$SNAP/meta"
  # 只保留最近 5 份快照（按目录名时间戳排序，删最旧）
  ls -1d "$RB"/snapshot-v* 2>/dev/null | sort | head -n -5 | while read -r old; do rm -rf "$old"; done
  echo "==> [0/5] 发布前快照已保存: $(basename "$SNAP") (发布前版本 v$PREV)"
else
  echo "⚠ 快照目录创建失败——本次发布将无自动回滚依据"
fi

echo "==> [1/5] 落位产物"
# v4.3.8 契约变更(2026-11-03):去除 dist/release 中转层——
# 产物保留在 incoming/v<ver>/ 交付目录,sync.js 直接扫描 incoming 读取;
# 仅 README.md 仍落位 EasySSH/(官网数据权威源不变);交付的源码包直接进 site/downloads
# v4.3.8:源码包改由开发机构建交付,优先使用交付版(否则 sync.js 将从本机 easyssh4 树重新打包)
if [ -f "$IN/EasySSH-$VER-source.zip" ]; then
  cp -f "$IN/EasySSH-$VER-source.zip" "$SITE/site/downloads/EasySSH-$VER-source.zip"
  echo "    已用交付的源码包更新 site/downloads"
fi
cp -f "$IN/README.md" "$APP/README.md"
cp -f "$EXE"  "$SITE/site/downloads/EasySSH-$VER.exe"

# ==> [1.5/5] easyssh-gateway 部署套件落位（R-20261120-009；可选交付）
# 口径（重要，勿改）:
#   * 仅「落位 + SHA256 校验」：新二进制写入 /opt/dsh/workspace/easyssh-gateway/releases/v<ver>/，
#     现网运行目录（current/、config.toml、密钥文件、nginx 三条精确路由）一律不动；
#   * **不自动重启网关服务**——避免误伤线上 SSH 隧道会话；
#   * 更新生效由运维会话按 docs/HANDOFF-server.md「网关部署/更新/回滚契约」章节手动执行
#     （切换 current 软链/复制 + systemctl restart easyssh-gateway）；
#   * 落位前先快照现行网关二进制到 backup/（保留最近 5 份），配合 rollback-release.sh gateway <ver> 单件回退。
GW=/opt/dsh/workspace/easyssh-gateway
GKIT=""
for d in "$IN"/easyssh-gateway-*; do [ -d "$d" ] && GKIT="$d" && break; done
if [ -z "$GKIT" ] && [ -f "$IN/EasySSH-$VER-gateway-deploy.zip" ]; then
  GKIT="$IN/.gw-kit"; rm -rf "$GKIT"; mkdir -p "$GKIT"
  unzip -q -o "$IN/EasySSH-$VER-gateway-deploy.zip" -d "$GKIT"
fi
if [ -n "$GKIT" ]; then
  GBIN=$(ls "$GKIT"/easyssh-gateway* 2>/dev/null | grep -v 'SHA256SUMS' | head -1) || true
  [ -n "$GBIN" ] || { echo "!! 网关部署套件缺 easyssh-gateway 二进制: $GKIT"; exit 1; }
  # SHA256 校验（套件自带 SHA256SUMS 与实际文件逐件比对；失败即拒收）
  if [ -f "$GKIT/SHA256SUMS" ]; then
    (cd "$GKIT" && sha256sum -c SHA256SUMS >/dev/null 2>&1) || { echo "!! 网关部署套件 SHA256 校验失败——拒绝落位"; exit 1; }
    echo "    网关套件 SHA256 校验 ✓"
  else
    echo "⚠ 网关套件无 SHA256SUMS，跳过校验（建议开发机构建补齐）"
  fi
  mkdir -p "$GW/releases"
  GDST="$GW/releases/v$VER"
  mkdir -p "$GDST"
  cp -f "$GBIN" "$GDST/$(basename "$GBIN")"
  chmod +x "$GDST/$(basename "$GBIN")" 2>/dev/null || true
  for f in config.toml.template deploy.sh keys.example.php; do
    [ -f "$GKIT/$f" ] && cp -f "$GKIT/$f" "$GDST/"
  done
  # 快照现行网关二进制（单件回退依据，保留最近 5 份）
  CURBIN=""
  for c in "$GW/current/easyssh-gateway" "$GW/current/easyssh-gateway.exe" "$GW/easyssh-gateway" "$GW/easyssh-gateway.exe"; do
    [ -f "$c" ] && CURBIN="$c" && break
  done
  if [ -n "$CURBIN" ]; then
    CURV=$(grep -oP '^version=\K\S+' "$GW/version" 2>/dev/null || echo unknown)
    BKS="$GW/backup/easyssh-gateway-$CURV-$(date +%Y%m%d-%H%M%S)"
    mkdir -p "$GW/backup"; cp -f "$CURBIN" "$BKS"; chmod +x "$BKS" 2>/dev/null || true
    ls -1dt "$GW"/backup/easyssh-gateway-* 2>/dev/null | tail -n +6 | while read -r old; do rm -rf "$old"; done
    echo "    已快照现行网关二进制 → $(basename "$BKS")（backup/ 保留最近 5 份）"
  else
    echo "    未发现现行网关二进制（首次部署），无快照"
  fi
  echo "    网关套件已落位: $GDST（未重启网关服务——按 docs/HANDOFF-server.md 由运维执行更新）"
else
  echo "    本版无网关部署套件，跳过网关落位"
fi

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

# 英文版功能页双语同步(2026-11-14 起,强校验)
if [ -f "$IN/en/features.html" ]; then
  mkdir -p "$SITE/site/en"
  cp -f "$IN/en/features.html" "$SITE/site/en/features.html"
  echo "    已用交付的 en/features.html 更新英文功能页"
elif grep -q "$VER" "$SITE/site/en/features.html" 2>/dev/null; then
  echo "    现有 en/features.html 已含 v$VER 标注(双语同步) ✓"
else
  echo "⚠ en/features.html 未含 v$VER 标注且交付目录未提供新版——中英文功能页必须同步更新后重跑"
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
[ -n "$SNAP" ] && echo "  回滚依据: bash tools/rollback-release.sh（发布前快照 $(basename "$SNAP")）"
echo "  （webroot 页面文件由 root 侧定时任务自动从 site/ 同步，约 5 分钟内生效）"
