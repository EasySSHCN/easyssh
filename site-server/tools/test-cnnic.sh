#!/usr/bin/env bash
V6="2400:3200::"
V6B="2408:8207::1"
V4="58.240.1.1"
echo "== CNNIC v6 (2400:3200::) =="
curl -s --max-time 8 "https://ipwhois.cnnic.net/bns/query/Query/ipwhoisQuery.do?txtquery=$V6&queryOption=ipv6" -o /tmp/c1.html -w "HTTP %{http_code}, %{size_download} bytes\n"
grep -oE '(netname|country|address|descr|remark)[^<]*<[^>]*>[^<]{0,60}' /tmp/c1.html | head -20
echo "--- plain text dump (first 600 chars, tags stripped) ---"
sed -e 's/<[^>]*>/ /g' /tmp/c1.html | tr -s ' \n' ' \n' | head -c 600; echo
echo "== CNNIC v6 (2408:8207::1) =="
curl -s --max-time 8 "https://ipwhois.cnnic.net/bns/query/Query/ipwhoisQuery.do?txtquery=$V6B&queryOption=ipv6" -o /tmp/c2.html -w "HTTP %{http_code}, %{size_download} bytes\n"
sed -e 's/<[^>]*>/ /g' /tmp/c2.html | tr -s ' \n' ' \n' | head -c 400; echo
echo "== CNNIC v4 (58.240.1.1) with queryOption=ipv4 =="
curl -s --max-time 8 "https://ipwhois.cnnic.net/bns/query/Query/ipwhoisQuery.do?txtquery=$V4&queryOption=ipv4" -o /tmp/c3.html -w "HTTP %{http_code}, %{size_download} bytes\n"
sed -e 's/<[^>]*>/ /g' /tmp/c3.html | tr -s ' \n' ' \n' | head -c 400; echo
