<?php
/**
 * tools/backfill-stats-region-lang.php —— 一次性回填（2026-11-11）
 * 把 stats-db.json 中已有记录的：
 *   1) 地域（"xx.xx" 两段 IP 前缀）→ 用代表 IP "xx.xx.1.1" 走归属地解析，补中文地域名
 *   2) 系统语言代码（LCID 数字或语言代码）→ 转为 "代码（中文）" 表示
 * 已含中文地域名 / 已含中文表示的记录跳过。先自动备份 stats-db.json.bak-backfill-<ts>。
 * 用法（服务器）：php3 tools/backfill-stats-region-lang.php
 */
require '/www/wwwroot/easyssh.cn/api/stats/common.php'; // 必须 require webroot 部署副本，ST_DB 才会解析到 /www/wwwroot/data/stats-db.json

/* Windows LCID 数字 → 标准语言代码（与 admin.html LANG_ZH_LCID 保持一致） */
$LCID = [
  '2052' => 'zh-CN', '1028' => 'zh-TW', '3076' => 'zh-HK', '4100' => 'zh-SG', '1025' => 'ar', '1031' => 'de-DE', '3079' => 'de-AT', '2055' => 'de-CH',
  '1033' => 'en-US', '2057' => 'en-GB', '4105' => 'en-CA', '3081' => 'en-AU', '16393' => 'en-IN', '1034' => 'es-ES', '3082' => 'es-MX', '11274' => 'es-AR',
  '1035' => 'fi', '1036' => 'fr-FR', '4108' => 'fr-CA', '1040' => 'it-IT', '1041' => 'ja-JP', '1042' => 'ko-KR', '1043' => 'nl-NL', '1038' => 'hu',
  '1030' => 'da', '1053' => 'sv-SE', '1044' => 'is', '1045' => 'pl', '1029' => 'cs', '1051' => 'sk', '1060' => 'sl', '1050' => 'hr',
  '419' => 'ru', '1046' => 'pt-BR', '2070' => 'pt-PT', '1048' => 'ro', '1049' => 'ru-RU', '1059' => 'hy', '1055' => 'tr-TR', '1058' => 'uk',
  '1037' => 'he', '1026' => 'bg', '1052' => 'sq', '1062' => 'lv', '1063' => 'lt', '1054' => 'th', '1066' => 'vi', '1057' => 'id', '1086' => 'ms', '1056' => 'fa',
];
$LANG_ZH = [
  'zh' => '中文', 'zh-CN' => '简体中文', 'zh-SG' => '简体中文(新加坡)', 'zh-TW' => '繁体中文(台湾)', 'zh-HK' => '繁体中文(香港)', 'zh-Hant' => '繁体中文', 'zh-Hans' => '简体中文',
  'en' => '英语', 'en-US' => '英语(美国)', 'en-GB' => '英语(英国)', 'en-AU' => '英语(澳大利亚)', 'en-CA' => '英语(加拿大)', 'en-IN' => '英语(印度)',
  'ja' => '日语', 'ja-JP' => '日语', 'ko' => '韩语', 'ko-KR' => '韩语',
  'fr' => '法语', 'fr-FR' => '法语', 'fr-CA' => '法语(加拿大)', 'de' => '德语', 'de-DE' => '德语', 'de-AT' => '德语(奥地利)', 'de-CH' => '德语(瑞士)',
  'es' => '西班牙语', 'es-ES' => '西班牙语', 'es-MX' => '西班牙语(墨西哥)', 'es-AR' => '西班牙语(阿根廷)',
  'pt' => '葡萄牙语', 'pt-BR' => '葡萄牙语(巴西)', 'pt-PT' => '葡萄牙语(葡萄牙)',
  'ru' => '俄语', 'ru-RU' => '俄语', 'it' => '意大利语', 'it-IT' => '意大利语',
  'nl' => '荷兰语', 'nl-NL' => '荷兰语', 'sv' => '瑞典语', 'sv-SE' => '瑞典语', 'da' => '丹麦语', 'fi' => '芬兰语', 'nb' => '挪威语', 'nn' => '挪威语(新)',
  'pl' => '波兰语', 'cs' => '捷克语', 'sk' => '斯洛伐克语', 'sl' => '斯洛文尼亚语', 'hr' => '克罗地亚语', 'sr' => '塞尔维亚语', 'bg' => '保加利亚语', 'ro' => '罗马尼亚语',
  'el' => '希腊语', 'hu' => '匈牙利语', 'tr' => '土耳其语', 'tr-TR' => '土耳其语', 'uk' => '乌克兰语', 'he' => '希伯来语', 'ar' => '阿拉伯语', 'fa' => '波斯语',
  'hi' => '印地语', 'th' => '泰语', 'vi' => '越南语', 'id' => '印尼语', 'ms' => '马来语', 'tl' => '菲律宾语', 'is' => '冰岛语', 'hy' => '亚美尼亚语',
  'sq' => '阿尔巴尼亚语', 'lv' => '拉脱维亚语', 'lt' => '立陶宛语',
];

