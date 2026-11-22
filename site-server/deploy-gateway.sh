#!/usr/bin/env bash
set -e
cp '/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh' /tmp/k2
chmod 600 /tmp/k2
BIN=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasySSH/easyssh-gateway/target/x86_64-unknown-linux-musl/release/easyssh-gateway
H=root@la.trae.show
P=4685
SSH="ssh -i /tmp/k2 -p $P -o StrictHostKeyChecking=accept-new $H"
# 1) 备份旧二进制 + 上传新版
$SSH 'cd /opt/dsh/workspace/easyssh-gateway/current && cp -a easyssh-gateway easyssh-gateway.bak-ttl && echo backup_ok'
scp -q -i /tmp/k2 -P $P "$BIN" "$H:/opt/dsh/workspace/easyssh-gateway/current/easyssh-gateway.new"
# 2) 原子替换（运行中二进制不能直接写，mv 替换 inode）+ 重启服务
$SSH 'cd /opt/dsh/workspace/easyssh-gateway/current && mv easyssh-gateway.new easyssh-gateway && chmod +x easyssh-gateway && systemctl restart easyssh-gateway && sleep 2 && systemctl is-active easyssh-gateway'
# 3) 回环验证：从站点 keys.php 取网关回环地址与 admin key，签发 1 天 token 验证钳制放开
$SSH '
KEYJSON=$(php -r "\$s=file_get_contents(\"/www/wwwroot/easyssh.cn/api/gateway/keys.php\"); \$s=preg_replace(\"/^<\\?php exit\\(0\\); \\?>\\\\s*/\",\"\",\$s); \$j=json_decode(\$s,true); echo \$j[\"gateway_url\"].\"|\".\$j[\"gateway_admin_key\"];")
GWURL=${KEYJSON%%|*}; GWKEY=${KEYJSON##*|}
RESP=$(curl -s -X POST "$GWURL/admin/issue" -H "Content-Type: application/json" -H "X-Admin-Key: $GWKEY" -d "{\"sub\":\"ttl-verify@test\",\"roles\":[\"user\"],\"ttl_seconds\":86400}")
echo "$RESP" | head -c 200; echo
EXP=$(echo "$RESP" | php -r "\$j=json_decode(stream_get_contents(STDIN),true); echo (int)(\$j[\"expires_in\"] ?? 0);")
[ "$EXP" = "86400" ] && echo TTL_86400_OK || echo "TTL_FAIL expires_in=$EXP"
'
rm -f /tmp/k2
