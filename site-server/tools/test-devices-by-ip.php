<?php
// 模拟 devices_by_ip 检索:用真实统计库验证按掩码 IP 前缀匹配
require "/www/wwwroot/easyssh.cn/api/stats/common.php";
$db = st_db_load();
$ip = "183.164.x.x";
$rows = [];
foreach ($db['clients'] as $id => $c) {
  $f = is_array($c['fields'] ?? null) ? $c['fields'] : [];
  if (strpos((string)($f['地域'] ?? ''), $ip) === false) continue;
  $rows[] = ['id' => $id, 'region' => $f['地域'], 'reports' => $c['reports'] ?? 1];
}
echo json_encode(['total' => count($rows), 'rows' => $rows], JSON_UNESCAPED_UNICODE);
