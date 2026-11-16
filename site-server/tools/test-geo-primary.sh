#!/usr/bin/env bash
php -l /opt/dsh/workspace/easyssh-site/site/api/stats/common.php >/dev/null && bash /opt/dsh/workspace/easyssh-site/tools/deploy.sh 2>&1 | tail -1
php -r 'require "/www/wwwroot/easyssh.cn/api/stats/common.php";
echo "primary(v4): "; var_dump(st_geo_query_primary("58.240.1.1"));
echo "primary(v6): "; var_dump(st_geo_query_primary("2408:8207::1"));
echo "primary(foreign): "; var_dump(st_geo_query_primary("8.8.8.8"));
echo "full resolve(v4): "; var_dump(st_region_name("58.240.9.9"));
'
