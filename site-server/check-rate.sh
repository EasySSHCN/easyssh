#!/usr/bin/env bash
set -e
cp '/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh' /tmp/k5
chmod 600 /tmp/k5
echo "--- EasyRDP ---"
ssh -i /tmp/k5 root@198.2.231.95 'grep -c ">= 30" /www/wwwroot/easyrdp.cn/api/gateway/issue.php; grep -c "30 次" /www/wwwroot/easyrdp.cn/api/gateway/issue.php'
echo "--- EasySSH ---"
ssh -i /tmp/k5 -p 4685 root@la.trae.show 'grep -n "GW_ISSUE_HOURLY_MAX" /www/wwwroot/easyssh.cn/api/gateway/issue.php | head -3'
rm -f /tmp/k5
