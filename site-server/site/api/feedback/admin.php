<?php
/**
 * POST /api/feedback/admin.php —— 反馈后台管理接口（管理员令牌鉴权）
 * 动作（JSON body）：
 *   { action:"login",    user, pass }                       → { ok, token }
 *   { action:"list",     user, token }                      → { ok, items:[...] }
 *   { action:"add",      user, token, type, contact, content, public }
 *   { action:"edit",     user, token, id, type?, contact?, content? }
 *   { action:"delete",   user, token, id }
 *   { action:"reply",    user, token, id, reply }           → 同时标记为已回复
 *   { action:"setpublic",user, token, id, public }          → 是否公开到前端 TOP10
 */
require __DIR__ . '/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);
$in = json_decode(file_get_contents('php://input'), true);
if (!is_array($in)) fb_fail('请求体必须是 JSON');
$action = (string)($in['action'] ?? '');
$user = (string)($in['user'] ?? '');
$token = (string)($in['token'] ?? '');
$keys = fb_keys();

if ($action === 'login') {
  $pass = (string)($in['pass'] ?? '');
  // v5.1.2:先校验图形验证码(一次性消费,防自动化爆破)
  $captchaR = fb_captcha_verify((string)($in['captchaId'] ?? ''), (string)($in['captchaCode'] ?? ''));
  if ($captchaR !== true) {
    usleep(300 * 1000);
    fb_fail($captchaR, 401);
  }
  // v5.1.1:按 IP+用户名限速,15 分钟窗口内失败 5 次即锁定
  $ip = fb_client_ip();
  if (fb_login_fail_hit($ip, $user)) {
    usleep(600 * 1000);
    fb_fail('失败次数过多,请 15 分钟后再试', 429);
  }
  if ($user !== ($keys['admin_user'] ?? '') || !password_verify($pass, (string)($keys['admin_hash'] ?? ''))) {
    fb_login_fail_record($ip, $user);
    usleep(400 * 1000); // 减缓爆破
    fb_fail('用户名或密码错误', 401);
  }
  fb_login_fail_reset($ip, $user);
  fb_json_out(['ok' => true, 'token' => fb_session_create($user), 'user' => $user]);
}

/* v5.1.2:找回密码(未登录;需用户名+绑定邮箱匹配;统一响应防枚举) */
if ($action === 'forgot_request') {
  $u = trim($user);
  $email = trim((string)($in['email'] ?? ''));
  $matched = ($u !== '' && $u === ($keys['admin_user'] ?? ''))
    && $email !== '' && strlen($email) <= 120
    && strtolower($email) === strtolower(fb_admin_email());
  if ($matched) {
    $key = 'reset|' . hash('sha256', strtolower($email) . '|' . $u . '|' . (string)($keys['token_secret'] ?? ''));
    $r = fb_code_issue($key);
    if ($r['ok']) {
      $m = fb_mail_send($email, 'EasySSH 后台 — 找回密码验证码',
        "您的找回密码验证码为:{$r['code']}\n\n15 分钟内有效。如非本人操作,请忽略本邮件并检查账号安全。\n\n— EasySSH 官网后台(自动邮件)");
    }
  }
  // 无论是否命中都返回统一文案,防用户名/邮箱枚举
  fb_json_out(['ok' => true, 'message' => '如信息匹配,验证码已发送至绑定邮箱(15 分钟内有效)']);
}

