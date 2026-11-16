#!/usr/bin/env bash
php -l /opt/dsh/workspace/easyssh-site/site/api/stats/common.php >/dev/null && bash /opt/dsh/workspace/easyssh-site/tools/deploy.sh 2>&1 | tail -1
php -r 'require "/www/wwwroot/easyssh.cn/api/stats/common.php";
echo "ipapi(v4): "; var_dump(st_geo_query_ipapi("58.240.1.1"));
echo "ipapi(v6): "; var_dump(st_geo_query_ipapi("2400:3200::1"));
echo "baidu(v4): "; var_dump(st_geo_query_baidu("111.55.1.1"));
echo "full fresh v6 (240e:100::1): "; var_dump(st_region_name("240e:100::1"));
echo "cached recheck: "; var_dump(st_region_name("240e:100::1"));
'
