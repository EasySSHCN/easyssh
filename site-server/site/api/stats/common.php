<?php
/**
 * EasySSH 客户端使用情况统计 — 公共函数（网站端，v5.1.0）
 *
 * 用途边界（合规声明）：客户端匿名统计上报，仅用于软件改进统计，
 * 不含主机名/网卡MAC/完整IP等身份信息，不与第三方共享，不作其他用途。
 *
 * 密钥复用反馈通道 keys.php（主密钥 update-keys.json 同源）：
 *   信封 = { "v":1, "alg":"A256GCM-HS256", "ts":<ms>, "nonce":b64, "ct":b64, "tag":b64, "mac":b64 }
 *   ct   = AES-256-GCM(HMAC(master,"enc"), nonce, 明文JSON)
 *   mac  = HMAC-SHA256(HMAC(master,"mac"), `${ts}.${nonce}.${ct}.${tag}`)
 * （与客户端 feedback.rs / stats.rs 同一套派生；服务端直接复用 fb_decrypt_envelope）
 *
 * 存储：data/stats-db.json（与 feedback-db.json 同目录同模式，原子写）
 *   { "clients": { <匿名ID>: {...} }, "daily": { "YYYY-MM-DD": <活跃数> }, "meta": {...} }
 */

// v5.1.1 安全修复:数据目录移出 webroot(与反馈模块同目录,webroot 上级的 data/)
define('ST_DATA_DIR', dirname(__DIR__, 3) . '/data');
define('ST_DB', ST_DATA_DIR . '/stats-db.json');
define('ST_MAX_FIELDS', 24);      // 字段白名单上限
define('ST_MAX_VALUE', 160);      // 单字段值最大长度
define('ST_MIN_REPORT_MS', 12 * 3600 * 1000); // 同设备两次上报最小间隔（服务端兜底去抖）
define('ST_RATE_FILE', ST_DATA_DIR . '/stats-ratelimit.json');
define('ST_GEO_CACHE', ST_DATA_DIR . '/stats-geocache.json'); // IP前缀 → 中文地域名 缓存
define('ST_GEO_CACHE_MAX', 20000); // 缓存条目上限
define('ST_GEO_API_TIMEOUT', 3);   // 归属地接口超时（秒）
// 主查询接口：ip.263.tw 加密归属地 API（Bearer token，服务器私有配置，禁止随源码包/仓库发布）
define('ST_GEO_PRIMARY', 'https://ip.263.tw/api/v1/');
define('ST_GEO_TOKEN', '33da2b6569fa5bd55a7c1a08a11e0cc91a11b607eb4335f00b438438f399fc90');
define('ST_RATE_PER_HOUR', 30);   // 同 IP 每小时上报上限
define('ST_RATE_MAX_KEYS', 10000); // v5.1.1:频控文件条目上限,防伪造 IP 撑爆磁盘
define('ST_TS_WINDOW_MS', 15 * 60 * 1000);
// v5.1.3:设备活跃度与提交流水(同设备 24h 后重复提交的兼容性记录)
define('ST_ACTIVITY_KEEP_DAYS', 181); // 每设备按日活跃计数保留天数(超期先归档到 archives/ 再清理)
define('ST_LOG_MAX', 500);           // 全局提交流水最多保留条数
// v5.1.6 归因关联:下载票据存储(独立文件,webroot 外)与归因参数
define('ST_TICKETS_FILE', ST_DATA_DIR . '/stats-tickets.json');
define('ST_TICKET_KEEP_MS', 90 * 24 * 3600 * 1000);      // 票据记录保留期(90天)
define('ST_TICKET_WINDOW_MS', 7 * 24 * 3600 * 1000);     // 归因匹配窗(默认7天,可调常量)
define('ST_TICKET_MAX', 5000);                           // 票据条数上限
define('ST_ORIGIN_MAX_DEVICES', 10);                     // 单票据最多归因设备数(绿色版复制到多台场景)
define('ST_HEURISTIC_WINDOW_MS', 7 * 24 * 3600 * 1000);  // 方案 B 启发式回溯窗(同 IP 访客流水)

require_once __DIR__ . '/../feedback/common.php';
require_once __DIR__ . '/../visit/common.php'; // vst_db_load / vst_src_classify / vst_src_label(归因共用)

function st_json_out($obj, $code = 200) {
  http_response_code($code);
  header('Content-Type: application/json; charset=utf-8');
  header('Cache-Control: no-store');
  echo json_encode($obj, JSON_UNESCAPED_UNICODE);
  exit;
}

function st_fail($msg, $code = 400) { st_json_out(['ok' => false, 'message' => $msg], $code); }

/** 统计库结构初始化 */
function st_db_default() {
  return [
    'clients' => [], 'daily' => [],
    // v5.1.3:activity = { <匿名ID>: { "YYYY-MM-DD": <当日提交次数> } };log = 提交流水(最新在尾)
    'activity' => [], 'log' => [],
    'meta' => ['created' => st_now_ms(), 'note' => '匿名使用统计,仅统计用途'],
  ];
}

