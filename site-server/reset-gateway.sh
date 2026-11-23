#!/usr/bin/env bash
set -e
cp '/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh' /tmp/k2
chmod 600 /tmp/k2
ssh -i /tmp/k2 -p 4685 root@la.trae.show '
set -e
echo "=== 1) 备份站点网关数据 ==="
D=/www/wwwroot/data
B=/root/gw-reset-backup-$(date +%Y%m%d%H%M%S)
mkdir -p "$B"
for f in gateway-users.json gateway-sessions.json gateway-resources.json gateway-userlogs.json gateway-issue-ratelimit.json; do
  [ -f "$D/$f" ] && cp -a "$D/$f" "$B/" && echo "backed: $f"
done
echo "=== 2) 清空站点网关用户数据（保留文件结构与权限）==="
reset_json() { ow=$(stat -c "%U:%G" "$D/$1"); m=$(stat -c "%a" "$D/$1"); echo "$2" > "$D/$1"; chown "$ow" "$D/$1"; chmod "$m" "$D/$1"; echo "reset: $1 -> $2"; }
reset_json gateway-users.json "{}"
reset_json gateway-sessions.json "{}"
reset_json gateway-resources.json "{}"
reset_json gateway-userlogs.json "[]"
reset_json gateway-issue-ratelimit.json "{}"
echo "=== 3) 重启网关（内存审计环形清零，ID 从头计数）==="
systemctl restart easyssh-gateway
sleep 2
systemctl is-active easyssh-gateway
echo "=== 4) 验证审计为空 ==="
KEYJSON=$(php -r "\$s=file_get_contents(\"/www/wwwroot/easyssh.cn/api/gateway/keys.php\"); \$s=preg_replace(\"/^<\\\\?php exit\\\\(0\\\\); \\\\?>\\\\s*/\",\"\",\$s); \$j=json_decode(\$s,true); echo \$j[\"gateway_url\"].\"|\".\$j[\"gateway_admin_key\"];")
GWURL=${KEYJSON%%|*}; GWKEY=${KEYJSON##*|}
curl -s "$GWURL/admin/audit?limit=5" -H "X-Admin-Key: $GWKEY"
echo
echo "=== 5) 备份位置: $B ==="
'
rm -f /tmp/k2
