<?php
/**
 * GET/POST /api/download/token.php —— 下载直链隐藏化：签名 Token 签发（R-20261119-007）
 *
 * 流程：前端点击下载按钮 → 先请求 ticket.php（归因）→ 再 POST 本接口（file=basename）
 *       → 返回 { ok:true, url:"/api/download/file.php?t=<token>" } → 前端跳转该 url。
 *
 * 白名单：动态扫描 webroot downloads/ 目录下实际存在的
 *   EasySSH-*.exe / EasySSH-*.zip / EasySSH-*-source.zip / SHA256SUMS；
 *   白名单外一律 403，绝不透传任意路径（防路径穿越）。
 *
 * token 策略：
 *   token = 随机 32 hex + "." + HMAC-SHA256(前半段, 密钥) hex
 *   密钥持久化在 webroot 之外 data/download-secret.txt（不存在时 0600 创建，64 hex）。
 *   会话记录 data/download-sessions.json：{ file, ip, created, last_activity, active_conns }
 *   有效期 = 滑动窗口：从「最后一次成功请求」起 10 分钟内同 token 可再次发起（断线续传不断线）；
 *   传输期间不校验；同 token 并发上限 16（file.php 校验），本接口同步预检；
 *   IP 绑定宽松模式 IP_BIND_MODE='loose24'：同 /24 网段放行（可 'strict'/'off'）。
 *
 * 存储：data/（webroot 之外，HTTP 不可直接访问）；同 IP 限频复用 fb_ip_rate_ok。
 */

require __DIR__ . '/../feedback/common.php';   // fb_client_ip / fb_ip_rate_ok / fb_json_file_load / fb_json_file_save

// ==================== 常量 ====================
define('DL_DATA_DIR', dirname(__DIR__, 3) . '/data');              // webroot 之外
define('DL_SECRET', DL_DATA_DIR . '/download-secret.txt');
define('DL_SESSIONS', DL_DATA_DIR . '/download-sessions.json');
define('DL_RATE_FILE', DL_DATA_DIR . '/download-token-ratelimit.json');
define('DL_WEBROOT', dirname(__DIR__, 2));                          // = /www/wwwroot/easyssh.cn
define('DL_DOWNLOADS', DL_WEBROOT . '/downloads');
define('DL_WINDOW_MS', 10 * 60 * 1000);        // 滑动窗口：最后一次成功请求起 10 分钟
define('DL_CONCURRENCY_MAX', 16);              // 同 token 并发上限
define('DL_RATE_PER_HOUR', 30);                // 同 IP 签发上限（滑动小时窗）
define('IP_BIND_MODE', 'loose24');             // 'loose24' | 'strict' | 'off'

// ==================== 工具 ====================
function dl_json_out($obj, $code = 200) {
  http_response_code($code);
  header('Content-Type: application/json; charset=utf-8');
  header('Cache-Control: no-store');
  echo json_encode($obj, JSON_UNESCAPED_UNICODE);
  exit;
}

function dl_fail($msg, $code = 400) { dl_json_out(['ok' => false, 'message' => $msg], $code); }

/** 读取（不存在则 0600 创建）HMAC 密钥（64 hex） */
function dl_secret() {
  if (is_readable(DL_SECRET)) {
    $s = trim((string)file_get_contents(DL_SECRET));
    if (preg_match('/^[0-9a-f]{64}$/', $s)) return $s;
  }
  if (!is_dir(DL_DATA_DIR)) @mkdir(DL_DATA_DIR, 0775, true);
  $s = bin2hex(random_bytes(32));
  if (@file_put_contents(DL_SECRET, $s . "\n", LOCK_EX) === false) dl_fail('密钥初始化失败', 500);
  @chmod(DL_SECRET, 0600);
  return $s;
}

/**
 * 白名单：扫描 downloads/ 下实际存在的产物。
 * 返回 basename => true 映射。
 */
