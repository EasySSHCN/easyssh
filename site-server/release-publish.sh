#!/usr/bin/env bash
# release-publish.sh —— EasySSH 发版级发布（替代原每 5 分钟的 easyssh-cron，改为手动触发）
# 场景：新版本 exe/zip 已上传服务器 /opt/dsh/workspace/EasySSH/ 后，手动执行一次：
#   sync.js   生成 site-data.json / 下载包拷贝与 SHA256 / 在线升级加密产物
#   deploy.sh 把 site/ 快照刷进 webroot
# 与页面级发布(publish-site.sh)互补：改页面用 publish-site.sh，发新版用本脚本。
set -e
KEY=/tmp/k_ezssh
SVKEY='/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh'
H=root@la.trae.show
P=4685
SSH="ssh -i $KEY -p $P -o StrictHostKeyChecking=accept-new $H"
cp "$SVKEY" "$KEY"; chmod 600 "$KEY"
$SSH 'cd /opt/dsh/workspace/easyssh-site && su dsh -s /bin/bash -c "/usr/local/bin/node tools/sync.js" 2>&1 | tail -3 && ./tools/deploy.sh 2>&1 | tail -2 && echo RELEASE_PUBLISH_OK'
rm -f "$KEY"
