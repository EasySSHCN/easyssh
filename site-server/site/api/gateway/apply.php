<?php
/**
 * POST /api/gateway/apply.php —— EasySSH 云网关服务申请页接口（R-20261120-005⑤）
 *
 * JSON 库存申请记录（存储在 webroot 之外 <站点上级>/data/gateway-applications.json，
 * HTTP 无法直接访问），每条记录含状态字段 status：
 *   pending   → 待审核
 *   approved  → 已开通
 *   rejected  → 拒绝
 * 申请经站长审核后人工在网关侧/资源库开通；审核动作由站内后台另行处理，本接口只负责提交与查询。
 *
 * 动作（JSON body）：
 *   { action:"apply", email, contact?, purpose? }  → { ok, message, id? }
 *   { action:"status", email }                     → { ok, status, statusText, appliedAt? }
 *
 * 防刷限流（复用 api/feedback/common.php 的 fb_ip_rate_ok，按来源 IP 滑动窗口）：
 *   apply  → 每小时每 IP 最多 5 次
 *   status → 每小时每 IP 最多 30 次
 * 同一 email 已存在 待审核/已开通 的申请时拒绝重复提交。
 */

require __DIR__ . '/../feedback/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);
$in = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($in)) fb_fail('请求体必须是 JSON');
$action = (string)($in['action'] ?? '');

define('GW_APP_FILE', FB_DATA_DIR . '/gateway-applications.json');
define('GW_APPLY_RATE_FILE', FB_DATA_DIR . '/gateway-apply-ratelimit.json');
define('GW_STATUS_RATE_FILE', FB_DATA_DIR . '/gateway-status-ratelimit.json');

const GW_STATUS_TEXT = ['pending' => '待审核', 'approved' => '已开通', 'rejected' => '拒绝'];

function gw_apps_load() {
  $j = fb_json_file_load(GW_APP_FILE);
  return is_array($j) ? $j : [];
}
function gw_apps_save($db) {
  if (count($db) > 5000) $db = array_slice($db, -5000, null, true);
  return fb_json_file_save(GW_APP_FILE, $db);
}
function gw_valid_email($e) {
  return is_string($e) && strlen($e) <= 254 && (bool)filter_var($e, FILTER_VALIDATE_EMAIL);
}

switch ($action) {

  case 'apply': {
    if (!fb_ip_rate_ok(GW_APPLY_RATE_FILE, 5, 3600 * 1000)) {
      fb_fail('提交过于频繁，请 1 小时后再试', 429);
    }
    $email = strtolower(trim((string)($in['email'] ?? '')));
    if (!gw_valid_email($email)) fb_fail('邮箱格式非法');
    $contact = trim((string)($in['contact'] ?? ''));
    if (mb_strlen($contact) > 120) fb_fail('联系方式过长（≤120 字符）');
    $purpose = trim((string)($in['purpose'] ?? ''));
    if (mb_strlen($purpose) > 500) fb_fail('用途说明过长（≤500 字符）');

    $db = gw_apps_load();
    $now = fb_now_ms();
    // 同 email 存在未终结（待审核/已开通）申请时拒绝重复提交
    foreach ($db as $rec) {
      if (!is_array($rec)) continue;
      if ((string)($rec['email'] ?? '') === $email && in_array($rec['status'] ?? '', ['pending', 'approved'], true)) {
        $txt = GW_STATUS_TEXT[$rec['status']] ?? $rec['status'];
        fb_fail('该邮箱已有一条「' . $txt . '」的申请，请勿重复提交');
      }
    }
    $id = 'gwapp-' . substr(bin2hex(random_bytes(12)), 0, 16);
    $db[$id] = [
      'id' => $id,
      'email' => $email,
      'contact' => $contact,
      'purpose' => $purpose,
      'status' => 'pending',
      'statusText' => GW_STATUS_TEXT['pending'],
      'ip' => fb_client_ip(),
      'appliedAt' => $now,
      'reviewedAt' => 0,
      'note' => '',
    ];
    if (!gw_apps_save($db)) fb_fail('服务器存储写入失败', 500);
    fb_json_out(['ok' => true, 'message' => '申请已提交，状态：待审核。审核结果将通过邮箱通知，也可随时用本页查询状态。', 'id' => $id]);
  }

  case 'status': {
    if (!fb_ip_rate_ok(GW_STATUS_RATE_FILE, 30, 3600 * 1000)) {
      fb_fail('查询过于频繁，请稍后再试', 429);
    }
    $email = strtolower(trim((string)($in['email'] ?? '')));
    if (!gw_valid_email($email)) fb_fail('邮箱格式非法');
    $db = gw_apps_load();
    // 取该邮箱最近一条申请
    $latest = null; $latestAt = 0;
    foreach ($db as $rec) {
      if (!is_array($rec) || (string)($rec['email'] ?? '') !== $email) continue;
      $at = (int)($rec['appliedAt'] ?? 0);
      if ($at >= $latestAt) { $latest = $rec; $latestAt = $at; }
    }
    if (!$latest) fb_json_out(['ok' => true, 'status' => '', 'statusText' => '无申请记录']);
    fb_json_out([
      'ok' => true,
      'status' => (string)($latest['status'] ?? ''),
      'statusText' => GW_STATUS_TEXT[$latest['status'] ?? ''] ?? (string)($latest['status'] ?? ''),
      'appliedAt' => $latestAt,
      'note' => (string)($latest['note'] ?? ''),
    ]);
  }

  default:
    fb_fail('action 非法，仅支持 apply / status');
}