function st_db_load() {
  if (!is_readable(ST_DB)) return st_db_default();
  /* v5.1.5:同一请求内按 mtime+size 缓存已解析的库,避免 summary/list 等多次调用重复 JSON 解析(跨请求不共享,写入路径不受影响) */
  static $cache = null, $cacheM = -1, $cacheS = '';
  if (!empty($GLOBALS['st_db_cache_dirty'])) { $cache = null; $GLOBALS['st_db_cache_dirty'] = false; } // 同请求内保存后强制失效,规避 mtime 秒级精度
  clearstatcache(false, ST_DB);
  $m = @filemtime(ST_DB);
  $s = (string)@filesize(ST_DB);
  if ($cache !== null && $m === $cacheM && $s === $cacheS) return $cache;
  $j = json_decode((string)file_get_contents(ST_DB), true);
  if (!is_array($j) || !isset($j['clients']) || !is_array($j['clients'])) return st_db_default();
  if (!isset($j['daily']) || !is_array($j['daily'])) $j['daily'] = [];
  if (!isset($j['activity']) || !is_array($j['activity'])) $j['activity'] = []; // 老库兼容
  if (!isset($j['log']) || !is_array($j['log'])) $j['log'] = [];
  $cache = $j; $cacheM = $m; $cacheS = $s;
  return $j;
}

function st_db_save($db) {
  $dir = dirname(ST_DB);
  if (!is_dir($dir)) @mkdir($dir, 02775, true);
  $tmp = ST_DB . '.tmp.' . getmypid();
  if (@file_put_contents($tmp, json_encode($db, JSON_UNESCAPED_UNICODE), LOCK_EX) === false) {
    st_fail('服务器存储写入失败', 500);
  }
  @chmod($tmp, 0664);
  if (!@rename($tmp, ST_DB)) { @unlink($tmp); st_fail('服务器存储写入失败', 500); }
  $GLOBALS['st_db_cache_dirty'] = true; // 写入后使同请求内的读缓存失效
}

function st_now_ms() { return (int)(microtime(true) * 1000); }
function st_today() { return date('Y-m-d'); }

/* ---------- v5.1.3:设备活跃度与提交流水 ---------- */

/** 归档机制(超保留期记录先落 archives/stats-YYYY-MM.json 再从活动库清理,看板/列表只读活动库不受影响) */
function st_archive_dir() {
  $d = ST_DATA_DIR . '/archives';
  if (!is_dir($d)) @mkdir($d, 02775, true);
  return $d;
}

/** 归档每日计数,按月分文件合并写:{ daily: {日期:次数} } / { activity: {日期:{匿名ID:次数}} },同键重归档以新覆盖旧(幂等) */
function st_archive_merge($month, array $data) {
  $f = st_archive_dir() . '/stats-' . $month . '.json';
  $a = is_readable($f) ? json_decode((string)file_get_contents($f), true) : [];
  if (!is_array($a)) $a = [];
  foreach ($data as $sect => $recs) {
    if (!isset($a[$sect]) || !is_array($a[$sect])) $a[$sect] = [];
    foreach ($recs as $k => $v) $a[$sect][$k] = $v;
  }
  fb_json_file_save($f, $a);
}


/** 每次成功入库的上报都记一笔：按日活跃计数 + 全局提交流水（去抖窗口内的静默接受不记） */
function st_touch_activity(&$db, $anonId, $version, $nowMs) {
  $d = st_today();
  if (!isset($db['activity'][$anonId]) || !is_array($db['activity'][$anonId])) $db['activity'][$anonId] = [];
  $db['activity'][$anonId][$d] = (int)($db['activity'][$anonId][$d] ?? 0) + 1;
  // 超保留期(181天)的日期键:先归档到 archives/stats-YYYY-MM.json 再清理
  $cut = date('Y-m-d', (int)(($nowMs - ST_ACTIVITY_KEEP_DAYS * 24 * 3600 * 1000) / 1000));
  $stale = [];
  foreach ($db['activity'][$anonId] as $k => $v) {
    if (strtotime($k) !== false && strtotime($k) < strtotime($cut)) { $stale[$k] = $v; unset($db['activity'][$anonId][$k]); }
  }
  foreach ($stale as $k => $v) st_archive_merge(substr($k, 0, 7), ['activity' => [$k => [$anonId => $v]]]);
  if (!count($db['activity'][$anonId])) unset($db['activity'][$anonId]);
  // 全局提交流水：保留最新 ST_LOG_MAX 条
  $db['log'][] = ['t' => $nowMs, 'id' => $anonId, 'v' => mb_substr((string)$version, 0, 40)];
  if (count($db['log']) > ST_LOG_MAX) $db['log'] = array_slice($db['log'], -ST_LOG_MAX);
}

