#!/usr/bin/env bash
php -r 'require "/www/wwwroot/easyssh.cn/api/stats/common.php";
echo "v6 via primary(2400:3200::1): "; var_dump(st_geo_query_primary("2400:3200::1"));
echo "full v6 resolve(2408:8207::1): "; var_dump(st_region_name("2408:8207::1"));
'