if ($action === 'forgot_reset') {
  $u = trim($user);
  $email = trim((string)($in['email'] ?? ''));
  $code = trim((string)($in['code'] ?? ''));
  $newpass = (string)($in['newpass'] ?? '');
  if ($u === '' || $u !== ($keys['admin_user'] ?? '') || $email === ''
    || strtolower($email) !== strtolower(fb_admin_email())) {
    fb_fail('信息不匹配', 401);
  }
  if (mb_strlen($newpass) < 8) fb_fail('新密码至少 8 个字符');
  $key = 'reset|' . hash('sha256', strtolower($email) . '|' . $u . '|' . (string)($keys['token_secret'] ?? ''));
  $r = fb_code_verify($key, $code);
  if ($r !== true) fb_fail($r, 401);
  $rsave = fb_keys_save(['admin_hash' => password_hash($newpass, PASSWORD_DEFAULT)]);
  if ($rsave !== true) fb_fail($rsave);
  fb_sessions_revoke_all();
  fb_json_out(['ok' => true, 'message' => '密码已重置,请用新密码登录']);
}

if (!fb_check_token($user, $token)) fb_fail('未登录或登录已过期，请重新登录', 401);

/* v5.1.2:账号资料与邮箱绑定(登录态;带邮箱有效性验证) */
if ($action === 'profile') {
  $email = fb_admin_email();
  fb_json_out(['ok' => true, 'user' => $user, 'email' => $email]);
}

if ($action === 'setemail_request') {
  $email = trim((string)($in['email'] ?? ''));
  if ($email === '' || strlen($email) > 120 || !filter_var($email, FILTER_VALIDATE_EMAIL)) {
    fb_fail('邮箱地址格式不正确');
  }
  if (strtolower($email) === strtolower(fb_admin_email())) {
    fb_fail('该邮箱已是当前绑定邮箱');
  }
  $key = 'bind|' . hash('sha256', strtolower($email) . '|' . $user . '|' . (string)($keys['token_secret'] ?? ''));
  $r = fb_code_issue($key);
  if (!$r['ok']) fb_fail($r['message'], 429);
  $m = fb_mail_send($email, 'EasySSH 后台 — 绑定邮箱验证码',
    "您正在为 EasySSH 官网后台绑定邮箱,验证码为:{$r['code']}\n\n15 分钟内有效。如非本人操作,请忽略本邮件。\n\n— EasySSH 官网后台(自动邮件)");
  if ($m !== true) fb_fail($m, 500);
  fb_json_out(['ok' => true, 'message' => '验证码已发送,请查收邮箱(15 分钟内有效)']);
}

if ($action === 'setemail_confirm') {
  $email = trim((string)($in['email'] ?? ''));
  $code = trim((string)($in['code'] ?? ''));
  if ($email === '' || strlen($email) > 120 || !filter_var($email, FILTER_VALIDATE_EMAIL)) {
    fb_fail('邮箱地址格式不正确');
  }
  $key = 'bind|' . hash('sha256', strtolower($email) . '|' . $user . '|' . (string)($keys['token_secret'] ?? ''));
  $r = fb_code_verify($key, $code);
  if ($r !== true) fb_fail($r, 401);
  // 写入独立数据文件(data/admin-meta.json),不再依赖易被部署覆盖的 keys.php
  $rsave = fb_admin_email_set($email);
  if ($rsave !== true) fb_fail($rsave);
  fb_json_out(['ok' => true, 'message' => '邮箱绑定成功']);
}

/* v5.1.0:管理员自行修改用户名/密码(需当前密码确认;改完需重新登录) */
if ($action === 'changepwd') {
  $curpass = (string)($in['curpass'] ?? '');
  if (!password_verify($curpass, (string)($keys['admin_hash'] ?? ''))) {
    usleep(300 * 1000);
    fb_fail('当前密码不正确', 401);
  }
  $newuser = trim((string)($in['newuser'] ?? ''));
  $newpass = (string)($in['newpass'] ?? '');
  if ($newuser === '') $newuser = $keys['admin_user'];
  $newhash = $keys['admin_hash'];
  if ($newpass !== '') {
    if (mb_strlen($newpass) < 8) fb_fail('新密码至少 8 个字符');
    $newhash = password_hash($newpass, PASSWORD_DEFAULT);
  }
  $r = fb_keys_save(['admin_user' => $newuser, 'admin_hash' => $newhash]);
  if ($r !== true) fb_fail($r);
  fb_sessions_revoke_all(); // v5.1.1:改用户名或改密码后吊销全部会话,前端引导重新登录
  fb_json_out(['ok' => true, 'user' => $newuser]);
}

