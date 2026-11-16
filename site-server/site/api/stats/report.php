<?php
/**
 * POST /api/stats/report.php —— 客户端匿名使用统计上报（v5.1.0）
 *
 * 请求体：反馈同族加密信封（AES-256-GCM + HMAC-SHA256 时效签名）
 * 明文 = { kind:"stats", anonId:<16-64位hex匿名ID>, fields:{...白名单字段...} }
 *
 * 处理：解密 → 白名单过滤 → 同设备去抖(≥12h) → upsert + 当日活跃计数
 *       v5.1.3:每次入库上报同时记 设备按日活跃计数 + 全局提交流水
 *       (同设备 24h 后重复提交:clients 记录原地更新,活跃度/流水/趋势新增可见记录)
 * 用途：仅统计；无身份信息；不与第三方共享。
 */
require __DIR__ . '/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') st_fail('仅支持 POST', 405);

$in = json_decode(file_get_contents('php://input'), true);
if (!is_array($in)) st_fail('请求体必须是 JSON');

st_rate_check(fb_client_ip());

$data = st_decrypt_envelope($in);
$anonId = $data['anonId'] ?? '';
if (!st_valid_anon_id($anonId)) st_fail('匿名ID非法');
$fields = st_sanitize($data['fields'] ?? []);
if (empty($fields)) st_fail('无有效统计字段');

// 归属地：以上报来源 IP（服务端可见的完整 IP）解析中文地域名，与 IP 前两段合并为 "中文地域 xx.xx"
$clientIp = fb_client_ip();
st_apply_region($fields, $clientIp);

$now = st_now_ms();
$db = st_db_load();

$c = $db['clients'][$anonId] ?? null;
if (is_array($c)) {
  if ($now - (int)($c['lastSeen'] ?? 0) < ST_MIN_REPORT_MS) {
    // v5.1.6:去抖窗口内静默接受;但若载荷带合法 tok(客户端二期携带)仍做重归因校验,不改其他数据
    $tokIn = isset($data['tok']) ? trim((string)$data['tok']) : '';
    if ($tokIn !== '') {
      $o = st_origin_apply($c, $anonId, $clientIp, $now, $tokIn);
      if ($o !== null) { $c['origin'] = $o; $db['clients'][$anonId] = $c; st_db_save($db); }
    }
    st_json_out(['ok' => true, 'dedup' => true]);
  }
  $c['fields'] = array_merge($c['fields'] ?? [], $fields); // 新字段覆盖旧值
  $c['lastSeen'] = $now;
  $c['reports'] = (int)($c['reports'] ?? 1) + 1;
} else {
  $c = ['fields' => $fields, 'firstSeen' => $now, 'lastSeen' => $now, 'reports' => 1];
}
$db['clients'][$anonId] = $c;

// v5.1.6 归因关联：新设备首报(票据匹配→方案B兜底)或携带 tok 的上报(精确归因/重归因)。
// origin 仅服务端写入设备档案,不在上报字段白名单内;老设备无 tok 且已有档案则零额外开销。
$tokIn = isset($data['tok']) ? trim((string)$data['tok']) : '';
$isFirst = (int)($c['reports'] ?? 1) === 1;
if ($tokIn !== '' || $isFirst) {
  $o = st_origin_apply($c, $anonId, $clientIp, $now, $tokIn);
  if ($o !== null) $c['origin'] = $o;
  $db['clients'][$anonId] = $c;
}

// v5.1.3:设备按日活跃计数 + 提交流水(后台"最近提交动态"与活跃度数据源)
st_touch_activity($db, $anonId, (string)($fields['软件版本'] ?? ''), $now);

// v5.1.7:移除原 daily 按上报次数累加的计数(看板趋势已改为按 activity 反推每日去重活跃设备数,
// 见 common.php st_summary;该计数器存在同设备一日双计及迁移重放导致单日虚增的问题)

st_db_save($db);
st_json_out(['ok' => true]);