/** 设备活跃度：近30天活跃天数 / 连续活跃天数(今天或昨天起算) / 累计活跃天数 */
function st_activity_of($db, $anonId) {
  $dates = isset($db['activity'][$anonId]) && is_array($db['activity'][$anonId]) ? $db['activity'][$anonId] : [];
  if (!count($dates)) return ['days30' => 0, 'streak' => 0, 'total' => 0];
  ksort($dates);
  $total = count($dates);
  $now = st_now_ms();
  $in30 = 0;
  foreach ($dates as $k => $v) {
    $t = strtotime($k);
    if ($t !== false && $t * 1000 >= $now - 30 * 24 * 3600 * 1000) $in30++;
  }
  // 连续活跃：从今天(或昨天,今天还没上报也算连续)往回数连续有记录的天数
  $streak = 0;
  $day = strtotime(st_today());
  if (!isset($dates[date('Y-m-d', $day)])) $day = strtotime('-1 day', $day); // 允许从昨天起算
  while (isset($dates[date('Y-m-d', $day)])) { $streak++; $day = strtotime('-1 day', $day); }
  return ['days30' => $in30, 'streak' => $streak, 'total' => $total];
}

/** 提交流水（倒序分页）：每条 {t, anonId, v} + 联查设备档案补充展示字段(v5.1.5) */
function st_log_recent($db, $page, $pageSize) {
  $rows = array_reverse(array_values($db['log']));
  $total = count($rows);
  $rows = array_slice($rows, ($page - 1) * $pageSize, $pageSize);
  foreach ($rows as $i => $r) {
    // 设备可能已被删除:档案缺失时字段留空,流水本身仍完整展示
    $c = isset($db['clients'][$r['id']]) && is_array($db['clients'][$r['id']]) ? $db['clients'][$r['id']] : null;
    $f = $c && is_array($c['fields'] ?? null) ? $c['fields'] : [];
    $rows[$i] = array_merge($r, [
      'os' => (string)($f['操作系统'] ?? ''),
      'arch' => (string)($f['系统架构'] ?? ''),
      'cpu' => (string)($f['CPU'] ?? ''),
      'mem' => (string)($f['内存'] ?? ''),
      'disk' => (string)($f['磁盘容量'] ?? ''),
      'gpu' => (string)($f['显卡'] ?? ''),
      'lang' => (string)($f['系统语言代码'] ?? ''),
      'region' => (string)($f['地域'] ?? ''),
      'clientType' => (string)($f['客户端类型'] ?? ''),
      'reports' => $c ? (int)($c['reports'] ?? 1) : 0,
      'lastSeen' => $c ? (int)($c['lastSeen'] ?? 0) : 0,
      'activityTotal' => $c ? (int)(st_activity_of($db, $r['id'])['total'] ?? 0) : 0,
      'origin' => $c ? ($c['origin'] ?? null) : null, // v5.1.6 动态行来源徽章
    ]);
  }
  return ['rows' => $rows, 'total' => $total];
}

/** 顶部统计卡片明细（v5.1.4）：all=累计设备/week=近7天活跃/day=近24小时活跃，倒序分页（含活跃度） */
function st_active_list($db, $window, $page, $pageSize) {
  $spans = ['week' => 7 * 24 * 3600 * 1000, 'day' => 24 * 3600 * 1000]; // 与 st_summary 口径一致
  $cut = isset($spans[$window]) ? st_now_ms() - $spans[$window] : 0;
  $rows = [];
  foreach ($db['clients'] as $id => $c) {
    $last = (int)($c['lastSeen'] ?? 0);
    if ($last < $cut) continue;
    $rows[] = array_merge(['anonId' => $id, 'firstSeen' => $c['firstSeen'] ?? 0, 'lastSeen' => $last,
                           'reports' => $c['reports'] ?? 1, 'activity' => st_activity_of($db, $id),
                           'origin' => $c['origin'] ?? null], $c['fields'] ?? []);
  }
  usort($rows, function ($a, $b) { return $b['lastSeen'] <=> $a['lastSeen']; });
  $total = count($rows);
  $rows = array_slice($rows, ($page - 1) * $pageSize, $pageSize);
  return ['rows' => $rows, 'total' => $total];
}

/** 校验并解密统计信封（复用反馈通道信封校验，额外要求 kind=stats 标记） */
function st_decrypt_envelope($env) {
  $data = fb_decrypt_envelope($env);
  if (($data['kind'] ?? '') !== 'stats') st_fail('非法统计载荷');
  return $data;
}

