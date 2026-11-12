<?php
/* rotate-master-20261112.php — R-20261111-001 master 轮换 Phase A(反馈/统计通道双密钥兼容窗口)
 * 由 dev 机 scp 到服务器 /tmp 执行,执行后立即删除。不在 argv 传密钥(从 stdin 读)。
 * 步骤:备份 keys.php/common.php → keys.php 增 master_prev、master 换新 →
 *       common.php fb_decrypt_envelope 改双密钥校验(新优先,回退 master_prev) → php -l → 自测新旧信封。
 */
error_reporting(E_ALL);
ini_set('display_errors', '1');

$FB_DIR   = '/opt/dsh/workspace/easyssh-site/site/api/feedback';
$KEYS     = $FB_DIR . '/keys.php';
$COMMON   = $FB_DIR . '/common.php';
$STAMP    = date('Ymd-His');
$stdin    = trim(stream_get_contents(STDIN));
if (!preg_match('/^[A-Za-z0-9+\/=]{43,44}$/', $stdin)) { fwrite(STDERR, "stdin 不是合法 base64 master\n"); exit(1); }
$newRaw   = base64_decode($stdin, true);
if ($newRaw === false || strlen($newRaw) !== 32) { fwrite(STDERR, "新 master 必须 32 字节\n"); exit(1); }

/* ---- 备份 ---- */
foreach ([$KEYS, $COMMON] as $f) {
  $bak = $f . '.bak-rot-' . $STAMP;
  if (!copy($f, $bak)) { fwrite(STDERR, "备份失败: $f\n"); exit(1); }
  chmod($bak, 0600);
  echo "backup: $bak\n";
}

/* ---- 1) keys.php:master_prev=旧, master=新 ---- */
$lines = file($KEYS, FILE_IGNORE_NEW_LINES);
$json  = implode("\n", array_slice($lines, 1));
$keys  = json_decode($json, true);
if (!is_array($keys) || empty($keys['master'])) { fwrite(STDERR, "keys.php 解析失败\n"); exit(1); }
if (isset($keys['master_prev']) && $keys['master_prev'] === $keys['master']) unset($keys['master_prev']);
if (!isset($keys['master_prev'])) $keys['master_prev'] = $keys['master']; // 旧值入 prev
$keys['master'] = $stdin; // 新值
$out = "<?php exit(0); ?>\n" . json_encode($keys, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE) . "\n";
if (file_put_contents($KEYS, $out, LOCK_EX) === false) { fwrite(STDERR, "keys.php 写入失败\n"); exit(1); }
chmod($KEYS, 0600);
echo "keys.php: master rotated (prev kept), fields=" . implode(',', array_keys($keys)) . "\n";

/* ---- 2) common.php:双密钥校验 ---- */
$src = file_get_contents($COMMON);
$oldK  = "  \$keys = fb_keys();\n  \$k = fb_derive(base64_decode(\$keys['master'], true));\n";
$newK  = "  \$keys = fb_keys();\n";
$oldV  = "  \$want = hash_hmac('sha256', \$ts . '.' . (\$env['nonce'] ?? '') . '.' . (\$env['ct'] ?? '') . '.' . (\$env['tag'] ?? ''), \$k['mac'], true);\n"
       . "  if (!hash_equals(\$want, \$mac)) fb_fail('信封签名校验失败');\n"
       . "  \$plain = openssl_decrypt(\$ct, 'aes-256-gcm', \$k['enc'], OPENSSL_RAW_DATA, \$nonce, \$tag);\n"
       . "  if (\$plain === false) fb_fail('解密失败');\n";
$newV  = "  \$macData = \$ts . '.' . (\$env['nonce'] ?? '') . '.' . (\$env['ct'] ?? '') . '.' . (\$env['tag'] ?? '');\n"
       . "  // R-20261111-001 密钥轮换:双密钥兼容窗口——新 master 优先,失败回退 master_prev(过渡期,窗口结束后移除回退分支)\n"
       . "  \$mList = [ \$keys['master'] ];\n"
       . "  if (!empty(\$keys['master_prev'])) \$mList[] = \$keys['master_prev'];\n"
       . "  \$plain = false;\n"
       . "  foreach (\$mList as \$mRaw) {\n"
       . "    \$km = fb_derive(base64_decode(\$mRaw, true));\n"
       . "    \$want = hash_hmac('sha256', \$macData, \$km['mac'], true);\n"
       . "    if (hash_equals(\$want, \$mac)) {\n"
       . "      \$plain = openssl_decrypt(\$ct, 'aes-256-gcm', \$km['enc'], OPENSSL_RAW_DATA, \$nonce, \$tag);\n"
       . "      if (\$plain !== false) break;\n"
       . "    }\n"
       . "  }\n"
       . "  if (\$plain === false) fb_fail('信封签名校验失败');\n";
