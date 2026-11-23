<?php
/**
 * POST /api/gateway/issue.php —— EasySSH 云网关用户侧自助服务接口（R-20261120-005⑥ / 006 / 010）
 *
 * 与站长 admin 后台（api/feedback 的 admin 会话）完全隔离：
 *   - 独立用户库 <站点上级>/data/gateway-users.json（email → {hash, createdAt}，password_hash 存储不落明文）
 *   - 独立会话 <站点上级>/data/gateway-sessions.json + HttpOnly Cookie（gw_sess，12 小时）
 *   - 独立资源库 <站点上级>/data/gateway-resources.json（email → [{resource_id,label,status}]）
 *
 * 动作（JSON body）：
 *   { action:"register", email, password }            → { ok, message }
 *   { action:"login",    email, password }            → { ok, email }（种 HttpOnly 会话 Cookie）
 *   { action:"my_resources" }                          → { ok, resources:[{resource_id,label,status}] }（Cookie 鉴权；401=未登录）
 *   { action:"issue", resource_id }                    → { ok, token, expires_in, resource_id, gateway:{host,port} }
 *   { action:"revoke", resource_id }                   → { ok, message }
 *
 * 签发：服务端内网回环 127.0.0.1:8443 调 easyssh-gateway（keys.php：gateway_url +
 * gateway_admin_key + issue_path），以 X-Admin-Key 请求签 role=user 的真实短期 JWT
 * （默认 600 秒）。token 只在响应中出现一次，服务端不持久化。
 *
 * 错误结构：{ ok:false, message }；会话无效一律 401；网关不可达 502；配置缺失 500。
 */

require __DIR__ . '/../feedback/common.php';
require __DIR__ . '/users-db.php'; // R-20261122-004 P4：users.db resources 表（自举建表）

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);
$in = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($in)) fb_fail('请求体必须是 JSON');
$action = (string)($in['action'] ?? '');

define('GW_USERS_FILE', FB_DATA_DIR . '/gateway-users.json');
define('GW_SESSIONS_FILE', FB_DATA_DIR . '/gateway-sessions.json');
define('GW_RESOURCES_FILE', FB_DATA_DIR . '/gateway-resources.json');
define('GW_ISSUE_RATE_FILE', FB_DATA_DIR . '/gateway-issue-ratelimit.json');
define('GW_COOKIE', 'gw_sess');
define('GW_SESSION_TTL_MS', 12 * 3600 * 1000);
define('GW_SESSIONS_MAX', 200);
define('GW_TOKEN_TTL_SEC', 600);          // 签发 token 有效期（秒）
define('GW_USERLOG_FILE', FB_DATA_DIR . '/gateway-userlogs.json');
define('GW_USERLOG_MAX', 5000); // 全局上限（按时间淘汰最旧）

/** 用户操作日志（append）：$detail 为字符串（签发记录存 JSON 明细文本） */
function gw_userlog($email, $action, $detail = '') {
  $db = fb_json_file_load(GW_USERLOG_FILE);
  if (!is_array($db)) $db = [];
  $db[] = ['ts' => fb_now_ms(), 'email' => $email, 'action' => $action, 'detail' => (string)$detail];
  if (count($db) > GW_USERLOG_MAX) $db = array_slice($db, -GW_USERLOG_MAX);
  fb_json_file_save(GW_USERLOG_FILE, $db);
}
define('GW_ISSUE_HOURLY_MAX', 30);        // 每 IP 每小时最多签发次数（GW-RATE-20261122：20→30）
define('GW_RES_QUOTA', 3);                // P4：每用户自助登记台数配额（仅限登记台数，签发不限次）

/* ---------- keys.php 网关配置 ---------- */
function gw_keys() {
  static $keys = null;
  if ($keys !== null) return $keys;
  $f = __DIR__ . '/keys.php';
  if (!is_readable($f)) fb_fail('网关配置缺失（api/gateway/keys.php 未放置），请联系管理员', 500);
  $keys = json_decode((string)preg_replace('/^<\?php exit\(0\); \?>\s*/', '', (string)file_get_contents($f)), true);
  if (!is_array($keys) || empty($keys['gateway_url']) || empty($keys['gateway_admin_key'])) {
    fb_fail('网关配置不完整（gateway_url / gateway_admin_key 缺失）', 500);
  }
  return $keys;
}

/** 服务端内网回环调网关（POST JSON + X-Admin-Key + X-Admin-Client-IP 转发用户真实 IP）。返回 [ok=true, data] 或 [ok=false, message, status] */
function gw_call($path, $body) {
  $k = gw_keys();
  $url = rtrim((string)$k['gateway_url'], '/') . $path;
  /* GW-REALIP-20261122：转发用户真实 IP（X-Admin-Client-IP，网关 admin_source_ip 认该头），
     审计不再显示 admin（对齐 EasyRDP GW-REALIP3 口径） */
  $realIp = (string)($_SERVER['REMOTE_ADDR'] ?? '');
  $ctx = stream_context_create(['http' => [
    'method'  => 'POST',
    'timeout' => 5,
    'header'  => "Content-Type: application/json; charset=utf-8\r\nX-Admin-Key: " . (string)$k['gateway_admin_key']
               . "\r\nUser-Agent: easyssh-site-issue-proxy\r\n"
               . ($realIp !== '' ? "X-Admin-Client-IP: " . $realIp . "\r\n" : ""),
    'content' => json_encode($body, JSON_UNESCAPED_UNICODE),
    'ignore_errors' => true,
  ]]);
  $resp = @file_get_contents($url, false, $ctx);
  if ($resp === false) return [false, '网关不可达（easyssh-gateway 连接失败或超时），请稍后重试', 502];
  $status = 0;
  foreach (($http_response_header ?? []) as $h) {
    if (preg_match('#^HTTP/\S+\s+(\d{3})#', $h, $m)) { $status = (int)$m[1]; break; }
  }
  $j = json_decode((string)$resp, true);
  if (!is_array($j) || ($j['ok'] ?? false) !== true) {
    $msg = is_array($j) && isset($j['message']) ? (string)$j['message'] : '网关响应格式异常';
    return [false, '网关返回错误（HTTP ' . ($status ?: '?') . '）：' . $msg, 502];
  }
  return [true, $j, 200];
}