/** 同 IP 频控（滑动小时窗口，记录在独立小文件，避免污染统计库） */
function st_rate_check($ip) {
  $now = st_now_ms();
  $j = is_readable(ST_RATE_FILE) ? json_decode((string)file_get_contents(ST_RATE_FILE), true) : null;
  if (!is_array($j) || !isset($j['hits']) || !is_array($j['hits'])) $j = ['hits' => []];
  $key = hash('sha256', (string)$ip);
  $window = $now - 3600 * 1000;
  $list = isset($j['hits'][$key]) && is_array($j['hits'][$key]) ? $j['hits'][$key] : [];
  $list = array_values(array_filter($list, function ($t) use ($window) { return $t > $window; }));
  if (count($list) >= ST_RATE_PER_HOUR) st_fail('上报过于频繁', 429);
  $list[] = $now;
  // 全库清理（所有 IP 一小时前记录）
  foreach ($j['hits'] as $k => $arr) {
    $j['hits'][$k] = array_values(array_filter($arr, function ($t) use ($window) { return $t > $window; }));
    if (!count($j['hits'][$k])) unset($j['hits'][$k]);
  }
  $j['hits'][$key] = $list;
  // v5.1.1:条目上限保护 + 原子写,防伪造 XFF 撑爆文件/并发写坏
  if (count($j['hits']) > ST_RATE_MAX_KEYS) {
    // 超量时保留最新写入的条目之外的部分丢弃(随机丢足够,防蓄意绕过)
    $keys = array_keys($j['hits']);
    shuffle($keys);
    $j['hits'] = array_intersect_key($j['hits'], array_flip(array_slice($keys, 0, ST_RATE_MAX_KEYS)));
  }
  fb_json_file_save(ST_RATE_FILE, $j);
}

/**
 * 字段白名单过滤：只接收明确允许的统计字段，值截断，字段数封顶。
 * 返回 [key => value] 关联数组（全部字符串）。
 */
function st_allowed_fields() {
  // 允许上报的字段名（与客户端 stats.rs 采集字段一一对应）
  return [
    '软件版本', '操作系统', '系统架构', 'CPU', '内存', '磁盘容量', '显卡',
    '系统语言代码', '地域', '启动次数', '首次上报', '客户端类型',
  ];
}

function st_sanitize($data) {
  $allowed = st_allowed_fields();
  $out = [];
  foreach ($allowed as $f) {
    if (!isset($data[$f])) continue;
    $v = trim((string)$data[$f]);
    if ($v === '' || mb_strlen($v) > ST_MAX_VALUE) continue;
    $out[$f] = mb_substr($v, 0, ST_MAX_VALUE);
    if (count($out) >= ST_MAX_FIELDS) break;
  }
  return $out;
}

/** 匿名ID合法性：64位十六进制以内（客户端 sha256 截断 hex） */
function st_valid_anon_id($id) {
  return is_string($id) && preg_match('/^[0-9a-f]{16,64}$/', $id) === 1;
}

/** 顶层数据地域字段规整：只允许 "xx.xx" 两段数字形式（客户端已截断，这里再兜底） */
function st_region_of($region) {
  if (!is_string($region)) return '';
  if (preg_match('/^(\d{1,3})\.(\d{1,3})$/', $region, $m)) return $m[1] . '.' . $m[2];
  return ''; // 非 IPv4 前缀形式一律丢弃
}

/* ---------- 归属地解析（上报时用完整 IP 查中文地域名，按前缀缓存，兼容 IPv4/IPv6） ---------- */

function st_geo_cache_load() {
  $j = is_readable(ST_GEO_CACHE) ? json_decode((string)file_get_contents(ST_GEO_CACHE), true) : null;
  return (is_array($j) && isset($j['map']) && is_array($j['map'])) ? $j['map'] : [];
}

/** 公网 IP 判定与前缀提取：返回 [family, prefix, probe]；非公网/非法返回 null。
 *  IPv4: prefix="a.b"  probe="a.b.1.1"
 *  IPv6: prefix="xxxx:xxxx"（前两个 hextet）  probe="xxxx:xxxx::1" */
function st_ip_prefix($ip) {
  $ip = (string)$ip;
  if (filter_var($ip, FILTER_VALIDATE_IP, FILTER_FLAG_IPV4 | FILTER_FLAG_NO_PRIV_RANGE | FILTER_FLAG_NO_RES_RANGE) !== false) {
    $seg = explode('.', $ip);
    return ['v4', $seg[0] . '.' . $seg[1], $seg[0] . '.' . $seg[1] . '.1.1'];
  }
  if (filter_var($ip, FILTER_VALIDATE_IP, FILTER_FLAG_IPV6 | FILTER_FLAG_NO_PRIV_RANGE | FILTER_FLAG_NO_RES_RANGE) !== false) {
    $h = bin2hex((string)inet_pton($ip)); // 32 位十六进制
    $prefix = substr($h, 0, 4) . ':' . substr($h, 4, 4);
    return ['v6', $prefix, $prefix . '::1'];
  }
  return null;
}

/** 掩码形态的地域标签：IPv4 → "a.b.x.x"；IPv6 → "xxxx:xxxx::/32"（只保留前 64 位路由前缀的前两段） */
function st_ip_mask_label($family, $prefix) {
  return $family === 'v6' ? $prefix . '::/32' : $prefix . '.x.x';
}

