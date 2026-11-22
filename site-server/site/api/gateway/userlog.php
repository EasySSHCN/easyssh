<?php
/**
 * POST /api/gateway/userlog.php —— 用户日志弹窗后端（GWLOG-20261122，对齐 EasyRDP grants-admin 口径）
 *
 * 鉴权：与 audit.php 同模式（{user, token} → fb_check_token 站长会话）。
 * 数据：data/gateway-userlogs.json（issue.php / admin.php 埋点写入）。
 *
 * 动作（JSON body）：
 *   { action:"user_log",  user, token, email, page?, pageSize? }
 *     → { ok, items:[{id,key,ts,op,detail,ip}], total, page, pageSize }
 *     （该用户操作日志分页，最新在前；id/key 为数组定位键，供删除）
 *   { action:"delete_ops", user, token, email, ids:[key...] }
 *     → { ok, removed, message }
 *     （删除该用户的操作日志行；仅限 email 匹配行，admin_* 留痕行 email=管理员名，天然受保护；
 *       删除动作本身留一条 admin_delete_ops 管理员留痕）
 *   { action:"user_audit", user, token, email, page?, pageSize? }
 *     → { ok, items:[{timestamp,user,event,source_ip,resource,result}], total, page, pageSize }
 *     （代理网关 GET /admin/audit?user=<email>&limit=500，服务端分页切片）
 *   { action:"userlog", ... } —— 旧版兼容保留（旧前端 glog 行格式）
 */

require __DIR__ . '/../feedback/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);
$in = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($in)) fb_fail('请求体必须是 JSON');
$action = (string)($in['action'] ?? '');
$user = (string)($in['user'] ?? '');
$token = (string)($in['token'] ?? '');
if (!fb_check_token($user, $token)) fb_fail('未登录或登录已过期，请重新登录', 401);

$logFile = FB_DATA_DIR . '/gateway-userlogs.json';

/** 管理员操作留痕（与 admin.php ga_admin_op 同构） */
function ul_admin_op(string $adminUser, string $op, string $detail): void {
  global $logFile;
  $ul = fb_json_file_load($logFile);
  if (!is_array($ul)) $ul = [];
  $maxId = 0;
  foreach ($ul as $e) { if (is_array($e) && (int)($e['id'] ?? 0) > $maxId) $maxId = (int)$e['id']; }
  $ul[] = ['id' => $maxId + 1, 'ts' => fb_now_ms(), 'email' => $adminUser,
    'action' => $op, 'detail' => $detail, 'ip' => fb_client_ip()];
  fb_json_file_save($logFile, $ul);
}

