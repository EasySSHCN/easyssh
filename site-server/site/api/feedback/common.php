<?php
/**
 * EasySSH 使用反馈 — 公共函数（网站端）
 *
 * 密钥与凭据存放于同目录 keys.php（PHP 退出守卫：首行 <?php exit(0); ?>，
 * 直接 HTTP 访问只会得到空响应，不会泄漏内容）。该文件属于敏感配置（SANITIZED 策略同
 * update-keys.json），不得随源码 zip / 版本库对外发布。
 *
 * 反馈协议（与在线升级检测接口同族，客户端 updater.js / main.js 同一套派生方式）：
 *   信封 = { "v":1, "alg":"A256GCM-HS256", "ts":<ms>, "nonce":b64, "ct":b64, "tag":b64, "mac":b64 }
 *   明文 = 反馈内容 JSON（UTF-8）
 *   ct   = AES-256-GCM(fbEncKey, nonce, 明文)
 *   mac  = HMAC-SHA256(fbMacKey, `${ts}.${nonce}.${ct}.${tag}`)  (b64)
 *   fbEncKey / fbMacKey = HKDF-SHA256(master, salt='easyssh-feedback-v1', info) 派生
 * 服务端校验：mac → ts 时效(±15 分钟) → 解密。
 */

define('FB_SALT', 'easyssh-feedback-v1');
// v5.1.1 安全修复:运行时数据目录移出 webroot。
// __DIR__ = <站点根>/api/feedback → dirname(__DIR__, 3) = webroot 的上级目录,
// 数据落在 <站点根上级>/data/,HTTP 无法再直接下载反馈库/统计库/会话文件。
define('FB_DATA_DIR', dirname(__DIR__, 3) . '/data');
define('FB_DB', FB_DATA_DIR . '/feedback-db.json');
define('FB_KEYS', __DIR__ . '/keys.php');
define('FB_TYPES', ['功能建议', '问题反馈', '其他']);
define('FB_MAX_CONTENT', 2000);
define('FB_MAX_CONTACT', 120);
define('FB_TS_WINDOW_MS', 15 * 60 * 1000);
define('FB_RATE_PER_HOUR', 5);
define('FB_RATE_MIN_GAP_MS', 60 * 1000);

function fb_json_out($obj, $code = 200) {
  http_response_code($code);
  header('Content-Type: application/json; charset=utf-8');
  header('Cache-Control: no-store');
  echo json_encode($obj, JSON_UNESCAPED_UNICODE);
  exit;
}

function fb_fail($msg, $code = 400) { fb_json_out(['ok' => false, 'message' => $msg], $code); }

/** 读取 keys.php 中 PHP 退出守卫之后的 JSON 配置 */
function fb_keys() {
  static $keys = null;
  if ($keys !== null) return $keys;
  if (!is_readable(FB_KEYS)) fb_fail('服务端配置缺失', 500);
  $lines = file(FB_KEYS, FILE_IGNORE_NEW_LINES);
  if (!$lines || count($lines) < 2) fb_fail('服务端配置损坏', 500);
  $json = implode("\n", array_slice($lines, 1));
  $keys = json_decode($json, true);
  if (!is_array($keys) || empty($keys['master'])) fb_fail('服务端配置损坏', 500);
  return $keys;
}

function fb_b64d($s) {
  $d = base64_decode((string)$s, true);
  if ($d === false) fb_fail('信封编码非法');
  return $d;
}

/** 派生反馈加解密密钥对（HMAC-SHA256 派生，跨 PHP/Node 行为完全一致，不依赖 HKDF） */
function fb_derive($masterRaw) {
  return [
    'enc' => hash_hmac('sha256', 'enc', $masterRaw, true),
    'mac' => hash_hmac('sha256', 'mac', $masterRaw, true),
  ];
}

/** 校验并解密请求信封，返回明文关联数组 */
function fb_decrypt_envelope($env) {
  if (!is_array($env) || ($env['alg'] ?? '') !== 'A256GCM-HS256' || ($env['v'] ?? 0) != 1) {
    fb_fail('信封格式非法');
  }
  $keys = fb_keys();
  $k = fb_derive(base64_decode($keys['master'], true));
  $ts = (string)($env['ts'] ?? '');
  $nonce = fb_b64d($env['nonce'] ?? '');
  $ct = fb_b64d($env['ct'] ?? '');
  $tag = fb_b64d($env['tag'] ?? '');
  $mac = fb_b64d($env['mac'] ?? '');
  if (strlen($nonce) !== 12 || strlen($tag) !== 16) fb_fail('信封参数非法');
  if (abs((int)$ts - (int)(microtime(true) * 1000)) > FB_TS_WINDOW_MS) fb_fail('请求已过期，请校对本机时间后重试');
  $want = hash_hmac('sha256', $ts . '.' . ($env['nonce'] ?? '') . '.' . ($env['ct'] ?? '') . '.' . ($env['tag'] ?? ''), $k['mac'], true);
  if (!hash_equals($want, $mac)) fb_fail('信封签名校验失败');
  $plain = openssl_decrypt($ct, 'aes-256-gcm', $k['enc'], OPENSSL_RAW_DATA, $nonce, $tag);
  if ($plain === false) fb_fail('解密失败');
  $data = json_decode($plain, true);
  if (!is_array($data)) fb_fail('解密后内容非法');
  return $data;
}

