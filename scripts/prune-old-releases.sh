#!/usr/bin/env bash
# prune-old-releases.sh —— 发布目录只保留最新 3 个版本（用户要求）
# 范围：/opt/dsh/workspace/EasySSH/dist 与 /release
# 规则：按文件名 EasySSH-<ver> 提取版本，语义比较排序，保留最新 3 个版本的所有产物，其余删除
set -euo pipefail
KEEP=3
DIST=/opt/dsh/workspace/EasySSH/dist
RELEASE=/opt/dsh/workspace/EasySSH/release

ver_key() {
  # 提取 EasySSH-<ver> 后缀中的版本号，转成可排序键（每段补零 6 位）
  sed -n 's/^EasySSH-\([0-9][0-9A-Za-z.-]*\)\(\.exe\|-win32-x64-portable\.zip\)$/\1/p' \
    | awk -F. '{ k=""; for(i=1;i<=NF;i++) k=k sprintf("%06d", $i+0) "."; print k }'
}

for DIR in "$DIST" "$RELEASE"; do
  [ -d "$DIR" ] || continue
  # 当前保留的版本集合（去重排序取最新 3 个）
  VERS=$(ls "$DIR" 2>/dev/null | ver_key | sort -u | tail -n "$KEEP" | awk -F. '{printf "%d.%d.%d ", $1+0, $2+0, $3+0}')
  echo "[$DIR] 保留版本: $VERS"
  for f in "$DIR"/*; do
    [ -f "$f" ] || continue
    base=$(basename "$f")
    case "$base" in
      EasySSH-*.exe|EasySSH-*.zip) ;;
      *) continue ;;
    esac
    keep=0
    for v in $VERS; do
      case "$base" in
        EasySSH-$v.exe|EasySSH-$v-*|EasySSH-$v-*) keep=1; break ;;
      esac
    done
    # 精确匹配版本段（EasySSH-<v>.exe 或 EasySSH-<v>-win32... 或 EasySSH-<v>-portable.exe）
    ver_in_name=0
    for v in $VERS; do
      if [ "${base#EasySSH-$v}" != "$base" ]; then ver_in_name=1; break; fi
    done
    if [ "$ver_in_name" = "0" ]; then
      echo "  清理超期产物: $base"
      rm -f "$f"
    fi
  done
done
echo "清理完成。"
