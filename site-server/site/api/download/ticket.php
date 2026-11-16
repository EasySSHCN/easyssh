<?php
/**
 * GET/POST /api/download/ticket.php —— 下载票据签发（归因关联服务端第一期）
 *
 * 流程：用户点击官网下载按钮 → JS 先请求本接口 → 签发票据并记录 → 随后前端再触发 exe 下载。
 * 票据 = { tok(随机32字节hex), id(访客流水ID), visitId, src(来源分类), url(完整来源URL),
 *          ip(完整IP), ts, expire(归因匹配窗截止), attributed:[](已归因设备,上限10台) }
 * 来源判定：取访客流水中「同完整IP + 10分钟内」最近一次访问的 rf(完整来源URL) 分类。
 * 匹配窗 7 天（窗口内首报精确归因 via=ticket/high；超窗未消费且 IP 全等仍归因但 confidence=low 疑似）；
 * 票据记录保留 90 天后清理；多设备共用一张票（绿色版复制场景），归因设备数上限 10 台。
 *
 * 隐私：tok 为随机数,不含任何个人身份信息;归因建立的是「匿名访问 ↔ 匿名设备」的弱关联。
 * 存储：data/stats-tickets.json（webroot 之外,与 stats-db.json 同目录,条数上限+原子写）。
 */

require __DIR__ . '/../feedback/common.php';   // fb_client_ip / fb_json_file_load / fb_json_file_save
require __DIR__ . '/../visit/common.php';      // vst_db_load / vst_src_classify 等(来源分类在 visit/common.php)

define('TK_FILE', dirname(__DIR__, 3) . '/data/stats-tickets.json');
define('TK_MATCH_WINDOW_MS', 7 * 24 * 3600 * 1000);  // 归因匹配窗(默认7天,可调)
define('TK_VISIT_LOOKBACK_MS', 10 * 60 * 1000);      // 签发时向前回溯访客流水的时间窗
define('TK_KEEP_MS', 90 * 24 * 3600 * 1000);         // 票据记录保留期(90天)
define('TK_MAX', 5000);                              // 票据条数上限(防灌水)
define('TK_RATE_PER_HOUR', 30);                      // 同 IP 每小时签发上限
define('TK_RATE_FILE', dirname(__DIR__, 3) . '/data/tickets-ratelimit.json');

header('Cache-Control: no-store');

$now = (int)(microtime(true) * 1000);
$ip = fb_client_ip();

// 同 IP 频控（滑动小时窗口,与验证码签发同一做法）
if (!fb_ip_rate_ok(TK_RATE_FILE, TK_RATE_PER_HOUR, 3600 * 1000, 5000)) {
  header('Content-Type: application/json; charset=utf-8');
  http_response_code(429);
  echo json_encode(['ok' => false, 'message' => '请求过于频繁'], JSON_UNESCAPED_UNICODE);
  exit;
}

// ---- 关联访客流水：找「同完整IP + TK_VISIT_LOOKBACK_MS 内」最近一次访问 ----
$vdb = vst_db_load();
$visit = null;
foreach ($vdb['recent'] as $r) {
  if (!is_array($r)) continue;
  if ((string)($r['f'] ?? '') !== (string)$ip) continue;
  if ($now - (int)($r['t'] ?? 0) > TK_VISIT_LOOKBACK_MS) continue; // recent 最新在前,超出窗口即可停止
  $visit = $r;
  break;
}

$refFull = $visit ? (string)($visit['rf'] ?? '') : '';
$src = vst_src_classify($refFull);

// ---- 票据入库（上限保护 + 过期清理 + 原子写） ----
$j = fb_json_file_load(TK_FILE);
if (!isset($j['tickets']) || !is_array($j['tickets'])) $j = ['tickets' => [], 'meta' => ['created' => $now]];
$tickets = &$j['tickets'];
// 清理：超过保留期(90天)的票据删除
foreach ($tickets as $k => $t) {
  if (!is_array($t) || (int)($t['ts'] ?? 0) < $now - TK_KEEP_MS) unset($tickets[$k]);
}
if (count($tickets) >= TK_MAX) {
  // 超量：丢弃最早签发的（下载票据量级=下载量,正常远低于上限）
  uasort($tickets, function ($a, $b) { return (int)($a['ts'] ?? 0) <=> (int)($b['ts'] ?? 0); });
  $tickets = array_slice($tickets, (int)(TK_MAX / 4), null, true);
}
$tok = bin2hex(random_bytes(32));
$tickets[$tok] = [
  'id' => $visit ? (string)($visit['id'] ?? '') : '',           // 访客流水ID(存量记录无 id 则为空,归因时按 t+ip 兜底匹配)
  'visitId' => $visit ? (string)($visit['id'] ?? '') : '',
  'src' => $src,
  'url' => $refFull,
  'ip' => (string)$ip,
  'ts' => $now,
  'expire' => $now + TK_MATCH_WINDOW_MS,
  'attributed' => [],
];
$j['meta']['updated'] = $now;
$j['meta']['count'] = count($tickets);
fb_json_file_save(TK_FILE, $j);

header('Content-Type: application/json; charset=utf-8');
echo json_encode(['ok' => true, 'tok' => $tok, 'src' => $src], JSON_UNESCAPED_UNICODE);
