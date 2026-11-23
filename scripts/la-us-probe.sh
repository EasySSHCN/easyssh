#!/usr/bin/env bash
set -e
KE=/tmp/kek
KF=/tmp/kf
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KF"; chmod 600 "$KF"
echo "== us -> la:4685 连通测试 =="
ssh -i "$KE" root@us.trae.show 'timeout 15 ssh -p 4685 -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -o BatchMode=yes root@la.trae.show "echo US-TO-LA-OK; hostname" 2>&1 | tail -1'
echo "== la -> us:22 连通测试 =="
ssh -i "$KF" -p 4685 root@la.trae.show 'timeout 15 ssh -p 22 -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -o BatchMode=yes root@us.trae.show "echo LA-TO-US-OK; hostname" 2>&1 | tail -1'
rm -f "$KE" "$KF"