/** 读取反馈库（不存在时返回空结构） */
function fb_db_load() {
  if (!is_readable(FB_DB)) return ['items' => []];
  $j = json_decode((string)file_get_contents(FB_DB), true);
  if (!is_array($j) || !isset($j['items']) || !is_array($j['items'])) return ['items' => []];
  return $j;
}

/** 原子写回反馈库 */
function fb_db_save($db) {
  $dir = dirname(FB_DB);
  if (!is_dir($dir)) @mkdir($dir, 02775, true);
  $tmp = FB_DB . '.tmp.' . getmypid();
  if (@file_put_contents($tmp, json_encode($db, JSON_UNESCAPED_UNICODE | JSON_PRETTY_PRINT), LOCK_EX) === false) {
    fb_fail('服务器存储写入失败', 500);
  }
  @chmod($tmp, 0664);
  if (!@rename($tmp, FB_DB)) { @unlink($tmp); fb_fail('服务器存储写入失败', 500); }
}

/* ==================== 通用 JSON 小文件存取（频控/会话等） ==================== */

function fb_json_file_load($path) {
  if (!is_readable($path)) return [];
  $j = json_decode((string)file_get_contents($path), true);
  return is_array($j) ? $j : [];
}

/** 原子写任意 JSON 小文件（tmp + rename，防并发写坏） */
function fb_json_file_save($path, $data) {
  $dir = dirname($path);
  if (!is_dir($dir)) @mkdir($dir, 02775, true);
  $tmp = $path . '.tmp.' . getmypid();
  if (@file_put_contents($tmp, json_encode($data, JSON_UNESCAPED_UNICODE), LOCK_EX) === false) return false;
  @chmod($tmp, 0664);
  if (!@rename($tmp, $path)) { @unlink($tmp); return false; }
  return true;
}

/* ==================== 客户端 IP：只信任可信代理的 X-Forwarded-For ==================== */

/** 判断 $ip 是否落在 $cidr（支持 IPv4 a.b.c.d/nn 与 IPv6） */
function fb_ip_in_cidr($ip, $cidr) {
  $cidr = trim($cidr);
  if ($cidr === '') return false;
  if (strpos($cidr, '/') === false) return $ip === $cidr;
  list($sub, $bits) = explode('/', $cidr, 2);
  $bits = (int)$bits;
  if (filter_var($ip, FILTER_VALIDATE_IP, FILTER_FLAG_IPV4) && filter_var($sub, FILTER_VALIDATE_IP, FILTER_FLAG_IPV4)) {
    if ($bits < 0 || $bits > 32) return false;
    return ($bits === 0) || ((ip2long($ip) >> (32 - $bits)) === (ip2long($sub) >> (32 - $bits)));
  }
  $i6 = @inet_pton($ip); $s6 = @inet_pton($sub);
  if ($i6 === false || $s6 === false || strlen($i6) !== strlen($s6)) return false;
  if ($bits < 0 || $bits > strlen($i6) * 8) return false;
  $full = intdiv($bits, 8);
  if (substr($i6, 0, $full) !== substr($s6, 0, $full)) return false;
  $rem = $bits % 8;
  if ($rem && (ord($i6[$full]) >> (8 - $rem)) !== (ord($s6[$full]) >> (8 - $rem))) return false;
  return true;
}

/**
 * v5.1.1 安全修复:不再盲目信任 X-Forwarded-For。
 * 仅当直连方(REMOTE_ADDR)是 keys.php 中 trusted_proxies 声明的反向代理时,
 * 才取 XFF 的最后一跳(单个可信代理场景下即代理追加的真实客户端 IP);
 * 否则一律用 REMOTE_ADDR,防止伪造头绕过 IP 频控。
 * 代理网段配置在 keys.php 的 "trusted_proxies" 字段(逗号分隔 CIDR,如 "127.0.0.1,::1,10.0.0.0/8"),
 * 也兼容环境变量 FB_TRUSTED_PROXIES。
 */
