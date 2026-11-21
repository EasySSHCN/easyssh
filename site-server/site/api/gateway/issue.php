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
define('GW_ISSUE_HOURLY_MAX', 20);        // 每 IP 每小时最多签发次数

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

/** 服务端内网回环调网关（POST JSON + X-Admin-Key）。返回 [ok=true, data] 或 [ok=false, message, status] */
function gw_call($path, $body) {
  $k = gw_keys();
  $url = rtrim((string)$k['gateway_url'], '/') . $path;
  $ctx = stream_context_create(['http' => [
    'method'  => 'POST',
    'timeout' => 5,
    'header'  => "Content-Type: application/json; charset=utf-8\r\nX-Admin-Key: " . (string)$k['gateway_admin_key']
               . "\r\nUser-Agent: easyssh-site-issue-proxy\r\n",
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

/** Cookie 鉴权。未登录/过期直接 401 退出 */
function gw_require_session() {
  $token = (string)($_COOKIE[GW_COOKIE] ?? '');
  if (strlen($token) !== 64 || !ctype_xdigit($token)) fb_fail('未登录或登录已过期，请重新登录', 401);
  $sessions = fb_json_file_load(GW_SESSIONS_FILE);
  $s = $sessions[$token] ?? null;
  if (!is_array($s) || (string)($s['user'] ?? '') === '' || (int)($s['expires'] ?? 0) <= fb_now_ms()) {
    fb_fail('未登录或登录已过期，请重新登录', 401);
  }
  return (string)$s['user'];
}

function gw_users_load() { $j = fb_json_file_load(GW_USERS_FILE); return is_array($j) ? $j : []; }
function gw_resources_load() { $j = fb_json_file_load(GW_RESOURCES_FILE); return is_array($j) ? $j : []; }

switch ($action) {

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
    $users[$email] = ['hash' => password_hash($pass, PASSWORD_DEFAULT), 'createdAt' => fb_now_ms()];
    if (!fb_json_file_save(GW_USERS_FILE, $users)) fb_fail('服务器存储写入失败', 500);
    fb_json_out(['ok' => true, 'message' => '注册成功，请登录']);
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
    fb_json_out(['ok' => true, 'email' => $email]);
  }

  /* ---------- 我的资源 ---------- */
  case 'my_resources': {
    $email = gw_require_session();
    $all = gw_resources_load();
    $list = [];
    foreach ((array)($all[$email] ?? []) as $r) {
      if (!is_array($r) || ($r['status'] ?? '') !== 'approved') continue; // 仅展示已开通资源
      $list[] = ['resource_id' => (string)($r['resource_id'] ?? ''), 'label' => (string)($r['label'] ?? ''), 'status' => 'approved'];
    }
    fb_json_out(['ok' => true, 'resources' => $list]);
  }

  /* ---------- 自助签发短期 token（role=user 真实 JWT，服务端内网回环调网关） ---------- */
  case 'issue': {
    $email = gw_require_session();
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
    // 服务端内网回环 127.0.0.1:8443 调网关签 role=user 真实 JWT
    $k = gw_keys();
    $path = (string)($k['issue_path'] ?? '/admin/issue');
    list($ok, $data) = gw_call($path, [
      // easyssh-gateway /admin/issue 契约：{"sub":"...","roles":["user"],"ttl_seconds":N}
      'sub' => $email,
      'roles' => ['user'],
      'ttl_seconds' => GW_TOKEN_TTL_SEC,
    ]);
    if (!$ok) fb_fail((string)$data, 502);
    $token = (string)($data['token'] ?? '');
    if ($token === '') fb_fail('网关未返回 token', 502);
    fb_json_out([
      'ok' => true,
      'token' => $token,
      'expires_in' => (int)($data['expires_in'] ?? GW_TOKEN_TTL_SEC),
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
    fb_json_out(['ok' => true, 'message' => '授权已吊销：已停止自助签发；已签发 token 将在 10 分钟内自然过期']);
  }

  default:
    fb_fail('action 非法，仅支持 register / login / my_resources / issue / revoke');
}