/** 主查询：ip.263.tw 加密归属地 API（/ip 加密载荷 → /decrypt 解密）。失败返回 ''。 */
function st_geo_query_primary($probe) {
  $ctx = stream_context_create(['http' => [
    'timeout' => ST_GEO_API_TIMEOUT,
    'header'  => "Authorization: Bearer " . ST_GEO_TOKEN . "\r\nContent-Type: application/x-www-form-urlencoded\r\n",
  ]]);
  // 1) 查询（返回 AES-256-GCM 加密载荷）
  $resp = @file_get_contents(rtrim(ST_GEO_PRIMARY, '/') . '/ip?q=' . urlencode($probe), false, $ctx);
  if ($resp === false || $resp === '') return '';
  $j = json_decode($resp, true);
  $payload = (is_array($j) && ($j['code'] ?? -1) === 0 && isset($j['data']['payload'])) ? (string)$j['data']['payload'] : '';
  if ($payload === '') return '';
  // 2) 解密载荷
  $resp2 = @file_get_contents(rtrim(ST_GEO_PRIMARY, '/') . '/decrypt', false,
    stream_context_create(['http' => [
      'timeout' => ST_GEO_API_TIMEOUT,
      'method'  => 'POST',
      'header'  => "Authorization: Bearer " . ST_GEO_TOKEN . "\r\nContent-Type: application/x-www-form-urlencoded\r\n",
      'content' => http_build_query(['payload' => $payload]),
    ]]));
  if ($resp2 === false || $resp2 === '') return '';
  $j2 = json_decode($resp2, true);
  $text = (is_array($j2) && ($j2['code'] ?? -1) === 0 && isset($j2['data']['text'])) ? (string)$j2['data']['text'] : '';
  if ($text === '') return '';
  $t = json_decode($text, true);
  if (!is_array($t)) return '';
  // 优先 raw（形如 "中国|江苏省@盐城市|联通"），分隔符转空格；缺失时由字段拼装
  $name = '';
  if (!empty($t['raw'])) $name = trim(str_replace(['@', '|'], ' ', (string)$t['raw']));
  if ($name === '') $name = trim((($t['country'] ?? '') . ' ' . ($t['region'] ?? '') . ($t['prov'] ?? '') . ' ' . ($t['city'] ?? '')));
  // 清洗：去掉尾部两位国家代码（如 " CN"）、去掉与备用接口冗余的开头 "中国 "
  $name = preg_replace('/\s+[A-Z]{2}$/u', '', $name);
  $name = preg_replace('/^中国\s+/u', '', $name);
  $name = trim($name);
  if ($name === '' || $name === '保留' || mb_strlen($name) > 40) return '';
  return $name;
}

/** 备用①：百度 opendata 归属地接口（UTF-8 JSON，location 形如 "江苏省盐城市 联通"）。
 *  ⚠ 实测仅支持 IPv4（v6 查询返回空 data:[]），只对 v4 探测使用。失败返回 ''。 */
function st_geo_query_baidu($probe) {
  $ctx = stream_context_create(['http' => [
    'timeout' => ST_GEO_API_TIMEOUT,
    'header'  => "User-Agent: Mozilla/5.0\r\n",
  ]]);
  $resp = @file_get_contents(
    'https://opendata.baidu.com/api.php?query=' . urlencode($probe) . '&co=&resource_id=6006&oe=utf8',
    false,
    $ctx
  );
  if ($resp === false || $resp === '') return '';
  $j = json_decode($resp, true);
  $loc = (is_array($j) && isset($j['data'][0]['location'])) ? trim((string)$j['data'][0]['location']) : '';
  if ($loc === '' || $loc === '保留' || mb_strlen($loc) > 40) return '';
  return $loc;
}

/** 备用②：ip-api.com（lang=zh-CN，v4/v6 均支持，中文结果）。
 *  免费档仅 HTTP、限频 45 次/分钟——仅在主接口与百度都失败时才调用。失败返回 ''。 */
function st_geo_query_ipapi($probe) {
  $ctx = stream_context_create(['http' => [
    'timeout' => ST_GEO_API_TIMEOUT,
    'header'  => "User-Agent: Mozilla/5.0\r\n",
  ]]);
  $resp = @file_get_contents(
    'http://ip-api.com/json/' . urlencode($probe) . '?lang=zh-CN&fields=status,country,regionName,city',
    false,
    $ctx
  );
  if ($resp === false || $resp === '') return '';
  $j = json_decode($resp, true);
  if (!is_array($j) || ($j['status'] ?? '') !== 'success') return '';
  $name = trim((string)($j['country'] ?? '') . ' ' . (string)($j['regionName'] ?? '') . ' ' . (string)($j['city'] ?? ''));
  $name = preg_replace('/^中国\s+/u', '', $name);
  if ($name === '' || mb_strlen($name) > 40) return '';
  return $name;
}

/** 解析公网 IP（IPv4/IPv6）的中文地域名：主备查询链。
 *  主=ip.263.tw(v4+v6) → 备①百度(v4专用) → 备②ip-api.com(v4+v6)。失败返回 ''。仅成功结果进缓存。 */
