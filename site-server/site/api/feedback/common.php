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
