<?php
/**
 * GET /api/download?token=<token> —— 下载中转入口（对齐 EasyRDP 的 /api/download?token= 口径）
 *
 * 与 /api/download/file.php?t=<token> 完全同一条校验与流式下载链路：
 *   兼容参数名 token（新）与 t（旧）；转发到 file.php 执行签名校验/滑动窗口/IP 绑定/
 *   并发记账/Range 206 流式输出。旧 file.php?t= 链接继续有效。
 */

if (isset($_GET['token']) && !isset($_GET['t'])) {
  $_GET['t'] = $_GET['token'];
}
require __DIR__ . '/file.php';
