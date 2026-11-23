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
 * 绝不修改信封加密字段(ct/tag/nonce)；静态文件缺失或非法时原样透传（不阻断升级检测）。
 *
 * v5.2.5 修复「升级响应已过期」：客户端自某版本起对信封 ts 做 ±15 分钟时效校验，而静态信封
 * 的 ts 是 sync.js 发布时烤死的，超过窗口即全军覆没。本层在回传前动态刷新 ts 并用密封密钥的
 * HKDF(macKey) 重算 HMAC（mac 覆盖 ts.nonce.ct.tag；ct 的 GCM AAD 为空，ts 不参与解密，
 * 密文/标签/nonce 原样保留）。密钥与客户端同源(update-keys.json)，重算失败(密钥不匹配等)
 * 时原样透传静态信封，退化为旧行为。
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

/* ---- v5.2.5: 动态刷新 ts 并重签 HMAC ---- */
/** HKDF-SHA256 (RFC 5869)，单块 OKM(≤32 字节) */
function upd_hkdf_sha256(string $ikm, string $salt, string $info, int $len): string {
  $prk = hash_hmac('sha256', $ikm, $salt, true);
  $okm = hash_hmac('sha256', $info . chr(1), $prk, true);
  return substr($okm, 0, $len);
}
/** 读取密封密钥列表(与 sync.js 同源)；命中即用，读不到返回空数组 */
function upd_master_keys(): array {
  static $cached = null;
  if ($cached !== null) return $cached;
  $candidates = [
    '/www/wwwroot/data/update-keys.json',
    '/opt/dsh/workspace/easyssh-site/tools/update-keys.json',
  ];
  $keys = [];
  foreach ($candidates as $p) {
    $raw = is_readable($p) ? (string)@file_get_contents($p) : '';
    if ($raw === '') continue;
    $j = json_decode($raw, true);
    if (!is_array($j)) continue;
    foreach (['masterKeyBase64', 'masterPrevBase64'] as $f) {
      $b = base64_decode((string)($j[$f] ?? ''), true);
      if ($b !== false && strlen($b) === 32) $keys[] = $b;
    }
    if ($keys) break;
  }
  $cached = $keys;
  return $keys;
}
/** 客户端同款 mac_key：HKDF-SHA256(master, "easyssh-update-check-v1", "hmac-sha256", 32) */
function upd_mac_key(string $master): string {
  return upd_hkdf_sha256($master, 'easyssh-update-check-v1', 'hmac-sha256', 32);
}
$oldTs = (int)($env['ts'] ?? 0);
$oldMacData = $oldTs . '.' . ($env['nonce'] ?? '') . '.' . ($env['ct'] ?? '') . '.' . ($env['tag'] ?? '');
$sealKey = null;
foreach (upd_master_keys() as $k) {
  $want = base64_encode(hash_hmac('sha256', $oldMacData, upd_mac_key($k), true));
  if (hash_equals($want, (string)($env['mac'] ?? ''))) { $sealKey = $k; break; }
}
if ($sealKey !== null) {
  $env['ts'] = (int)round(microtime(true) * 1000);
  $macData = $env['ts'] . '.' . $env['nonce'] . '.' . $env['ct'] . '.' . $env['tag'];
  $env['mac'] = base64_encode(hash_hmac('sha256', $macData, upd_mac_key($sealKey), true));
}
// 密钥不匹配/缺失时不动信封：客户端按 ts 校验报过期，行为与修复前一致（不造假签名）

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
