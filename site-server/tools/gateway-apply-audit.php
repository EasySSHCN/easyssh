<?php
/**
 * gateway-apply-audit.php —— 云网关服务申请定时审核任务（R-20261121-002）
 *
 * 每天两次（10:00 / 16:00 北京时间，crontab 注册）由人工或 AI 定时审核受理：
 *   - AI 启发式规则审核（v1，保守口径）：
 *       * 邮箱合法（必过，提交时已校验）
 *       * 用途说明 ≥10 字且不含垃圾/滥用黑名单关键词 → approved
 *       * 用途说明不足 / 命中黑名单 → rejected（附原因）
 *     全部给出结论（approved 或 rejected），保证「受理通过与否都有邮箱通知」的闭环；
 *     拒绝理由写明可改正点，用户可重新提交（rejected 不阻断新申请）。
 *   - reviewedBy = 'ai-cron@v1'；管理员可在数据文件中人工复核改判（人工改判后下次运行不再覆盖）。
 *   - 审核结果（通过与拒绝）均发送邮件通知申请人。
 *
 * 用法：php tools/gateway-apply-audit.php [--dry-run]
 * 依赖：与 site/api/feedback/common.php 同源（邮件通道 keys.php 'mail'，数据目录 FB_DATA_DIR）。
 * 并发：flock 文件锁防重入。
 */

if (PHP_SAPI !== 'cli') { http_response_code(403); exit("CLI only\n"); }

$DRY = in_array('--dry-run', $argv, true);
$SITE = dirname(__DIR__) . '/site'; // easyssh-site/site/api/...
require $SITE . '/api/feedback/common.php';

// 数据目录口径：线上 web PHP（webroot /www/wwwroot/easyssh.cn/api/...）的 FB_DATA_DIR 解析为 /www/wwwroot/data，
// 与源码树 easyssh-site/data 不是同一目录；审核必须与 web API 读写同一份数据。
$GW_DATA_DIR = is_dir('/www/wwwroot/data') ? '/www/wwwroot/data' : FB_DATA_DIR;
$APP_FILE = $GW_DATA_DIR . '/gateway-applications.json';
$LOCK_FILE = $GW_DATA_DIR . '/gateway-apply-audit.lock';

/* ---------- flock 防重入 ---------- */
$lockDir = dirname($LOCK_FILE);
if (!is_dir($lockDir)) @mkdir($lockDir, 0775, true);
$lockFp = fopen($LOCK_FILE, 'c');
if (!$lockFp || !flock($lockFp, LOCK_EX | LOCK_NB)) {
  echo "[audit] another run holds the lock, exit\n";
  exit(0);
}

$blacklist = ['赌博', '博彩', '刷单', '代刷', '外挂', '色情', '撞库', 'ddos', 'cc攻击', '攻击', '爬虫抓取', 'spam'];

/** AI 启发式审核（v1）。返回 [decision, note] */
function gw_audit_review(array $rec): array {
  $purpose = trim((string)($rec['purpose'] ?? ''));
  $mbLen = function_exists('mb_strlen') ? mb_strlen($purpose) : strlen($purpose);
  if ($mbLen < 10) {
    return ['rejected', '用途说明不足（至少 10 字）。请补充使用场景后重新申请（被拒绝的申请不阻断重新提交）。'];
  }
  global $blacklist;
  $low = mb_strtolower($purpose);
  foreach ($blacklist as $kw) {
    if ($kw !== '' && mb_strpos($low, mb_strtolower($kw)) !== false) {
      return ['rejected', '用途说明包含不允许的使用场景，如有疑问请联系管理员申诉。'];
    }
  }
  if ($mbLen < 20) {
    return ['approved', 'AI 审核通过（用途说明较简短，如后续用途变更请联系管理员更新备案）。'];
  }
  return ['approved', 'AI 审核通过。'];
}

function gw_audit_mail($to, array $rec, string $decision, string $note) {
  if (preg_match('#@(test|example|invalid)\.(local|test|example|invalid)$#i', $to)) return true; // 测试域免发（防退信）
  $id = (string)($rec['id'] ?? '-');
  if ($decision === 'approved') {
    $subject = '[EasySSH] 云网关服务申请审核通过';
    $text = "你的云网关服务申请已审核通过（编号 {$id}）。\n\n"
          . "下一步：登录云网关用户服务台 https://easyssh.cn/gw-admin.html （使用申请邮箱注册/登录的账号），"
          . "在「我的资源」中自助签发短期 Token，即可在 EasySSH 客户端「网关（可选）」中接入。\n"
          . "具体操作见操作手册：https://easyssh.cn/manual/gateway.html\n\n"
          . "审核：AI 定时审核（每天 10:00 / 16:00，北京时间）\n"
          . "—— EasySSH 云网关服务（本邮件为系统自动发送）";
  } else {
    $subject = '[EasySSH] 云网关服务申请未通过';
    $text = "很遗憾，你的云网关服务申请未通过本次审核（编号 {$id}）。\n\n"
          . "原因：{$note}\n\n"
          . "被拒绝的申请不阻断重新提交：可修正后重新申请（服务台内提交，或申请页 https://easyssh.cn/apply.html）。\n"
          . "如需人工复核，请联系管理员并提供申请编号。\n\n"
          . "审核：AI 定时审核（每天 10:00 / 16:00，北京时间）\n"
          . "—— EasySSH 云网关服务（本邮件为系统自动发送）";
  }
  return fb_mail_send($to, $subject, $text);
}

$db = fb_json_file_load($APP_FILE);
if (!is_array($db)) $db = [];
$now = fb_now_ms();
$reviewed = 0; $approved = 0; $rejected = 0; $mailFail = 0;

foreach ($db as $id => &$rec) {
  if (!is_array($rec)) continue;
  if (($rec['status'] ?? '') !== 'pending') continue;
  // 人工已改判过的（reviewedBy 以 human 开头）不再覆盖
  if (strpos((string)($rec['reviewedBy'] ?? ''), 'human') === 0) continue;

  list($decision, $note) = gw_audit_review($rec);
  $rec['status'] = $decision;
  $rec['statusText'] = $decision === 'approved' ? '已开通' : '拒绝';
  $rec['reviewedAt'] = $now;
  $rec['reviewedBy'] = 'ai-cron@v1';
  $rec['note'] = $note;
  $reviewed++;
  if ($decision === 'approved') $approved++; else $rejected++;

  $email = (string)($rec['email'] ?? '');
  if ($email !== '') {
    $r = gw_audit_mail($email, $rec, $decision, $note);
    if ($r !== true) { $mailFail++; echo "[audit] mail fail {$email}: {$r}\n"; }
  }
  echo "[audit] {$id} {$email} -> {$decision}" . ($note !== '' ? " ({$note})" : '') . "\n";
}
unset($rec);

if ($DRY) {
  echo "[audit] DRY-RUN: no changes saved. reviewed={$reviewed} approved={$approved} rejected={$rejected}\n";
  flock($lockFp, LOCK_UN); fclose($lockFp);
  exit(0);
}

if (!fb_json_file_save($APP_FILE, $db)) {
  echo "[audit] ERROR: save failed\n";
  flock($lockFp, LOCK_UN); fclose($lockFp);
  exit(1);
}
echo "[audit] done. reviewed={$reviewed} approved={$approved} rejected={$rejected} mailFail={$mailFail}\n";
flock($lockFp, LOCK_UN); fclose($lockFp);