function fb_client_ip() {
  $remote = $_SERVER['REMOTE_ADDR'] ?? '';
  $cfg = (string)(fb_keys()['trusted_proxies'] ?? '');
  if ($cfg === '') $cfg = (string)(getenv('FB_TRUSTED_PROXIES') ?: '');
  $trusted = array_filter(array_map('trim', explode(',', $cfg)));
  foreach ($trusted as $cidr) {
    if (fb_ip_in_cidr($remote, $cidr)) {
      $parts = array_map('trim', explode(',', $_SERVER['HTTP_X_FORWARDED_FOR'] ?? ''));
      $parts = array_values(array_filter($parts, function ($p) { return $p !== ''; }));
      if ($parts) return (string)end($parts); // 最后一跳 = 可信代理追加的来源
      break;
    }
  }
  return $remote;
}

function fb_now_ms() { return (int)(microtime(true) * 1000); }

function fb_date_str($ms) { return date('Y-m-d H:i', (int)($ms / 1000)); }

/* ==================== 管理员会话（v5.1.1:可吊销随机 token,替代静态当日 HMAC） ==================== */

define('FB_SESSION_TTL_MS', 12 * 3600 * 1000);          // 会话有效期 12 小时
define('FB_SESSIONS_FILE', FB_DATA_DIR . '/admin-sessions.json');
define('FB_SESSIONS_MAX', 50);                           // 最多同时保留 50 个会话
define('FB_LOGIN_FAIL_FILE', FB_DATA_DIR . '/login-fail.json');
define('FB_LOGIN_FAIL_WINDOW_MS', 15 * 60 * 1000);       // 失败计数窗口
define('FB_LOGIN_FAIL_MAX', 5);                          // 窗口内最多失败次数
define('FB_CODES_FILE', FB_DATA_DIR . '/verify-codes.json');   // 邮箱绑定/找回密码验证码
define('FB_CAPTCHA_FILE', FB_DATA_DIR . '/captcha-store.json'); // 图形验证码
define('FB_CAPTCHA_RATE_FILE', FB_DATA_DIR . '/captcha-ratelimit.json'); // v5.1.3:验证码签发按 IP 频控
define('FB_CODE_TTL_MS', 15 * 60 * 1000);                // 邮箱验证码有效期
define('FB_CODE_RESEND_GAP_MS', 60 * 1000);              // 两次发送最小间隔
define('FB_CODE_HOURLY_MAX', 5);                         // 同目标每小时最多发送次数
define('FB_CODE_ATTEMPT_MAX', 5);                        // 单码最多校验次数

/** 创建会话,返回随机 token（存服务端,可随时吊销） */
function fb_session_create($user) {
  $sessions = fb_json_file_load(FB_SESSIONS_FILE);
  $now = fb_now_ms();
  // 清理过期会话
  foreach ($sessions as $t => $s) {
    if (!is_array($s) || (int)($s['expires'] ?? 0) <= $now) unset($sessions[$t]);
  }
  // 超量时丢弃最旧的
  if (count($sessions) >= FB_SESSIONS_MAX) {
    uasort($sessions, function ($a, $b) { return (int)($a['expires'] ?? 0) <=> (int)($b['expires'] ?? 0); });
    $sessions = array_slice($sessions, count($sessions) - FB_SESSIONS_MAX + 1, null, true);
  }
  $token = bin2hex(random_bytes(32));
  $sessions[$token] = ['user' => (string)$user, 'expires' => $now + FB_SESSION_TTL_MS, 'created' => $now];
  fb_json_file_save(FB_SESSIONS_FILE, $sessions);
  return $token;
}

/** 校验会话（user + token 必须都匹配且未过期） */
function fb_check_token($user, $token) {
  if (!is_string($token) || strlen($token) !== 64 || !ctype_xdigit($token)) return false;
  $sessions = fb_json_file_load(FB_SESSIONS_FILE);
  $s = $sessions[$token] ?? null;
  if (!is_array($s)) return false;
  if ((string)($s['user'] ?? '') !== (string)$user) return false;
  if ((int)($s['expires'] ?? 0) <= fb_now_ms()) return false;
  return true;
}

/** 吊销全部会话（修改密码/用户名后调用） */
function fb_sessions_revoke_all() {
  fb_json_file_save(FB_SESSIONS_FILE, []);
}

/* ==================== 登录失败限速（按 IP+用户名） ==================== */

