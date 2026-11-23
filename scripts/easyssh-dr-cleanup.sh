#!/usr/bin/env bash
set -e
KE=/tmp/kek
KF=/tmp/kf
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KF"; chmod 600 "$KF"
echo "== us 清理临时文件 =="
ssh -i "$KE" root@us.trae.show 'rm -f /www/wwwroot/easyrdp.cn/downloads/easyssh-dr-tmp-20261122.zip /root/easyssh-dr-20261122.zip; ls /www/wwwroot/easyrdp.cn/downloads/ | grep -c easyssh-dr || echo NO-TMP-LEFT'
echo "== 撤销 us 富规则(la) =="
ssh -i "$KE" root@us.trae.show "firewall-cmd --permanent --zone=public --remove-rich-rule='rule family=\"ipv4\" source address=\"107.148.90.44\" port port=\"22\" protocol=\"tcp\" accept'; firewall-cmd --zone=public --remove-rich-rule='rule family=\"ipv4\" source address=\"107.148.90.44\" port port=\"22\" protocol=\"tcp\" accept'; grep -vF 'iv-yelqu1y3nkf8b8x6uff2' /root/.ssh/authorized_keys > /tmp/ak && mv /tmp/ak /root/.ssh/authorized_keys && chmod 600 /root/.ssh/authorized_keys; echo US-RESTORED; firewall-cmd --zone=public --list-rich-rules"
echo "== la 清理临时打包 =="
timeout 20 ssh -i "$KF" -p 4685 root@la.trae.show 'rm -rf /root/dr-pkg/easyssh-dr /root/dr-pkg/easyssh-dr-20261122.zip /root/.ssh/id_ed25519 /root/.ssh/id_ed25519.pub; echo LA-CLEANED'
rm -f "$KE" "$KF"
