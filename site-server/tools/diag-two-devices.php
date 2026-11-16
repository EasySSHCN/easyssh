<?php
require "/www/wwwroot/easyssh.cn/api/stats/common.php";
$db = st_db_load();
foreach (["9efc0b33383a4b1d8e3f440a4364167b", "e35781fbf9e11310e067541d7b4406c3"] as $id) {
  $c = $db['clients'][$id] ?? null;
  if (!$c) { echo "$id : 不存在\n"; continue; }
  $n = 0; $times = [];
  foreach ($db['log'] as $e) { if (($e['id'] ?? '') === $id) { $n++; $times[] = date('Y-m-d H:i:s', (int)round($e['t']/1000)); } }
  echo substr($id,0,12) . " : reports=" . ($c['reports'] ?? 0) . " firstSeen=" . date('Y-m-d H:i:s', (int)round(($c['firstSeen'] ?? 0)/1000)) . " log_entries=$n [" . implode(' | ', $times) . "]\n";
}
