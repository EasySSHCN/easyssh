<?php
/**
 * GET /api/feedback/top.php —— 已回复且公开的反馈 TOP10（公开接口，明文 JSON）
 * 响应：{ ok:true, items:[{ id, type, date, content, reply, replyAt }] }
 */
require __DIR__ . '/common.php';

$db = fb_db_load();
$items = array_values(array_filter($db['items'], function ($it) {
  return ($it['public'] ?? false) && trim((string)($it['reply'] ?? '')) !== '';
}));
// 按回复时间(分钟粒度,同一批自动回复秒级有差异)倒序；再按反馈提交时间倒序，取最新 10 条
usort($items, function ($a, $b) {
  $ra = intdiv((int)($a['replyAt'] ?? 0), 60);
  $rb = intdiv((int)($b['replyAt'] ?? 0), 60);
  if ($ra !== $rb) return $rb <=> $ra;
  // 次级排序:按提交时间字符串(date 形如 2026-11-19 19:34,可直接字符串比较),再退化到提交时间戳
  $da = (string)($a['date'] ?? '');
  $db2 = (string)($b['date'] ?? '');
  if ($da !== $db2) return strcmp($db2, $da);
  return (int)($b['ts'] ?? 0) <=> (int)($a['ts'] ?? 0);
});
$items = array_slice($items, 0, 10);

fb_json_out(['ok' => true, 'items' => array_map(function ($it) {
  return [
    'id' => $it['id'],
    'type' => $it['type'] ?? '反馈',
    'date' => $it['date'] ?? '',
    'content' => mb_substr(trim((string)$it['content']), 0, 300),
    'reply' => (string)$it['reply'],
    'replyAt' => fb_date_str($it['replyAt'] ?: (int)($it['ts'] ?? 0)),
  ];
}, $items)]);
