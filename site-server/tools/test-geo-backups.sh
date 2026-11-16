#!/usr/bin/env bash
V6="2400:3200::1"
V4="58.240.1.1"
echo "== 1. baidu opendata v6 =="
curl -s --max-time 5 "https://opendata.baidu.com/api.php?query=$V6&co=&resource_id=6006&oe=utf8" | head -c 300; echo
echo "== 2. ip-api.com zh-CN v4 =="
curl -s --max-time 5 "http://ip-api.com/json/$V4?lang=zh-CN&fields=status,country,regionName,city,isp" ; echo
echo "== 3. ip-api.com zh-CN v6 =="
curl -s --max-time 5 "http://ip-api.com/json/$V6?lang=zh-CN&fields=status,country,regionName,city,isp" ; echo
echo "== 4. vore.top v4 =="
curl -s --max-time 5 "https://api.vore.top/api/IPdata?ip=$V4" | head -c 400; echo
echo "== 5. vore.top v6 =="
curl -s --max-time 5 "https://api.vore.top/api/IPdata?ip=$V6" | head -c 400; echo
