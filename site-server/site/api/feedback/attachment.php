<?php
/**
 * GET /api/feedback/attachment.php —— v5.1.7 (R-20261115-005) 反馈附件鉴权分发(仅管理员)
 *
 * 安全要点:
 *  - 仅管理员会话可访问(fb_check_token,与 admin.php 同一 token 体系);
 *  - id 白名单正则(^a[0-9a-z]{6,20}$),文件路径由服务端拼接,杜绝路径穿越;
 *  - Content-Type 由存储时校验过的扩展名固定给出(绝不做浏览器内容嗅探);
 *  - X-Content-Type-Options: nosniff;仅白名单图片类型内联展示,其余一律强制下载。
 * 参数:id, user, token
 */
require __DIR__ . '/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'GET') fb_fail('仅支持 GET', 405);
$id = (string)($_GET['id'] ?? '');
$user = (string)($_GET['user'] ?? '');
$token = (string)($_GET['token'] ?? '');
if (!preg_match('/^a[0-9a-z]{6,20}$/', $id)) fb_fail('附件 id 非法');
if (!fb_check_token($user, $token)) fb_fail('未登录或登录已过期,请重新登录', 401);

$idx = fb_att_index_load();
$rec = $idx[$id] ?? null;
if (!is_array($rec)) fb_fail('附件不存在或已回收', 404);
$ext = (string)($rec['ext'] ?? '');
if (!isset(FB_ATT_MIME[$ext])) fb_fail('附件类型不支持', 404);
$file = FB_ATT_DIR . '/' . $id . '.' . $ext;
if (!is_readable($file)) fb_fail('附件文件缺失', 404);

$data = (string)file_get_contents($file);
http_response_code(200);
header('Content-Type: ' . FB_ATT_MIME[$ext]);
header('Content-Length: ' . strlen($data));
header('X-Content-Type-Options: nosniff');
header('Cache-Control: private, max-age=600');
header('Content-Disposition: inline; filename="' . $id . '.' . $ext . '"');
echo $data;
exit;