if (substr_count($src, $oldK) !== 1) { fwrite(STDERR, "common.php 定位失败(master 派生行)——中止,文件未改\n"); exit(1); }
if (substr_count($src, $oldV) !== 1) { fwrite(STDERR, "common.php 定位失败(校验块)——中止,文件未改\n"); exit(1); }
$src = str_replace($oldK, $newK, $src);
$src = str_replace($oldV, $newV, $src);
if (file_put_contents($COMMON, $src, LOCK_EX) === false) { fwrite(STDERR, "common.php 写入失败\n"); exit(1); }
echo "common.php: dual-key verify installed\n";

/* ---- 3) lint ---- */
exec('php -l ' . escapeshellarg($COMMON) . ' 2>&1', $o, $rc);
echo implode("\n", $o) . "\n";
if ($rc !== 0) { fwrite(STDERR, "php -l 失败,回滚\n"); copy($COMMON . '.bak-rot-' . $STAMP, $COMMON); exit(1); }

/* ---- 4) 自测:旧/新 master 信封均可通过 fb_decrypt_envelope ---- */
require $COMMON;
function make_env($masterRaw, $payload) {
  $enc = hash_hmac('sha256', 'enc', $masterRaw, true);
  $macK = hash_hmac('sha256', 'mac', $masterRaw, true);
  $nonce = random_bytes(12);
  $tag = '';
  $ct = openssl_encrypt(json_encode($payload), 'aes-256-gcm', $enc, OPENSSL_RAW_DATA, $nonce, $tag);
  $ctB = base64_encode($ct); $tagB = base64_encode($tag); $nB = base64_encode($nonce);
  $ts = (string)round(microtime(true) * 1000);
  $mac = base64_encode(hash_hmac('sha256', $ts . '.' . $nB . '.' . $ctB . '.' . $tagB, $macK, true));
  return ['v'=>1,'alg'=>'A256GCM-HS256','ts'=>$ts,'nonce'=>$nB,'ct'=>$ctB,'tag'=>$tagB,'mac'=>$mac];
}
$oldRaw = base64_decode($keys['master_prev'], true);
$ok = 0;
try { $d = fb_decrypt_envelope(make_env($oldRaw, ['probe'=>'old'])); if (($d['probe'] ?? '') === 'old') { echo "SELFTEST old-master envelope: PASS\n"; $ok++; } } catch (Throwable $e) { echo "SELFTEST old-master: FAIL " . $e->getMessage() . "\n"; }
try { $d = fb_decrypt_envelope(make_env($newRaw, ['probe'=>'new'])); if (($d['probe'] ?? '') === 'new') { echo "SELFTEST new-master envelope: PASS\n"; $ok++; } } catch (Throwable $e) { echo "SELFTEST new-master: FAIL " . $e->getMessage() . "\n"; }
/* 负例:随机密钥信封必须被拒(fb_fail 为 exit,放子进程跑) */
$neg = 'chdir("/tmp"); require ' . var_export($COMMON, true) . '; '
     . '$m=random_bytes(32); $enc=hash_hmac("sha256","enc",$m,true); $mk=hash_hmac("sha256","mac",$m,true); '
     . '$n=random_bytes(12); $tag=""; $ct=openssl_encrypt("{}","aes-256-gcm",$enc,OPENSSL_RAW_DATA,$n,$tag); '
     . '$ctB=base64_encode($ct);$tB=base64_encode($tag);$nB=base64_encode($n);$ts=(string)round(microtime(true)*1000); '
     . '$mac=base64_encode(hash_hmac("sha256",$ts.".".$nB.".".$ctB.".".$tB,$mk,true)); '
     . 'fb_decrypt_envelope(["v"=>1,"alg"=>"A256GCM-HS256","ts"=>$ts,"nonce"=>$nB,"ct"=>$ctB,"tag"=>$tB,"mac"=>$mac]); '
     . 'echo "WRONG-KEY ACCEPTED";';
exec('php -r ' . escapeshellarg($neg) . ' 2>&1', $o2, $rc2);
if ($rc2 !== 0 || strpos(implode("\n", $o2), 'WRONG-KEY ACCEPTED') === false) { echo "SELFTEST wrong-key rejected: PASS\n"; $ok++; } else { echo "SELFTEST wrong-key: FAIL(被接受了!)\n"; }
echo "RESULT ok=$ok/3\n";
exit($ok === 3 ? 0 : 1);
