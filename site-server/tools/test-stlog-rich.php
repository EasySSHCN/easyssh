<?php
require "/www/wwwroot/easyssh.cn/api/stats/common.php";
$db = st_db_load();
$r = st_log_recent($db, 1, 2);
echo json_encode($r, JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES);
