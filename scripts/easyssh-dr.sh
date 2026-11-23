#!/usr/bin/env bash
set -e
K=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$K"; chmod 600 "$K"
DEST=/mnt/b/EasySSH-DR-20261122
mkdir -p "$DEST"
ssh -i "$K" -p 4685 root@la.trae.show 'bash -s' <<REMOTE
set -e
P=/root/dr-pkg/easyssh-dr
rm -rf "\$P"; mkdir -p "\$P"
cd /opt/dsh/workspace
tar -czf "\$P/workspace.tar.gz" easyssh-site easyssh-gateway incoming EasySSH EasyRDP keys.php.bak-20261111 RELEASE-HANDOFF.md reqpool-mirror.json
cd /root/dr-pkg
rm -f easyssh-dr-20261122.zip
zip -r -q -P 'EasySSH-DR-20261122' easyssh-dr-20261122.zip easyssh-dr
unzip -t -P 'EasySSH-DR-20261122' easyssh-dr-20261122.zip >/dev/null && echo LA-ZIP-OK
ls -lh easyssh-dr-20261122.zip
REMOTE
rsync --partial --append-verify --timeout=120 --info=progress2 -e "ssh -i $K -p 4685 -o StrictHostKeyChecking=accept-new" root@la.trae.show:/root/dr-pkg/easyssh-dr-20261122.zip "$DEST/easyssh-dr-20261122.zip"
echo PULL-DONE
ls -lh "$DEST/easyssh-dr-20261122.zip"
rm -f "$K"