/** 语言代码 → "代码（中文）"，未收录原样返回 */
function lang_disp($code, $LCID, $LANG_ZH) {
  $code = trim((string)$code);
  if ($code === '') return $code;
  if (isset($LCID[$code])) $code = $LCID[$code]; // LCID 数字先转标准代码
  if (isset($LANG_ZH[$code])) return $code . '（' . $LANG_ZH[$code] . '）';
  $base = explode('-', $code)[0];
  if (isset($LANG_ZH[$base])) return $code . '（' . $LANG_ZH[$base] . '）';
  return $code;
}

echo "== EasySSH 统计回填开始 " . date('Y-m-d H:i:s') . " ==\n";

/* 备份 */
if (is_readable(ST_DB)) {
  $bak = ST_DB . '.bak-backfill-' . date('YmdHis');
  copy(ST_DB, $bak);
  echo "已备份: $bak\n";
}

$db = st_db_load();
$nRegion = 0; $nLang = 0; $nSkipRegion = 0; $nSkipLang = 0;

foreach ($db['clients'] as $id => $c) {
  $f = $c['fields'] ?? [];

  /* 1) 地域回填 */
  $reg = trim((string)($f['地域'] ?? ''));
  if ($reg !== '') {
    // 已有中文地域名但前缀缺 ".x.x" 掩码 → 补全为 "中文名 xx.xx.x.x"
    if (preg_match('/^(\S.*) (\d{1,3}\.\d{1,3})$/u', $reg, $mm)) {
      $db['clients'][$id]['fields']['地域'] = $mm[1] . ' ' . $mm[2] . '.x.x';
      $nRegion++;
      echo "  [$id] 地域掩码补全: $reg => {$db['clients'][$id]['fields']['地域']}\n";
    } elseif (preg_match('/^(\d{1,3})\.(\d{1,3})(?:\.|$)/', $reg, $m)) {
      $name = st_region_name($m[1] . '.' . $m[2] . '.1.1'); // 代表 IP，结果按前两段缓存
      if ($name !== '') {
        $db['clients'][$id]['fields']['地域'] = $name . ' ' . $m[1] . '.' . $m[2];
        $nRegion++;
        echo "  [$id] 地域: $reg => {$db['clients'][$id]['fields']['地域']}\n";
      } else {
        echo "  [$id] 地域: $reg 解析失败，保留原值\n";
      }
    } else {
      $nSkipRegion++; // 已含中文地域名或其他格式，跳过
    }
  }

  /* 2) 语言回填 */
  $lang = trim((string)($f['系统语言代码'] ?? ''));
  if ($lang !== '' && mb_strpos($lang, '（') === false) { // 未含中文表示才转
    $disp = lang_disp($lang, $LCID, $LANG_ZH);
    if ($disp !== $lang) {
      $db['clients'][$id]['fields']['系统语言代码'] = $disp;
      $nLang++;
      echo "  [$id] 语言: $lang => $disp\n";
    } else {
      $nSkipLang++;
    }
  } else {
    $nSkipLang++;
  }
}

if ($nRegion || $nLang) {
  st_db_save($db);
  @chown(ST_DB, 'www'); @chgrp(ST_DB, 'www'); // 保持 php-fpm(www) 可写
  @chmod(ST_DB, 0664);
}

echo "回填完成: 地域更新 $nRegion 条(跳过 $nSkipRegion), 语言更新 $nLang 条(跳过 $nSkipLang)\n";