function fb_login_fail_key($ip, $user) {
  return hash('sha256', $ip . '|' . $user . '|' . (string)(fb_keys()['token_secret'] ?? ''));
}

/** 命中限速返回 true（应拒绝登录） */
function fb_login_fail_hit($ip, $user) {
  $now = fb_now_ms();
  $db = fb_json_file_load(FB_LOGIN_FAIL_FILE);
  $key = fb_login_fail_key($ip, $user);
  $rec = $db[$key] ?? null;
  if (is_array($rec) && (int)($rec['window'] ?? 0) > $now - FB_LOGIN_FAIL_WINDOW_MS) {
    return (int)($rec['count'] ?? 0) >= FB_LOGIN_FAIL_MAX;
  }
  return false;
}

function fb_login_fail_record($ip, $user) {
  $now = fb_now_ms();
  $db = fb_json_file_load(FB_LOGIN_FAIL_FILE);
  $key = fb_login_fail_key($ip, $user);
  $rec = $db[$key] ?? null;
  if (is_array($rec) && (int)($rec['window'] ?? 0) > $now - FB_LOGIN_FAIL_WINDOW_MS) {
    $rec['count'] = (int)($rec['count'] ?? 0) + 1;
    $rec['window'] = max((int)$rec['window'], $now); // 滑动窗口:失败即顺延
  } else {
    $rec = ['count' => 1, 'window' => $now];
  }
  $db[$key] = $rec;
  // 清理过期记录与超量条目（防文件无限膨胀）
  foreach ($db as $k => $r) {
    if (!is_array($r) || (int)($r['window'] ?? 0) <= $now - FB_LOGIN_FAIL_WINDOW_MS) unset($db[$k]);
  }
  if (count($db) > 5000) $db = array_slice($db, -5000, null, true);
  fb_json_file_save(FB_LOGIN_FAIL_FILE, $db);
}

function fb_login_fail_reset($ip, $user) {
  $db = fb_json_file_load(FB_LOGIN_FAIL_FILE);
  unset($db[fb_login_fail_key($ip, $user)]);
  fb_json_file_save(FB_LOGIN_FAIL_FILE, $db);
}

/* ==================== 邮件发送（v5.1.2:后台验证码通道） ==================== */

/** 发送邮件。keys.php 'mail' => {url, key}。返回 true 或错误消息字符串 */
function fb_mail_send($to, $subject, $text) {
  $mail = fb_keys()['mail'] ?? null;
  if (!is_array($mail) || empty($mail['url']) || empty($mail['key'])) return '邮件通道未配置';
  $url = (string)$mail['url'];
  if (substr($url, -8) !== '/v1/send') $url = rtrim($url, '/') . '/v1/send';
  $body = json_encode(['to' => [(string)$to], 'subject' => (string)$subject, 'text' => (string)$text], JSON_UNESCAPED_UNICODE);
  $ch = curl_init($url);
  curl_setopt_array($ch, [
    CURLOPT_POST => true,
    CURLOPT_POSTFIELDS => $body,
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_TIMEOUT => 20,
    CURLOPT_HTTPHEADER => ['Content-Type: application/json; charset=utf-8', 'Authorization: Bearer ' . (string)$mail['key']],
  ]);
  $res = curl_exec($ch);
  $err = curl_errno($ch) ? curl_error($ch) : '';
  $code = (int)curl_getinfo($ch, CURLINFO_RESPONSE_CODE);
  curl_close($ch);
  if ($res === false) return '邮件发送失败:' . $err;
  $j = json_decode((string)$res, true);
  if ($code >= 400 || !is_array($j) || empty($j['ok'])) return '邮件发送失败:' . (string)($j['message'] ?? ('HTTP ' . $code));
  return true;
}

/* ==================== 邮箱验证码（绑定/找回共用，服务端一次性消费） ==================== */

function fb_code_hash($code) {
  return hash('sha256', (string)$code . '|' . (string)(fb_keys()['token_secret'] ?? ''));
}

/**
 * 颁发 6 位数字验证码。
 * 返回 ['ok'=>true,'code'=>'xxxxxx'] 或 ['ok'=>false,'message'=>错误]。
 * $key 由调用方拼接用途+身份维度，例如 'bind|<user>|<emailHash>'、'reset|<user>|<emailHash>'。
 */
