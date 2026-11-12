<?php
/* patch-lang-lcid.php — 后台语言展示在中文标签后附原始 LCID(仅数字 LCID 追加 [xxxx] 后缀)
 * 修的三个点:①分布卡片构建 ②详细列表弹窗 distEntries ③设备统计字段详情。 */
error_reporting(E_ALL);
 ini_set('display_errors', '1');
$F = '/opt/dsh/workspace/easyssh-site/site/admin.html';
$bak = $F . '.bak-lcid-' . date('Ymd-His');
if (!copy($F, $bak)) { fwrite(STDERR, "备份失败\n"); exit(1); }
chmod($bak, 0600);
echo "backup: $bak\n";
$src = file_get_contents($F);

$reps = [
  // langDisp 之后新增 langDispKey 助手
  array(
    "      return zh ? code + \"（\" + zh + \"）\" : code;\n    }\n",
    "      return zh ? code + \"（\" + zh + \"）\" : code;\n    }\n"
    . "    /* R-20261111-002 复盘:原始 LCID 数字(如 2052/0804)标签可能相同但分组键不同——\n"
    . "     * 展示时在中文标签后附 [原始LCID] 后缀以便区分;非数字代码原样 */\n"
    . "    function langDispKey(code) {\n"
    . "      var raw = String(code || \"\").trim();\n"
    . "      var d = langDisp(raw);\n"
    . "      return (/^\\d{1,5}\$/.test(raw) && String(d).indexOf(\"[\" + raw + \"]\") < 0) ? d + \" [\" + raw + \"]\" : d;\n"
    . "    }\n",
    1),
  array(
    "          Object.keys(s.byLang || {}).forEach(function (k) { out[langDisp(k)] = s.byLang[k]; });\n",
    "          Object.keys(s.byLang || {}).forEach(function (k) { out[langDispKey(k)] = s.byLang[k]; });\n",
    1),
  array(
    "        return { name: key === \"byLang\" ? langDisp(k) : k, count: b[k] };\n",
    "        return { name: key === \"byLang\" ? langDispKey(k) : k, count: b[k] };\n",
    1),
  array(
    "          if (k === \"系统语言代码\") v = langDisp(v); // 语言代码附加中文表示\n",
    "          if (k === \"系统语言代码\") v = langDispKey(v); // 语言代码附加中文表示+原始LCID\n",
    1),
];
foreach ($reps as $i => $r) {
  list($old, $new) = $r;
  $n = substr_count($src, $old);
  if ($n !== 1) { fwrite(STDERR, "定位失败 #$i (出现 $n 次)——中止,文件未改\n"); exit(1); }
  $src = str_replace($old, $new, $src);
  echo "patched #$i\n";
}
if (file_put_contents($F, $src, LOCK_EX) === false) { fwrite(STDERR, "写入失败\n"); exit(1); }
echo "OK admin.html updated\n";
