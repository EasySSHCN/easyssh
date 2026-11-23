#!/usr/bin/env bash
set -e
KE=/tmp/kek
KF=/tmp/kf
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KF"; chmod 600 "$KF"
echo "== us 放行 la:22 =="
ssh -i "$KE" root@us.trae.show "firewall-cmd --permanent --zone=public --add-rich-rule='rule family=\"ipv4\" source address=\"107.148.90.44\" port port=\"22\" protocol=\"tcp\" accept' && firewall-cmd --zone=public --add-rich-rule='rule family=\"ipv4\" source address=\"107.148.90.44\" port port=\"22\" protocol=\"tcp\" accept' && echo FW-OPEN"
echo "== 注入 la 公钥到 us =="
PUB=$(timeout 20 ssh -i "$KF" -p 4685 root@la.trae.show 'if [ ! -f /root/.ssh/id_ed25519.pub ]; then ssh-keygen -q -t ed25519 -N "" -f /root/.ssh/id_ed25519; fi; cat /root/.ssh/id_ed25519.pub')
echo "la pub: ${PUB:0:30}..."
ssh -i "$KE" root@us.trae.show "grep -qF '$PUB' /root/.ssh/authorized_keys || printf '\n%s\n' '$PUB' >> /root/.ssh/authorized_keys; echo INJECT-OK"
echo "== la→us 推包 =="
timeout 600 ssh -i "$KF" -p 4685 root@la.trae.show 'rsync --partial --append-verify --timeout=60 --info=progress2 -e "ssh -o StrictHostKeyChecking=accept-new" /root/dr-pkg/easyssh-dr-20261122.zip root@us.trae.show:/root/easyssh-dr-20261122.zip && ssh -o StrictHostKeyChecking=accept-new root@us.trae.show "unzip -t -P EasySSH-DR-20261122 /root/easyssh-dr-20261122.zip >/dev/null && echo US-ZIP-OK; cp /root/easyssh-dr-20261122.zip /www/wwwroot/easyrdp.cn/downloads/easyssh-dr-tmp-20261122.zip; chown www:www /www/wwwroot/easyrdp.cn/downloads/easyssh-dr-tmp-20261122.zip; echo URL-READY"' 2>&1 | tail -4
rm -f "$KE" "$KF"
