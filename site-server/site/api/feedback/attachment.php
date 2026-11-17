<?php
/**
 * POST /api/feedback/attachment.php —— v5.1.7 (R-20261115-005) 反馈附件鉴权分发(仅管理员)
 * v5.1.8 修复 (R-20261116-002):鉴权改走 POST + 请求头,不再经 URL 传参——
 *   此前 user/token 出现在查询串中,会暴露管理员用户名与Bearer token(浏览器历史/日志/Referer 风险)。
 *   现请求体 = JSON { id },请求头 X-Fb-User / X-Fb-Token;后台前端以 fetch→blob 方式加载图片。
 *
 * 安全要点:
 *  - 仅管理员会话可访问(fb_check_token,与 admin.php 同一 token 体系,请求头传递);
 *  - id 白名单正则(^a[0-9a-z]{6,20}$),文件路径由服务端拼接,杜绝路径穿越;
 *  - Content-Type 由存储时校验过的扩展名固定给出(绝不做浏览器内容嗅探);
 *  - X-Content-Type-Options: nosniff;仅白名单图片类型内联展示,其余一律强制下载。
 */
require __DIR__ . '/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);
$in = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($in)) fb_fail('请求体必须是 JSON');
$id = (string)($in['id'] ?? '');
$thumb = !empty($in['thumb']); // v5.1.8 (R-20261116-003):列表缩略图
$user = (string)($_SERVER['HTTP_X_FB_USER'] ?? '');
$token = (string)($_SERVER['HTTP_X_FB_TOKEN'] ?? '');
if (!preg_match('/^a[0-9a-z]{6,20}$/', $id)) fb_fail('附件 id 非法');
if (!fb_check_token($user, $token)) fb_fail('未登录或登录已过期，请重新登录', 401);

$idx = fb_att_index_load();
$rec = $idx[$id] ?? null;
if (!is_array($rec)) fb_fail('附件不存在或已回收', 404);
$ext = (string)($rec['ext'] ?? '');
if (!isset(FB_ATT_MIME[$ext])) fb_fail('附件类型不支持', 404);
if ($thumb) {
  // 缩略图(固定 WebP);缺失或不可读时自动回退原图
  $file = FB_ATT_DIR . '/' . $id . '.thumb.webp';
  if (!is_readable($file)) $file = FB_ATT_DIR . '/' . $id . '.' . $ext;
  $mime = 'image/webp';
  if (substr($file, -strlen('.' . $ext)) === '.' . $ext) $mime = FB_ATT_MIME[$ext];
} else {
  $file = FB_ATT_DIR . '/' . $id . '.' . $ext;
  $mime = FB_ATT_MIME[$ext];
}
if (!is_readable($file)) fb_fail('附件文件缺失', 404);

$data = (string)file_get_contents($file);
http_response_code(200);
header('Content-Type: ' . $mime);
header('Content-Length: ' . strlen($data));
header('X-Content-Type-Options: nosniff');
header('Cache-Control: private, no-store');
header('Content-Disposition: inline; filename="' . $id . '.' . $ext . '"');
echo $data;
exit;
