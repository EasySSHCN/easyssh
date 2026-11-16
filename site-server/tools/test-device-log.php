<?php
// 模拟 device_log 动作的过滤与分页逻辑(不经过鉴权),用真实 stats 库验证
require "/www/wwwroot/easyssh.cn/api/stats/common.php";
$db = st_db_load();
$id = "91816616cd85a4eb9166e9138f7fbfda";
$rows = array_reverse(array_values(array_filter($db['log'] ?? [], function ($e) use ($id) {
  return is_array($e) && (($e['id'] ?? $e['anonId'] ?? '') === $id);
})));
$total = count($rows);
$out = array_slice($rows, 0, 5);
echo json_encode(['total' => $total, 'sample' => $out], JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES);