function fb_code_issue($key, $ttlMs = FB_CODE_TTL_MS) {
  $now = fb_now_ms();
  $db = fb_json_file_load(FB_CODES_FILE);
  foreach ($db as $k => $v) if (!is_array($v) || (int)($v['expires'] ?? 0) <= $now) unset($db[$k]);
  $sent = [];
  $rec = $db[$key] ?? null;
  if (is_array($rec)) {
    if ($now - (int)($rec['issuedAt'] ?? 0) < FB_CODE_RESEND_GAP_MS) {
      return ['ok' => false, 'message' => '发送过于频繁,请 1 分钟后再试'];
    }
    $sent = array_values(array_filter(is_array($rec['sent'] ?? null) ? $rec['sent'] : [],
      function ($t) use ($now) { return $t > $now - 3600 * 1000; }));
    if (count($sent) >= FB_CODE_HOURLY_MAX) {
      return ['ok' => false, 'message' => '该目标发送次数已达上限,请稍后再试'];
    }
  }
  if (count($db) > 500) $db = array_slice($db, -500, null, true);
  $code = str_pad((string)random_int(0, 999999), 6, '0', STR_PAD_LEFT);
  $db[$key] = [
    'hash' => fb_code_hash($code),
    'expires' => $now + $ttlMs,
    'attempts' => 0,
    'issuedAt' => $now,
    'sent' => array_merge($sent, [$now]),
  ];
  fb_json_file_save(FB_CODES_FILE, $db);
  return ['ok' => true, 'code' => $code];
}

/** 校验验证码。返回 true 或错误消息字符串。正确即销毁(一次性)。 */
function fb_code_verify($key, $code, $maxAttempts = FB_CODE_ATTEMPT_MAX) {
  $now = fb_now_ms();
  $db = fb_json_file_load(FB_CODES_FILE);
  $rec = $db[$key] ?? null;
  if (!is_array($rec)) return '请先获取验证码';
  if ((int)($rec['expires'] ?? 0) <= $now) { unset($db[$key]); fb_json_file_save(FB_CODES_FILE, $db); return '验证码已过期,请重新获取'; }
  if ((int)($rec['attempts'] ?? 0) >= $maxAttempts) { unset($db[$key]); fb_json_file_save(FB_CODES_FILE, $db); return '验证码错误次数过多,请重新获取'; }
  $rec['attempts'] = (int)($rec['attempts'] ?? 0) + 1;
  $ok = is_string($code) && strlen($code) === 6 && ctype_digit($code)
    && hash_equals((string)($rec['hash'] ?? ''), fb_code_hash($code));
  if (!$ok) { $db[$key] = $rec; fb_json_file_save(FB_CODES_FILE, $db); return '验证码错误'; }
  unset($db[$key]);
  fb_json_file_save(FB_CODES_FILE, $db);
  return true;
}

/* ==================== 图形验证码（v5.1.2:登录口） ==================== */

/**
 * v5.1.3:按 IP 滑动窗口频控(通用)。返回 true=放行,false=超限。
 * $key 维度为 sha256(client ip);条目数封顶防文件膨胀;原子写。
 */
function fb_ip_rate_ok($file, $max, $windowMs = 3600000, $maxKeys = 5000) {
  $key = hash('sha256', (string)fb_client_ip());
  $now = fb_now_ms();
  $db = fb_json_file_load($file);
  $list = array_values(array_filter(is_array($db[$key] ?? null) ? $db[$key] : [],
    function ($t) use ($now, $windowMs) { return $t > $now - $windowMs; }));
  if (count($list) >= $max) return false;
  $list[] = $now;
  $db[$key] = $list;
  foreach ($db as $k => $arr) {
    $db[$k] = array_values(array_filter(is_array($arr) ? $arr : [],
      function ($t) use ($now, $windowMs) { return $t > $now - $windowMs; }));
    if (!count($db[$k])) unset($db[$k]);
  }
  if (count($db) > $maxKeys) $db = array_slice($db, -$maxKeys, null, true);
  fb_json_file_save($file, $db);
  return true;
}

/** 校验并消费图形验证码。返回 true 或错误消息字符串(不区分大小写) */
function fb_captcha_verify($id, $code) {
  $now = fb_now_ms();
  $db = fb_json_file_load(FB_CAPTCHA_FILE);
  $rec = $db[(string)$id] ?? null;
  if (!is_array($rec)) return '图形验证码无效,请刷新后重试';
  unset($db[(string)$id]); // 一次性:无论对错都销毁,防重放
  fb_json_file_save(FB_CAPTCHA_FILE, $db);
  if ((int)($rec['expires'] ?? 0) <= $now) return '图形验证码已过期,请刷新后重试';
  if (!is_string($code) || strlen($code) < 4 || strlen($code) > 8
    || !hash_equals((string)($rec['code'] ?? ''), strtolower(trim($code)))) {
    return '图形验证码错误';
  }
  return true;
}