switch ($action) {

  /* ---------- 用户操作日志分页（最新在前；id/key 供 delete_ops 定位） ---------- */
  case 'user_log': {
    $email = strtolower(trim((string)($in['email'] ?? '')));
    if (!filter_var($email, FILTER_VALIDATE_EMAIL)) fb_fail('邮箱格式非法');
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = (int)($in['pageSize'] ?? 20);
    if ($pageSize < 1 || $pageSize > 100) $pageSize = 20;
    $db = fb_json_file_load($logFile);
    if (!is_array($db)) $db = [];
    $rows = [];
    foreach ($db as $key => $rec) {
      if (!is_array($rec)) continue;
      if (strtolower((string)($rec['email'] ?? '')) !== $email) continue;
      $rows[] = ['key' => (int)$key, 'id' => (int)($rec['id'] ?? 0) ?: (int)$key,
        'ts' => (int)($rec['ts'] ?? 0), 'op' => (string)($rec['action'] ?? ''),
        'detail' => (string)($rec['detail'] ?? ''), 'ip' => (string)($rec['ip'] ?? '-')];
    }
    $rows = array_reverse($rows); // 最新在前
    $total = count($rows);
    $pages = max(1, (int)ceil($total / $pageSize));
    $page = min($page, $pages);
    $items = array_slice($rows, ($page - 1) * $pageSize, $pageSize);
    header('Content-Type: application/json; charset=utf-8');
    header('Cache-Control: no-store');
    echo json_encode(['ok' => true, 'items' => $items, 'total' => $total, 'page' => $page,
      'pageSize' => $pageSize, 'pages' => $pages], JSON_UNESCAPED_UNICODE);
    exit;
  }

  /* ---------- 删除该用户操作日志（仅限 email 匹配行；删除动作留 admin_delete_ops） ---------- */
  case 'delete_ops': {
    $email = strtolower(trim((string)($in['email'] ?? '')));
    if (!filter_var($email, FILTER_VALIDATE_EMAIL)) fb_fail('邮箱格式非法');
    $ids = $in['ids'] ?? [];
    if (!is_array($ids) || !count($ids) || count($ids) > 500) fb_fail('ids 为空或超限（1-500）');
    $keys = [];
    foreach ($ids as $k) {
      if (!is_numeric($k) || (int)$k < 0) fb_fail('存在不合法的日志 id');
      $keys[] = (int)$k;
    }
    $db = fb_json_file_load($logFile);
    if (!is_array($db)) fb_fail('日志库不可用', 500);
    $removed = 0;
    foreach ($keys as $k) {
      if (!isset($db[$k]) || !is_array($db[$k])) continue;
      if (strtolower((string)($db[$k]['email'] ?? '')) !== $email) continue; // 仅限本人日志行
      unset($db[$k]);
      $removed++;
    }
    if (!fb_json_file_save($logFile, $db)) fb_fail('服务器存储写入失败', 500);
    ul_admin_op($user, 'admin_delete_ops', '管理员删除用户操作日志：' . $email . ' 成功 ' . $removed . ' / 请求 ' . count($keys) . ' 条');
    fb_json_out(['ok' => true, 'removed' => $removed,
      'message' => '已删除 ' . $removed . ' 条操作日志']);
  }

  /* ---------- 该用户的网关审计事件分页（代理网关 ?user= 过滤，limit=500 后切片） ---------- */
  case 'user_audit': {
    $email = strtolower(trim((string)($in['email'] ?? '')));
    if (!filter_var($email, FILTER_VALIDATE_EMAIL)) fb_fail('邮箱格式非法');
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = (int)($in['pageSize'] ?? 20);
    if ($pageSize < 1 || $pageSize > 100) $pageSize = 20;
    $keysFile = __DIR__ . '/keys.php';
    if (!is_readable($keysFile)) fb_fail('网关配置缺失（keys.php）', 500);
    $k = json_decode((string)preg_replace('/^<\?php exit\(0\); \?>\s*/', '', (string)file_get_contents($keysFile)), true);
    $url = rtrim((string)$k['gateway_url'], '/') . '/admin/audit?user=' . urlencode($email) . '&limit=500';
    $ctx = stream_context_create(['http' => ['method' => 'GET', 'timeout' => 5, 'ignore_errors' => true,
      "header" => "X-Admin-Key: " . (string)$k['gateway_admin_key'] . "\r\nUser-Agent: easyssh-site-userlog\r\n"]]);
    $resp = @file_get_contents($url, false, $ctx);
    if ($resp === false) fb_fail('网关不可达，请稍后重试', 502);
    $j = json_decode((string)$resp, true);
    if (!is_array($j) || ($j['ok'] ?? false) !== true || !isset($j['events']) || !is_array($j['events'])) {
      fb_fail('网关审计读取失败，请稍后重试', 502);
    }
    $events = array_values($j['events']); // 网关已按最新在前排序
    $total = count($events);
    $pages = max(1, (int)ceil($total / $pageSize));
    $page = min($page, $pages);
    $items = array_slice($events, ($page - 1) * $pageSize, $pageSize);
    header('Content-Type: application/json; charset=utf-8');
    header('Cache-Control: no-store');
    echo json_encode(['ok' => true, 'items' => $items, 'total' => $total,
      'page' => $page, 'pageSize' => $pageSize, 'pages' => $pages], JSON_UNESCAPED_UNICODE);
    exit;
  }

  /* ---------- 旧版兼容：userlog 行格式（保留给历史调用方） ---------- */
  case 'userlog': {
    $email = strtolower(trim((string)($in['email'] ?? '')));
    if (!filter_var($email, FILTER_VALIDATE_EMAIL)) fb_fail('邮箱格式非法');
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = (int)($in['page_size'] ?? 20);
    if (!in_array($pageSize, [10, 20, 50, 100], true)) $pageSize = 20;
    $db = fb_json_file_load($logFile);
    if (!is_array($db)) $db = [];
    $mine = [];
    foreach ($db as $rec) {
      if (!is_array($rec)) continue;
      if (strtolower((string)($rec['email'] ?? '')) !== $email) continue;
      $mine[] = ['ts' => (int)($rec['ts'] ?? 0), 'email' => (string)($rec['email'] ?? $email),
        'action' => (string)($rec['action'] ?? ''), 'detail' => (string)($rec['detail'] ?? '')];
    }
    $mine = array_reverse($mine);
    $total = count($mine);
    $pages = max(1, (int)ceil($total / $pageSize));
    $page = min($page, $pages);
    $records = array_slice($mine, ($page - 1) * $pageSize, $pageSize);
    header('Content-Type: application/json; charset=utf-8');
    header('Cache-Control: no-store');
    echo json_encode(['ok' => true, 'total' => $total, 'page' => $page, 'page_size' => $pageSize,
      'pages' => $pages, 'records' => $records], JSON_UNESCAPED_UNICODE);
    exit;
  }

  default:
    fb_fail('action 非法，仅支持 user_log / delete_ops / user_audit / userlog');
}
