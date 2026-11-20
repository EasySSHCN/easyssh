<?php
/**
 * GET /api/download/file.php?t=<token> —— 签名 Token 中转流式下载（R-20261119-007）
 *
 * 校验链：签名 → 会话存在 → 滑动窗口(最后成功请求起 10 分钟) → IP 绑定 → 并发上限(16)。
 * 通过后直接流式输出 downloads/ 下真实文件（绝不 302 重定向）：
 *   Content-Type: application/octet-stream
 *   Content-Disposition: attachment; filename="原名"
 *   Accept-Ranges: bytes / Content-Length / Range 206 断点续传
 * 每次成功请求（含 206）更新 last_activity 与下载日志 data/download-log.json（保留最近 5000 条）。
 * 传输期间不再校验（会话校验只在响应开始前）。
 */

require __DIR__ . '/../feedback/common.php';   // fb_client_ip / fb_json_file_load / fb_json_file_save

// ==================== 常量（与 token.php 保持一致） ====================
define('DL_DATA_DIR', dirname(__DIR__, 3) . '/data');
define('DL_SECRET', DL_DATA_DIR . '/download-secret.txt');
define('DL_SESSIONS', DL_DATA_DIR . '/download-sessions.json');
define('DL_LOG', DL_DATA_DIR . '/download-log.json');
define('DL_WEBROOT', dirname(__DIR__, 2));   // = /www/wwwroot/easyssh.cn
define('DL_DOWNLOADS', DL_WEBROOT . '/downloads');
define('DL_WINDOW_MS', 10 * 60 * 1000);
define('DL_CONCURRENCY_MAX', 16);
define('DL_CHUNK', 64 * 1024);          // 流式分块大小
define('IP_BIND_MODE', 'loose24');      // 'loose24' | 'strict' | 'off'

function dl_fail($msg, $code = 400) {
  http_response_code($code);
  header('Content-Type: application/json; charset=utf-8');
  header('Cache-Control: no-store');
  echo json_encode(['ok' => false, 'message' => $msg], JSON_UNESCAPED_UNICODE);
  exit;
}

function dl_secret() {
  if (!is_readable(DL_SECRET)) dl_fail('下载服务未初始化', 500);
  $s = trim((string)file_get_contents(DL_SECRET));
  if (!preg_match('/^[0-9a-f]{64}$/', $s)) dl_fail('下载服务配置异常', 500);
  return $s;
}

function dl_ip_key($ip) {
  if (IP_BIND_MODE === 'off') return '';
  if (IP_BIND_MODE === 'strict') return (string)$ip;
  if (filter_var($ip, FILTER_VALIDATE_IP, FILTER_FLAG_IPV4)) return long2ip(ip2long($ip) & 0xFFFFFF00);
  return (string)$ip;
}

function dl_log_add($file, $range) {
  $now = (int)(microtime(true) * 1000);
  $j = fb_json_file_load(DL_LOG);
  if (!isset($j['items']) || !is_array($j['items'])) $j = ['items' => [], 'meta' => ['created' => $now]];
  $j['items'][] = ['file' => $file, 'ip' => (string)fb_client_ip(), 'range' => $range, 'ts' => $now];
  if (count($j['items']) > 5000) $j['items'] = array_slice($j['items'], -5000);
  $j['meta']['updated'] = $now;
  $j['meta']['count'] = count($j['items']);
  fb_json_file_save(DL_LOG, $j);
}

// ==================== 校验 ====================
header('Cache-Control: no-store');
$now = (int)(microtime(true) * 1000);
$ip = fb_client_ip();
$secret = dl_secret();

$token = isset($_GET['t']) ? (string)$_GET['t'] : '';
if (strlen($token) !== 97 || substr($token, 32, 1) !== '.') dl_fail('token 非法', 403);
$id = substr($token, 0, 32);
$sig = substr($token, 33);
if (!ctype_xdigit($id) || !ctype_xdigit($sig)) dl_fail('token 非法', 403);
if (!hash_equals(hash_hmac('sha256', $id, $secret), $sig)) dl_fail('token 签名校验失败', 403);

$sessions = fb_json_file_load(DL_SESSIONS);
if (!is_array($sessions)) $sessions = [];

// 清理过期会话（顺带）
foreach ($sessions as $k => $s) {
  if (!is_array($s) || $now - (int)($s['last_activity'] ?? 0) > DL_WINDOW_MS) unset($sessions[$k]);
}

