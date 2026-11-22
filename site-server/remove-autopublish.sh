#!/usr/bin/env bash
set -e
cp '/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh' /tmp/k2
chmod 600 /tmp/k2
SSH="ssh -i /tmp/k2 -p 4685 -o StrictHostKeyChecking=accept-new root@la.trae.show"

echo "=== 1) 备份 crontab 并移除 easyssh-cron 发布条目 ==="
$SSH '
crontab -l > /root/crontab-backup-20261122.txt
echo "--- 移除前 ---"; grep -n "easyssh-cron" /root/crontab-backup-20261122.txt || true
crontab -l | grep -v "easyssh-cron$" | grep -v "node tools/sync.js" > /tmp/newcron || true
# 精确删除那一行（按行内容匹配）
crontab -l | grep -v "cd /opt/dsh/workspace/easyssh-site" > /tmp/newcron
crontab /tmp/newcron
echo "--- 移除后 root crontab ---"
crontab -l | grep -v "^#" | grep -v "^$"
'

echo "=== 2) 确认快照与 webroot 当前一致（移除前基线）==="
$SSH '
diff -q /opt/dsh/workspace/easyssh-site/site/gw-admin.html /www/wwwroot/easyssh.cn/gw-admin.html && echo HTML_SAME
diff -q /opt/dsh/workspace/easyssh-site/site/api/gateway/issue.php /www/wwwroot/easyssh.cn/api/gateway/issue.php && echo PHP_SAME
ls /opt/dsh/workspace/easyssh-site/logs/deploy.log | xargs tail -2
'

echo "=== 3) 保留的 cron（DDNS 安全类不动）==="
$SSH 'crontab -l | grep -v "^#" | grep -v "^$"'
rm -f /tmp/k2
