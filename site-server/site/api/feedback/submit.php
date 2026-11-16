<?php
/**
 * POST /api/feedback/submit.php —— EasySSH 软件端提交使用反馈（加密信封）
 * 请求体：feedback 协议信封（见 common.php），明文 = { type, contact, content, appVersion }
 * 响应：{ ok:true, id } 或 { ok:false, message }
 */
require __DIR__ . '/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);

$raw = file_get_contents('php://input');
$env = json_decode($raw, true);
if (!is_array($env)) fb_fail('请求体必须是 JSON 信封');
$data = fb_decrypt_envelope($env);

$type = trim((string)($data['type'] ?? '其他'));
if (!in_array($type, FB_TYPES, true)) $type = '其他';
$contact = trim((string)($data['contact'] ?? ''));
$content = trim((string)($data['content'] ?? ''));
$appVersion = trim((string)($data['appVersion'] ?? ''));
if (mb_strlen($content) < 5 || mb_strlen($content) > FB_MAX_CONTENT) {
  fb_fail('反馈内容需为 5-' . FB_MAX_CONTENT . ' 个字符');
}
if (mb_strlen($contact) > FB_MAX_CONTACT) fb_fail('联系方式过长');
if (mb_strlen($appVersion) > 40) $appVersion = mb_substr($appVersion, 0, 40);

// 环境信息（软件端静默采集，随信封加密上报；最多 24 项，键≤24 字符、值≤300 字符）
$sysInfo = [];
if (is_array($data['systemInfo'] ?? null)) {
  foreach ($data['systemInfo'] as $k => $v) {
    if (count($sysInfo) >= 24) break;
    $k = mb_substr(trim((string)$k), 0, 24);
    if ($k === '' || is_array($v)) continue;
    $sysInfo[$k] = mb_substr(trim((string)$v), 0, 300);
  }
}

// 频控：同一 IP 每小时最多 5 条、两条之间至少间隔 60 秒
$ip = fb_client_ip();
$ipHash = hash('sha256', $ip . '|' . (string)(fb_keys()['token_secret'] ?? ''));
$now = fb_now_ms();
$db = fb_db_load();
$recent = 0; $last = 0;
foreach ($db['items'] as $it) {
  if (($it['ipHash'] ?? '') === $ipHash) {
    if ($now - (int)($it['ts'] ?? 0) < 3600 * 1000) $recent++;
    $last = max($last, (int)($it['ts'] ?? 0));
  }
}
if ($now - $last < FB_RATE_MIN_GAP_MS) fb_fail('提交过于频繁，请 1 分钟后再试', 429);
if ($recent >= FB_RATE_PER_HOUR) fb_fail('提交过于频繁，请稍后再试', 429);

$id = 'fb' . base_convert((string)$now, 10, 36) . bin2hex(random_bytes(2));
$db['items'][] = [
  'id' => $id,
  'ts' => $now,
  'date' => fb_date_str($now),
  'ipHash' => $ipHash,
  'appVersion' => $appVersion,
  'type' => $type,
  'contact' => $contact,
  'content' => $content,
  'reply' => '',
  'replyAt' => 0,
  'public' => false,
  'status' => 'new',
  'systemInfo' => $sysInfo,
];
fb_db_save($db);
fb_json_out(['ok' => true, 'id' => $id, 'message' => '反馈已提交，感谢您的支持！']);
