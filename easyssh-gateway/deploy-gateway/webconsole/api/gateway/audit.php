<?php
/**
 * POST /api/gateway/audit.php —— 网关审计代理（EasySSH · 🛡网关审计标签页）
 * （管理控制台第一步：把 easyssh-gateway 内存环形审计日志经受保护 API 开放到官网后台）
 *
 * 鉴权：与 api/stats/admin.php 同一套会话模式——POST JSON { user, token }，
 *   经 api/feedback/common.php 的 fb_check_token() 校验管理员登录会话（与用户侧
 *   gw-admin.html / issue.php 的网关会话完全隔离）。
 *
 * 配置：__DIR__/keys.php（参照 keys.example.php 模板人工放置，真实密钥绝不入库）：
 *   gateway_url        网关服务端内网回环地址（如 http://127.0.0.1:8443）
 *   gateway_admin_key   与 easyssh-gateway config.toml [admin].admin_key 一致的共享密钥
 *
 * 动作（JSON body）：
 *   { action:"audit", user, token, limit? }  → { ok, count, events:[{timestamp,event,source_ip,resource,result}] }
 *     limit 仅接受 50/100/200（默认 100，其余值回退 100）；网关侧再做钳制。
 *
 * 错误结构（明确 JSON，HTTP 状态对应）：
 *   { ok:false, message:"..." }
 *   配置缺失 → 500；会话无效 → 401；网关不可达 → 502；网关拒绝（密钥错/禁用）→ 502（透传 message）。
 */

require __DIR__ . '/../feedback/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);
$in = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($in)) fb_fail('请求体必须是 JSON');
if ((string)($in['action'] ?? '') !== 'audit') fb_fail('action 非法，仅支持 audit');
$user = (string)($in['user'] ?? '');
$token = (string)($in['token'] ?? '');
if (!fb_check_token($user, $token)) fb_fail('未登录或登录已过期，请重新登录', 401);

/* ---------- 加载网关配置（keys.php 人工放置；缺失 = 未部署） ---------- */
$keysFile = __DIR__ . '/keys.php';
if (!is_readable($keysFile)) fb_fail('网关配置缺失（api/gateway/keys.php 未放置），请联系管理员', 500);
$keys = json_decode((string)preg_replace('/^<\?php exit\(0\); \?>\s*/', '', (string)file_get_contents($keysFile)), true);
if (!is_array($keys) || empty($keys['gateway_url']) || empty($keys['gateway_admin_key'])) {
  fb_fail('网关配置不完整（gateway_url / gateway_admin_key 缺失）', 500);
}
$gwBase = rtrim((string)$keys['gateway_url'], '/');
$gwKey = (string)$keys['gateway_admin_key'];

/* ---------- limit 白名单（50/100/200，默认 100） ---------- */
$limit = (int)($in['limit'] ?? 100);
if (!in_array($limit, [50, 100, 200], true)) $limit = 100;

/* ---------- 服务端代理请求网关 GET /admin/audit?limit=N（X-Admin-Key） ---------- */
$url = $gwBase . '/admin/audit?limit=' . $limit;
$ctx = stream_context_create(['http' => [
  'method'  => 'GET',
  'timeout' => 5,
  'header'  => "X-Admin-Key: " . $gwKey . "\r\nUser-Agent: easyssh-site-audit-proxy\r\n",
  'ignore_errors' => true, // 4xx/5xx 也要读到响应体以透传原因
]]);
$resp = @file_get_contents($url, false, $ctx);
if ($resp === false) {
  fb_fail('网关不可达（easyssh-gateway 连接失败或超时），请稍后重试', 502);
}
// $http_response_header 形如 ["HTTP/1.1 200 OK", ...]；取第一个状态行
$status = 0;
foreach (($http_response_header ?? []) as $h) {
  if (preg_match('#^HTTP/\S+\s+(\d{3})#', $h, $m)) { $status = (int)$m[1]; break; }
}
$j = json_decode((string)$resp, true);
if (!is_array($j) || ($j['ok'] ?? false) !== true || !isset($j['events']) || !is_array($j['events'])) {
  $msg = is_array($j) && isset($j['message']) ? (string)$j['message'] : '网关响应格式异常';
  fb_fail('网关返回错误（HTTP ' . ($status ?: '?') . '）：' . $msg, 502);
}

header('Content-Type: application/json; charset=utf-8');
header('Cache-Control: no-store');
echo json_encode(['ok' => true, 'count' => (int)($j['count'] ?? count($j['events'])),
  'events' => array_values($j['events'])], JSON_UNESCAPED_UNICODE);
exit;
