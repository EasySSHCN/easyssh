#!/usr/bin/env bash
set -e
cp '/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh' /tmp/k2
chmod 600 /tmp/k2
ssh -i /tmp/k2 -p 4685 root@la.trae.show '
T1=$(grep -c GW-TOKPAGE /www/wwwroot/easyssh.cn/gw-admin.html)
T2=$(grep -c GW-TOKPAGE /opt/dsh/workspace/easyssh-site/site/gw-admin.html)
R=$(grep -c remaining /www/wwwroot/easyssh.cn/api/gateway/issue.php)
D=$(date "+%H:%M")
echo "webroot标记=$T1 源头标记=$T2 issue剩余字段=$R 时间=$D"
'
rm -f /tmp/k2