function dl_whitelist() {
  $out = [];
  foreach ((array)@glob(DL_DOWNLOADS . '/EasySSH-*.exe') as $f) { $out[basename($f)] = true; }
  foreach ((array)@glob(DL_DOWNLOADS . '/EasySSH-*.zip') as $f) { $out[basename($f)] = true; }
  foreach ((array)@glob(DL_DOWNLOADS . '/EasySSH-*-source.zip') as $f) { $out[basename($f)] = true; }
  foreach ((array)@glob(DL_DOWNLOADS . '/SHA256SUMS') as $f) { $out[basename($f)] = true; }
  return $out;
}

/** 校验 token 签名，合法返回 32 hex id，否则 null */
function dl_token_verify($token, $secret) {
  if (!is_string($token) || strlen($token) !== 97 || substr($token, 32, 1) !== '.') return null;
  $id = substr($token, 0, 32);
  $sig = substr($token, 33);
  if (!ctype_xdigit($id) || !ctype_xdigit($sig)) return null;
  if (!hash_equals(hash_hmac('sha256', $id, $secret), $sig)) return null;
  return $id;
}

/** /24 网段化（IPv4），IPv6 按完整地址 */
function dl_ip_key($ip) {
  if (IP_BIND_MODE === 'off') return '';
  if (IP_BIND_MODE === 'strict') return (string)$ip;
  if (filter_var($ip, FILTER_VALIDATE_IP, FILTER_FLAG_IPV4)) return long2ip(ip2long($ip) & 0xFFFFFF00); // /24
  return (string)$ip;
}

// ==================== 主流程 ====================
header('Cache-Control: no-store');

$now = (int)(microtime(true) * 1000);
$ip = fb_client_ip();

// 同 IP 限频（滑动小时窗口，复用 feedback/common.php）
if (!fb_ip_rate_ok(DL_RATE_FILE, DL_RATE_PER_HOUR, 3600 * 1000, 5000)) {
  dl_fail('请求过于频繁，请稍后再试', 429);
}

// file 参数：仅 basename，白名单校验
$file = isset($_REQUEST['file']) ? basename((string)$_REQUEST['file']) : '';
if ($file === '' || !isset(dl_whitelist()[$file])) {
  dl_fail('文件不存在或不在下载清单内', 403);
}

// ---- 会话管理 ----
$sessions = fb_json_file_load(DL_SESSIONS);
if (!is_array($sessions)) $sessions = [];

// 清理：滑动窗口过期的会话
foreach ($sessions as $id => $s) {
  if (!is_array($s) || $now - (int)($s['last_activity'] ?? 0) > DL_WINDOW_MS) unset($sessions[$id]);
}
if (count($sessions) > 3000) $sessions = array_slice($sessions, -1500, null, true); // 防膨胀

// 已有同 file+同网段(或同IP)且仍在窗口内的活跃会话 → 直接复用（避免刷 token）
$reuse = null;
foreach ($sessions as $id => $s) {
  if (!is_array($s) || (string)($s['file'] ?? '') !== $file) continue;
  if ($now - (int)($s['last_activity'] ?? 0) > DL_WINDOW_MS) continue;
  if ((int)($s['active_conns'] ?? 0) >= DL_CONCURRENCY_MAX) continue;
  if (dl_ip_key((string)($s['ip'] ?? '')) !== dl_ip_key($ip)) continue;
  $reuse = $id;
  break;
}

$secret = dl_secret();

if ($reuse !== null) {
  $sessions[$reuse]['last_activity'] = $now;
  fb_json_file_save(DL_SESSIONS, $sessions);
  $id = $reuse; // 会话 id 即 token 前半段；以当前密钥重签（密钥不变则同串）
  $token = $id . '.' . hash_hmac('sha256', $id, $secret);
  dl_json_out(['ok' => true, 'url' => '/api/download/file.php?t=' . $token]);
}

// ---- 新签发 ----
$id = bin2hex(random_bytes(16));
$sessions[$id] = [
  'file' => $file,
  'ip' => (string)$ip,
  'created' => $now,
  'last_activity' => $now,
  'active_conns' => 0,
];
fb_json_file_save(DL_SESSIONS, $sessions);

$token = $id . '.' . hash_hmac('sha256', $id, $secret);
dl_json_out(['ok' => true, 'url' => '/api/download/file.php?t=' . $token]);
