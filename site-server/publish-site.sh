#!/usr/bin/env bash
# publish-site.sh —— EasySSH 官网页面级发布（与 EasyRDP deploy-token-list.sh 同口径）
# 用法: ./publish-site.sh <相对 site/ 的文件路径...>   例: ./publish-site.sh gw-admin.html api/gateway/issue.php
# 直接发布到服务器 webroot（EasySSH 服务器已无 5 分钟自动覆盖 cron，webroot 即终态）
set -e
[ $# -ge 1 ] || { echo "用法: $0 <相对 site/ 的文件路径...>"; exit 1; }
KEY=/tmp/k_ezssh
SVKEY='/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh'
SRC=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasySSH/site-server/site
H=root@la.trae.show
P=4685
W=/www/wwwroot/easyssh.cn
SSH="ssh -i $KEY -p $P -o StrictHostKeyChecking=accept-new $H"
cp "$SVKEY" "$KEY"; chmod 600 "$KEY"
# 逐文件：webroot 侧备份 → 上传 → 权限
for f in "$@"; do
  $SSH "[ -f $W/$f ] && cp -a $W/$f $W/$f.bak-$(date +%Y%m%d%H%M) || true"
  scp -q -i "$KEY" -P $P "$SRC/$f" "$H:$W/$f"
  $SSH "chown www:www $W/$f && chmod 644 $W/$f && echo published: $f"
done
rm -f "$KEY"
echo "✅ 已直接发布到 $W（无自动覆盖 cron，改动即终态）"
