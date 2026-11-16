<?php
// 验证反命题:同一 IP 掩码下,设备的归属地中文名是否始终一致(即使上报时间不同/档案更新时间不同)
require "/www/wwwroot/easyssh.cn/api/stats/common.php";
$db = st_db_load();
$byIP = [];
foreach ($db['clients'] as $id => $c) {
  $f = is_array($c['fields'] ?? null) ? $c['fields'] : [];
  $r = trim((string)($f['地域'] ?? ''));
  if ($r === '') continue;
  if (preg_match('/((?:\d{1,3}\.){2}x\.x)\s*$/', $r, $m)) {
    $name = trim(substr($r, 0, strlen($r) - strlen($m[1])));
    $byIP[$m[1]][] = $name;
  }
}
$bad = 0;
foreach ($byIP as $ip => $names) {
  $u = array_unique($names);
  if (count($u) > 1) { $bad++; echo "INCONSISTENT ip=$ip names=[" . implode(' / ', $u) . "]\n"; }
}
echo "ip_prefix_groups=" . count($byIP) . " inconsistent=$bad\n";
