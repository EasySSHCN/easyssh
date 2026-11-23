#!/usr/bin/env bash
set -e
cp '/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh' /tmp/k2
chmod 600 /tmp/k2
ssh -i /tmp/k2 -p 4685 root@la.trae.show "
KEYJSON=\$(php -r '\$s=file_get_contents(\"/www/wwwroot/easyssh.cn/api/gateway/keys.php\"); \$s=preg_replace(\"/^<\\\\?php exit\\\\(0\\\\); \\\\?>\\\\s*/\",\"\",\$s); \$j=json_decode(\$s,true); echo \$j[\"gateway_url\"].\"|\".\$j[\"gateway_admin_key\"];')
GWURL=\${KEYJSON%%|*}; GWKEY=\${KEYJSON##*|}
curl -s -X POST \"\$GWURL/admin/audit-delete\" -H 'Content-Type: application/json' -H \"X-Admin-Key: \$GWKEY\" -d '{\"entries\":[{\"timestamp\":1795321133,\"user\":\"ip-verify@test\"}]}' | head -c 120
echo
"
rm -f /tmp/k2
