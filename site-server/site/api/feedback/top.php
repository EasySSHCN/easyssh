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
// 按回复时间倒序，取最新 10 条
usort($items, function ($a, $b) { return (int)($b['replyAt'] ?? 0) <=> (int)($a['replyAt'] ?? 0); });
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
