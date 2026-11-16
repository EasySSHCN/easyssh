<?php
/**
 * tools/backfill-origin-heuristic.php —— 存量设备来源回填（方案 B 启发式,幂等）
 *
 * 用法（在发布机上执行）：
 *   php backfill-origin-heuristic.php            # dry-run：只打印将要写入的归因,不改库
 *   php backfill-origin-heuristic.php --write    # 实写：把归因写入 stats 库
 *
 * 原理：
 *   存量设备档案只保留「地域 = 中文名 + a.b.x.x」IP 前缀掩码（历史未存完整 IP），
 *   故只能做前缀级启发式：在访客流水(visit-db recent 1000 条)中找
 *   「掩码前缀相同 + 时间在 firstSeen 前 7 天内」最近一次访问，以其 rf 完整来源分类。
 *   所有回填一律 via=heuristic（后台展示「疑似来源」）；NAT/公司网络可能误归因。
 * 幂等：已有 origin 的设备跳过；重复执行安全。
 */

require "/www/wwwroot/easyssh.cn/api/stats/common.php";

$write = in_array('--write', $argv, true);
$now = st_now_ms();

$db = st_db_load();
$vdb = vst_db_load();

// 预建访客流水的「掩码前缀 → 访问记录[]」索引（时间升序便于取 firstSeen 前最近一条）
$byMask = [];
foreach ($vdb['recent'] as $r) {
  if (!is_array($r)) continue;
  $m = (string)($r['ip'] ?? '');
  if ($m === '') $m = vst_ip_mask((string)($r['f'] ?? ''));
  if ($m === '' || $m === '内网/未知') continue;
  $byMask[$m][] = $r;
}
foreach ($byMask as $m => $rows) usort($rows, function ($a, $b) { return (int)($a['t'] ?? 0) <=> (int)($b['t'] ?? 0); });
// usort 对副本排序,需重新赋值
foreach (array_keys($byMask) as $m) {
  usort($byMask[$m], function ($a, $b) { return (int)($a['t'] ?? 0) <=> (int)($b['t'] ?? 0); });
}

$stats = ['total' => 0, 'skip' => 0, 'hit' => 0, 'miss' => 0, 'visitTrimmed' => 0];
$plan = [];

foreach ($db['clients'] as $id => $c) {
  $stats['total']++;
  if (isset($c['origin']) && is_array($c['origin'])) { $stats['skip']++; continue; }
  $firstSeen = (int)($c['firstSeen'] ?? 0);
  $region = (string)(is_array($c['fields'] ?? null) ? ($c['fields']['地域'] ?? '') : '');
  // 地域字段形如「中文地名 a.b.x.x」或「xxxx:xxxx::/32」——提取尾部的掩码前缀
  $mask = '';
  if (preg_match('/((?:\d{1,3}\.){2}x\.x)\s*$/', $region, $m1)) $mask = $m1[1];
  elseif (preg_match('/([0-9a-f]{1,4}:[0-9a-f]{1,4})::\/32\s*$/i', $region, $m2)) $mask = $m2[1] . '::/32';
  if ($mask === '') { $stats['miss']++; $plan[] = [$id, null, '设备无地域字段/掩码,无法启发式归因']; continue; }
  $rows = isset($byMask[$mask]) ? $byMask[$mask] : [];
  $best = null;
  foreach ($rows as $r) {
    $t = (int)($r['t'] ?? 0);
    if ($t > $firstSeen || $firstSeen - $t > ST_HEURISTIC_WINDOW_MS) continue;
    if ($best === null || $t > (int)($best['t'] ?? 0)) $best = $r;
  }
  if ($best === null) {
    // 流水仅保留 1000 条,更早的访问已归档——这类设备只能留 unknown
    $stats['miss']++;
    $plan[] = [$id, null, "掩码 {$mask} 在 firstSeen 前 7 天内无访客流水（或流水已归档）"];
    continue;
  }
  $rf = (string)($best['rf'] ?? '');
  $origin = [
    'src' => vst_src_classify($rf),
    'url' => $rf,
    'visitId' => (string)($best['id'] ?? ''),
    'visitTs' => (int)($best['t'] ?? 0),
    'via' => 'heuristic',
    'attributedAt' => $now,
  ];
  $stats['hit']++;
  $plan[] = [$id, $origin, "掩码 {$mask} · 访问 " . date('Y-m-d H:i', (int)($origin['visitTs'] / 1000)) . " · {$origin['src']} · " . ($rf ?: '(直接访问)')];
}

/* ---------- 输出 ---------- */
echo ($write ? "【实写模式】" : "【dry-run 预览,加 --write 实写】") . "设备总数 {$stats['total']} · 已有 origin 跳过 {$stats['skip']} · 可归因 {$stats['hit']} · 无法归因 {$stats['miss']}\n\n";
foreach ($plan as [$id, $origin, $desc]) {
  echo substr($id, 0, 12) . "…  " . ($origin ? ("→ src={$origin['src']} via=heuristic") : "→ 不写入") . "\n    {$desc}\n";
}

if (!$write) { echo "\n(dry-run 结束,未修改任何数据)\n"; exit(0); }

$applied = 0;
foreach ($plan as [$id, $origin, $desc]) {
  if ($origin === null) continue;
  if (isset($db['clients'][$id]['origin']) && is_array($db['clients'][$id]['origin'])) continue; // 幂等护栏
  $db['clients'][$id]['origin'] = $origin;
  $applied++;
}
st_db_save($db);
echo "\n已写入 {$applied} 台设备的 origin(via=heuristic)。\n";
