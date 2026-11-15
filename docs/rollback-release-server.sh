#!/usr/bin/env bash
# rollback-release.sh —— 官网发布一键回滚（v5.1.6 新增，配合 intake-release.sh 的发布前快照）
#
# 用法:
#   bash tools/rollback-release.sh                 # 交互式: 列出快照与当前版本,回滚到最近一次发布前的状态
#   bash tools/rollback-release.sh <旧版本号>      # 回滚到指定版本(如 5.1.4)
#   bash tools/rollback-release.sh --list          # 仅列出可用快照
#   bash tools/rollback-release.sh <旧版本号> --check  # 预检(dry-run):只打印将要执行的动作
#
# 原理: intake-release.sh 在任何落位动作前,把「发布前状态」快照到 rollback/snapshot-v<新ver>-<时间戳>/
# (含 site-data.json / SHA256SUMS / features.html / en/features.html / README.md / meta)。
# 回滚 = ① 把比目标版本新的 incoming/v* 改名隔离 → ② 恢复快照中的 features/site-data/README →
# ③ 重跑 sync.js(以旧版 incoming 重算 latest/downloads/升级清单与加密包,旧 .enc 已存在则复用) →
# ④ 自检 + healthcheck + latestVersion 校验。产物文件本就按版本命名共存于 downloads/(保留3版),
# 无需移动大文件;客户端升级通道随 check 信封与清单回指旧版,已升级用户不受影响。
set -euo pipefail
SITE=/opt/dsh/workspace/easyssh-site
RB="$SITE/rollback"
APP=/opt/dsh/workspace/EasySSH

current_ver() {
  python3 -c "import json;print(json.load(open('$SITE/site/data/site-data.json'))['latestVersion'])" 2>/dev/null || echo "?"
}

list_snapshots() {
  ls -1dt "$RB"/snapshot-v* 2>/dev/null || true
}

snap_prev_ver() { cat "$1/meta" 2>/dev/null | grep -oP 'previousVersion=\K\S+' || true; }

TARGET="${1:-}"
DRY=0
[ "${2:-}" = "--check" ] && DRY=1

if [ "$TARGET" = "--list" ]; then
  echo "当前线上版本: v$(current_ver)"
  echo "可用快照(新→旧):"
  for s in $(list_snapshots); do
    echo "  $(basename $s)  (发布前版本: v$(snap_prev_ver $s))"
  done
  exit 0
fi

if [ -z "$TARGET" ]; then
  LAST=$(list_snapshots | head -1)
  [ -n "$LAST" ] || { echo "!! 无可用快照(快照自 v5.1.6 起生成)"; exit 1; }
  TARGET=$(snap_prev_ver "$LAST")
  SNAP="$LAST"
else
  # 找「发布前版本 = TARGET」的最近快照
  SNAP=""
  for s in $(list_snapshots); do
    if [ "$(snap_prev_ver "$s")" = "$TARGET" ]; then SNAP="$s"; break; fi
  done
fi
[ -n "$SNAP" ] && [ -d "$SNAP" ] || { echo "!! 找不到回滚到 v$TARGET 所需的快照(rollback/ 下无 previousVersion=$TARGET 的快照)"; exit 1; }

CUR=$(current_ver)
echo "==> 回滚计划: v$CUR → v$TARGET (快照 $(basename "$SNAP"))"
[ "$CUR" != "$TARGET" ] || { echo "!! 当前已是 v$TARGET,无需回滚(如确需重跑请先核对)"; exit 1; }

# 预检:目标版本 incoming 交付目录完整性
IN="$SITE/../incoming/v$TARGET"
for f in "$IN/README.md"; do [ -f "$f" ] || { echo "!! incoming/v$TARGET/README.md 缺失,无法回滚"; exit 1; }; done
ls "$IN"/EasySSH-*update.exe >/dev/null 2>&1 || { echo "!! incoming/v$TARGET 缺升级载荷"; exit 1; }
ls /www/wwwroot/easyssh.cn/downloads/ 2>/dev/null | grep -q "EasySSH-$TARGET-update.exe.enc" \
  || ls "$SITE/../incoming/v$TARGET"/EasySSH-$TARGET-update.exe >/dev/null 2>&1 \
  || { echo "!! 旧版加密升级包(.enc)不存在且无法重生成来源"; exit 1; }

if [ "$DRY" = "1" ]; then
  echo "-- dry-run: 将执行以下动作 --"
  echo "  1) 隔离比 v$TARGET 新的 incoming/v* 目录(改名 *-rolledback-<ts>)"
  echo "  2) 恢复 $SNAP 内的 features.html / en/features.html / EasySSH/README.md"
  echo "  3) cd $SITE && node tools/sync.js"
  echo "  4) node tools/update-selftest.js && bash tools/healthcheck.sh"
  echo "  5) 校验 site-data latestVersion == $TARGET"
  echo "  6) 在 $SNAP/ROLLBACK_DONE 落标记,并追加隔离目录清单"
  exit 0
fi

echo "==> [1/6] 隔较高版本 incoming 目录"
TS=$(date +%Y%m%d-%H%M%S)
MOVED=""
for d in "$SITE"/../incoming/v[0-9]*; do
  [ -d "$d" ] || continue
  v=$(basename "$d" | sed 's/^v//')
  # 只隔离「比目标版本新」的目录(数值比较)
  NEWER=$(python3 -c "
def t(s): return [int(x) for x in s.split('.')]
print('yes' if t('$v') > t('$TARGET') else 'no')")
  if [ "$NEWER" = "yes" ]; then
    mv "$d" "$d-rolledback-$TS"; MOVED="$MOVED $(basename $d)"
    echo "    $(basename $d) → $(basename $d)-rolledback-$TS"
  fi
done
[ -n "$MOVED" ] || echo "    无更高版本目录"

echo "==> [2/6] 恢复快照文件"
cp -f "$SNAP/features.html"     "$SITE/site/features.html"     2>/dev/null || echo "    (快照无 features.html,保留现有)"
cp -f "$SNAP/en.features.html"  "$SITE/site/en/features.html"  2>/dev/null || echo "    (快照无 en/features.html,保留现有)"
cp -f "$SNAP/README.md"         "$APP/README.md"
echo "    features/en.features/README 已恢复到发布前状态"

echo "==> [3/6] 重跑官网同步"
cd "$SITE" && node tools/sync.js | tail -3

echo "==> [4/6] 自检"
node tools/update-selftest.js >/dev/null && echo "    加密协议自测 ✓"
bash tools/healthcheck.sh | tail -1

echo "==> [5/6] 校验 latestVersion"
LV=$(python3 -c "import json;print(json.load(open('site/data/site-data.json'))['latestVersion'])")
[ "$LV" = "$TARGET" ] || { echo "!! 回滚后 latestVersion=$LV != $TARGET"; exit 1; }

echo "==> [6/6] 落回滚标记"
printf 'rolledbackAt=%s\nfrom=%s\nto=%s\nmovedIncoming=%s\n' "$(date -Is)" "$CUR" "$TARGET" "$MOVED" > "$SNAP/ROLLBACK_DONE"
echo "✔ 已回滚: v$CUR → v$TARGET（快照 $(basename "$SNAP") 已标记 ROLLBACK_DONE）"
echo "  （webroot 由 root 侧定时任务约 5 分钟内同步;如需立即生效可手动 bash $SITE/tools/deploy.sh）"
echo "  注意:已通过在线升级升到 v$CUR 的用户不受影响;如需让全部用户退回,应发一个修复版而非反复回滚。"