/* ---------- 会话（HttpOnly Cookie + 服务端可吊销随机 token，独立于 admin 会话） ---------- */
function gw_session_create($email) {
  $sessions = fb_json_file_load(GW_SESSIONS_FILE);
  $now = fb_now_ms();
  foreach ($sessions as $t => $s) {
    if (!is_array($s) || (int)($s['expires'] ?? 0) <= $now) unset($sessions[$t]);
  }
  if (count($sessions) >= GW_SESSIONS_MAX) {
    uasort($sessions, function ($a, $b) { return (int)($a['expires'] ?? 0) <=> (int)($b['expires'] ?? 0); });
    $sessions = array_slice($sessions, count($sessions) - GW_SESSIONS_MAX + 1, null, true);
  }
  $token = bin2hex(random_bytes(32));
  $sessions[$token] = ['user' => (string)$email, 'expires' => $now + GW_SESSION_TTL_MS, 'created' => $now];
  fb_json_file_save(GW_SESSIONS_FILE, $sessions);
  setcookie(GW_COOKIE, $token, [
    'expires'  => 0, // 会话 Cookie：关浏览器即失效（服务端仍按 12h 过期回收）
    'path'     => '/',
    'secure'   => (($_SERVER['HTTPS'] ?? '') === 'on') || (($_SERVER['HTTP_X_FORWARDED_PROTO'] ?? '') === 'https'),
    'httponly' => true,
    'samesite' => 'Lax',
  ]);
  return $token;
}

/** Cookie 鉴权。未登录/过期直接 401 退出。
 * GW-SESSIONCHK-20261122（对齐 EasyRDP）：同时校验会话用户在用户库中仍存在——
 * 用户被删除/注销/库被清而会话文件未同步时，立即吊销该会话并 401（消灭幽灵会话）。 */
function gw_require_session() {
  $token = (string)($_COOKIE[GW_COOKIE] ?? '');
  if (strlen($token) !== 64 || !ctype_xdigit($token)) fb_fail('未登录或登录已过期，请重新登录', 401);
  $sessions = fb_json_file_load(GW_SESSIONS_FILE);
  $s = $sessions[$token] ?? null;
  if (!is_array($s) || (string)($s['user'] ?? '') === '' || (int)($s['expires'] ?? 0) <= fb_now_ms()) {
    fb_fail('未登录或登录已过期，请重新登录', 401);
  }
  $email = (string)$s['user'];
  $users = gw_users_load();
  if (!isset($users[$email]) || !is_array($users[$email])) {
    // 幽灵会话：用户已不存在 → 服务端吊销该会话 + 401（前端自动回登录口）
    unset($sessions[$token]);
    fb_json_file_save(GW_SESSIONS_FILE, $sessions);
    fb_fail('账号不存在或已被删除，请重新注册或登录', 401);
  }
  return $email;
}

function gw_users_load() { $j = fb_json_file_load(GW_USERS_FILE); return is_array($j) ? $j : []; }

/** 邮箱验证通过联动：把该邮箱名下全部非 approved 资源授权自动置为 approved（自动审批），返回审批数量 */
function gw_auto_approve_resources(string $email): int {
  $res = gw_resources_load();
  if (!isset($res[$email]) || !is_array($res[$email])) return 0;
  $n = 0;
  foreach ($res[$email] as $i => $g) {
    if (is_array($g) && ($g["status"] ?? "") !== "approved") { $res[$email][$i]["status"] = "approved"; $n++; }
  }
  if ($n > 0 && !fb_json_file_save(GW_RESOURCES_FILE, $res)) return 0;
  return $n;
}

/* ---------- 邮箱验证（注册后须验证邮箱才能正常使用） ---------- */
/** 账号是否已完成邮箱验证。存量账号（无 verified 字段）视为已验证，避免打扰。 */
function gw_is_verified(array $users, string $email): bool {
  $u = $users[$email] ?? null;
  if (!is_array($u)) return false;
  return array_key_exists('verified', $u) ? ((int)$u['verified'] === 1) : true;
}
/** 发送验证码邮件（15 分钟有效）。返回 true 或错误串 */
function gw_verify_mail(string $email, string $code) {
  return fb_mail_send($email, '[EasySSH] 云网关账号邮箱验证码',
    "你的云网关账号邮箱验证码：{$code}（15 分钟内有效）\n\n" .
    "请回到云网关用户服务台 https://easyssh.cn/gw-admin.html 输入验证码完成邮箱验证；" .
    "验证通过后管理员授权的资源才会对你开放签发。\n\n" .
    "—— EasySSH 云网关服务（本邮件为系统自动发送，请勿回复）");
}
/** 生成并记录验证码（覆盖旧码），发邮件。返回 true 或错误串 */
function gw_verify_issue_code(array &$users, string $email) {
  $code = str_pad((string)random_int(0, 999999), 6, '0', STR_PAD_LEFT);
  $users[$email]['vcodeHash'] = hash('sha256', $code . '|' . $email . '|' . fb_now_ms());
  $users[$email]['vcodeExp'] = fb_now_ms() + 15 * 60 * 1000;
  if (!fb_json_file_save(GW_USERS_FILE, $users)) return '服务器存储写入失败';
  return gw_verify_mail($email, $code);
}
function gw_resources_load() { $j = fb_json_file_load(GW_RESOURCES_FILE); return is_array($j) ? $j : []; }

