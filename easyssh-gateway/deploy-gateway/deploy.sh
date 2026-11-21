#!/usr/bin/env bash
# EasySSH 网关部署脚本 —— 在网关服务器上执行（参考 EasyRDP deploy-gateway 形态）。
# 用法：把本目录（easyssh-gateway(.exe) / config.toml / keys.php）上传到服务器任意目录，
#       然后运行本脚本。幂等，可重复执行。
# 注意：keys.php 是真实密钥文件，绝不入库；模板见 keys.example.php。
set -euo pipefail

GATEWAY_DIR=${GATEWAY_DIR:-/opt/easyssh-gateway}   # 网关程序与 config.toml 的安装目录
SITE_API=${SITE_API:-/www/wwwroot/easyssh.example.com/api/gateway}  # 官网 api/gateway 目录
HERE=$(cd "$(dirname "$0")" && pwd)

echo "[1/4] 放置官网后台网关配置 keys.php"
if [ ! -f "$HERE/keys.php" ]; then echo "缺少 keys.php（请按 keys.example.php 生成，勿入库）"; exit 1; fi
mkdir -p "$SITE_API"
install -m 600 -o www -g www "$HERE/keys.php" "$SITE_API/keys.php" 2>/dev/null \
  || { install -m 600 "$HERE/keys.php" "$SITE_API/keys.php" && chown www:www "$SITE_API/keys.php"; }
php -r '
$t = file_get_contents($argv[1]);
$j = json_decode(preg_replace("/^<\?php exit\(0\); \?>\s*/", "", $t), true);
exit(empty($j["gateway_url"]) || empty($j["gateway_admin_key"]) ? 1 : 0);
' "$SITE_API/keys.php" && echo "  keys.php 校验 OK"

echo "[2/4] 安装网关程序与配置"
mkdir -p "$GATEWAY_DIR"
BIN=$(ls "$HERE"/easyssh-gateway* 2>/dev/null | grep -v '\.sh$' | head -n1)
[ -n "$BIN" ] || { echo "缺少网关二进制 easyssh-gateway(.exe)"; exit 1; }
install -m 755 "$BIN" "$GATEWAY_DIR/easyssh-gateway"
install -m 600 "$HERE/config.toml" "$GATEWAY_DIR/config.toml"
# 生产安全口径校验：config.toml 必须显式 dev_mode = false
grep -Eq '^dev_mode\s*=\s*false' "$GATEWAY_DIR/config.toml" \
  || { echo "config.toml 必须显式 dev_mode = false（生产口径）"; exit 1; }

echo "[3/4] 创建 systemd 服务（Linux；Windows 服务器请用计划任务/NSSM 自行常驻）"
if command -v systemctl >/dev/null 2>&1; then
cat > /etc/systemd/system/easyssh-gateway.service <<EOF
[Unit]
Description=EasySSH Cloud Gateway
After=network.target

[Service]
ExecStart=$GATEWAY_DIR/easyssh-gateway
WorkingDirectory=$GATEWAY_DIR
Environment=EASYSSH_CONFIG=$GATEWAY_DIR/config.toml
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now easyssh-gateway
sleep 1
systemctl --no-pager -l status easyssh-gateway | head -n 8
else
  echo "  非 systemd 环境，请手动常驻启动: EASYSSH_CONFIG=$GATEWAY_DIR/config.toml $GATEWAY_DIR/easyssh-gateway"
fi

echo "[4/4] 验证"
KEY=$(php -r '$j=json_decode(preg_replace("/^<\?php exit\(0\); \?>\s*/","",file_get_contents($argv[1])),true); echo $j["gateway_admin_key"];' "$SITE_API/keys.php")
URL=$(php -r '$j=json_decode(preg_replace("/^<\?php exit\(0\); \?>\s*/","",file_get_contents($argv[1])),true); echo $j["gateway_url"];' "$SITE_API/keys.php")
echo -n "  /admin/audit 探测: "
curl -s -H "X-Admin-Key: $KEY" "$URL/admin/audit?limit=5" | head -c 300; echo
echo "完成。"