/** v5.1.0:原子保存 keys.php(保持 PHP 退出守卫行,仅允许改 admin_user/admin_hash) */
/* ==================== 管理员绑定邮箱独立存储 ====================
 * 历史问题:绑定邮箱只写进 keys.php 的 admin_email 字段,而 keys.php 属于敏感凭据文件,
 * 站点重新部署/覆盖上传时会被仓库内的 keys.php 覆盖,导致已绑定的邮箱"莫名丢失"。
 * 现改为写入运行时数据目录 data/admin-meta.json(部署不覆盖),keys.php 中的 admin_email
 * 仅作为旧数据兼容回退;忘记密码校验同样走 fb_admin_email()。 */
define('FB_ADMIN_META', FB_DATA_DIR . '/admin-meta.json');

function fb_admin_meta_load() {
  $j = fb_json_file_load(FB_ADMIN_META);
  return is_array($j) ? $j : [];
}

/** 当前生效的绑定邮箱:优先 data/admin-meta.json,回退 keys.php 旧字段 */
function fb_admin_email() {
  $meta = fb_admin_meta_load();
  $e = trim((string)($meta['admin_email'] ?? ''));
  if ($e !== '') return $e;
  return trim((string)(fb_keys()['admin_email'] ?? ''));
}

/** 写入绑定邮箱到独立数据文件,返回 true 或错误消息 */
function fb_admin_email_set($email) {
  $meta = fb_admin_meta_load();
  $meta['admin_email'] = (string)$email;
  $meta['updatedAt'] = fb_now_ms();
  return fb_json_file_save(FB_ADMIN_META, $meta) ? true : '绑定信息写入失败';
}

function fb_keys_save($keys) {
  $cur = fb_keys();
  $new = [
    'master'      => (string)$cur['master'],
    'admin_user'  => (string)($keys['admin_user'] ?? $cur['admin_user']),
    'admin_hash'  => (string)($keys['admin_hash'] ?? $cur['admin_hash']),
    'token_secret'=> (string)$cur['token_secret'],
    'trusted_proxies' => (string)($cur['trusted_proxies'] ?? ''), // v5.1.1:保留可信代理配置
    'admin_email' => (string)($keys['admin_email'] ?? ($cur['admin_email'] ?? '')), // v5.1.2:管理员绑定邮箱
    'mail'        => is_array($cur['mail'] ?? null) ? $cur['mail'] : [],            // v5.1.2:邮件通道配置
  ];
  if (strlen($new['admin_user']) < 4 || mb_strlen($new['admin_user']) > 40) return '新用户名需 4-40 个字符';
  if (strpos($new['admin_hash'], '$2y$') !== 0 && strpos($new['admin_hash'], '$2a$') !== 0) return '哈希格式非法';
  $out = "<?php exit(0); ?>\n" . json_encode($new, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES) . "\n";
  $tmp = FB_KEYS . '.tmp.' . getmypid();
  if (@file_put_contents($tmp, $out, LOCK_EX) === false) return '凭据文件写入失败';
  @chmod($tmp, 0660);
  if (!@rename($tmp, FB_KEYS)) { @unlink($tmp); return '凭据文件替换失败'; }
  return true;
}

/* ==================== 反馈附件(v5.1.7 / R-20261115-005:截图等多媒体,严格安全设计) ====================
 * 设计要点:
 *  - 存储在 webroot 之外(FB_DATA_DIR/attachments/),HTTP 无法直接访问,仅 attachment.php 鉴权分发;
 *  - 文件名一律服务端随机生成(attId),绝不使用用户提供的文件名,杜绝路径穿越;
 *  - magic bytes 二次校验(PNG/JPEG/GIF/WebP),与客户端声明的后缀无关;
 *  - 元数据剥离:PNG/JPEG/WebP 经 GD 解码→重编码(天然剥离 EXIF/GPS/ICC 等);GIF 保留动画帧,
 *    按结构级清洗——丢弃 Comment/Application/PlainText 扩展块,仅保留图形控制扩展与图像数据;
 *  - 限流:附件上传按 IP 每小时 ≤30 次;每条反馈最多绑定 3 个附件;单文件 ≤8MB;
 *  - 未绑定附件 1 小时后自动回收(文件删除),防孤儿文件堆积。
 */
define('FB_ATT_DIR', FB_DATA_DIR . '/attachments');
define('FB_ATT_INDEX', FB_DATA_DIR . '/attachments-index.json');
define('FB_ATT_MAX_BYTES', 8 * 1024 * 1024);          // 单文件上限 8MB
define('FB_ATT_MAX_PER_FB', 3);                        // 每条反馈最多 3 个附件
define('FB_ATT_PENDING_TTL_MS', 3600 * 1000);          // 未绑定附件保留 1 小时
define('FB_ATT_MIME', [
  'png'  => 'image/png',
  'jpg'  => 'image/jpeg',
  'gif'  => 'image/gif',
  'webp' => 'image/webp',
]);