/* ---------- 网关审计留痕（回环 POST /admin/audit/record，X-Admin-Key；与 EasyRDP 同口径） ----------
 * 站点侧动作（注册/登录/验证/自动审批/注销）原本只落 gateway-userlogs.json，
 * 网关内存环形审计里看不到；现统一回环写入网关审计，站长后台「网关审计」标签页可见。
 * 留痕失败不影响主流程。 */
function gw_audit_record(string $user, string $event, string $resource = '-'): void {
  try {
    $k = gw_keys();
    $realIp = (string)($_SERVER['REMOTE_ADDR'] ?? '');
    $ctx = stream_context_create(['http' => [
      'method' => 'POST', 'timeout' => 3,
      'header' => "Content-Type: application/json\r\nX-Admin-Key: " . (string)$k['gateway_admin_key']
        . "\r\nUser-Agent: easyssh-site-record\r\n"
        . ($realIp !== '' ? "X-Admin-Client-IP: " . $realIp . "\r\n" : ""),
      'content' => (string)json_encode(['user' => $user, 'event' => $event, 'resource' => $resource]),
      'ignore_errors' => true,
    ]]);
    @file_get_contents(rtrim((string)$k['gateway_url'], '/') . '/admin/audit/record', false, $ctx);
  } catch (Throwable $e) { /* 留痕失败不影响主流程 */ }
}

/* ---------- P4 资源自助链路（R-20261122-004）：网关动态资源表状态查询 ----------
 * 回环 POST /admin/resource {action:"status"}（全量）或 {action:"status", id}（单条），
 * 带回 agent_online 在线态与 cleanup_due 计算态；失败返回 null（不影响列表兜底展示）。 */
function gw_res_status_all() {
  list($ok, $data) = gw_call('/admin/resource', ['action' => 'status']);
  if (!$ok || !isset($data['resources']) || !is_array($data['resources'])) return null;
  return $data['resources'];
}

/** 由全量状态行提取某资源的在线态（true/false/null=未知）。 */
function gw_res_online_of(?array $statusRows, string $rid) {
  if (!$statusRows) return null;
  foreach ($statusRows as $r) {
    if (is_array($r) && (string)($r['id'] ?? '') === $rid) {
      $online = $r['agent_online'] ?? null;
      return is_bool($online) ? $online : null;
    }
  }
  return null;
}