function st_region_name($ip) {
  $pp = st_ip_prefix($ip);
  if ($pp === null) return ''; // 内网/保留段/非法 IP 不解析
  list($family, $prefix, $probe) = $pp;

  $map = st_geo_cache_load();
  if (array_key_exists($prefix, $map)) return (string)$map[$prefix];

  // 主备链
  $name = st_geo_query_primary($probe);
  if ($name === '' && $family === 'v4') $name = st_geo_query_baidu($probe); // 百度不支持 v6
  if ($name === '') $name = st_geo_query_ipapi($probe);
  if ($name === '') return '';

  // 成功才写缓存（带上限保护，防伪造 XFF 撑爆）
  if (count($map) >= ST_GEO_CACHE_MAX) $map = array_slice($map, (int)(ST_GEO_CACHE_MAX / 4), null, true);
  $map[$prefix] = $name;
  fb_json_file_save(ST_GEO_CACHE, ['map' => $map]);
  return $name;
}

/** 上报入库前：把 "中文地域名 + IP前缀掩码形态" 写入地域字段（中文名在前），兼容 IPv4/IPv6 */
function st_apply_region(&$fields, $ip) {
  $pp = st_ip_prefix($ip);
  if ($pp === null) { unset($fields['地域']); return; } // 非公网/非法：不留地域字段
  list($family, $prefix) = $pp;
  $name = st_region_name($ip);
  $fields['地域'] = trim(($name !== '' ? $name . ' ' : '') . st_ip_mask_label($family, $prefix));
  if ($fields['地域'] === '') unset($fields['地域']);
}

/* ---------- v5.1.6 归因关联：下载票据匹配 / 方案 B 启发式 / origin 写入 ---------- */

/** 下载票据表读取（独立小文件,不存在返回空结构） */
function st_tickets_load() {
  $j = fb_json_file_load(ST_TICKETS_FILE);
  if (!isset($j['tickets']) || !is_array($j['tickets'])) return ['tickets' => []];
  return $j;
}

/** 票据归因设备数上限判断：已满 10 台的票据不再归因新设备 */
function st_ticket_available($t) {
  return is_array($t) && count(is_array($t['attributed'] ?? null) ? $t['attributed'] : []) < ST_ORIGIN_MAX_DEVICES;
}

/**
 * 为设备匹配下载票据。返回 null 或 ['tok'=>..., 'ticket'=>..., 'confidence'=>'high'|'low']。
 * $tok 非空（客户端二期携带）→ 精确查找该票据(须未超保留期)；
 * 否则扫描该 IP 名下未满额票据：优先「窗内(下载在 ST_TICKET_WINDOW_MS 内且早于首报)」最近一张(confidence=high),
 * 窗内无 → 取该 IP 保留期内(90天)最近一张未满额票据(confidence=low,展示「疑似」)。
 */
function st_origin_find_ticket($ip, $now, $tok = null) {
  $j = st_tickets_load();
  $tickets = $j['tickets'];
  if ($tok !== null && $tok !== '') {
    $t = isset($tickets[(string)$tok]) ? $tickets[(string)$tok] : null;
    if (!is_array($t) || (int)($t['ts'] ?? 0) < $now - ST_TICKET_KEEP_MS) return null;
    return ['tok' => (string)$tok, 'ticket' => $t, 'confidence' => 'high'];
  }
  $inWin = []; $outWin = [];
  foreach ($tickets as $k => $t) {
    if (!is_array($t) || (string)($t['ip'] ?? '') !== (string)$ip) continue;
    if (!st_ticket_available($t)) continue;
    $ts = (int)($t['ts'] ?? 0);
    if ($ts > $now + 60 * 1000) continue;          // 未来票据(时钟偏移容忍1分钟)不参与
    if ($ts < $now - ST_TICKET_KEEP_MS) continue;  // 超保留期不参与
    if ($now - $ts <= ST_TICKET_WINDOW_MS) $inWin[$k] = $t;
    else $outWin[$k] = $t;
  }
  if (count($inWin)) {
    uasort($inWin, function ($a, $b) { return (int)($b['ts'] ?? 0) <=> (int)($a['ts'] ?? 0); });
    $k = array_key_first($inWin);
    return ['tok' => (string)$k, 'ticket' => $inWin[$k], 'confidence' => 'high'];
  }
  if (count($outWin)) {
    uasort($outWin, function ($a, $b) { return (int)($b['ts'] ?? 0) <=> (int)($a['ts'] ?? 0); });
    $k = array_key_first($outWin);
    return ['tok' => (string)$k, 'ticket' => $outWin[$k], 'confidence' => 'low'];
  }
  return null;
}

/**
 * 方案 B 启发式：在访客流水中找「同完整IP + 设备 firstSeen 前 ST_HEURISTIC_WINDOW_MS 内」最近一次访问。
 * 返回 origin 数组(不含 tok) 或 null。注意 NAT/公司网络会误归因,调用方一律按 via=heuristic 展示「疑似」。
 */