/** magic bytes 判定真实图片类型,返回扩展名(png/jpg/gif/webp)或 null */
function fb_att_detect($b) {
  if (!is_string($b) || strlen($b) < 12) return null;
  if (substr($b, 0, 8) === "\x89PNG\r\n\x1a\n") return 'png';
  if (substr($b, 0, 3) === "\xff\xd8\xff") return 'jpg';
  if (substr($b, 0, 6) === 'GIF87a' || substr($b, 0, 6) === 'GIF89a') return 'gif';
  if (strlen($b) >= 12 && substr($b, 0, 4) === 'RIFF' && substr($b, 8, 4) === 'WEBP') return 'webp';
  return null;
}

/**
 * GIF 结构级清洗(保留动画):逐块解析,丢弃 Comment(0xFE)/Application(0xFF)/PlainText(0x01)
 * 扩展块(这些块可携带任意元数据/HTML 注入内容),保留 Graphic Control(0xF9)、图像描述符与数据。
 * 解析不合法即抛异常,调用方拒收。返回清洗后的 GIF 二进制。
 */
function fb_gif_sanitize($d) {
  if (strlen($d) < 14) throw new RuntimeException('GIF 过短');
  $sig = substr($d, 0, 6);
  if ($sig !== 'GIF87a' && $sig !== 'GIF89a') throw new RuntimeException('GIF 签名非法');
  $packed = ord($d[10]);
  $i = 13;
  if ($packed & 0x80) {
    $gct = 3 * (2 << ($packed & 7));
    if ($i + $gct > strlen($d)) throw new RuntimeException('GIF 全局色表越界');
    $i += $gct;
  }
  $out = substr($d, 0, $i);
  $len = strlen($d);
  $hasImage = false;
  while ($i < $len) {
    $b = ord($d[$i]);
    if ($b === 0x3B) { // trailer
      $out .= $d[$i]; $i++;
      break;
    }
    if ($b === 0x2C) { // image descriptor
      if ($i + 10 > $len) throw new RuntimeException('GIF 图像描述符越界');
      $out .= substr($d, $i, 10); $i += 10;
      $p = ord($out[strlen($out) - 1]);
      if ($p & 0x80) {
        $lct = 3 * (2 << ($p & 7));
        if ($i + $lct > $len) throw new RuntimeException('GIF 局部色表越界');
        $out .= substr($d, $i, $lct); $i += $lct;
      }
      if ($i >= $len) throw new RuntimeException('GIF 缺少 LZW 最小码长');
      $out .= $d[$i]; $i++;
      while (true) { // data sub-blocks
        if ($i >= $len) throw new RuntimeException('GIF 数据块未终止');
        $s = ord($d[$i]); $out .= $d[$i]; $i++;
        if ($s === 0) break;
        if ($i + $s > $len) throw new RuntimeException('GIF 数据子块越界');
        $out .= substr($d, $i, $s); $i += $s;
      }
      $hasImage = true;
    } elseif ($b === 0x21) { // extension
      if ($i + 2 > $len) throw new RuntimeException('GIF 扩展块越界');
      $label = ord($d[$i + 1]); $i += 2;
      $blocks = '';
      while (true) {
        if ($i >= $len) throw new RuntimeException('GIF 扩展块未终止');
        $s = ord($d[$i]); $i++;
        if ($s === 0) break;
        if ($i + $s > $len) throw new RuntimeException('GIF 扩展子块越界');
        $blocks .= substr($d, $i, $s); $i += $s;
      }
      // 仅保留图形控制扩展(动画延时/透明所需的 4 字节);其余全部丢弃
      if ($label === 0xF9 && strlen($blocks) === 4) {
        $out .= "\x21\xF9" . $blocks . "\x00";
      }
    } else {
      throw new RuntimeException('GIF 出现未知块 0x' . dechex($b));
    }
  }
  if (!$hasImage) throw new RuntimeException('GIF 缺少图像数据');
  if ($i !== $len || substr($out, -1) !== "\x3B") throw new RuntimeException('GIF 尾部非法');
  return $out;
}

