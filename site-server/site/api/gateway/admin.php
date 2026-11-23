<?php
/**
 * POST /api/gateway/admin.php —— 云网关申请管理（EasySSH admin.html「📋 网关申请」标签页后端）
 *
 * 鉴权：与 audit.php 同模式——POST JSON {user, token}，fb_check_token() 校验站长会话。
 * 数据：申请库 <站点上级>/data/gateway-applications.json；资源授权库 <站点上级>/data/gateway-resources.json
 *       （与 issue.php/apply.php/定时审核完全同源，webroot 之外）。
 *
 * 动作（JSON body）：
 *   { action:"list",   user, token }                       → { ok, applications:[{id,email,status,statusText,purpose,contact,resource,granted:[...],appliedAt,reviewedAt,reviewedBy,note}] }
 *   { action:"set_status", user, token, id, status:"approved"|"rejected", note? } → { ok }（人工改判，reviewedBy=human:<user>；approved 会发结果邮件）
 *   { action:"grant",  user, token, email, resource_id, label? }   → { ok }（登记资源授权 → 用户服务台「我的资源」立即可见）
 *   { action:"revoke_r", user, token, email, resource_id }          → { ok }
 *
 * 注意：登记授权只写资源库；网关侧 [[resources]]（后端主机注册）需按 HANDOFF 修改 config.toml 并在运维窗口重启网关生效。
 */

require __DIR__ . '/../feedback/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);
$in = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($in)) fb_fail('请求体必须是 JSON');
$action = (string)($in['action'] ?? '');
$user = (string)($in['user'] ?? '');
$token = (string)($in['token'] ?? '');
if (!fb_check_token($user, $token)) fb_fail('未登录或登录已过期，请重新登录', 401);

define('GA_RES_FILE', FB_DATA_DIR . '/gateway-resources.json');
define('GA_USERS_FILE', FB_DATA_DIR . '/gateway-users.json');
define('GA_APP_FILE2', FB_DATA_DIR . '/gateway-applications.json');
define('GA_USERLOG_FILE', FB_DATA_DIR . '/gateway-userlogs.json');
define('GW_KEYS', __DIR__ . '/keys.php');

function ga_res_load()  { $j = fb_json_file_load(GA_RES_FILE);  return is_array($j) ? $j : []; }
function ga_users_load() { $j = fb_json_file_load(GA_USERS_FILE); return is_array($j) ? $j : []; }
function ga_res_save($db) { return fb_json_file_save(GA_RES_FILE, $db); }
const GA_STATUS_TEXT = ['pending' => '待审核', 'approved' => '已开通', 'rejected' => '拒绝'];

/* ---------- 管理员操作留痕（GW-OPSMODAL-20261122，对齐 EasyRDP user_ops 口径） ----------
 * 追加到 gateway-userlogs.json，action 一律 admin_* 前缀（email 字段存操作人管理员名），
 * 与内存环形审计相互独立、永久留存；后台「管理员操作日志」弹窗查看/删除。 */
function ga_admin_op($adminUser, $op, $detail) {
  $ul = fb_json_file_load(GA_USERLOG_FILE);
  if (!is_array($ul)) $ul = [];
  $maxId = 0;
  foreach ($ul as $e) { if (is_array($e) && (int)($e['id'] ?? 0) > $maxId) $maxId = (int)$e['id']; }
  $ul[] = ['id' => $maxId + 1, 'ts' => fb_now_ms(), 'email' => (string)$adminUser,
    'action' => (string)$op, 'detail' => (string)$detail, 'ip' => fb_client_ip()];
  fb_json_file_save(GA_USERLOG_FILE, $ul);
}
const GA_OPS_PAGE_MAX = 100;

/* ---------- admin_ops：管理员操作留痕分页（admin_* 前缀 + 存量 account_deleted，最新在前） ---------- */
function ga_ops_items($page, $pageSize) {
  $ul = fb_json_file_load(GA_USERLOG_FILE);
  if (!is_array($ul)) $ul = [];
  $rows = [];
  foreach ($ul as $key => $e) {
    if (!is_array($e)) continue;
    $op = (string)($e['action'] ?? '');
    if (strpos($op, 'admin_') === 0 || $op === 'account_deleted') $rows[] = ['key' => (int)$key] + $e;
  }
  $rows = array_reverse($rows); // 最新在前
  $total = count($rows);
  $items = array_slice($rows, ($page - 1) * $pageSize, $pageSize);
  $items = array_map(function ($e) {
    return ['id' => (int)($e['id'] ?? 0), 'key' => (int)$e['key'], 'ts' => (int)($e['ts'] ?? 0),
      'action' => (string)($e['action'] ?? ''), 'detail' => (string)($e['detail'] ?? ''),
      'email' => (string)($e['email'] ?? ''), 'ip' => (string)($e['ip'] ?? '-')];
  }, $items);
  return [$total, $items];
}

