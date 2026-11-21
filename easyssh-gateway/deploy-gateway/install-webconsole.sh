#!/usr/bin/env bash
# install-webconsole.sh —— 云网关 Web 服务台安装器（R-20261121-002 配套，自建部署）
#
# 把云网关的「申请 / 用户服务台 / 定时审核」Web 组件安装进既有 PHP 站点：
#   - api/gateway/{apply,audit,issue,keys.example}.php   （申请/审计代理/自助签发接口）
#   - gw-admin.html（用户自助服务台：注册/登录/自动判定/鉴权申请/签发）
#   - apply.html（公共申请页：未注册用户辅助通道 + 状态查询）
#   - 定时审核任务 cron（每天 10:00 / 16:00 两次人工或 AI 审核，双结果邮件通知）
#
# 前提：
#   1) webroot 为可执行 PHP 的站点（nginx/php-fpm 已就绪）；
#   2) api/feedback/common.php 已存在于站点（fb_* 助手与邮件通道 fb_mail_send 的载体，
#      即 api/feedback/keys.php 中需配置 "mail" => {url, key}，否则受理/结果邮件不发）；
#   3) keys.php（gateway_url + gateway_admin_key 真件）已按 keys.example.php 放置（本脚本不覆盖）。
#
# 用法：
#   sudo bash install-webconsole.sh [WEBROOT]
#   WEBROOT 默认 /www/wwwroot/easyssh.example.com
#
# 数据目录口径（与线上 web PHP 一致）：common.php 的 FB_DATA_DIR = dirname(webroot)/data，
# 申请记录 / 用户库 / 会话 / 资源库 / 定时审核锁 全部落在该目录（webroot 之外，HTTP 不可直读）。
set -euo pipefail

WEBROOT=${1:-/www/wwwroot/easyssh.example.com}
HERE=$(cd "$(dirname "$0")" && pwd)
WC="$HERE/webconsole"
DATA_DIR=$(dirname "$WEBROOT")/data
LOGDIR=$(dirname "$WEBROOT")/logs

[ -d "$WC/api/gateway" ] || { echo "缺少 $WC/api/gateway（部署套件 webconsole/ 不完整）"; exit 1; }
[ -d "$WEBROOT/api/feedback" ] || echo "⚠ 未检测到 $WEBROOT/api/feedback（fb_* 助手/邮件通道载体），接口将不可用"
[ -f "$WEBROOT/api/feedback/keys.php" ] && php -r \
  '$j=json_decode(preg_replace("/^<\?php exit\(0\); \?>\s*/","",file_get_contents($argv[1])),true); \
   echo isset($j["mail"]["url"]) ? "[mail] 邮件通道已配置\n" : "[mail] ⚠ keys.php 未配置 mail 通道：受理/结果邮件不会发送\n";' \
  "$WEBROOT/api/feedback/keys.php" || echo "⚠ 未找到 api/feedback/keys.php：邮件通道状态未知，请确认 mail 配置"

echo "[1/5] 安装 api/gateway 接口"
mkdir -p "$WEBROOT/api/gateway"
for f in apply.php audit.php issue.php keys.example.php; do
  [ -f "$WC/api/gateway/$f" ] && install -m 644 "$WC/api/gateway/$f" "$WEBROOT/api/gateway/$f"
done
if [ ! -f "$WEBROOT/api/gateway/keys.php" ]; then
  echo "  ⚠ keys.php 真件不存在：请按 keys.example.php 生成（gateway_url=http://127.0.0.1:8443、gateway_admin_key 与网关 config.toml [admin].admin_key 一致），放到 $WEBROOT/api/gateway/keys.php"
fi

echo "[2/5] 安装前台页面（gw-admin.html / apply.html）"
for f in gw-admin.html apply.html; do
  [ -f "$WC/$f" ] && install -m 644 "$WC/$f" "$WEBROOT/$f"
done

echo "[3/5] 准备数据目录（webroot 之外）与权限"
mkdir -p "$DATA_DIR" "$LOGDIR"
chown www:www "$DATA_DIR" 2>/dev/null || true
chmod 775 "$DATA_DIR"

echo "[4/5] 安装定时审核任务（每天 10:00 / 16:00 两次，人工或 AI 口径）"
PHPBIN=$(command -v php || echo /www/server/php/82/bin/php)
CRONLINE="0 10,16 * * * cd $(dirname "$WC" 2>/dev/null || echo "$HERE/..") && $PHPBIN $HERE/gateway-apply-audit.php >> $LOGDIR/gw-audit.log 2>&1 # gw-apply-audit"
# gateway-apply-audit.php 与本脚本同目录（部署套件 tools 形态）；若在套件根目录则修正查找
[ -f "$HERE/gateway-apply-audit.php" ] || CRONLINE="0 10,16 * * * cd $WEBROOT && $PHPBIN $HERE/gateway-apply-audit.php >> $LOGDIR/gw-audit.log 2>&1 # gw-apply-audit"
( crontab -l 2>/dev/null | grep -v '# gw-apply-audit'; echo "$CRONLINE" ) | crontab -
crontab -l | grep gw-apply-audit
cp -f "$HERE/gateway-apply-audit.php" "$HERE/gateway-apply-audit.php" 2>/dev/null || true

echo "[5/5] 校验"
FAIL=0
for f in apply.php audit.php issue.php; do
  php -l "$WEBROOT/api/gateway/$f" >/dev/null || { echo "php -l 失败: $f"; FAIL=1; }
done
# 数据目录口径自检：模拟 common.php 的解析（dirname(webroot)/data）
php -r '$w=$argv[1];$d=dirname($w)."/data";exit(is_dir($d)?0:1);' "$WEBROOT" || { echo "数据目录缺失"; FAIL=1; }
[ "$FAIL" = 0 ] && echo "Web 服务台安装完成。" || { echo "存在失败项，请按上方提示处理"; exit 1; }
echo "后续：nginx 放行 /api/gateway/*.php（PHP 站点默认即放行）；网关侧建议仅放行 =/api/auth/login、=/api/resources、^~ /tunnel/ 三条精确路由并 deny /admin/*。"
