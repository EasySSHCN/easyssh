<?php
/**
 * POST /api/stats/admin.php —— 统计后台管理接口（复用反馈管理员令牌鉴权）
 * 动作（JSON body）：
 *   { action:"summary", user, token }                    → { ok, summary:{...看板聚合...} }
 *   { action:"list",    user, token, page?, pageSize? }  → { ok, rows:[...], total }
 *   { action:"log",     user, token, page?, pageSize?, anonId? } → { ok, rows:[{t,anonId,v,...}...], total } 最近提交动态(v5.1.3;v5.1.5 联查设备档案附展示字段;daysfix2 anonId 可选=按设备过滤)
 *   { action:"active_days_top", user, token, top? }      → { ok, rows:[{anonId,days,...}], full:{anonId:days} } 累计活跃天数TOP5(v5.1.7)
 *   { action:"bucket_list", user, token, bucket, value, page?, pageSize? }
 *            → { ok, rows:[...], total }  分布维度明细:列出某卡片某取值下的设备记录(按最近上报倒序)
 *   { action:"delete",  user, token, anonId | anonIds:[...] } → { ok } 删除单台/多台设备记录
 *   { action:"update",  user, token, anonId, fields:{...} }   → { ok } 编辑设备统计字段(白名单内)
 *   { action:"purge",   user, token }                    → { ok } 清空统计库（保留结构）
 *   { action:"device_log", user, token, anonId, page?, pageSize? } → { ok, rows:[{t,v}...], total } 单台设备上报明细流水(v5.1.5)
 * 鉴权：复用 api/feedback 的 admin_user + 当日 HMAC token（统一后台一次登录两模块通用）。
 */
require __DIR__ . '/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') st_fail('仅支持 POST', 405);
$in = json_decode(file_get_contents('php://input'), true);
if (!is_array($in)) st_fail('请求体必须是 JSON');
$action = (string)($in['action'] ?? '');
$user = (string)($in['user'] ?? '');
$token = (string)($in['token'] ?? '');

if (!fb_check_token($user, $token)) st_fail('未登录或登录已过期，请重新登录', 401);

$db = st_db_load();