switch ($action) {

  /* ---------- 注销账号（删号：账号 + 全部会话 + 全部资源授权，不可恢复） ---------- */
  case 'delete_account': {
    $email = gw_require_session();
    $pass = (string)($in['password'] ?? '');
    $users = gw_users_load();
    if (!isset($users[$email])) fb_fail('账号不存在', 404);
    if (!password_verify($pass, (string)$users[$email]['hash'])) fb_fail('密码校验失败，请输入当前账号密码以确认注销');
    // 1) 注销全部资源授权
    $res = gw_resources_load();
    if (isset($res[$email])) { unset($res[$email]); fb_json_file_save(GW_RESOURCES_FILE, $res); }
    // 2) 删除该账号全部会话
    $sessions = fb_json_file_load(GW_SESSIONS_FILE);
    if (is_array($sessions)) {
      foreach ($sessions as $tk => $s) { if ((string)($s['user'] ?? '') === $email) unset($sessions[$tk]); }
      fb_json_file_save(GW_SESSIONS_FILE, $sessions);
    }
    // 3) 删除账号
    unset($users[$email]);
    if (!fb_json_file_save(GW_USERS_FILE, $users)) fb_fail('服务器存储写入失败', 500);
    gw_audit_record($email, 'user_self_deleted');
    setcookie(GW_COOKIE, '', [
      'expires' => 0, 'path' => '/', 'httponly' => true, 'samesite' => 'Lax',
    ]);
    gw_userlog($email, 'delete_account', '账号注销（账号/会话/授权已删除）');
    fb_json_out(['ok' => true, 'message' => '账号已注销：账号、会话与资源授权已全部删除']);
  }

  /* ---------- 注册 ---------- */
  case 'register': {
    if (!fb_ip_rate_ok(GW_ISSUE_RATE_FILE, 10, 3600 * 1000)) fb_fail('操作过于频繁，请稍后再试', 429);
    $email = strtolower(trim((string)($in['email'] ?? '')));
    if (!is_string($email) || strlen($email) > 254 || !filter_var($email, FILTER_VALIDATE_EMAIL)) fb_fail('邮箱格式非法');
    $pass = (string)($in['password'] ?? '');
    if (strlen($pass) < 10 || strlen($pass) > 128) fb_fail('密码长度需 10-128 位');
    $users = gw_users_load();
    if (isset($users[$email])) fb_fail('该邮箱已注册，请直接登录');
    if (count($users) > 20000) fb_fail('注册暂未开放，请联系管理员', 503);
    $users[$email] = ['hash' => password_hash($pass, PASSWORD_DEFAULT), 'createdAt' => fb_now_ms(), 'verified' => 0];
    if (!fb_json_file_save(GW_USERS_FILE, $users)) fb_fail('服务器存储写入失败', 500);
    gw_userlog($email, 'register', '账号注册');
    gw_audit_record($email, 'register');
    // 注册即发邮箱验证码（15 分钟有效；验证通过才能正常使用云网关）
    // 复用 gw_verify_issue_code：保证哈希算法（code|email|ts）与 verify_code 校验一致
    $err = gw_verify_issue_code($users, $email);
    if ($err !== true) fb_fail('验证码邮件发送失败：' . $err, 500);
    gw_userlog($email, 'verify_send', '注册触发邮箱验证码发送');
    fb_json_out(['ok' => true, 'message' => '注册成功，请登录并完成邮箱验证（验证码已发送至邮箱，15 分钟内有效）']);
  }

  /* ---------- 登录 ---------- */
  case 'login': {
    $ip = fb_client_ip();
    // 登录失败限速（复用 feedback 侧失败计数器，key 前缀区分，与 admin 登录互相独立）
    if (fb_login_fail_hit($ip, 'gw|' . (string)($in['email'] ?? ''))) {
      fb_fail('失败次数过多，请 15 分钟后再试', 429);
    }
    $email = strtolower(trim((string)($in['email'] ?? '')));
    $pass = (string)($in['password'] ?? '');
    $users = gw_users_load();
    $u = $users[$email] ?? null;
    if (!is_array($u) || !password_verify($pass, (string)($u['hash'] ?? ''))) {
      fb_login_fail_record($ip, 'gw|' . $email);
      fb_fail('邮箱或密码错误');
    }
    fb_login_fail_reset($ip, 'gw|' . $email);
    gw_session_create($email);
    gw_userlog($email, 'login', '登录成功（IP ' . fb_client_ip() . '）');
    gw_audit_record($email, 'login');
    fb_json_out(['ok' => true, 'email' => $email]);
  }

  /* ---------- 我的资源（P4 升级：静态授权 + 自助登记合并，带 mode/status/在线态） ---------- */
  case 'my_resources': {
    $email = gw_require_session();
    $users = gw_users_load();
    if (!gw_is_verified($users, $email)) fb_fail('请先完成邮箱验证后再使用云网关服务', 403);
    $statusRows = gw_res_status_all(); // 一次回环取全量在线态；失败 null 兜底
    $list = [];
    // 1) 静态授权（管理员开通的资源池授权，mode 恒 direct/静态）
    $all = gw_resources_load();
    foreach ((array)($all[$email] ?? []) as $r) {
      if (!is_array($r) || ($r['status'] ?? '') !== 'approved') continue; // 仅展示已开通
      $rid = (string)($r['resource_id'] ?? '');
      if ($rid === '') continue;
      $list[] = [
        'resource_id' => $rid,
        'label' => (string)($r['label'] ?? $rid),
        'status' => 'approved',
        'mode' => 'direct',
        'backend' => '',            // 静态资源后端不透出（网关配置侧）
        'origin' => 'granted',
        'online' => null,
      ];
    }
    // 2) 自助登记（users.db resources 表，u- 动态 ID），软删 revoked 不展示
    foreach (udb_resources_by_owner($email) as $r) {
      if (!is_array($r) || ($r['status'] ?? '') === 'revoked') continue;
      $rid = (string)($r['id'] ?? '');
      if ($rid === '' || in_array($rid, array_column($list, 'resource_id'), true)) continue;
      $list[] = [
        'resource_id' => $rid,
        'label' => (string)($r['label'] ?? $rid),
        'status' => (string)($r['status'] ?? 'pending'),
        'mode' => (string)($r['mode'] ?? 'direct'),
        'backend' => (string)($r['backend'] ?? ''),
        'origin' => 'self',
        'online' => gw_res_online_of($statusRows, $rid),
        'created_at' => (int)($r['created_at'] ?? 0),
      ];
    }
    fb_json_out(['ok' => true, 'resources' => $list, 'quota' => ['used' => udb_resources_active_count($email), 'max' => GW_RES_QUOTA]]);
  }

  /* ---------- P4 自助登记资源（direct：网关 3s 试连 + SSH-2.0 banner 探测后落库；
                agent：返回一次性 enrollment token，10 分钟、绑定 rid） ---------- */
  case 'register_resource': {
    $email = gw_require_session();
    $users = gw_users_load();
    if (!gw_is_verified($users, $email)) fb_fail('请先完成邮箱验证后再登记资源', 403);
    if (!fb_ip_rate_ok(GW_ISSUE_RATE_FILE, 10, 3600 * 1000)) fb_fail('操作过于频繁，请稍后再试', 429);
    $mode = strtolower(trim((string)($in['mode'] ?? 'direct')));
    if ($mode !== 'direct' && $mode !== 'agent') fb_fail('mode 非法（direct/agent）');
    $label = trim((string)($in['label'] ?? ''));
    if (mb_strlen($label) > 64) fb_fail('资源名称过长（≤64 字）');
    // 配额预检（网关侧还有第二道；双口径一致=每用户 3 台，仅限登记台数）
    if (udb_resources_active_count($email) >= GW_RES_QUOTA) {
      fb_fail('已达到每用户登记上限（' . GW_RES_QUOTA . ' 台）；请先删除不再使用的资源', 403);
    }
    $body = ['action' => 'register', 'mode' => $mode, 'owner' => $email];
    if ($label !== '') $body['label'] = $label;
    if ($mode === 'direct') {
      $host = trim((string)($in['host'] ?? ''));
      $port = (int)($in['port'] ?? 22);
      if ($host === '' || strlen($host) > 64) fb_fail('host 非法（仅支持公网 IP，网关不代查域名）');
      if ($port < 1 || $port > 65535) fb_fail('端口非法（1-65535）');
      $body['host'] = $host;
      $body['port'] = $port;
    }
    list($ok, $data) = gw_call('/admin/resource', $body);
    if (!$ok) {
      // 透传网关错误（探测失败 / SSRF 拒绝 / 配额），前端直接展示
      fb_fail((string)$data, 502);
    }
    $res = is_array($data['resource'] ?? null) ? $data['resource'] : [];
    $rid = (string)($res['id'] ?? '');
    if ($rid === '' || strpos($rid, 'u-') !== 0) fb_fail('网关未返回合法动态资源 ID', 502);
    // 落库 users.db（direct=approved；agent=pending 待 Agent 上线）
    $status = ($mode === 'direct') ? 'approved' : 'pending';
    udb_resource_put([
      'id' => $rid, 'owner_email' => $email, 'label' => (string)($res['label'] ?? ($label !== '' ? $label : $rid)),
      'mode' => $mode, 'backend' => (string)($res['backend'] ?? ''), 'status' => $status,
      'created_at' => fb_now_ms(),
    ]);
    gw_userlog($email, 'register_resource', json_encode([
      'resource_id' => $rid, 'mode' => $mode, 'backend' => (string)($res['backend'] ?? ''),
    ], JSON_UNESCAPED_UNICODE));
    gw_audit_record($email, $mode === 'direct' ? 'resource_registered_direct' : 'resource_registered_agent', $rid);
    $out = [
      'ok' => true,
      'resource_id' => $rid,
      'mode' => $mode,
      'status' => $status,
      'label' => (string)($res['label'] ?? ''),
      'backend' => (string)($res['backend'] ?? ''),
      'message' => (string)($data['message'] ?? '资源已登记'),
    ];
    if ($mode === 'agent') {
      // enrollment token 仅此一次返回（10 分钟有效；目标机 easyssh-agent --enroll <token>）
      $out['enrollment_token'] = (string)($data['enrollment_token'] ?? '');
      $out['enrollment_expires_in'] = (int)($data['enrollment_expires_in'] ?? 600);
      $out['hint'] = '请在 10 分钟内在目标机执行：easyssh-agent --enroll <token>（Agent 上线后资源自动翻「在线」）';
    }
    fb_json_out($out);
  }

  /* ---------- P4 修改资源（label/backend；mode 不可改） ---------- */
  case 'update_resource': {
    $email = gw_require_session();
    $users = gw_users_load();
    if (!gw_is_verified($users, $email)) fb_fail('请先完成邮箱验证后再修改资源', 403);
    if (isset($in['mode'])) fb_fail('mode 不可修改（如需更换接入形态请删除后重新登记）');
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($rid === '' || strpos($rid, 'u-') !== 0) fb_fail('资源 ID 非法（仅支持自助登记的 u- 动态资源）');
    $row = udb_resource_get($rid);
    if (!$row || strtolower((string)$row['owner_email']) !== $email || ($row['status'] ?? '') === 'revoked') {
      fb_fail('资源不存在或不属于当前账号', 403);
    }
    $label = trim((string)($in['label'] ?? ''));
    $body = ['action' => 'update', 'id' => $rid, 'owner' => $email];
    if ($label !== '') { if (mb_strlen($label) > 64) fb_fail('资源名称过长（≤64 字）'); $body['label'] = $label; }
    if (isset($in['host']) && trim((string)$in['host']) !== '') {
      $host = trim((string)$in['host']);
      $port = (int)($in['port'] ?? 0);
      if (strlen($host) > 64) fb_fail('host 非法（仅支持公网 IP）');
      if ($port < 1 || $port > 65535) fb_fail('端口非法（1-65535）');
      $body['host'] = $host;
      $body['port'] = $port;
    }
    list($ok, $data) = gw_call('/admin/resource', $body);
    if (!$ok) fb_fail((string)$data, 502);
    $res = is_array($data['resource'] ?? null) ? $data['resource'] : [];
    $fields = [];
    if ($label !== '') $fields['label'] = (string)($res['label'] ?? $label);
    if (isset($body['host'])) $fields['backend'] = (string)($res['backend'] ?? '');
    if ($fields) udb_resource_update($rid, $fields);
    gw_userlog($email, 'update_resource', '修改资源 ' . $rid . '（' . json_encode($fields, JSON_UNESCAPED_UNICODE) . '）');
    gw_audit_record($email, 'resource_updated', $rid);
    fb_json_out(['ok' => true, 'message' => '资源已更新', 'resource_id' => $rid, 'resource' => $res]);
  }

  /* ---------- P4 删除资源（软删 revoked + 清 grants + 通知网关摘除动态注册表与 Agent 通道） ---------- */
  case 'delete_resource': {
    $email = gw_require_session();
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($rid === '') fb_fail('资源 ID 非法');
    $row = udb_resource_get($rid);
    $isSelf = $row && strtolower((string)$row['owner_email']) === $email && strpos($rid, 'u-') === 0;
    // 静态授权资源也可吊销（走 grants 软删；不通知网关——静态资源属运营方配置）
    if (!$isSelf) {
      $all = gw_resources_load();
      $mine = null;
      foreach ((array)($all[$email] ?? []) as $r) {
        if (is_array($r) && (string)($r['resource_id'] ?? '') === $rid) { $mine = $r; break; }
      }
      if (!$mine || ($mine['status'] ?? '') !== 'approved') fb_fail('资源不存在或不属于当前账号', 403);
      foreach ($all[$email] as $i => $r) {
        if (is_array($r) && (string)($r['resource_id'] ?? '') === $rid) {
          $all[$email][$i]['status'] = 'revoked';
          $all[$email][$i]['revokedAt'] = fb_now_ms();
        }
      }
      fb_json_file_save(GW_RESOURCES_FILE, $all);
      gw_userlog($email, 'delete_resource', '吊销静态授权：' . $rid);
      gw_audit_record($email, 'resource_revoked', $rid);
      fb_json_out(['ok' => true, 'message' => '授权已吊销']);
    }
    // 自助登记资源：软删 + 清 grants + 通知网关摘除（动态注册表 + Agent 通道强断）
    list($ok, $data) = gw_call('/admin/resource', ['action' => 'delete', 'id' => $rid, 'owner' => $email]);
    if (!$ok) fb_fail('网关摘除失败：' . (string)$data, 502);
    udb_resource_revoke($rid);
    // 清 grants（该 rid 若同时存在于静态授权库中则一并清除）
    $all = gw_resources_load();
    $changed = false;
    foreach (($all[$email] ?? []) as $i => $r) {
      if (is_array($r) && (string)($r['resource_id'] ?? '') === $rid) {
        $all[$email][$i]['status'] = 'revoked';
        $all[$email][$i]['revokedAt'] = fb_now_ms();
        $changed = true;
      }
    }
    if ($changed) fb_json_file_save(GW_RESOURCES_FILE, $all);
    gw_userlog($email, 'delete_resource', '删除资源 ' . $rid . '（软删 revoked + 网关摘除）');
    gw_audit_record($email, 'resource_deleted', $rid);
    fb_json_out(['ok' => true, 'message' => '资源已删除：授权已清空，网关侧已摘除（Agent 通道已断开）']);
  }

  /* ---------- 自助签发短期 token（role=user 真实 JWT，服务端内网回环调网关） ---------- */
  case 'issue': {
    $email = gw_require_session();
    $users = gw_users_load();
    if (!gw_is_verified($users, $email)) fb_fail('请先完成邮箱验证后再签发', 403);
    if (!fb_ip_rate_ok(GW_ISSUE_RATE_FILE, GW_ISSUE_HOURLY_MAX, 3600 * 1000)) {
      fb_fail('签发过于频繁，请稍后再试', 429);
    }
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($rid === '' || strlen($rid) > 128) fb_fail('资源 ID 非法');
    // 资源归属校验：必须是本人名下且状态已开通
    $all = gw_resources_load();
    $mine = null;
    foreach ((array)($all[$email] ?? []) as $r) {
      if (is_array($r) && (string)($r['resource_id'] ?? '') === $rid) { $mine = $r; break; }
    }
    if (!$mine || ($mine['status'] ?? '') !== 'approved') fb_fail('资源不存在或未开通', 403);
    /* GW-TTL-20261122：用户自定义有效期，白名单钳制（对齐 EasyRDP），默认 600（10 分钟） */
    $allowedTtl = [600, 1800, 3600, 21600, 86400, 604800, 2592000, 31536000];
    $ttl = (int)($in['ttl_seconds'] ?? GW_TOKEN_TTL_SEC);
    if (!in_array($ttl, $allowedTtl, true)) $ttl = GW_TOKEN_TTL_SEC;
    // 服务端内网回环 127.0.0.1:8443 调网关签 role=user 真实 JWT
    $k = gw_keys();
    $path = (string)($k['issue_path'] ?? '/admin/issue');
    list($ok, $data) = gw_call($path, [
      // easyssh-gateway /admin/issue 契约：{"sub":"...","roles":["user"],"ttl_seconds":N}
      'sub' => $email,
      'roles' => ['user'],
      'ttl_seconds' => $ttl,
    ]);
    if (!$ok) fb_fail((string)$data, 502);
    $token = (string)($data['token'] ?? '');
    if ($token === '') fb_fail('网关未返回 token', 502);
    // 用户操作日志：签发明细（含 token 源文，供管理员审计查看）
    gw_userlog($email, 'issue', json_encode([
      'resource_id' => $rid,
      'gateway' => ['host' => (string)($k['gateway_host'] ?? 'easyssh.cn'), 'port' => (int)($k['gateway_port'] ?? 443), 'tls' => true],
      'auth' => 'Token (JWT)',
      'expires_in' => (int)($data['expires_in'] ?? $ttl),
      'token' => $token,
    ], JSON_UNESCAPED_UNICODE));
    fb_json_out([
      'ok' => true,
      'token' => $token,
      'expires_in' => (int)($data['expires_in'] ?? $ttl),
      'resource_id' => $rid,
      'gateway' => [
        'host' => (string)($k['gateway_host'] ?? 'easyssh.cn'),
        'port' => (int)($k['gateway_port'] ?? 443),
      ],
      'hint' => '在 EasySSH 客户端「网关（可选）」中按以上参数填写，认证方式选 Token（JWT）',
    ]);
  }

  /* ---------- 吊销授权 ---------- */
  case 'revoke': {
    $email = gw_require_session();
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($rid === '' || strlen($rid) > 128) fb_fail('资源 ID 非法');
    $all = gw_resources_load();
    $mine = null;
    foreach ((array)($all[$email] ?? []) as $r) {
      if (is_array($r) && (string)($r['resource_id'] ?? '') === $rid) { $mine = $r; break; }
    }
    if (!$mine || ($mine['status'] ?? '') !== 'approved') fb_fail('资源不存在或未开通', 403);
    // 吊销语义：本地置 revoked（阻止后续自助签发）。网关 JWT 为无状态短期票据
    //（≤600 秒自然过期），easyssh-gateway 不提供 /admin/revoke 吊销端点；
    // 如需即时封禁个别用户，由运维在网关侧策略矩阵/资源注册中处理。
    foreach ($all[$email] as $i => $r) {
      if (is_array($r) && (string)($r['resource_id'] ?? '') === $rid) {
        $all[$email][$i]['status'] = 'revoked';
        $all[$email][$i]['revokedAt'] = fb_now_ms();
      }
    }
    fb_json_file_save(GW_RESOURCES_FILE, $all);
    gw_userlog($email, 'revoke_resource', '吊销资源授权：' . $rid);
    fb_json_out(['ok' => true, 'message' => '授权已吊销：已停止自助签发；已签发 token 将在 10 分钟内自然过期']);
  }

  /* ---------- 邮箱验证状态（登录后前端自动判定路由） ---------- */
  case 'verify_status': {
    $email = gw_require_session();
    $users = gw_users_load();
    if (!isset($users[$email])) fb_fail('账号不存在', 404);
    fb_json_out(['ok' => true, 'verified' => gw_is_verified($users, $email), 'email' => $email]);
  }

  /* ---------- 重发验证码（每邮箱 10 分钟 1 次） ---------- */
  case 'verify_send': {
    $email = gw_require_session();
    $users = gw_users_load();
    if (!isset($users[$email])) fb_fail('账号不存在', 404);
    if (gw_is_verified($users, $email)) fb_json_out(['ok' => true, 'verified' => true, 'message' => '已完成验证，无需重发']);
    $exp = (int)($users[$email]['vcodeExp'] ?? 0);
    if ($exp - 5 * 60 * 1000 > fb_now_ms()) fb_fail('验证码刚发送，请稍后再试', 429);
    $err = gw_verify_issue_code($users, $email);
    if ($err !== true) fb_fail('验证码邮件发送失败：' . $err, 500);
    gw_userlog($email, 'verify_send', '重发邮箱验证码');
    fb_json_out(['ok' => true, 'message' => '验证码已重新发送，请查收邮箱（15 分钟内有效）']);
  }

  /* ---------- 校验验证码（通过后置 verified=1，账号正式可用） ---------- */
  case 'verify_code': {
    $email = gw_require_session();
    $code = trim((string)($in['code'] ?? ''));
    if (!preg_match('/^\d{6}$/', $code)) fb_fail('验证码格式非法（6 位数字）');
    $users = gw_users_load();
    if (!isset($users[$email])) fb_fail('账号不存在', 404);
    if (gw_is_verified($users, $email)) fb_json_out(['ok' => true, 'verified' => true, 'message' => '已完成验证']);
    $hash = (string)($users[$email]['vcodeHash'] ?? '');
    $exp = (int)($users[$email]['vcodeExp'] ?? 0);
    if ($hash === '' || fb_now_ms() > $exp) fb_fail('验证码已过期，请点击「重新发送」获取新验证码');
    // 容忍发送与校验时刻的时钟跨度：hash 内嵌发送时间戳，±10 秒窗口重算比对
    $ok = false;
    for ($i = 0; $i <= 10; $i++) {
      if (hash_equals($hash, hash('sha256', $code . '|' . $email . '|' . ($exp - 15 * 60 * 1000 - $i * 1000)))) { $ok = true; break; }
    }
    if (!$ok) { fb_login_fail_record(fb_client_ip(), 'gw-verify|' . $email); fb_fail('验证码错误'); }
    $users[$email]['verified'] = 1;
    $users[$email]['verifiedAt'] = fb_now_ms();
    unset($users[$email]['vcodeHash'], $users[$email]['vcodeExp']);
    if (!fb_json_file_save(GW_USERS_FILE, $users)) fb_fail('服务器存储写入失败', 500);
    gw_userlog($email, 'verify_code', '邮箱验证通过，账号正式可用');
    gw_audit_record($email, 'email_verified');
    // 联动自动审批：邮箱验证通过后，该邮箱名下全部资源授权自动置为 approved（用户即可自助签发 token）
    $autoN = 0;
    $res = gw_resources_load();
    if (isset($res[$email]) && is_array($res[$email])) {
      foreach ($res[$email] as $i => $g) {
        if (is_array($g) && ($g["status"] ?? "") !== "approved") {
          $res[$email][$i]["status"] = "approved";
          $autoN++;
          gw_audit_record($email, 'resource_auto_granted', (string)($g['resource_id'] ?? '-'));
        }
      }
      if ($autoN > 0) fb_json_file_save(GW_RESOURCES_FILE, $res);
    }
    // 对齐 EasyRDP 口径：邮箱名下无任何授权时，自动审批资源池第一项（keys.php resources；池为空则跳过）
    $hasApproved = false;
    foreach ((array)($res[$email] ?? []) as $g) { if (is_array($g) && ($g['status'] ?? '') === 'approved') { $hasApproved = true; break; } }
    if (!$hasApproved) {
      try {
        $k = gw_keys();
        $pool = is_array($k['resources'] ?? null) ? $k['resources'] : [];
        $r0 = $pool[0] ?? null;
        if (is_array($r0) && preg_match('/^[A-Za-z0-9._-]{1,64}$/', (string)($r0['id'] ?? ''))) {
          $rid = (string)$r0['id'];
          $label = trim((string)($r0['label'] ?? '')) !== '' ? (string)$r0['label'] : $rid;
          $res[$email][] = ['resource_id' => $rid, 'label' => $label, 'status' => 'approved',
            'grantedAt' => fb_now_ms(), 'grantedBy' => 'auto:email_verified'];
          if (fb_json_file_save(GW_RESOURCES_FILE, $res)) {
            $autoN++;
            gw_audit_record($email, 'resource_auto_granted', $rid);
            gw_userlog($email, 'auto_grant', '邮箱验证联动自动审批资源池默认资源：' . $rid);
          }
        }
      } catch (Throwable $e) { /* 资源池缺失/不可达时保持原行为（管理员手工授权） */ }
    }
    gw_userlog($email, 'resource_auto_approve', '邮箱验证通过联动自动审批资源授权 ' . $autoN . ' 项');
    gw_userlog($email, 'verify_done', '验证完成（自动审批 ' . $autoN . ' 项资源）');
    fb_json_out(['ok' => true, 'verified' => true, 'message' => '邮箱验证通过，云网关服务已对你开放']);
  }

  /* ---------- 退出登录：服务端吊销会话 + 清 Cookie（防止刷新后被 my_resources 探测自动回登录态） ---------- */
  case 'logout': {
    $token = (string)($_COOKIE[GW_COOKIE] ?? '');
    // 日志先取会话邮箱（会话删除前）
    $logEmail = '';
    if (strlen($token) === 64 && ctype_xdigit($token)) {
      $sessions = fb_json_file_load(GW_SESSIONS_FILE);
      if (isset($sessions[$token])) {
        $logEmail = (string)($sessions[$token]['user'] ?? '');
        unset($sessions[$token]); fb_json_file_save(GW_SESSIONS_FILE, $sessions);
      }
    }
    if ($logEmail !== '') gw_userlog($logEmail, 'logout', '退出登录');
    /* 多路径清除兜底：防止历史遗留的同名不同 path 的 gw_sess 残留导致退出后仍被判定已登录 */
    $secure = (($_SERVER['HTTPS'] ?? '') === 'on') || (($_SERVER['HTTP_X_FORWARDED_PROTO'] ?? '') === 'https');
    foreach (['/', '/webconsole', '/api', '/api/gateway'] as $p) {
      setcookie(GW_COOKIE, '', [
        'expires'  => time() - 3600,
        'path'     => $p,
        'secure'   => $secure,
        'httponly' => true,
        'samesite' => 'Lax',
      ]);
    }
    fb_json_out(['ok' => true, 'message' => '已退出登录']);
  }

  /* ---------- 我的签发记录（对齐 EasyRDP GW-TOKENLIST-20261122：最近 50 条，
     含 token 源文 / 资源 / 网关 / 签发时间 / 剩余秒数，供前端脱敏列表+分页+倒数） ---------- */
  case 'my_issues': {
    $email = gw_require_session();
    $users = gw_users_load();
    if (!gw_is_verified($users, $email)) fb_fail('请先完成邮箱验证后再使用云网关服务', 403);
    $logs = fb_json_file_load(GW_USERLOG_FILE);
    if (!is_array($logs)) $logs = [];
    $list = [];
    // 从新到旧扫描，只取本人 issue 记录，最多 50 条（GW-TOKPAGE：与 EasyRDP 口径一致）
    for ($i = count($logs) - 1; $i >= 0 && count($list) < 50; $i--) {
      $e = $logs[$i];
      if (!is_array($e) || (string)($e['email'] ?? '') !== $email || (string)($e['action'] ?? '') !== 'issue') continue;
      $d = json_decode((string)($e['detail'] ?? ''), true);
      if (!is_array($d) || empty($d['token'])) continue;
      $ts = (int)($e['ts'] ?? 0);
      $expIn = (int)($d['expires_in'] ?? GW_TOKEN_TTL_SEC);
      $expAt = $ts + $expIn * 1000;
      $gw = is_array($d['gateway'] ?? null) ? $d['gateway'] : [];
      $list[] = [
        'ts' => $ts,
        'resource_id' => (string)($d['resource_id'] ?? '-'),
        'gateway' => [
          'host' => (string)($gw['host'] ?? 'easyssh.cn'),
          'port' => (int)($gw['port'] ?? 443),
        ],
        'expires_in' => $expIn,
        'expire_at' => $expAt,
        'expired' => (fb_now_ms() > $expAt),
        /* GW-TOKTICK：剩余秒数实时计算，供前端每秒倒数 */
        'remaining' => max(0, (int)ceil(($expAt - fb_now_ms()) / 1000)),
        'token' => (string)$d['token'],
      ];
    }
    fb_json_out(['ok' => true, 'issues' => $list]);
  }

  default:
    fb_fail('action 非法，仅支持 register / login / my_resources / issue / revoke / logout / delete_account / verify_status / verify_send / verify_code / my_issues / register_resource / update_resource / delete_resource');
}
