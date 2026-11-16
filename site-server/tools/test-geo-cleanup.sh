#!/usr/bin/env bash
php -l /opt/dsh/workspace/easyssh-site/site/api/stats/common.php >/dev/null && bash /opt/dsh/workspace/easyssh-site/tools/deploy.sh 2>&1 | tail -1
php -r 'require "/www/wwwroot/easyssh.cn/api/stats/common.php";
echo "fresh v4 prefix(223.104): "; var_dump(st_region_name("223.104.3.4"));
echo "full resolve 223.104.3.4: "; var_dump(st_region_name("223.104.3.4"));
'