function st_origin_heuristic($ip, $firstSeen) {
  $vdb = vst_db_load();
  $best = null;
  foreach ($vdb['recent'] as $r) {
    if (!is_array($r)) continue;
    if ((string)($r['f'] ?? '') !== (string)$ip) continue;
    $t = (int)($r['t'] ?? 0);
    if ($t > $firstSeen || $firstSeen - $t > ST_HEURISTIC_WINDOW_MS) continue;
    if ($best === null || $t > (int)($best['t'] ?? 0)) $best = $r;
  }
  if ($best === null) return null;
  $rf = (string)($best['rf'] ?? '');
  return [
    'src' => vst_src_classify($rf),
    'url' => $rf,
    'visitId' => (string)($best['id'] ?? ''),
    'visitTs' => $t2 = (int)($best['t'] ?? 0),
    'via' => 'heuristic',
    'attributedAt' => st_now_ms(),
  ];
}

/**
 * 归因主入口（report.php 调用）：新设备首报 或 携带 tok 的上报。
 * 1) 载荷带合法 tok → 精确归因/重归因（票据写入 attributed,支持清档重装后重新挂载）;
 * 2) 新设备无 tok → 票据 IP+窗匹配（high/low confidence）→ 方案 B 启发式兜底 → 均未命中 = unknown 不写 origin。
 * origin 仅服务端写入设备档案,不在上报字段白名单内。
 * 返回写入的 origin 或 null。
 */
function st_origin_apply(&$c, $anonId, $clientIp, $now, $tok = null) {
  $tok = is_string($tok) ? trim($tok) : '';
  if ($tok !== '' && preg_match('/^[0-9a-f]{32,128}$/', $tok) !== 1) $tok = ''; // 非法 tok 一律忽略
  $origin = null;

  if ($tok !== '') {
    $hit = st_origin_find_ticket($clientIp, $now, $tok);
    if ($hit !== null) {
      $t = $hit['ticket'];
      $origin = [
        'src' => (string)($t['src'] ?? 'unknown'),
        'url' => (string)($t['url'] ?? ''),
        'visitId' => (string)($t['visitId'] ?? ''),
        'visitTs' => st_origin_visit_ts((string)($t['visitId'] ?? ''), (int)($t['ts'] ?? 0), (string)($t['ip'] ?? '')),
        'tok' => $tok,
        'downloadAt' => (int)($t['ts'] ?? 0),
        'attributedAt' => $now,
        'via' => 'ticket',
        'confidence' => ($now - (int)($t['ts'] ?? 0) <= ST_TICKET_WINDOW_MS) ? 'high' : 'low',
      ];
    }
  }

  // 无 tok 或 tok 票据未命中 → 仅新设备(尚无 origin)走票据扫描 + 启发式兜底;老设备零额外开销
  if ($origin === null && !isset($c['origin'])) {
    $hit = st_origin_find_ticket($clientIp, $now, null);
    if ($hit !== null) {
      $t = $hit['ticket'];
      $origin = [
        'src' => (string)($t['src'] ?? 'unknown'),
        'url' => (string)($t['url'] ?? ''),
        'visitId' => (string)($t['visitId'] ?? ''),
        'visitTs' => st_origin_visit_ts((string)($t['visitId'] ?? ''), (int)($t['ts'] ?? 0), (string)($t['ip'] ?? '')),
        'tok' => $hit['tok'],
        'downloadAt' => (int)($t['ts'] ?? 0),
        'attributedAt' => $now,
        'via' => 'ticket',
        'confidence' => $hit['confidence'],
      ];
    } else {
      $origin = st_origin_heuristic($clientIp, (int)($c['firstSeen'] ?? $now));
    }
  }

  if ($origin === null) return null;

  // 票据侧登记归因设备（多设备共用,上限 ST_ORIGIN_MAX_DEVICES 台）
  $tk = (string)($origin['tok'] ?? '');
  if ($tk !== '') {
    $j = st_tickets_load();
    if (isset($j['tickets'][$tk]) && is_array($j['tickets'][$tk])) {
      $t = $j['tickets'][$tk];
      if (!isset($t['attributed']) || !is_array($t['attributed'])) $t['attributed'] = [];
      if (!in_array($anonId, $t['attributed'], true) && count($t['attributed']) < ST_ORIGIN_MAX_DEVICES) {
        $t['attributed'][] = $anonId;
        $j['tickets'][$tk] = $t;
        fb_json_file_save(ST_TICKETS_FILE, $j);
      }
    }
  }
  return $origin;
}

/** 按 visitId 反查访客流水时间(存量票据/记录无 id 时按 t+ip 在窗内兜底),取不到返回票据签发时间 */
function st_origin_visit_ts($visitId, $fallbackTs, $ip) {
  if ($visitId === '') return $fallbackTs;
  $vdb = vst_db_load();
  foreach ($vdb['recent'] as $r) {
    if (is_array($r) && (string)($r['id'] ?? '') === $visitId) return (int)($r['t'] ?? $fallbackTs);
  }
  return $fallbackTs;
}

/* ---------- 看板聚合（api/stats/admin.php 使用） ---------- */