if (!isset($sessions[$id]) || !is_array($sessions[$id])) dl_fail('token 已失效', 403);
$s = $sessions[$id];

// 滑动窗口
if ($now - (int)($s['last_activity'] ?? 0) > DL_WINDOW_MS) { unset($sessions[$id]); fb_json_file_save(DL_SESSIONS, $sessions); dl_fail('token 已过期', 403); }

// IP 绑定（宽松 /24）
if (dl_ip_key((string)($s['ip'] ?? '')) !== dl_ip_key($ip)) dl_fail('来源校验失败', 403);

// 并发上限（传输期间不校验——校验只在开始前；active_conns 由开始前自增/结束时递减）
if ((int)($s['active_conns'] ?? 0) >= DL_CONCURRENCY_MAX) dl_fail('该下载并发已达上限', 429);

// 文件白名单二次确认
$file = basename((string)($s['file'] ?? ''));
$path = DL_DOWNLOADS . '/' . $file;
if ($file === '' || !is_file($path)) dl_fail('文件不存在', 404);
$size = filesize($path);
if ($size === false) dl_fail('文件读取失败', 500);

// ---- 锁会话：并发 +1、刷新滑动窗口 ----
$s['active_conns'] = (int)($s['active_conns'] ?? 0) + 1;
$s['last_activity'] = $now;
$sessions[$id] = $s;
fb_json_file_save(DL_SESSIONS, $sessions);
$connLocked = true;

// ---------- Range 解析（支持 bytes=start-end / bytes=start- / bytes=-suffix） ----------
$start = 0; $end = $size - 1; $rangeHeader = '';
$h = isset($_SERVER['HTTP_RANGE']) ? trim((string)$_SERVER['HTTP_RANGE']) : '';
if ($h !== '') {
  if (!preg_match('/^bytes=(\d*)-(\d*)$/i', $h, $m) || ($m[1] === '' && $m[2] === '')) {
    // 单一 Range 以外的多段/非法 Range：忽略按 200 全量（简化，兼容性好）
  } else {
    if ($m[1] !== '') {
      $start = (int)$m[1];
      $end = ($m[2] !== '') ? min((int)$m[2], $size - 1) : $size - 1;
    } else {
      $suffix = (int)$m[2];
      $start = max(0, $size - $suffix);
      $end = $size - 1;
    }
    if ($start > $end || $start >= $size) {
      header('Content-Range: bytes */' . $size);
      dl_fail('请求范围不符合当前文件大小', 416);
    }
    $rangeHeader = $h;
    http_response_code(206);
    header('Content-Range: bytes ' . $start . '-' . $end . '/' . $size);
  }
}

// ---------- 输出头 ----------
$length = $end - $start + 1;
header('Content-Type: application/octet-stream');
header('Content-Disposition: attachment; filename="' . $file . '"');
header('Accept-Ranges: bytes');
header('Content-Length: ' . $length);
header('X-Content-Type-Options: nosniff');
// 中转下载：禁止任何缓存/代理缓存
header('Cache-Control: private, no-store');

// 成功请求记日志（含 206）
dl_log_add($file, $rangeHeader !== '' ? $rangeHeader : 'full');

// ---------- 流式输出 ----------
ignore_user_abort(true);
@set_time_limit(0);
while (ob_get_level() > 0) { @ob_end_flush(); }

$fh = @fopen($path, 'rb');
if (!$fh) dl_fail('文件读取失败', 500);
fseek($fh, $start);

$remaining = $length;
while ($remaining > 0 && !feof($fh)) {
  $chunk = fread($fh, min(DL_CHUNK, $remaining));
  if ($chunk === false || $chunk === '') break;
  echo $chunk;
  $remaining -= strlen($chunk);
  flush();
  if (connection_aborted()) break;
}
fclose($fh);

// ---------- 结束：并发 -1、刷新滑动窗口 ----------
if ($connLocked) {
  $sessions = fb_json_file_load(DL_SESSIONS);
  if (isset($sessions[$id]) && is_array($sessions[$id])) {
    $sessions[$id]['active_conns'] = max(0, (int)($sessions[$id]['active_conns'] ?? 1) - 1);
    $sessions[$id]['last_activity'] = (int)(microtime(true) * 1000); // 最后一次成功请求 → 滑动窗口续期
    fb_json_file_save(DL_SESSIONS, $sessions);
  }
}
exit;