switch ($action) {
  case 'summary':
    st_json_out(['ok' => true, 'summary' => st_summary($db)]);

  case 'list': {
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(200, max(1, (int)($in['pageSize'] ?? 20)));
    $r = st_recent($db, $page, $pageSize);
    st_json_out(['ok' => true, 'rows' => $r['rows'], 'total' => $r['total']]);
  }

  case 'log': {
    // v5.1.3:最近提交动态——每次入库上报一条(含同设备 24h 后的重复提交),倒序分页
    // daysfix2:可选 anonId=只返回该设备的上报动态记录(数据层 common.php st_log_recent 过滤),行结构与全量动态一致
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(200, max(1, (int)($in['pageSize'] ?? 20)));
    $anonId = isset($in['anonId']) ? trim((string)$in['anonId']) : null;
    $r = st_log_recent($db, $page, $pageSize, $anonId);
    st_json_out(['ok' => true, 'rows' => $r['rows'], 'total' => $r['total']]);
  }

  case 'active_list': {
    // v5.1.4:顶部统计卡片明细——window=all 累计设备/week 近7天/day 近24小时,设备记录倒序分页
    $window = (string)($in['window'] ?? 'all');
    if (!in_array($window, ['all', 'week', 'day'], true)) st_fail('窗口非法');
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(200, max(1, (int)($in['pageSize'] ?? 10)));
    $r = st_active_list($db, $window, $page, $pageSize);
    st_json_out(['ok' => true, 'rows' => $r['rows'], 'total' => $r['total']]);
  }

  case 'active_days_top': {
    // v5.1.7:累计活跃天数 TOP5 —— 每台设备上报历史中的不重复日期数(活动库+归档月文件合并去重),取前 N 台;full 为全量降序(供弹窗)
    $top = min(20, max(1, (int)($in['top'] ?? 5)));
    $fullRows = st_active_days_top($db, 0);
    $full = [];
    foreach ($fullRows as $r) $full[$r['anonId']] = $r['days'];
    st_json_out(['ok' => true, 'rows' => array_slice($fullRows, 0, $top), 'full' => $full]);
  }

  case 'bucket_list': {
    // 分布维度 → 统计字段映射(与 common.php st_summary 聚合口径一致);bySrc 按设备档案 origin.src 匹配(v5.1.6)
    $fieldMap = [
      'byVersion' => '软件版本', 'byOs' => '操作系统', 'byCpu' => 'CPU',
      'byMem' => '内存', 'byDisk' => '磁盘容量', 'byGpu' => '显卡', 'byLang' => '系统语言代码', 'byRegion' => '地域',
    ];
    $bucket = (string)($in['bucket'] ?? '');
    if ($bucket !== 'bySrc' && !isset($fieldMap[$bucket])) st_fail('维度非法');
    $value = trim((string)($in['value'] ?? ''));
    if ($value === '') st_fail('取值非法');
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(200, max(1, (int)($in['pageSize'] ?? 10)));
    $rows = [];
    foreach ($db['clients'] as $id => $c) {
      $f = $c['fields'] ?? [];
      if ($bucket === 'bySrc') {
        $v = (string)($c['origin']['src'] ?? 'unknown');
      } else {
        $field = $fieldMap[$bucket];
        $v = trim((string)($f[$field] ?? ''));
        if ($bucket === 'byOs') $v = preg_replace('/\(Build [^)]*\)/', '', $v); // 与聚合一致去掉 Build 号
        if ($bucket === 'byRegion' && $v === '') $v = '未知';
      }
      if ($v !== $value) continue;
      $rows[] = array_merge(['anonId' => $id, 'firstSeen' => $c['firstSeen'] ?? 0,
                             'lastSeen' => $c['lastSeen'] ?? 0, 'reports' => $c['reports'] ?? 1,
                             'activity' => st_activity_of($db, $id), 'origin' => $c['origin'] ?? null], $f);
    }
    usort($rows, function ($a, $b) { return (int)$b['lastSeen'] <=> (int)$a['lastSeen']; });
    $total = count($rows);
    $rows = array_slice($rows, ($page - 1) * $pageSize, $pageSize);
    st_json_out(['ok' => true, 'rows' => $rows, 'total' => $total]);
  }

  case 'device': {
    // 单台设备详细记录(最近提交动态中点击设备 ID 打开)
    $id = (string)($in['anonId'] ?? '');
    if ($id === '') st_fail('缺少 anonId');
    if (!isset($db['clients'][$id])) st_fail('记录不存在', 404);
    $c = $db['clients'][$id];
    $f = is_array($c['fields'] ?? null) ? $c['fields'] : [];
    st_json_out(['ok' => true, 'row' => array_merge(['anonId' => $id, 'firstSeen' => $c['firstSeen'] ?? 0,
      'lastSeen' => $c['lastSeen'] ?? 0, 'reports' => $c['reports'] ?? 1,
      'activity' => st_activity_of($db, $id), 'origin' => $c['origin'] ?? null], $f)]);
  }

  case 'device_log': {
    // 单台设备的上报明细流水(来自全局提交流水,倒序分页;流水仅保留最新 ST_LOG_MAX 条)
    $id = (string)($in['anonId'] ?? '');
    if ($id === '') st_fail('缺少 anonId');
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(200, max(1, (int)($in['pageSize'] ?? 20)));
    $rows = array_reverse(array_values(array_filter($db['log'] ?? [], function ($e) use ($id) {
      return is_array($e) && (($e['id'] ?? $e['anonId'] ?? '') === $id);
    })));
    $total = count($rows);
    $rows = array_slice($rows, ($page - 1) * $pageSize, $pageSize);
    st_json_out(['ok' => true, 'rows' => $rows, 'total' => $total]);
  }

  case 'devices_by_ip': {
    // 按掩码 IP 前缀(如 "183.164.x.x")检索设备详细记录列表:匹配 fields['地域'] 中包含该 IP 前缀的所有设备
    $ip = trim((string)($in['ip'] ?? ''));
    if ($ip === '' || !preg_match('/^\d{1,3}\.\d{1,3}\.x\.x$/', $ip)) st_fail('IP 前缀非法');
    $rows = [];
    foreach ($db['clients'] as $id => $c) {
      $f = is_array($c['fields'] ?? null) ? $c['fields'] : [];
      $region = (string)($f['地域'] ?? '');
      if (strpos($region, $ip) === false) continue;
      $rows[] = array_merge(['anonId' => $id, 'firstSeen' => $c['firstSeen'] ?? 0,
                             'lastSeen' => $c['lastSeen'] ?? 0, 'reports' => $c['reports'] ?? 1,
                             'activity' => st_activity_of($db, $id), 'origin' => $c['origin'] ?? null], $f);
    }
    usort($rows, function ($a, $b) { return (int)$b['lastSeen'] <=> (int)$a['lastSeen']; });
    st_json_out(['ok' => true, 'rows' => $rows, 'total' => count($rows)]);
  }

  case 'devices_by_region': {
    // 按归属地中文名检索设备详细记录列表:匹配 fields['地域'] 以该名称开头的所有设备(地域字段为 "中文名 + IP掩码")
    $region = trim((string)($in['region'] ?? ''));
    if ($region === '' || mb_strlen($region) > 100) st_fail('归属地非法');
    $rows = [];
    foreach ($db['clients'] as $id => $c) {
      $f = is_array($c['fields'] ?? null) ? $c['fields'] : [];
      $r = trim((string)($f['地域'] ?? ''));
      if ($r === '' || ($r !== $region && strpos($r, $region . ' ') !== 0)) continue;
      $rows[] = array_merge(['anonId' => $id, 'firstSeen' => $c['firstSeen'] ?? 0,
                             'lastSeen' => $c['lastSeen'] ?? 0, 'reports' => $c['reports'] ?? 1,
                             'activity' => st_activity_of($db, $id), 'origin' => $c['origin'] ?? null], $f);
    }
    usort($rows, function ($a, $b) { return (int)$b['lastSeen'] <=> (int)$a['lastSeen']; });
    st_json_out(['ok' => true, 'rows' => $rows, 'total' => count($rows)]);
  }

  case 'devices_by_time': {
    // 按时间检索动态记录列表:同一秒内可能有多台设备的上报,逐条返回 {t,v,device(设备详细记录,可能为null)}
    $ts = (int)($in['ts'] ?? 0);
    if ($ts <= 0) st_fail('时间戳非法');
    $rows = [];
    foreach (array_reverse($db['log'] ?? []) as $e) {
      if (!is_array($e) || abs((int)($e['t'] ?? 0) - $ts) >= 1000) continue; // 同一秒内视为相同时间
      $id = (string)($e['id'] ?? $e['anonId'] ?? '');
      $device = null;
      if ($id !== '' && isset($db['clients'][$id]) && is_array($db['clients'][$id])) {
        $c = $db['clients'][$id];
        $f = is_array($c['fields'] ?? null) ? $c['fields'] : [];
        $device = array_merge(['anonId' => $id, 'firstSeen' => $c['firstSeen'] ?? 0,
                               'lastSeen' => $c['lastSeen'] ?? 0, 'reports' => $c['reports'] ?? 1,
                               'activity' => st_activity_of($db, $id), 'origin' => $c['origin'] ?? null], $f);
      }
      $rows[] = ['t' => (int)($e['t'] ?? 0), 'v' => (string)($e['v'] ?? ''), 'anonId' => $id, 'device' => $device];
    }
    st_json_out(['ok' => true, 'rows' => $rows, 'total' => count($rows)]);
  }

  case 'visit_link': {
    // v5.1.6:按访客流水ID查询该次访问详情(动态来源徽章/设备来源归因区块联查)
    $vid = trim((string)($in['visitId'] ?? ''));
    $fallbackIp = trim((string)($in['ip'] ?? ''));        // 存量记录无 id:按 t+ip 兜底匹配
    $fallbackTs = (int)($in['ts'] ?? 0);
    if ($vid === '' && ($fallbackIp === '' || $fallbackTs <= 0)) st_fail('缺少 visitId');
    require_once __DIR__ . '/../visit/common.php';
    $vdb = vst_db_load();
    $row = null;
    foreach ($vdb['recent'] as $r) {
      if (!is_array($r)) continue;
      if ($vid !== '' && (string)($r['id'] ?? '') === $vid) { $row = $r; break; }
      if ($vid === '' && (string)($r['f'] ?? '') === $fallbackIp
        && abs((int)($r['t'] ?? 0) - $fallbackTs) <= 10 * 60 * 1000) { $row = $r; break; }
    }
    if ($row === null) st_fail('访客记录不存在（流水仅保留最新 1000 条）', 404);
    $row['region'] = vst_region((string)($row['f'] ?? $row['ip'] ?? ''));
    st_json_out(['ok' => true, 'row' => $row]);
  }

  case 'tickets': {
    // v5.1.6:下载票据表只读查看(最新在前,可选只看某 IP),用于核对归因链
    $j = st_tickets_load();
    $rows = [];
    foreach ($j['tickets'] as $tok => $t) {
      if (!is_array($t)) continue;
      $rows[] = ['tok' => substr((string)$tok, 0, 12) . '…', 'visitId' => (string)($t['visitId'] ?? ''),
                 'src' => (string)($t['src'] ?? ''), 'url' => (string)($t['url'] ?? ''),
                 'ip' => (string)($t['ip'] ?? ''), 'ts' => (int)($t['ts'] ?? 0),
                 'expire' => (int)($t['expire'] ?? 0), 'attributed' => is_array($t['attributed'] ?? null) ? $t['attributed'] : []];
    }
    usort($rows, function ($a, $b) { return $b['ts'] <=> $a['ts']; });
    $fip = trim((string)($in['ip'] ?? ''));
    if ($fip !== '') $rows = array_values(array_filter($rows, function ($r) use ($fip) { return $r['ip'] === $fip; }));
    st_json_out(['ok' => true, 'rows' => $rows, 'total' => count($rows)]);
  }

  case 'delete': {
    // 支持单删(anonId)与批量删(anonIds 数组),均逐一校验存在性
    $ids = isset($in['anonIds']) && is_array($in['anonIds'])
      ? array_values(array_filter(array_map('strval', $in['anonIds']), 'strlen'))
      : [(string)($in['anonId'] ?? '')];
    if (!count($ids)) st_fail('缺少 anonId');
    foreach ($ids as $id) {
      if (!isset($db['clients'][$id])) st_fail('记录不存在: ' . $id, 404);
    }
    foreach ($ids as $id) unset($db['clients'][$id]);
    st_db_save($db);
    st_json_out(['ok' => true, 'deleted' => count($ids)]);
  }

  case 'update': {
    // 编辑设备统计字段:仅允许白名单字段,值截断与上报入库同规则
    $id = (string)($in['anonId'] ?? '');
    if (!isset($db['clients'][$id])) st_fail('记录不存在', 404);
    $fields = (isset($in['fields']) && is_array($in['fields'])) ? $in['fields'] : [];
    if (!count($fields)) st_fail('没有需要修改的字段');
    $allowed = st_allowed_fields();
    $cur = is_array($db['clients'][$id]['fields'] ?? null) ? $db['clients'][$id]['fields'] : [];
    foreach ($fields as $k => $v) {
      if (!in_array((string)$k, $allowed, true)) continue; // 白名单外字段忽略
      $val = trim((string)$v);
      if ($val === '') { unset($cur[(string)$k]); continue; } // 留空 = 清除该字段
      if (mb_strlen($val) > ST_MAX_VALUE) continue;
      $cur[(string)$k] = mb_substr($val, 0, ST_MAX_VALUE);
    }
    $db['clients'][$id]['fields'] = $cur;
    st_db_save($db);
    st_json_out(['ok' => true]);
  }

  case 'purge':
    $db = st_db_default();
    st_db_save($db);
    st_json_out(['ok' => true]);

  default:
    st_fail('未知动作');
}
