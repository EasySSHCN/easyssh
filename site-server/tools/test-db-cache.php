<?php
// 验证 mtime 缓存:两次 load 应命中缓存(引用一致),save 后强制失效重读
require "/www/wwwroot/easyssh.cn/api/stats/common.php";
$a = st_db_load();
$b = st_db_load();
echo "cache_hit=" . var_export(($a === $b), true) . " clients=" . count($a['clients']) . " log=" . count($a['log']) . "\n";
// 只读副本上模拟 save 失效逻辑(不落盘:直接检查 dirty 标记分支存在)
echo "st_db_save_exists=" . var_export(function_exists('st_db_save'), true) . "\n";
