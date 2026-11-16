#!/usr/bin/env bash
php -l /opt/dsh/workspace/easyssh-site/site/api/stats/common.php >/dev/null && bash /opt/dsh/workspace/easyssh-site/tools/deploy.sh 2>&1 | tail -1
php -r 'require "/www/wwwroot/easyssh.cn/api/stats/common.php";
$p6 = st_ip_prefix("2408:8207:1a2b:3c4d::9");
echo "v6 prefix: "; var_dump($p6);
echo "v6 name: "; var_dump(st_region_name("2408:8207:1a2b:3c4d::9"));
echo "v6 label: "; var_dump(st_ip_mask_label("v6", "2408:8207"));
echo "v4: "; var_dump(st_ip_prefix("58.240.1.1"));
echo "v4 loopback: "; var_dump(st_ip_prefix("127.0.0.1"));
echo "v6 linklocal: "; var_dump(st_ip_prefix("fe80::1"));
$fields = [];
st_apply_region($fields, "2408:8207:1a2b:3c4d::9");
echo "apply_region v6: "; var_dump($fields["地域"] ?? "(unset)");
$fields = [];
st_apply_region($fields, "58.240.6.7");
echo "apply_region v4: "; var_dump($fields["地域"] ?? "(unset)");
'
