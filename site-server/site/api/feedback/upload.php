<?php
/**
 * POST /api/feedback/upload.php —— v5.1.7 (R-20261115-005) 反馈附件上传
 *
 * 协议:与文本反馈同一套加密体系(AES-256-GCM + HMAC-SHA256 时效签名),但密文走二进制通道:
 *   请求头: X-Fb-Ts: <ms>   X-Fb-Nonce: b64(12B)   X-Fb-Tag: b64(16B)   X-Fb-Mac: b64
 *   请求体: application/octet-stream —— AES-GCM 密文原始字节
 *   mac   = HMAC-SHA256(fbMacKey, `${ts}.${nonce_b64}.${b64(请求体)}.${tag_b64}`)(ct 以 b64 形式参与签名,
 *           与文本反馈信封完全一致;服务端对收到的原始字节重算 b64 后校验)
 *   明文  = 原始图片文件字节(PNG/JPEG/GIF/WebP)
 *
 * 安全设计(与 common.php 注释一致):magic bytes 二次校验 → 元数据剥离(GIF 结构级清洗保留动画,
 * 其余 GD 重编码)→ 服务端随机文件名 → 存 webroot 之外隔离目录 → 按 IP 限流 → 未绑定 1h 回收。
 * 响应:{ ok:true, id } 或 { ok:false, message }
 */
require __DIR__ . '/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);

// ── 1. 读取密文(硬性大小上限,先看 Content-Length 再读) ──
$clen = (int)($_SERVER['CONTENT_LENGTH'] ?? 0);
if ($clen <= 0 || $clen > FB_ATT_MAX_BYTES) fb_fail('附件大小非法(需 1B-8MB)');
$ct = file_get_contents('php://input', false, null, 0, FB_ATT_MAX_BYTES + 1);
if ($ct === false || $ct === '' || strlen($ct) > FB_ATT_MAX_BYTES) fb_fail('附件读取失败或超过 8MB 上限');
if (strlen($ct) < 16) fb_fail('附件内容非法');

// ── 2. 校验签名信封(与文本反馈同一密钥派生与 MAC 公式) ──
$keys = fb_keys();
$k = fb_derive(base64_decode($keys['master'], true));
$ts = (string)($_SERVER['HTTP_X_FB_TS'] ?? '');
$nonceS = (string)($_SERVER['HTTP_X_FB_NONCE'] ?? '');
$tagS = (string)($_SERVER['HTTP_X_FB_TAG'] ?? '');
$macS = (string)($_SERVER['HTTP_X_FB_MAC'] ?? '');
$nonce = fb_b64d($nonceS);
$tag = fb_b64d($tagS);
$mac = fb_b64d($macS);
if (strlen($nonce) !== 12 || strlen($tag) !== 16) fb_fail('信封参数非法');
if (!preg_match('/^\d{13,16}$/', $ts) || abs((int)$ts - (int)(microtime(true) * 1000)) > FB_TS_WINDOW_MS) {
  fb_fail('请求已过期,请校对本机时间后重试');
}
$ctS = base64_encode($ct);
$want = hash_hmac('sha256', $ts . '.' . $nonceS . '.' . $ctS . '.' . $tagS, $k['mac'], true);
if (!hash_equals($want, $mac)) fb_fail('信封签名校验失败');
$plain = openssl_decrypt($ct, 'aes-256-gcm', $k['enc'], OPENSSL_RAW_DATA, $nonce, $tag);
if ($plain === false) fb_fail('解密失败');

// ── 3. magic bytes 二次校验 ──
$ext = fb_att_detect($plain);
if ($ext === null) fb_fail('仅支持 PNG/JPEG/GIF/WebP 图片文件');

// ── 4. 限流:按 IP 每小时最多 30 次上传 ──
if (!fb_ip_rate_ok(FB_DATA_DIR . '/att-ratelimit.json', 30, 3600 * 1000)) {
  fb_fail('上传过于频繁,请稍后再试', 429);
}

// ── 5. 元数据剥离 / 结构清洗 ──
try {
  $clean = fb_att_sanitize($plain, $ext);
} catch (RuntimeException $e) {
  fb_fail('附件校验失败: ' . $e->getMessage());
}
if (strlen($clean) > FB_ATT_MAX_BYTES) fb_fail('附件清洗后超过 8MB 上限');

// ── 6. 服务端随机文件名 + 隔离存储(webroot 之外) ──
$ipHash = hash('sha256', fb_client_ip() . '|' . (string)($keys['token_secret'] ?? ''));
$now = fb_now_ms();
$idx = fb_att_gc(fb_att_index_load());
if (!is_dir(FB_ATT_DIR)) @mkdir(FB_ATT_DIR, 02770, true);
$attId = 'a' . base_convert((string)$now, 10, 36) . bin2hex(random_bytes(5));
$file = FB_ATT_DIR . '/' . $attId . '.' . $ext;
if (strpos($attId, '/') !== false || strpos($attId, '\\') !== false) fb_fail('内部错误', 500);
if (@file_put_contents($file, $clean, LOCK_EX) === false) fb_fail('服务器存储写入失败', 500);
@chmod($file, 0664);
// v5.1.8 (R-20261116-003):生成列表缩略图(失败不影响上传,分发端自动回退原图)
fb_att_make_thumb($clean, FB_ATT_DIR . '/' . $attId . '.thumb.webp');
$idx[$attId] = [
  'ext' => $ext,
  'size' => strlen($clean),
  'ts' => $now,
  'ipHash' => $ipHash,
  'bound' => '',
];
fb_att_index_save($idx);
fb_json_out(['ok' => true, 'id' => $attId, 'ext' => $ext, 'size' => strlen($clean)]);
