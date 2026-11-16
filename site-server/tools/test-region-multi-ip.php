<?php
// 验证:同归属地、不同 IP 掩码的设备都能被 devices_by_region 逻辑命中
require "/www/wwwroot/easyssh.cn/api/stats/common.php";
$db = st_db_load();
// 1) 统计所有地域字段,找出中文归属地相同但 IP 掩码不同的例子
$groups = [];
foreach ($db['clients'] as $id => $c) {
  $f = is_array($c['fields'] ?? null) ? $c['fields'] : [];
  $r = trim((string)($f['地域'] ?? ''));
  if ($r === '') continue;
  $name = preg_replace('/\s+(?:\d{1,3}\.){2}x\.x$/', '', $r);
  $groups[$name][] = $r;
}
foreach ($groups as $name => $list) {
  $uniqIPs = array_unique($list);
  if (count($list) > 1 || count($uniqIPs) > 1) {
    echo "归属地[$name] 设备数=" . count($list) . " 不同IP掩码数=" . count($uniqIPs) . " : " . implode(' / ', $uniqIPs) . "\n";
  }
}
// 2) 模拟 devices_by_region 检索其中第一个归属地
$testName = key(array_filter($groups, function ($l) { return count(array_unique($l)) > 1; }) ?: [$groups ? array_key_first($groups) : '', []]);
if ($testName) {
  $hit = 0; $ips = [];
  foreach ($db['clients'] as $id => $c) {
    $f = is_array($c['fields'] ?? null) ? $c['fields'] : [];
    $r = trim((string)($f['地域'] ?? ''));
    if ($r === '' || ($r !== $testName && strpos($r, $testName . ' ') !== 0)) continue;
    $hit++; $ips[] = $r;
  }
  echo "检索[$testName] 命中=$hit 台, IP掩码列表: " . implode(' / ', $ips) . "\n";
}