$db = fb_db_load();
$find = function ($id) use (&$db) {
  foreach ($db['items'] as $i => $it) if (($it['id'] ?? '') === $id) return $i;
  return -1;
};
$now = fb_now_ms();

switch ($action) {
  case 'list':
    // 按提交时间倒序
    usort($db['items'], function ($a, $b) { return (int)($b['ts'] ?? 0) <=> (int)($a['ts'] ?? 0); });
    fb_json_out(['ok' => true, 'items' => $db['items'], 'total' => count($db['items'])]);

  case 'add':
    $type = trim((string)($in['type'] ?? '其他'));
    if (!in_array($type, FB_TYPES, true)) $type = '其他';
    $content = trim((string)($in['content'] ?? ''));
    if (mb_strlen($content) < 5 || mb_strlen($content) > FB_MAX_CONTENT) fb_fail('内容需为 5-' . FB_MAX_CONTENT . ' 个字符');
    $contact = mb_substr(trim((string)($in['contact'] ?? '')), 0, FB_MAX_CONTACT);
    $id = 'fb' . base_convert((string)$now, 10, 36) . bin2hex(random_bytes(2));
    $db['items'][] = [
      'id' => $id, 'ts' => $now, 'date' => fb_date_str($now), 'ipHash' => '',
      'appVersion' => '', 'type' => $type, 'contact' => $contact, 'content' => $content,
      'reply' => '', 'replyAt' => 0, 'public' => (bool)($in['public'] ?? false), 'status' => 'new',
    ];
    fb_db_save($db);
    fb_json_out(['ok' => true, 'id' => $id]);

  case 'edit':
    $i = $find((string)($in['id'] ?? ''));
    if ($i < 0) fb_fail('反馈不存在', 404);
    foreach (['type' => FB_TYPES, 'contact' => null, 'content' => null] as $f => $allowed) {
      if (array_key_exists($f, $in)) {
        $v = trim((string)$in[$f]);
        if (is_array($allowed) && !in_array($v, $allowed, true)) fb_fail('类型非法');
        if ($f === 'content' && (mb_strlen($v) < 5 || mb_strlen($v) > FB_MAX_CONTENT)) fb_fail('内容需为 5-' . FB_MAX_CONTENT . ' 个字符');
        if ($f === 'contact') $v = mb_substr($v, 0, FB_MAX_CONTACT);
        $db['items'][$i][$f] = $v;
      }
    }
    fb_db_save($db);
    fb_json_out(['ok' => true]);

  case 'delete':
    $i = $find((string)($in['id'] ?? ''));
    if ($i < 0) fb_fail('反馈不存在', 404);
    array_splice($db['items'], $i, 1);
    fb_db_save($db);
    fb_json_out(['ok' => true]);

  case 'reply':
    $i = $find((string)($in['id'] ?? ''));
    if ($i < 0) fb_fail('反馈不存在', 404);
    $reply = trim((string)($in['reply'] ?? ''));
    if ($reply === '' || mb_strlen($reply) > FB_MAX_CONTENT) fb_fail('回复内容需为 1-' . FB_MAX_CONTENT . ' 个字符');
    $db['items'][$i]['reply'] = $reply;
    $db['items'][$i]['replyAt'] = $now;
    $db['items'][$i]['status'] = 'replied';
    fb_db_save($db);
    fb_json_out(['ok' => true]);

  case 'setpublic':
    $i = $find((string)($in['id'] ?? ''));
    if ($i < 0) fb_fail('反馈不存在', 404);
    $db['items'][$i]['public'] = (bool)($in['public'] ?? false);
    fb_db_save($db);
    fb_json_out(['ok' => true]);

  default:
    fb_fail('未知动作');
}
