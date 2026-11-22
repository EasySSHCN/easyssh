#!/usr/bin/env bash
set -e
cp '/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh' /tmp/k2
chmod 600 /tmp/k2
ssh -i /tmp/k2 -p 4685 root@la.trae.show '
echo "=== current 目录内容 ==="
ls /opt/dsh/workspace/easyssh-gateway/current/ | head -20
echo "=== 是否有源码 Cargo.toml ==="
ls /opt/dsh/workspace/easyssh-gateway/current/Cargo.toml 2>/dev/null || echo "(纯二进制部署)"
find /opt/dsh/workspace/easyssh-gateway -maxdepth 3 -name Cargo.toml 2>/dev/null | head
echo "=== 二进制里的 clamp 痕迹确认 ==="
grep -c "min(3600)" /opt/dsh/workspace/easyssh-gateway/current/src/audit.rs 2>/dev/null || true
echo "=== systemd unit ==="
systemctl status easyssh-gateway --no-pager 2>/dev/null | head -5 || systemctl list-units | grep -i gateway | head -3
'
rm -f /tmp/k2