switch ($action) {

  case 'list': {
    $users = ga_users_load();
    $res = ga_res_load();
    $userList = [];
    foreach ($users as $email => $u) {
      if (!is_array($u)) continue;
      $verified = array_key_exists('verified', $u) ? ((int)$u['verified'] === 1) : true; // 存量账号豁免
      $userList[] = ['email' => (string)$email, 'createdAt' => (int)($u['createdAt'] ?? 0), 'verified' => $verified];
    }
    $grants = [];
    foreach ($res as $email => $list) {
      foreach ((array)$list as $g) {
        if (!is_array($g)) continue;
        $grants[] = ['email' => (string)$email,
          'resource_id' => (string)($g['resource_id'] ?? ''),
          'label' => (string)($g['label'] ?? ''),
          'status' => (string)($g['status'] ?? ''),
          'grantedAt' => (int)($g['grantedAt'] ?? 0),
          'grantedBy' => (string)($g['grantedBy'] ?? '')];
      }
    }
    header('Content-Type: application/json; charset=utf-8');
    header('Cache-Control: no-store');
    echo json_encode(['ok' => true, 'users' => array_values($userList), 'grants' => $grants], JSON_UNESCAPED_UNICODE);
    exit;
  }
  case 'set_status': {
    $id = (string)($in['id'] ?? '');
    $st = (string)($in['status'] ?? '');
    if ($id === '' || !in_array($st, ['approved', 'rejected', 'pending'], true)) fb_fail('参数非法');
    $db = ga_apps_load();
    if (!isset($db[$id]) || !is_array($db[$id])) fb_fail('申请不存在', 404);
    $rec = &$db[$id];
    $rec['status'] = $st;
    $rec['statusText'] = GA_STATUS_TEXT[$st] ?? $st;
    $rec['reviewedAt'] = fb_now_ms();
    $rec['reviewedBy'] = 'human:' . $user;
    $note = trim((string)($in['note'] ?? ''));
    if ($note !== '') $rec['note'] = $note;
    if (!fb_json_file_save(GA_APP_FILE, $db)) fb_fail('服务器存储写入失败', 500);
    unset($rec);
    // 结果邮件（与定时审核同文案口径）
    $email = (string)($db[$id]['email'] ?? '');
    if ($email !== '' && $st !== 'pending') {
      if ($st === 'approved') {
        $subject = '[EasySSH] 云网关服务申请审核通过';
        $text = "你的云网关服务申请已审核通过（编号 {$id}，管理员人工复核）。\n\n"
              . "下一步：登录云网关用户服务台 https://easyssh.cn/gw-admin.html 在「我的资源」自助签发短期 Token。";
      } else {
        $reason = (string)$db[$id]['note'];
        $subject = '[EasySSH] 云网关服务申请未通过';
        $text = "很遗憾，你的云网关服务申请未通过本次审核（编号 {$id}）。\n原因：{$reason}\n\n可修正后重新提交。";
      }
      fb_mail_send($email, $subject, $text . "\n\n—— EasySSH 云网关服务（本邮件为系统自动发送）");
    }
    fb_json_out(['ok' => true, 'message' => '状态已更新']);
  }


  case 'grant': {
    $email = strtolower(trim((string)($in['email'] ?? '')));
    $rid = trim((string)($in['resource_id'] ?? ''));
    $label = trim((string)($in['label'] ?? $rid));
    if (!filter_var($email, FILTER_VALIDATE_EMAIL)) fb_fail('邮箱非法');
    if ($rid === '' || strlen($rid) > 128) fb_fail('资源 ID 非法');
    $host = strtolower(trim((string)($in['res_host'] ?? '')));
    $port = (int)($in['res_port'] ?? 0);
    $needRegister = ($host !== "" && $port >= 1 && $port <= 65535);
    if ($needRegister) {
    // 调网关运行时注册端点（X-Admin-Key，127.0.0.1 回环，即时生效零重启）
    $keysFile = __DIR__ . '/keys.php';
    if (!is_readable($keysFile)) fb_fail('网关配置缺失（keys.php）', 500);
    $k = json_decode((string)preg_replace('/^<\?php exit\(0\); \?>\s*/', '', (string)file_get_contents($keysFile)), true);
    $url = rtrim((string)$k['gateway_url'], '/') . '/admin/register-resource';
    $body = json_encode(['id'=>$rid, 'name'=>$label, 'host'=>$host, 'port'=>$port, 'allowed_roles'=>['user','admin']], JSON_UNESCAPED_UNICODE);
    $ctx = stream_context_create(['http' => ['method'=>'POST','timeout'=>8,'ignore_errors'=>true,
      "header"=>"Content-Type: application/json; charset=utf-8\r\nX-Admin-Key: ".(string)$k["gateway_admin_key"]."\r\nX-Admin-Client-IP: ".fb_client_ip(),
      'content'=>$body]]);
    $resp = @file_get_contents($url, false, $ctx);
    $code = 0; foreach (($http_response_header ?? []) as $h) { if (preg_match('#^HTTP/\S+\s+(\d{3})#', $h, $m)) { $code = (int)$m[1]; break; } }
    $jr = json_decode((string)$resp, true);
    if ($code >= 400 || !is_array($jr) || empty($jr['ok'])) fb_fail("网关资源注册失败（HTTP {$code}）：".(string)($jr['message'] ?? '响应异常'), 502);
    } // end needRegister

    $db = ga_res_load();
    foreach ((array)($db[$email] ?? []) as $g) {
      if (is_array($g) && (string)($g['resource_id'] ?? '') === $rid && ($g['status'] ?? '') === 'approved') {
        fb_json_out(['ok' => true, 'message' => '该资源已在授权列表中']);
      }
    }
    $db[$email][] = ['resource_id' => $rid, 'label' => $label, 'status' => 'approved', 'grantedAt' => fb_now_ms(), 'grantedBy' => $user];
    if (!ga_res_save($db)) fb_fail('服务器存储写入失败', 500);
    ga_admin_op($user, 'admin_grant', '管理员开通资源授权：' . $email . ' ← ' . $rid . ($needRegister ? '（含网关运行时注册）' : ''));
    fb_json_out(['ok' => true, 'message' => '资源已注册并登记授权，用户服务台「我的资源」立即可见并可自助签发']);
  }

  /* ---------- audit_delete：代理网关按 (timestamp,user) 批量删除审计事件（后台「删除选中」） ---------- */
  case 'audit_delete': {
    $keysFile = __DIR__ . '/keys.php';
    if (!is_readable($keysFile)) fb_fail('网关配置缺失（keys.php）', 500);
    $k = json_decode((string)preg_replace('/^<\?php exit\(0\); \?>\s*/', '', (string)file_get_contents($keysFile)), true);
    $entries = $in['entries'] ?? null;
    if (!is_array($entries) || count($entries) < 1 || count($entries) > 500) fb_fail('entries 为空或超限（1-500）');
    $clean = [];
    foreach ($entries as $e) {
      if (!is_array($e)) continue;
      $ts = (int)($e['timestamp'] ?? 0);
      $u = trim((string)($e['user'] ?? ''));
      if ($ts <= 0 || $u === '') continue;
      $clean[] = ['timestamp' => $ts, 'user' => $u];
    }
    if (!count($clean)) fb_fail('entries 无有效项');
    $url = rtrim((string)$k['gateway_url'], '/') . '/admin/audit-delete';
    $ctx = stream_context_create(['http' => ['method'=>'POST','timeout'=>8,'ignore_errors'=>true,
      "header"=>"Content-Type: application/json; charset=utf-8\r\nX-Admin-Key: ".(string)$k['gateway_admin_key']."\r\nX-Admin-Client-IP: ".fb_client_ip(),
      'content'=>json_encode(['entries'=>$clean], JSON_UNESCAPED_UNICODE)]]);
    $resp = @file_get_contents($url, false, $ctx);
    $code = 0; foreach (($http_response_header ?? []) as $h) { if (preg_match('#^HTTP/\S+\s+(\d{3})#', $h, $m)) { $code = (int)$m[1]; break; } }
    $jr = json_decode((string)$resp, true);
    if ($code >= 400 || !is_array($jr) || empty($jr['ok'])) fb_fail('网关删除失败（HTTP '.$code.'）：'.(string)($jr['message'] ?? '响应异常'), 502);
    $removedN = (int)($jr['removed'] ?? 0);
    // 永久管理员操作留痕（GW-AUDIT-DELOP 同规范：删除动作本身留 admin_audit_delete）
    $usersJoined = implode('、', array_values(array_unique(array_column($clean, 'user'))));
    ga_admin_op($user, 'admin_audit_delete', '管理员删除网关审计记录：成功 ' . $removedN . ' / 请求 ' . count($clean) . ' 条（' . $usersJoined . '）');
    fb_json_out(['ok' => true, 'removed' => $removedN]);
  }

  case 'revoke_r': {
    $email = strtolower(trim((string)($in['email'] ?? '')));
    $rid = trim((string)($in['resource_id'] ?? ''));
    $db = ga_res_load();
    $found = false;
    foreach ((array)($db[$email] ?? []) as $i => $g) {
      if (is_array($g) && (string)($g['resource_id'] ?? '') === $rid) { $db[$email][$i]['status'] = 'revoked'; $found = true; }
    }
    if (!$found) fb_fail('未找到该授权', 404);
    if (!ga_res_save($db)) fb_fail('服务器存储写入失败', 500);
    ga_admin_op($user, 'admin_revoke', '管理员吊销资源授权：' . $email . ' ✕ ' . $rid);
    fb_json_out(['ok' => true, 'message' => '授权已吊销（已签发 token 将在约 10 分钟内自然过期）']);
  }

  /* ---------- 删除用户（完整清除：申请记录/授权/用户/会话 + 网关关联审计清空留删除审计 + 邮件通知） ---------- */
  case 'delete_user': {
    $email = strtolower(trim((string)($in['email'] ?? '')));
    if (!filter_var($email, FILTER_VALIDATE_EMAIL)) fb_fail('邮箱非法');
    // 1) 删除申请记录
    $apps = fb_json_file_load(GA_APP_FILE2);
    if (is_array($apps)) { foreach ($apps as $k => $r) { if (is_array($r) && (string)($r['email'] ?? '') === $email) unset($apps[$k]); } fb_json_file_save(GA_APP_FILE2, $apps); }
    // 2) 删除授权
    $res = ga_res_load();
    if (isset($res[$email])) { unset($res[$email]); ga_res_save($res); }
    // 3) 删除用户与全部会话
    $users = fb_json_file_load(GA_USERS_FILE);
    if (is_array($users) && isset($users[$email])) { unset($users[$email]); fb_json_file_save(GA_USERS_FILE, $users); }
    $sessFile = FB_DATA_DIR . '/gateway-sessions.json';
    $sess = fb_json_file_load($sessFile);
    if (is_array($sess)) { foreach ($sess as $tk => $s) { if (is_array($s) && (string)($s['user'] ?? '') === $email) unset($sess[$tk]); } fb_json_file_save($sessFile, $sess); }
    // 4) 网关侧：清空该用户关联审计 + 留删除审计（X-Admin-Key 回环）
    $keysFile = __DIR__ . '/keys.php';
    $removed = 0;
    if (is_readable($keysFile)) {
      $k = json_decode((string)preg_replace('/^<\?php exit\(0\); \?>\s*/', '', (string)file_get_contents($keysFile)), true);
      $url = rtrim((string)$k['gateway_url'], '/') . '/admin/audit-clear';
      $ctx = stream_context_create(['http' => ['method'=>'POST','timeout'=>8,'ignore_errors'=>true,
        "header"=>"Content-Type: application/json; charset=utf-8\r\nX-Admin-Key: ".(string)$k["gateway_admin_key"]."\r\nX-Admin-Client-IP: ".fb_client_ip(),
        "content"=>json_encode(["user"=>$email], JSON_UNESCAPED_UNICODE)]]);
      $resp = @file_get_contents($url, false, $ctx);
      $j = json_decode((string)$resp, true);
      $removed = (int)($j['removed'] ?? 0);
    }
    // 5) 站点操作日志留删除记录（不随用户删除，供审计追溯；admin_* 前缀进「管理员操作日志」）
    ga_admin_op($user, 'admin_delete_user',
      '管理员删除用户：' . $email . '（申请记录/授权/账号/会话已清除，网关关联审计清空 ' . $removed . ' 条，留 audit_cleared 删除审计）');
    // 6) 邮件通知用户
    fb_mail_send($email, '[EasySSH] 云网关账号已被删除',
      "你的云网关服务账号（{$email}）已被管理员删除：账号、会话与资源授权均已清除。\n\n如对此有疑问，请联系管理员。\n\n—— EasySSH 云网关服务（本邮件为系统自动发送）");
    fb_json_out(['ok' => true, 'message' => '用户已删除（申请记录/授权/账号/会话清除，网关关联审计清空 ' . $removed . ' 条并留删除审计，已邮件通知用户）']);
  }

  /* ---------- set_grant：恢复被吊销的授权（状态 revoked → approved） ---------- */
  case 'set_grant': {
    $email = strtolower(trim((string)($in['email'] ?? '')));
    $rid = trim((string)($in['resource_id'] ?? ''));
    if (!filter_var($email, FILTER_VALIDATE_EMAIL) || $rid === '') fb_fail('参数非法');
    $db = ga_res_load();
    $found = false;
    foreach ((array)($db[$email] ?? []) as $i => $g) {
      if (is_array($g) && (string)($g['resource_id'] ?? '') === $rid) { $db[$email][$i]['status'] = 'approved'; $found = true; }
    }
    if (!$found) fb_fail('未找到该授权', 404);
    if (!ga_res_save($db)) fb_fail('服务器存储写入失败', 500);
    ga_admin_op($user, 'admin_grant', '管理员恢复资源授权（revoked → approved）：' . $email . ' ← ' . $rid);
    fb_json_out(['ok' => true, 'message' => '授权已恢复，用户服务台「我的资源」立即可见']);
  }

  /* ---------- admin_ops：管理员操作留痕分页（GW-OPSMODAL-20261122，对齐 EasyRDP） ---------- */
  case 'admin_ops': {
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = (int)($in['pageSize'] ?? 20);
    if ($pageSize < 1 || $pageSize > GA_OPS_PAGE_MAX) $pageSize = 20;
    list($total, $items) = ga_ops_items($page, $pageSize);
    fb_json_out(['ok' => true, 'items' => $items, 'total' => $total, 'page' => $page, 'pageSize' => $pageSize]);
  }

  /* ---------- admin_op_delete：批量删除管理员操作留痕（按 key 定位，admin_* 前缀与存量 account_deleted 可删；
     用户侧日志（register/login/verify 等）不在权限内；删除动作本身留 admin_ops_delete 痕 ---------- */
  case 'admin_op_delete': {
    $keys = $in['keys'] ?? null;
    if (!is_array($keys) || !count($keys) || count($keys) > 500) fb_fail('keys 为空或超限（1-500）');
    $clean = [];
    foreach ($keys as $k) {
      if (!is_numeric($k) || (int)$k < 0) fb_fail('存在不合法的日志 key');
      $clean[] = (int)$k;
    }
    $ul = fb_json_file_load(GA_USERLOG_FILE);
    if (!is_array($ul)) fb_fail('日志库不可用', 500);
    $removed = 0;
    foreach ($clean as $k) {
      if (!isset($ul[$k]) || !is_array($ul[$k])) continue;
      $op = (string)($ul[$k]['action'] ?? '');
      if (strpos($op, 'admin_') !== 0 && $op !== 'account_deleted') continue; // 用户侧日志受保护
      unset($ul[$k]);
      $removed++;
    }
    if (!fb_json_file_save(GA_USERLOG_FILE, $ul)) fb_fail('服务器存储写入失败', 500);
    ga_admin_op($user, 'admin_ops_delete', '管理员删除管理员操作日志：成功 ' . $removed . ' / 请求 ' . count($clean) . ' 条');
    fb_json_out(['ok' => true, 'removed' => $removed,
      'message' => '已删除 ' . $removed . ' 条管理员操作日志']);
  }

  /* ---------- P4 用户资源管理（R-20261122-004）：网关动态资源全量 / 删除封禁 / 强制摘除 Agent ----------
   * 数据源：网关 /admin/resource（X-Admin-Key 回环）动态注册表 + 站点 users.db resources 表合并；
   * 删除封禁 = 网关摘除（注册表 + Agent 通道强断）+ users.db 软删 revoked + 清 grants；
   * 摘除 Agent = 仅强断 Agent 通道（登记保留，目标机重连即恢复）。 */
  function ga_gw_call($path, $body) {
    $f = GW_KEYS;
    if (!is_readable($f)) fb_fail('网关配置缺失（api/gateway/keys.php 未放置）', 500);
    $k = json_decode((string)preg_replace('/^<\?php exit\(0\); \?>\s*/', '', (string)file_get_contents($f)), true);
    if (!is_array($k) || empty($k['gateway_url']) || empty($k['gateway_admin_key'])) fb_fail('网关配置不完整', 500);
    $ctx = stream_context_create(['http' => [
      'method' => 'POST', 'timeout' => 5,
      'header' => "Content-Type: application/json; charset=utf-8\r\nX-Admin-Key: " . (string)$k['gateway_admin_key'] . "\r\nUser-Agent: easyssh-admin-res\r\n",
      'content' => json_encode($body, JSON_UNESCAPED_UNICODE), 'ignore_errors' => true,
    ]]);
    $resp = @file_get_contents(rtrim((string)$k['gateway_url'], '/') . $path, false, $ctx);
    if ($resp === false) fb_fail('网关不可达（easyssh-gateway 连接失败或超时）', 502);
    $j = json_decode((string)$resp, true);
    if (!is_array($j) || ($j['ok'] ?? false) !== true) {
      fb_fail('网关返回错误：' . (is_array($j) && isset($j['message']) ? (string)$j['message'] : '响应格式异常'), 502);
    }
    return $j;
  }

  case 'resources': {
    $gw = ga_gw_call('/admin/resource', ['action' => 'status']);
    $rows = is_array($gw['resources'] ?? null) ? $gw['resources'] : [];
    // 合并 users.db（SQLite/JSON 镜像）侧登记行（网关缺失时兜底展示 DB 行）
    $seen = [];
    foreach ($rows as $r) { if (is_array($r)) $seen[(string)($r['id'] ?? '')] = true; }
    require __DIR__ . '/users-db.php';
    foreach (udb_resources_all() as $r) {
      $rid = (string)($r['id'] ?? '');
      if ($rid === '' || isset($seen[$rid])) continue;
      $rows[] = ['id' => $rid, 'owner' => (string)($r['owner_email'] ?? ''), 'label' => (string)($r['label'] ?? ''),
        'mode' => (string)($r['mode'] ?? 'direct'), 'backend' => (string)($r['backend'] ?? ''),
        'status' => (string)($r['status'] ?? 'pending'), 'agent_online' => null,
        'created_at' => (int)($r['created_at'] ?? 0)];
    }
    ga_admin_op($user, 'admin_resources_list', '管理员查看用户资源全量列表（' . count($rows) . ' 条）');
    fb_json_out(['ok' => true, 'resources' => $rows]);
  }

  case 'resource_delete': {
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($rid === '') fb_fail('resource_id 非法');
    $owner = strtolower(trim((string)($in['owner'] ?? '')));
    $row = null;
    require __DIR__ . '/users-db.php';
    $row = udb_resource_get($rid);
    if ($owner === '' && $row) $owner = (string)$row['owner_email'];
    ga_gw_call('/admin/resource', array_filter(['action' => 'delete', 'id' => $rid, 'owner' => $owner !== '' ? $owner : null], function ($v) { return $v !== null; }));
    if ($row) udb_resource_revoke($rid);
    // 清 grants（静态授权库中同 rid 一并吊销）
    $db = ga_res_load();
    foreach ($db as $email => $list) {
      if (!is_array($list)) continue;
      foreach ($list as $i => $g) {
        if (is_array($g) && (string)($g['resource_id'] ?? '') === $rid && ($g['status'] ?? '') === 'approved') {
          $db[$email][$i]['status'] = 'revoked';
          $db[$email][$i]['revokedAt'] = fb_now_ms();
          $db[$email][$i]['revokedBy'] = 'admin:' . $user;
        }
      }
    }
    ga_res_save($db);
    ga_admin_op($user, 'admin_resource_delete', '管理员删除用户资源：' . $rid . '（owner=' . $owner . '；网关摘除 + users.db 软删 + grants 清除）');
    fb_json_out(['ok' => true, 'message' => '资源已删除封禁（网关摘除 + Agent 通道断开 + grants 清除）']);
  }

  case 'resource_detach': {
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($rid === '') fb_fail('resource_id 非法');
    $gw = ga_gw_call('/admin/resource', ['action' => 'detach', 'id' => $rid]);
    ga_admin_op($user, 'admin_resource_detach', '管理员强制摘除 Agent：' . $rid . '（detached=' . (($gw['detached'] ?? false) ? '1' : '0') . '）');
    fb_json_out(['ok' => true, 'message' => (string)($gw['message'] ?? 'Agent 通道已摘除'), 'detached' => (bool)($gw['detached'] ?? false)]);
  }

  default:
    fb_fail('action 非法，仅支持 list / set_status / grant / revoke_r / delete_user / set_grant / admin_ops / admin_op_delete / resources / resource_delete / resource_detach');
}
