#!/usr/bin/env bash
K=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$K"; chmod 600 "$K"
ssh -i "$K" -p 4685 root@la.trae.show 'ls -la /opt/dsh/workspace/ 2>/dev/null; du -sh /opt/dsh/workspace/* 2>/dev/null | head -15'
rm -f "$K"
