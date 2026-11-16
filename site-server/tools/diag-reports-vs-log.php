<?php
// 诊断:找出 reports 与其在全局流水中条数不一致的设备,并核对 ST_LOG_MAX
require "/www/wwwroot/easyssh.cn/api/stats/common.php";
echo "ST_LOG_MAX=" . ST_LOG_MAX . "\n";
$db = st_db_load();
echo "log_total=" . count($db['log']) . "\n";
foreach ($db['clients'] as $id => $c) {
  $n = 0;
  foreach ($db['log'] as $e) { if (($e['id'] ?? '') === $id) $n++; }
  $r = (int)($c['reports'] ?? 0);
  if ($n !== $r) {
    $times = [];
    foreach ($db['log'] as $e) { if (($e['id'] ?? '') === $id) $times[] = date('Y-m-d H:i:s', (int)round($e['t'] / 1000)); }
    echo "MISMATCH id=$id reports=$r log_entries=$n firstSeen=" . date('Y-m-d H:i:s', (int)round(($c['firstSeen'] ?? 0) / 1000)) .
         " lastSeen=" . date('Y-m-d H:i:s', (int)round(($c['lastSeen'] ?? 0) / 1000)) .
         " log_times=[" . implode(' | ', $times) . "]\n";
  }
}
