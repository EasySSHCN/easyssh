#!/usr/bin/env bash
# prune-v4-intake.sh —— 发布完成后清理接收区旧产物，只保留最新 KEEP 个版本
# 范围：/opt/dsh/workspace/EasySSH/dist 与 /release（intake 落位目录）
# 历史版本文件只保留在开发机本地归档与已发布的 GitHub/Gitee，服务器不留。
set -uo pipefail
KEEP="${1:-3}"
APP=/opt/dsh/workspace/EasySSH

ver_key() {
  # 从 EasySSH-<ver>* 文件名提取版本号,转成可排序键(每段补零 6 位)
  sed -n 's/^EasySSH-\([0-9][0-9A-Za-z.-]*\)\..*/\1/p' \
    | awk -F. '{ k=""; for(i=1;i<=NF;i++) k=k sprintf("%06d", $i+0) "."; print k }'
}

for DIR in "$APP/dist" "$APP/release"; do
  [ -d "$DIR" ] || continue
  # v4.3.7 修复:排序键须换算回真实版本号再做保留匹配(此前误用补零键导致全删)
  VERS=$(ls "$DIR" 2>/dev/null | ver_key | sort -u | tail -n "$KEEP" | awk -F. '{printf "%d.%d.%d ", $1+0, $2+0, $3+0}')
  [ -n "$VERS" ] || continue
  echo "[$DIR] 保留版本: $VERS"
  for f in "$DIR"/*; do
    [ -f "$f" ] || continue
    base=$(basename "$f")
    case "$base" in EasySSH-*) ;; *) continue ;; esac
    keep=0
    for v in $VERS; do
      if [ "${base#EasySSH-$v}" != "$base" ]; then keep=1; break; fi
    done
    if [ "$keep" = "0" ]; then
      echo "  清理超期: $base"
      rm -f "$f"
    fi
  done
  echo "[$DIR] 清理完成"
done

# v4.3.7 扩展:incoming 交付目录同样只保留最新 KEEP 个版本
# (DEV-STATUS.md 双向状态日志永远保留)
INC=/opt/dsh/workspace/incoming
if [ -d "$INC" ]; then
  VERS=$(ls "$INC" 2>/dev/null | grep -E '^v[0-9]' | sed 's/^v//' | sort -t. -k1,1n -k2,2n -k3,3n | tail -n "$KEEP")
  [ -n "$VERS" ] || { echo 'PRUNE_DONE'; exit 0; }
  echo "[$INC] 保留版本: $VERS"
  for d in "$INC"/v*; do
    [ -d "$d" ] || continue
    base=$(basename "$d" | sed 's/^v//')
    keep=0
    for v in $VERS; do
      [ "$base" = "$v" ] && keep=1 && break
    done
    if [ "$keep" = "0" ]; then
      echo "  清理超期交付: $(basename "$d")"
      rm -rf "$d"
    fi
  done
  echo "[$INC] 清理完成"
fi

# v4.3.7 扩展:对接发布执行记录日志只保留最新 181 天
STATUS_LOG=/opt/dsh/workspace/incoming/DEV-STATUS.md
if [ -f "$STATUS_LOG" ]; then
  python3 - "$STATUS_LOG" << 'PYEOF'
import sys, io, datetime, re
p = sys.argv[1]
try:
    lines = io.open(p, encoding='utf-8', errors='replace').read().splitlines()
except Exception:
    sys.exit(0)
cut = datetime.date.today() - datetime.timedelta(days=181)
kept = []
removed = 0
for ln in lines:
    m = re.match(r'^- (\d{4}-\d{2}-\d{2}) ', ln)
    if m:
        try:
            if datetime.date.fromisoformat(m.group(1)) >= cut:
                kept.append(ln)
            else:
                removed += 1
            continue
        except Exception:
            pass
    kept.append(ln)
if removed:
    io.open(p, 'w', encoding='utf-8').write('\n'.join(kept) + '\n')
    print('[DEV-STATUS] 已清理 %d 条超过 181 天的执行记录' % removed)
PYEOF
fi
echo 'PRUNE_DONE'