function st_summary($db) {
  $now = st_now_ms();
  $weekAgo = $now - 7 * 24 * 3600 * 1000;
  $dayAgo  = $now - 24 * 3600 * 1000;
  $total = count($db['clients']);
  $week = 0; $day = 0;
  $byVer = []; $byOs = []; $byCpu = []; $byMem = []; $byDisk = []; $byGpu = []; $byLang = []; $byRegion = []; $bySrc = [];
  foreach ($db['clients'] as $c) {
    $last = (int)($c['lastSeen'] ?? 0);
    if ($last > $weekAgo) $week++;
    if ($last > $dayAgo) $day++;
    $f = $c['fields'] ?? [];
    $inc = function (array &$bucket, $key) { if ($key !== '') $bucket[$key] = ($bucket[$key] ?? 0) + 1; };
    $os = trim((string)($f['操作系统'] ?? ''));
    $inc($byOs, preg_replace('/\(Build [^)]*\)/', '', $os));
    $inc($byVer, trim((string)($f['软件版本'] ?? '')));
    $inc($byCpu, trim((string)($f['CPU'] ?? '')));
    $inc($byMem, trim((string)($f['内存'] ?? '')));
    $inc($byDisk, trim((string)($f['磁盘容量'] ?? '')));
    $inc($byGpu, trim((string)($f['显卡'] ?? '')));
    $inc($byLang, trim((string)($f['系统语言代码'] ?? '')));
    $inc($byRegion, trim((string)($f['地域'] ?? '')) ?: '未知');
    // v5.1.6 来源分布：按设备档案 origin.src 聚合（无 origin 的设备计入 unknown）
    $inc($bySrc, (string)($c['origin']['src'] ?? 'unknown'));
  }
  $top = function ($bucket, $n = 5) {
    arsort($bucket);
    return array_slice($bucket, 0, $n, true);
  };
  $sortFull = function ($bucket) {
    arsort($bucket);
    return $bucket; // 全量，降序
  };
  // 活跃趋势：近 30 天,按「当日活跃设备数(去重)」统计。
  // 修复：不再读 daily 计数器(它按上报次数累加,同设备一天可计 2 次,且历史迁移/重放会把
  // 单日计数虚增至累计设备数,如 2026-11-15 出现的 19 台异常柱——其中 16 条 activity 流水
  // 是全量同步灌入的孤儿记录,对应设备在 clients 中无档案)。改由 activity
  // (每设备×每日一条)反推每日去重设备数,且仅计入 clients 中真实存在的设备。
  $dailyDevices = [];
  foreach (($db['activity'] ?? []) as $anonId => $dates) {
    if (!is_array($dates) || !isset($db['clients'][$anonId])) continue; // 排除孤儿活跃流水
    foreach ($dates as $k => $v) {
      if (strtotime((string)$k) !== false) $dailyDevices[(string)$k] = ($dailyDevices[(string)$k] ?? 0) + 1;
    }
  }
  $trend = [];
  for ($i = 29; $i >= 0; $i--) {
    $d = date('Y-m-d', (int)(($now - $i * 24 * 3600 * 1000) / 1000));
    $trend[] = ['date' => $d, 'count' => (int)($dailyDevices[$d] ?? 0)];
  }
  return [
    'total' => $total, 'weekActive' => $week, 'dayActive' => $day,
    'byVersion' => $top($byVer), 'byOs' => $top($byOs), 'byCpu' => $top($byCpu),
    'byMem' => $top($byMem), 'byDisk' => $top($byDisk), 'byGpu' => $top($byGpu), 'byLang' => $top($byLang),
    'byRegion' => $top($byRegion), 'bySrc' => $top($bySrc), 'trend' => $trend,
    // 全量桶（供后台卡片点击弹出的详细列表使用，卡片本身只展示 TOP5）
    'full' => [
      'byVersion' => $sortFull($byVer), 'byOs' => $sortFull($byOs), 'byCpu' => $sortFull($byCpu),
      'byMem' => $sortFull($byMem), 'byDisk' => $sortFull($byDisk), 'byGpu' => $sortFull($byGpu), 'byLang' => $sortFull($byLang),
      'byRegion' => $sortFull($byRegion), 'bySrc' => $sortFull($bySrc),
    ],
  ];
}

/** 最近上报设备列表（仅匿名ID与统计字段，倒序，分页；v5.1.3 附带活跃度） */
function st_recent($db, $page, $pageSize) {
  $rows = [];
  foreach ($db['clients'] as $id => $c) {
    $rows[] = array_merge(['anonId' => $id, 'firstSeen' => $c['firstSeen'] ?? 0, 'lastSeen' => $c['lastSeen'] ?? 0,
                           'reports' => $c['reports'] ?? 1, 'activity' => st_activity_of($db, $id),
                           'origin' => $c['origin'] ?? null], $c['fields'] ?? []);
  }
  usort($rows, function ($a, $b) { return (int)$b['lastSeen'] <=> (int)$a['lastSeen']; });
  $total = count($rows);
  $rows = array_slice($rows, ($page - 1) * $pageSize, $pageSize);
  return ['rows' => $rows, 'total' => $total];
}
