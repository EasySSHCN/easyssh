<?php
/**
 * /api/update/check.php —— 升级检测动态层（归因关联第二期,v5.1.6）
 *
 * 说明：客户端请求路径保持 /api/update/check 不变（nginx location = 精确匹配转本文件）。
 * 行为 = 读同目录静态信封文件 check（sync.js 发布时生成的 AES-256-GCM + HMAC-SHA256 时效
 * 签名信封）原样回传，并仅当「客户端 IP 名下存在窗内(≤7天)未满额下载票据」时，在响应 JSON
 * 顶层追加 download_ticket 字段：
 *   - 该字段为明文随机数，不参与签名(HMAC 只覆盖 ts.nonce.ct.tag)与解密链路；
 *   - 老版本客户端不认识该字段自动忽略，升级协议完全兼容；
 *   - 新版客户端(updater.rs)读取并持久化到 cfgDir/download-ticket.json，此后每次统计上报
 *     附带 tok，服务端凭 tok 精确归因/重归因（下载在网络 A、运行在网络 B 也能关联）。
 * 绝不修改信封任何签名/加密字段；静态文件缺失或非法时原样透传（不阻断升级检测）。
 */

header('Content-Type: application/json; charset=utf-8');
header('Cache-Control: no-store');

$static = __DIR__ . '/check';
$raw = is_readable($static) ? (string)file_get_contents($static) : '';
if ($raw === '') {
  http_response_code(404);
  header('Content-Type: text/plain; charset=utf-8');
  echo 'check envelope missing';
  exit;
}
$env = json_decode($raw, true);
if (!is_array($env) || ($env['v'] ?? 0) != 1 || ($env['alg'] ?? '') !== 'A256GCM-HS256') {
  // 信封异常：原样透传，让客户端按既有校验逻辑报错（不吞错、不造假信封）
  echo $raw;
  exit;
}

// 下发判定：同完整 IP + 窗内(≤7天) + 未满额(归因设备<10)的最近一张票
require_once __DIR__ . '/../stats/common.php';
$now = st_now_ms();
$ip = fb_client_ip();
$tok = null;
$tickets = st_tickets_load()['tickets'];
foreach ($tickets as $k => $t) {
  if (!is_array($t) || (string)($t['ip'] ?? '') !== (string)$ip) continue;
  if (!st_ticket_available($t)) continue;
  $ts = (int)($t['ts'] ?? 0);
  if ($ts > $now + 60 * 1000) continue;                       // 时钟偏移容忍
  if ($now - $ts > ST_TICKET_WINDOW_MS) continue;             // 只下发窗内票据
  if ($tok === null || $ts > (int)($tickets[$tok]['ts'] ?? 0)) $tok = (string)$k;
}
if ($tok !== null) $env['download_ticket'] = $tok;

echo json_encode($env, JSON_UNESCAPED_SLASHES);