/** PNG/JPEG/WebP 经 GD 解码→重编码,剥离全部元数据;失败返回 null */
function fb_att_reencode($b, $ext) {
  if (!function_exists('imagecreatefromstring')) return null;
  $im = @imagecreatefromstring($b);
  if (!$im) return null;
  ob_start();
  $ok = false;
  if ($ext === 'png') {
    if (function_exists('imagealphablending')) imagealphablending($im, false);
    if (function_exists('imagesavealpha')) imagesavealpha($im, true);
    $ok = @imagepng($im, null, 6);
  } elseif ($ext === 'jpg') {
    $ok = @imagejpeg($im, null, 85);
  } elseif ($ext === 'webp' && function_exists('imagewebp')) {
    $ok = @imagewebp($im, null, 85);
  }
  @imagedestroy($im);
  if (!$ok) { ob_end_clean(); return null; }
  $out = ob_get_clean();
  return is_string($out) && strlen($out) > 0 ? $out : null;
}

/** 附件清洗总入口:返回清洗后的字节,失败抛 RuntimeException(调用方拒收) */
function fb_att_sanitize($b, $ext) {
  if (strlen($b) > FB_ATT_MAX_BYTES) throw new RuntimeException('附件超过 8MB 上限');
  if ($ext === 'gif') return fb_gif_sanitize($b);
  $out = fb_att_reencode($b, $ext);
  if ($out === null) throw new RuntimeException('图像解码失败,已拒收(仅支持真实图片文件)');
  if (!fb_att_detect($out)) throw new RuntimeException('重编码结果异常,已拒收');
  return $out;
}

/** 附件索引读写 */
function fb_att_index_load() {
  $j = fb_json_file_load(FB_ATT_INDEX);
  return is_array($j) ? $j : [];
}
function fb_att_index_save($idx) {
  if (count($idx) > 500) $idx = array_slice($idx, -500, null, true);
  return fb_json_file_save(FB_ATT_INDEX, $idx);
}

/**
 * 回收应清理的附件(删文件 + 出索引),返回清理后的索引:
 *  ① 未绑定且超过保留期(FB_ATT_PENDING_TTL_MS);
 *  ② v5.1.8 (R-20261116-004) 兜底:bound 指向的反馈 id 已不存在于反馈库(宿主被删的孤儿)。
 */
function fb_att_gc($idx = null) {
  if ($idx === null) $idx = fb_att_index_load();
  $now = fb_now_ms();
  // 反馈库现存 id 集合(用于悬空 bound 判定)
  static $fbIds = null;
  if ($fbIds === null) {
    $db = fb_db_load();
    $fbIds = [];
    foreach ($db['items'] as $it) $fbIds[(string)($it['id'] ?? '')] = true;
  }
  foreach ($idx as $id => $rec) {
    if (!is_array($rec)) { unset($idx[$id]); continue; }
    $bound = (string)($rec['bound'] ?? '');
    $orphan = ($bound !== '' && !isset($fbIds[$bound]));
    if ($orphan || ($bound === '' && $now - (int)($rec['ts'] ?? 0) > FB_ATT_PENDING_TTL_MS)) {
      @unlink(FB_ATT_DIR . '/' . $id . '.' . (string)($rec['ext'] ?? 'bin'));
      @unlink(FB_ATT_DIR . '/' . $id . '.thumb.webp');
      unset($idx[$id]);
    }
  }
  return $idx;
}

/**
 * v5.1.8 (R-20261116-003):生成缩略图(最长边 400px WebP,GIF 取首帧),失败返回 false(可回退原图)。
 * 缩略图存为 <attId>.thumb.webp,与原图同目录,由 fb_att_gc 一并回收。
 */
define('FB_ATT_THUMB_MAX_SIDE', 400);
function fb_att_make_thumb($b, $thumbPath) {
  if (!function_exists('imagecreatefromstring')) return false;
  $im = @imagecreatefromstring($b);
  if (!$im) return false;
  $w = imagesx($im); $h = imagesy($im);
  $max = max($w, $h);
  if ($max <= FB_ATT_THUMB_MAX_SIDE) {
    $tw = $w; $th = $h;
  } else {
    $scale = FB_ATT_THUMB_MAX_SIDE / $max;
    $tw = max(1, (int)round($w * $scale));
    $th = max(1, (int)round($h * $scale));
  }
  $ok = false;
  if (function_exists('imagescale')) {
    $tm = @imagescale($im, $tw, $th);
    if ($tm) {
      if (function_exists('imagewebp')) $ok = @imagewebp($tm, $thumbPath, 80);
      if (!$ok && function_exists('imagejpeg')) $ok = @imagejpeg($tm, $thumbPath, 80);
      @imagedestroy($tm);
    }
  }
  @imagedestroy($im);
  return $ok;
}
